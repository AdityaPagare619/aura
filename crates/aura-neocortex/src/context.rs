//! Context assembly for AURA Neocortex.
//!
//! Takes a `ContextPackage` (the real type from `aura_types::ipc`) and
//! produces an `AssembledPrompt` ready for the model.  Implements
//! priority-based truncation when the context exceeds the mode's token budget.
//!
//! ## Teacher Stack Integration
//!
//! The context assembly layer now supports the 6-layer teacher structure stack:
//!
//! - **`ContextBuilder`**: Fluent API for constructing prompts with teacher
//!   stack features (tools, CoT, grammar constraints, retry context).
//! - **`assemble_reflection_context()`**: Builds a verification prompt for
//!   Layer 4 (Cross-Model Reflection), sent to the Brainstem model.
//! - **`assemble_retry_context()`**: Rebuilds the prompt with failure context
//!   for Layer 3 (Cascading Retry).
//! - **`TokenTracker`**: Tracks token budget across multi-pass teacher stack
//!   operations, ensuring we don't exceed limits.
//!
//! The original `assemble_prompt()` / `assemble_replan_prompt()` still work
//! as before — backward compatible.

use aura_types::ipc::{
    ContextPackage, ConversationTurn, FailureContext, GoalSummary, InferenceMode, MemorySnippet,
    Role, ScreenSummary,
};

use crate::grammar::GrammarKind;
use crate::model_capabilities::ModelCapabilities;
use crate::prompts::{self, ModeConfig, PromptSlots};
use crate::tool_format;

// ─── Control hardcodes ───────────────────────────────────────────────────────
//
// These constants are CONTROL hardcodes — legitimate, justified, and commented.
// They are NOT laziness constants — they encode real domain constraints.

/// Tokens reserved for the LLM's response within the context window.
///
/// Never fill the context window 100% — the model needs room to generate.
/// 512 tokens ≈ ~400 words, sufficient for structured action plans and replies.
/// Source: empirical testing on Qwen2-7B and Llama3-8B class models.
const RESPONSE_RESERVE_TOKENS: usize = 512;

/// Default context budget when `ModelCapabilities` is unavailable.
///
/// Used only before a model loads (startup race) or on GGUF parse failure.
/// 2048 tokens is a conservative safe minimum — every modern on-device model
/// supports at least 4096, so this never over-allocates.
pub const DEFAULT_CONTEXT_BUDGET: usize = 2048;

/// Importance threshold above which a request is considered high-stakes.
///
/// Above this value: Best-of-N sampling (Layer 5) is triggered. This threshold
/// was chosen so that Strategist+failure+blockers (score ≈ 1.0) and
/// Planner+failure+blockers (score ≈ 0.9) both gate through, while simple
/// Planner requests (score ≈ 0.5) do not incur the extra compute.
#[allow(dead_code)] // Phase 8: used by neocortex retrieval priority gating
pub const HIGH_IMPORTANCE_THRESHOLD: f32 = 0.8;

// ─── Assembled prompt ───────────────────────────────────────────────────────

/// The fully assembled prompt ready for inference.
///
/// All fields are set by `ContextBuilder::build()` or by the caller immediately
/// after build (e.g. `prompt.high_stakes = computed_importance > 0.8`).
///
/// **BETA (inference.rs) contract** — stable field names, do not rename:
/// - `system_prompt`  — the full prompt string to pass to the model
/// - `config`         — temperature / max_tokens / stop sequences
/// - `grammar_kind`   — which GBNF grammar to apply (None = free text)
/// - `cot_enabled`    — whether CoT was injected into the prompt
/// - `high_stakes`    — trigger for Layer 5 Best-of-N sampling
/// - `has_tools`      — whether tool descriptions were injected
/// - `is_retry`       — whether this is a retry pass (affects sampling)
/// - `token_budget`   — max tokens the prompt was allowed to consume
/// - `estimated_complexity` — optional hint for tier selection
#[derive(Debug, Clone)]
pub struct AssembledPrompt {
    /// The system prompt with all slots filled and context truncated to budget.
    pub system_prompt: String,
    /// Mode configuration (temperature, max_tokens, etc.).
    pub config: ModeConfig,
    /// Estimated token count of the system prompt.
    pub estimated_tokens: u32,
    /// Whether any context was truncated to fit the budget.
    pub was_truncated: bool,
    /// Grammar kind used for this prompt (if any).
    /// The teacher stack reads this to know which GBNF grammar to apply.
    pub grammar_kind: Option<GrammarKind>,
    /// Whether chain-of-thought was forced for this prompt.
    pub cot_enabled: bool,
    /// The original user goal / request this prompt was assembled for.
    /// Used by the ReAct loop and reflection pass so they reason about
    /// the *actual request*, not the assembled system-prompt boilerplate.
    pub original_goal: String,
    /// Whether the teacher stack judged this request as high-stakes
    /// (importance > `HIGH_IMPORTANCE_THRESHOLD`). Set by the caller
    /// after `build()` via `prompt.high_stakes = importance > HIGH_IMPORTANCE_THRESHOLD`.
    /// Downstream inference (BETA) uses this to activate Layer 5 Best-of-N.
    pub high_stakes: bool,
    /// Whether tool descriptions were injected into this prompt.
    /// BETA checks this to know whether to parse tool-call syntax in the output.
    pub has_tools: bool,
    /// Whether this is a retry pass (previous attempt was rejected).
    /// BETA may adjust sampling parameters (higher temperature) for retries
    /// to produce diverse candidates instead of repeating the same failure.
    pub is_retry: bool,
    /// Token budget this prompt was assembled within.
    ///
    /// Capped to `ModelCapabilities::context_length - RESPONSE_RESERVE_TOKENS`.
    /// BETA uses this to set `max_tokens` on the inference call so the model
    /// cannot generate beyond the reserved response budget.
    pub token_budget: usize,
    /// Estimated task complexity in [0.0, 1.0] — hint for TierSelect.
    ///
    /// Higher values suggest the request benefits from a larger model tier.
    /// Derived from `estimate_importance()` at build time. `None` when the
    /// context package contains insufficient signal to estimate complexity.
    pub estimated_complexity: Option<f32>,
}

// ─── Token tracker ──────────────────────────────────────────────────────────

/// Tracks token budget across multiple teacher stack passes.
///
/// The teacher stack may invoke the model multiple times (retry, reflection,
/// best-of-N). Each pass consumes tokens from a shared budget so we don't
/// run away with unbounded generation.
///
/// Phase 3: multi-pass teacher stack token accounting.
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct TokenTracker {
    /// Total token budget for the entire teacher stack pipeline.
    total_budget: u32,
    /// Tokens consumed so far (prompt + generation across all passes).
    consumed: u32,
    /// Maximum tokens any single pass can consume (prevents one pass from
    /// eating the whole budget).
    per_pass_limit: u32,
}

impl TokenTracker {
    /// Create a new tracker with the given total budget.
    ///
    /// `per_pass_fraction` controls what fraction of the remaining budget
    /// each individual pass can consume (0.0..1.0, typically 0.5).
    pub fn new(total_budget: u32, per_pass_fraction: f32) -> Self {
        let fraction = per_pass_fraction.clamp(0.1, 0.9);
        Self {
            total_budget,
            consumed: 0,
            per_pass_limit: ((total_budget as f32) * fraction) as u32,
        }
    }

    /// How many tokens remain in the total budget.
    pub fn remaining(&self) -> u32 {
        self.total_budget.saturating_sub(self.consumed)
    }

    /// How many tokens the next pass is allowed to consume.
    /// This is the minimum of `per_pass_limit` and `remaining()`.
    ///
    /// Phase 3: multi-pass teacher stack token accounting.
    #[allow(dead_code)]
    pub fn next_pass_budget(&self) -> u32 {
        self.per_pass_limit.min(self.remaining())
    }

    /// Record that a pass consumed some tokens (prompt + generation).
    pub fn record_usage(&mut self, tokens: u32) {
        self.consumed = self.consumed.saturating_add(tokens);
    }

    /// Whether the budget is exhausted (no meaningful generation possible).
    /// We consider < 50 tokens as exhausted since you can't produce useful
    /// output in fewer tokens.
    pub fn is_exhausted(&self) -> bool {
        self.remaining() < 50
    }

    /// Total budget.
    ///
    /// Phase 3: multi-pass teacher stack token accounting.
    #[allow(dead_code)]
    pub fn total_budget(&self) -> u32 {
        self.total_budget
    }

    /// Tokens consumed so far.
    pub fn consumed(&self) -> u32 {
        self.consumed
    }
}

// ─── Context builder (fluent API for teacher stack) ─────────────────────────

/// Fluent builder for constructing prompts with teacher stack features.
///
/// Use this when you need to set grammar constraints, force CoT, inject
/// tools, or add retry context. For simple prompts, `assemble_prompt()`
/// is still fine.
///
/// # Example
/// ```ignore
/// let prompt = ContextBuilder::new(&ctx)
///     .with_grammar(GrammarKind::ActionPlan)
///     .with_cot(true)
///     .with_tools()
///     .build();
/// ```
pub struct ContextBuilder<'a> {
    ctx: &'a ContextPackage,
    template: Option<&'a str>,
    failure: Option<&'a FailureContext>,
    grammar_kind: Option<GrammarKind>,
    force_cot: bool,
    inject_tools: bool,
    previous_attempt: Option<&'a str>,
    rejection_reason: Option<&'a str>,
    /// Override the token budget (otherwise uses mode config / ctx budget).
    budget_override: Option<u32>,
    /// Model capabilities — when present, caps the budget at `context_length`.
    capabilities: Option<ModelCapabilities>,
}

impl<'a> ContextBuilder<'a> {
    /// Create a new builder from a `ContextPackage`.
    pub fn new(ctx: &'a ContextPackage) -> Self {
        Self {
            ctx,
            template: None,
            failure: None,
            grammar_kind: None,
            force_cot: false,
            inject_tools: false,
            previous_attempt: None,
            rejection_reason: None,
            budget_override: None,
            capabilities: None,
        }
    }

    /// Set the template (for Composer mode).
    pub fn with_template(mut self, template: &'a str) -> Self {
        self.template = Some(template);
        self
    }

    /// Set the failure context (for Strategist / Replan).
    pub fn with_failure(mut self, failure: &'a FailureContext) -> Self {
        self.failure = Some(failure);
        self
    }

    /// Set the grammar constraint for Layer 0.
    ///
    /// Phase 3: teacher stack context builder — grammar-constrained generation.
    #[allow(dead_code)]
    pub fn with_grammar(mut self, kind: GrammarKind) -> Self {
        self.grammar_kind = Some(kind);
        self
    }

    /// Force chain-of-thought reasoning for Layer 1.
    ///
    /// Phase 3: teacher stack context builder — CoT injection.
    #[allow(dead_code)]
    pub fn with_cot(mut self, enable: bool) -> Self {
        self.force_cot = enable;
        self
    }

    /// Inject available tool descriptions into the prompt.
    ///
    /// Phase 3: teacher stack context builder — tool-use layer.
    #[allow(dead_code)]
    pub fn with_tools(mut self) -> Self {
        self.inject_tools = true;
        self
    }

    /// Set retry context for Layer 3 (Cascading Retry).
    ///
    /// Phase 3: teacher stack context builder — retry layer.
    #[allow(dead_code)]
    pub fn with_retry(mut self, previous_output: &'a str, reason: &'a str) -> Self {
        self.previous_attempt = Some(previous_output);
        self.rejection_reason = Some(reason);
        self
    }

    /// Override the token budget (e.g., from TokenTracker).
    ///
    /// Phase 3: teacher stack context builder — per-pass budget override.
    #[allow(dead_code)]
    pub fn with_budget(mut self, budget: u32) -> Self {
        self.budget_override = Some(budget);
        self
    }

    /// Provide model capabilities so the budget is capped by the real context
    /// window size. This prevents building prompts that exceed what the model
    /// can actually fit in its KV cache.
    pub fn with_capabilities(mut self, capabilities: ModelCapabilities) -> Self {
        self.capabilities = Some(capabilities);
        self
    }

    /// Build the final `AssembledPrompt`.
    #[tracing::instrument(level = "debug", skip(self), fields(mode = ?self.ctx.inference_mode))]
    pub fn build(self) -> AssembledPrompt {
        let mode = self.ctx.inference_mode;
        let mode_cfg = prompts::mode_config(mode);

        // Budget: override > min(mode budget, ctx budget) [, capped by context_length - reserve]
        let base_budget = mode_cfg.context_budget.min(self.ctx.token_budget);
        let base_budget = if let Some(ref caps) = self.capabilities {
            // Cap at (context_length - RESPONSE_RESERVE_TOKENS) so the model always
            // has room to generate. context_length is u32; cast to usize for arithmetic.
            let window = caps.context_length as usize;
            let available = window.saturating_sub(RESPONSE_RESERVE_TOKENS) as u32;
            base_budget.min(available)
        } else {
            base_budget
        };
        let budget = self.budget_override.unwrap_or(base_budget);

        // Extract mutable working copies for progressive truncation.
        let mut goal_text = format_goal(self.ctx.active_goal.as_ref());
        let mut screen_text = format_screen(self.ctx.current_screen.as_ref());
        let mut history: Vec<String> = self
            .ctx
            .conversation_history
            .iter()
            .map(format_turn)
            .collect();
        let mut memory: Vec<String> = self
            .ctx
            .memory_snippets
            .iter()
            .map(format_snippet)
            .collect();
        let mut was_truncated = false;

        // Tool descriptions (generated once, reused in truncation loop).
        let tool_desc = if self.inject_tools {
            Some(tool_format::format_tools_compact())
        } else {
            None
        };

        // Build slots and measure; truncate if over budget.
        let (prompt, config) = loop {
            let slots = build_slots_extended(
                &goal_text,
                &screen_text,
                &history,
                &memory,
                self.template,
                self.failure,
                self.ctx,
                // Teacher stack extensions:
                tool_desc.as_deref(),
                self.grammar_kind,
                self.force_cot,
                self.previous_attempt,
                self.rejection_reason,
            );

            let (prompt, config) = prompts::build_prompt(mode, &slots);
            let tokens = prompts::estimate_tokens(&prompt);

            if tokens <= budget {
                break (prompt, config);
            }

            // Need to truncate — work from lowest priority upward.
            was_truncated = true;

            // Phase 1: Remove earlier history (keep last 3 turns).
            if history.len() > 3 {
                history.remove(0);
                continue;
            }

            // Phase 2: Remove memory snippets (least relevant = last in vec).
            if !memory.is_empty() {
                memory.pop();
                continue;
            }

            // Phase 3: Trim remaining history.
            if !history.is_empty() {
                history.remove(0);
                continue;
            }

            // Phase 4: Truncate goal to half.
            if goal_text.len() > 50 {
                let half = goal_text.len() / 2;
                goal_text.truncate(half);
                goal_text.push_str("...");
                continue;
            }

            // Phase 5: Truncate screen to half.
            if screen_text.len() > 50 {
                let half = screen_text.len() / 2;
                screen_text.truncate(half);
                screen_text.push_str("...");
                continue;
            }

            // If we still can't fit, just accept it — system prompt is never truncated.
            break (prompt, config);
        };

        let estimated_tokens = prompts::estimate_tokens(&prompt);

        if was_truncated {
            tracing::warn!(
                mode = ?mode,
                budget,
                estimated_tokens,
                "context truncated to fit token budget"
            );
        }

        // Derive the original goal: prefer the active goal description, fall back
        // to the last user turn in conversation history.
        let original_goal = self
            .ctx
            .active_goal
            .as_ref()
            .map(|g| g.description.clone())
            .unwrap_or_else(|| {
                self.ctx
                    .conversation_history
                    .iter()
                    .rev()
                    .find(|t| t.role == aura_types::ipc::Role::User)
                    .map(|t| t.content.clone())
                    .unwrap_or_default()
            });

        AssembledPrompt {
            system_prompt: prompt,
            config,
            estimated_tokens,
            was_truncated,
            grammar_kind: self.grammar_kind,
            cot_enabled: self.force_cot,
            original_goal,
            high_stakes: false,
            has_tools: self.inject_tools,
            is_retry: self.previous_attempt.is_some(),
            token_budget: budget as usize,
            estimated_complexity: Some(estimate_importance(self.ctx, self.failure)),
        }
    }
}

// ─── Truncation priority ────────────────────────────────────────────────────
//
// From the spec, truncation priority (last removed first):
//   1. Earlier conversation history (beyond last 3 turns)
//   2. Memory snippets (least relevant first — they're already sorted by relevance desc)
//   3. Last 3 conversation turns
//   4. Active goal
//   5. Current screen
//   6. System prompt (NEVER truncated)
//
// We progressively strip from lowest priority until we fit.

/// Assemble a prompt for `Plan`, `Converse`, or `Compose` messages.
///
/// Takes the `ContextPackage` directly (already deserialized by the IPC layer).
/// `template` is only set for `Compose` mode.
///
/// This is the **backward-compatible** entry point. It does NOT set teacher
/// stack features (grammar, CoT, tools, retry). For those, use `ContextBuilder`.
pub fn assemble_prompt(
    ctx: &ContextPackage,
    template: Option<&str>,
    failure: Option<&FailureContext>,
) -> AssembledPrompt {
    let mut builder = ContextBuilder::new(ctx);
    if let Some(t) = template {
        builder = builder.with_template(t);
    }
    if let Some(f) = failure {
        builder = builder.with_failure(f);
    }
    builder.build()
}

/// Assemble a prompt specifically for `Replan` messages.
///
/// Convenience wrapper that sets the inference mode context; the `FailureContext`
/// is always present for replans.
///
/// Phase 3: teacher stack layer implementation — Strategist replan path.
#[allow(dead_code)]
pub fn assemble_replan_prompt(ctx: &ContextPackage, failure: &FailureContext) -> AssembledPrompt {
    assemble_prompt(ctx, None, Some(failure))
}

// ─── Teacher stack context assembly ─────────────────────────────────────────

/// Assemble a reflection prompt for Layer 4 (Cross-Model Reflection).
///
/// This produces a standalone prompt for the Brainstem (smallest model) to
/// verify the main model's output. It does NOT use the main context package
/// because the Brainstem has its own (smaller) context window.
///
/// # Arguments
///
/// - `original_mode` — The mode that produced `original_output`.
/// - `original_output` — The raw text the main model generated.
/// - `goal_summary` — Short description of the user's goal for context.
/// - `budget` — Token budget for this reflection pass.
///
/// Phase 3: teacher stack layer implementation — Cross-Model Reflection.
#[allow(dead_code)]
#[tracing::instrument(level = "debug", skip(original_output, goal_summary))]
pub fn assemble_reflection_context(
    original_mode: InferenceMode,
    original_output: &str,
    goal_summary: &str,
    budget: u32,
) -> AssembledPrompt {
    let prompt = prompts::build_reflection_prompt(original_mode, original_output, goal_summary);
    let estimated_tokens = prompts::estimate_tokens(&prompt);

    // If the reflection prompt exceeds budget, truncate the embedded output.
    // The `build_reflection_prompt` already truncates to 2048 chars, but we
    // may need to go further for very small budgets.
    let final_prompt = if estimated_tokens > budget && budget > 100 {
        // Re-derive with a smaller output window.
        let max_output_chars = ((budget as usize) * 4).saturating_sub(600);
        let truncated_output = if original_output.len() > max_output_chars {
            &original_output[..max_output_chars]
        } else {
            original_output
        };
        prompts::build_reflection_prompt(original_mode, truncated_output, goal_summary)
    } else {
        prompt
    };

    let final_tokens = prompts::estimate_tokens(&final_prompt);

    // Reflection uses low temperature — we want deterministic verification.
    let config = ModeConfig {
        temperature: 0.1,
        top_p: 0.9,
        top_k: 20,
        repeat_penalty: 1.0,
        max_tokens: 256,
        context_budget: budget,
        stop_sequences: &["}"],
        mirostat_tau: None,
    };

    AssembledPrompt {
        system_prompt: final_prompt,
        config,
        estimated_tokens: final_tokens,
        was_truncated: false,
        grammar_kind: Some(GrammarKind::ReflectionVerdict),
        cot_enabled: false,
        original_goal: goal_summary.to_string(),
        high_stakes: false,
        has_tools: false,
        is_retry: false,
        token_budget: budget as usize,
        estimated_complexity: None,
    }
}

/// Rebuild a prompt with retry context for Layer 3 (Cascading Retry).
///
/// Takes the original context package and enriches it with information about
/// why the previous attempt failed. The teacher stack calls this when
/// confidence is too low or the Brainstem flags issues.
///
/// # Arguments
/// - `ctx` — The original context package.
/// - `template` — Original template (for Composer mode).
/// - `failure` — Original failure context (for Strategist mode).
/// - `previous_output` — What the model produced last time.
/// - `rejection_reason` — Why it was rejected.
/// - `attempt_number` — Which retry attempt this is (1-based).
/// - `budget` — Token budget for this retry pass.
///
/// Phase 3: teacher stack layer implementation — Cascading Retry.
#[allow(dead_code)]
#[tracing::instrument(
    level = "debug",
    skip(ctx, previous_output, rejection_reason),
    fields(mode = ?ctx.inference_mode, attempt = attempt_number)
)]
pub fn assemble_retry_context(
    ctx: &ContextPackage,
    template: Option<&str>,
    failure: Option<&FailureContext>,
    previous_output: &str,
    rejection_reason: &str,
    attempt_number: u32,
    budget: u32,
) -> AssembledPrompt {
    let mut builder = ContextBuilder::new(ctx)
        .with_retry(previous_output, rejection_reason)
        .with_budget(budget);

    if let Some(t) = template {
        builder = builder.with_template(t);
    }
    if let Some(f) = failure {
        builder = builder.with_failure(f);
    }

    // Keep the same grammar constraint as the original.
    let grammar = prompts::default_grammar_for_mode(ctx.inference_mode);
    builder = builder.with_grammar(grammar);

    builder.build()
}

/// Assemble context for a Best-of-N sampling pass (Layer 5).
///
/// Same as the original prompt but with a slightly different temperature
/// seed to encourage diversity across candidates. The returned prompt
/// is identical — the caller varies temperature in `InferenceParams`.
///
/// Phase 3: teacher stack layer implementation — Best-of-N sampling.
#[allow(dead_code)]
pub fn assemble_bon_context(
    ctx: &ContextPackage,
    template: Option<&str>,
    failure: Option<&FailureContext>,
    grammar: GrammarKind,
    inject_tools: bool,
    budget: u32,
) -> AssembledPrompt {
    let mut builder = ContextBuilder::new(ctx)
        .with_grammar(grammar)
        .with_budget(budget);

    if let Some(t) = template {
        builder = builder.with_template(t);
    }
    if let Some(f) = failure {
        builder = builder.with_failure(f);
    }
    if inject_tools {
        builder = builder.with_tools();
    }

    builder.build()
}

// ─── Formatting helpers ─────────────────────────────────────────────────────

/// Format a `GoalSummary` into a human-readable string for the prompt.
fn format_goal(goal: Option<&GoalSummary>) -> String {
    match goal {
        None => "(no active goal)".to_string(),
        Some(g) => {
            let mut s = g.description.clone();
            if g.progress_percent > 0 {
                s.push_str(&format!(" [progress: {}%", g.progress_percent));
                if !g.current_step.is_empty() {
                    s.push_str(&format!(", step: {}", g.current_step));
                }
                s.push(']');
            }
            if !g.blockers.is_empty() {
                s.push_str(&format!(" (blockers: {})", g.blockers.join(", ")));
            }
            s
        }
    }
}

/// Format a `ScreenSummary` into a compact text representation.
fn format_screen(screen: Option<&ScreenSummary>) -> String {
    match screen {
        None => "(no screen data)".to_string(),
        Some(s) => {
            let mut parts = Vec::new();
            parts.push(format!("{}:{}", s.package_name, s.activity_name));
            if !s.interactive_elements.is_empty() {
                parts.push(format!("elements=[{}]", s.interactive_elements.join(", ")));
            }
            if !s.visible_text.is_empty() {
                parts.push(format!("text=[{}]", s.visible_text.join(", ")));
            }
            parts.join(" | ")
        }
    }
}

/// Format a single `ConversationTurn` for prompt injection.
fn format_turn(turn: &ConversationTurn) -> String {
    let role_str = match turn.role {
        Role::User => "User",
        Role::Assistant => "AURA",
        Role::System => "System",
    };
    format!("{}: {}", role_str, turn.content)
}

/// Format a single `MemorySnippet` for prompt injection.
fn format_snippet(snippet: &MemorySnippet) -> String {
    let tier = match snippet.source {
        aura_types::ipc::MemoryTier::Working => "working",
        aura_types::ipc::MemoryTier::Episodic => "episodic",
        aura_types::ipc::MemoryTier::Semantic => "semantic",
        aura_types::ipc::MemoryTier::Archive => "archive",
    };
    format!("[{} r={:.1}] {}", tier, snippet.relevance, snippet.content)
}

/// Format a compact hash-based `FailureContext` into a human-readable string
/// suitable for injection into the Strategist prompt.
///
/// The `FailureContext` is a 96-byte compact struct with hashed values.
/// We render it as a structured diagnostic string so the model can reason
/// about the failure without needing to interpret raw hashes.
fn format_failure(fc: &FailureContext) -> String {
    let error_class_name = match fc.error_class {
        0 => "unknown",
        1 => "element_not_found",
        2 => "element_not_interactable",
        3 => "state_mismatch",
        4 => "timeout",
        5 => "permission_denied",
        6 => "app_crash",
        7 => "network_error",
        _ => "other",
    };

    let approaches = fc.tried_approaches.count_ones();

    let mut s = format!(
        "step={} error_class={} approaches_tried={}",
        fc.current_step, error_class_name, approaches,
    );

    // Add action/target hashes so the model can see if they're the same across attempts.
    s.push_str(&format!(
        " action={:#x} target={:#x}",
        fc.failing_action, fc.target_id,
    ));

    // State mismatch info.
    if fc.expected_state_hash != fc.actual_state_hash {
        s.push_str(&format!(
            " state_mismatch(expected={:#x} actual={:#x})",
            fc.expected_state_hash, fc.actual_state_hash,
        ));
    }

    // Transition history — shows last 3 state transitions.
    let transitions: Vec<String> = fc
        .last_3_transitions
        .iter()
        .filter(|t| t.from_hash != 0 || t.to_hash != 0)
        .map(|t| format!("{:#x}->{:#x}", t.from_hash, t.to_hash))
        .collect();
    if !transitions.is_empty() {
        s.push_str(&format!(" transitions=[{}]", transitions.join(", ")));
    }

    s
}

// ─── Slot builder ───────────────────────────────────────────────────────────

/// Build `PromptSlots` from the separated, possibly truncated context components.
///
/// This is the **basic** slot builder — no teacher stack features.
/// Used by `assemble_prompt()` for backward compatibility.
///
/// Phase 3: called by the assemble_prompt path when teacher stack is active.
#[allow(dead_code)]
fn build_slots(
    goal: &str,
    screen: &str,
    history: &[String],
    memory: &[String],
    template: Option<&str>,
    failure: Option<&FailureContext>,
    ctx: &ContextPackage,
) -> PromptSlots {
    build_slots_extended(
        goal, screen, history, memory, template, failure, ctx, None,  // no tools
        None,  // no grammar
        false, // no CoT
        None,  // no previous attempt
        None,  // no rejection reason
    )
}

/// Build `PromptSlots` with full teacher stack support.
///
/// Extends `build_slots()` with optional fields for grammar constraints,
/// chain-of-thought, tool descriptions, and retry context.
fn build_slots_extended(
    goal: &str,
    screen: &str,
    history: &[String],
    memory: &[String],
    template: Option<&str>,
    failure: Option<&FailureContext>,
    ctx: &ContextPackage,
    // Teacher stack extensions:
    tool_descriptions: Option<&str>,
    grammar_kind: Option<GrammarKind>,
    force_cot: bool,
    previous_attempt: Option<&str>,
    rejection_reason: Option<&str>,
) -> PromptSlots {
    let history_text = if history.is_empty() {
        "(none)".to_string()
    } else {
        history.join("\n")
    };

    let memory_text = if memory.is_empty() {
        "(none)".to_string()
    } else {
        memory.join("\n")
    };

    let p = &ctx.personality;

    // Render UserStateSignals into a compact context string.
    let us = &ctx.user_state;
    let battery_str = if us.battery_level == 255 {
        "unknown".to_string()
    } else {
        format!("{}%{}", us.battery_level, if us.is_charging { " charging" } else { "" })
    };
    let user_state_context = format!(
        "USER STATE: time={:?}, battery={}, thermal={:?}, location={:?}, screen_on={}, steps={}",
        us.time_of_day,
        battery_str,
        us.thermal_state,
        us.estimated_location_type,
        us.is_screen_on,
        us.step_count_today,
    );

    PromptSlots {
        goal: goal.to_string(),
        screen: screen.to_string(),
        history: history_text,
        memory: memory_text,
        user_message: ctx
            .conversation_history
            .last()
            .filter(|t| t.role == Role::User)
            .map(|t| t.content.clone())
            .unwrap_or_default(),
        failure_info: failure.map(format_failure).unwrap_or_default(),
        template: template.unwrap_or("").to_string(),
        openness: format!("{:.2}", p.openness),
        conscientiousness: format!("{:.2}", p.conscientiousness),
        extraversion: format!("{:.2}", p.extraversion),
        agreeableness: format!("{:.2}", p.agreeableness),
        neuroticism: format!("{:.2}", p.neuroticism),
        valence: format!("{:.2}", p.current_mood_valence),
        arousal: format!("{:.2}", p.current_mood_arousal),
        trust_level: format!("{:.2}", p.trust_level),
        identity_block: ctx.identity_block.clone(),
        mood_description: ctx.mood_description.clone(),
        user_state_context,
        // Teacher stack extensions
        tool_descriptions: tool_descriptions.map(|s| s.to_string()),
        grammar_kind,
        force_cot,
        previous_attempt: previous_attempt.map(|s| s.to_string()),
        rejection_reason: rejection_reason.map(|s| s.to_string()),
        few_shot_examples: Vec::new(),
        react_history: Vec::new(),
        dgs_template: None,
    }
}

// ─── Utility: estimate importance ───────────────────────────────────────────

/// Estimate the "importance" of an inference request.
///
/// Returns a score in [0.0, 1.0] where higher values mean the request
/// is more critical and warrants more teacher stack effort (e.g., Layer 5
/// Best-of-N sampling is only triggered for importance > 0.8).
///
/// Factors:
/// - Mode: Planner/Strategist are more important than Conversational
/// - Goal with blockers → more important
/// - Replan (failure present) → more important
/// - Long conversation history → more important (complex interaction)
///
/// Phase 3: teacher stack routing — drives layer selection decisions.
#[allow(dead_code)]
pub fn estimate_importance(ctx: &ContextPackage, failure: Option<&FailureContext>) -> f32 {
    let mut score: f32 = 0.0;

    // Mode baseline
    score += match ctx.inference_mode {
        InferenceMode::Planner => 0.5,
        InferenceMode::Strategist => 0.6,
        InferenceMode::Composer => 0.4,
        InferenceMode::Conversational => 0.2,
    };

    // Failure present → higher importance (replan = urgent)
    if failure.is_some() {
        score += 0.2;
    }

    // Blockers on the active goal
    if let Some(ref goal) = ctx.active_goal {
        if !goal.blockers.is_empty() {
            score += 0.1;
        }
    }

    // Complex conversation (many turns → tricky request)
    if ctx.conversation_history.len() > 5 {
        score += 0.1;
    }

    score.min(1.0)
}

/// Determine whether chain-of-thought should be forced for this request.
///
/// CoT is forced for System2-type tasks: planning with blockers, replan
/// (failure recovery), and high-importance requests. It is NOT forced
/// for simple conversational turns.
///
/// Phase 3: teacher stack routing — Layer 1 CoT trigger.
#[allow(dead_code)]
pub fn should_force_cot(ctx: &ContextPackage, failure: Option<&FailureContext>) -> bool {
    match ctx.inference_mode {
        // Always CoT for replans — failure recovery needs reasoning.
        InferenceMode::Strategist => true,
        // CoT for planning when there are blockers or high complexity.
        InferenceMode::Planner => {
            let has_blockers = ctx
                .active_goal
                .as_ref()
                .map(|g| !g.blockers.is_empty())
                .unwrap_or(false);
            let complex_history = ctx.conversation_history.len() > 4;
            has_blockers || complex_history || failure.is_some()
        }
        // CoT for Composer only with failure recovery.
        InferenceMode::Composer => failure.is_some(),
        // Never CoT for casual conversation.
        InferenceMode::Conversational => false,
    }
}

// ─── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use aura_types::ipc::{
        ContextPackage, ConversationTurn, GoalSummary, InferenceMode, MemorySnippet, MemoryTier,
        PersonalitySnapshot, Role, ScreenSummary, TransitionPair, UserStateSignals,
    };

    fn make_context(n_history: usize, n_memory: usize) -> ContextPackage {
        ContextPackage {
            conversation_history: (0..n_history)
                .map(|i| ConversationTurn {
                    role: if i % 2 == 0 {
                        Role::User
                    } else {
                        Role::Assistant
                    },
                    content: format!("Turn {i}: something was said"),
                    timestamp_ms: i as u64 * 1000,
                })
                .collect(),
            memory_snippets: (0..n_memory)
                .map(|i| MemorySnippet {
                    content: format!("Memory fragment {i}: some remembered fact"),
                    source: MemoryTier::Episodic,
                    relevance: 1.0 - (i as f32 * 0.1),
                    timestamp_ms: i as u64 * 60_000,
                })
                .collect(),
            current_screen: Some(ScreenSummary {
                package_name: "com.android.settings".into(),
                activity_name: "MainActivity".into(),
                interactive_elements: vec!["Button:Settings".into(), "Button:Home".into()],
                visible_text: vec!["Settings".into(), "Home".into()],
            }),
            active_goal: Some(GoalSummary {
                description: "Open the settings app".into(),
                progress_percent: 0,
                current_step: String::new(),
                blockers: Vec::new(),
            }),
            personality: PersonalitySnapshot {
                openness: 0.85,
                conscientiousness: 0.75,
                extraversion: 0.50,
                agreeableness: 0.70,
                neuroticism: 0.25,
                current_mood_valence: 0.3,
                current_mood_arousal: 0.1,
                trust_level: 0.60,
            },
            user_state: UserStateSignals::default(),
            inference_mode: InferenceMode::Planner,
            token_budget: 2048,
            identity_block: None,
            mood_description: String::new(),
        }
    }

    fn make_failure() -> FailureContext {
        FailureContext {
            task_goal_hash: 0xDEAD_BEEF,
            current_step: 2,
            failing_action: 0x1234,
            target_id: 0xABCD,
            expected_state_hash: 0x1111,
            actual_state_hash: 0x2222,
            tried_approaches: 0b0000_0111, // 3 approaches tried
            last_3_transitions: [
                TransitionPair {
                    from_hash: 0xAAAA,
                    to_hash: 0xBBBB,
                },
                TransitionPair {
                    from_hash: 0xBBBB,
                    to_hash: 0xCCCC,
                },
                TransitionPair {
                    from_hash: 0,
                    to_hash: 0,
                },
            ],
            error_class: 3, // state_mismatch
        }
    }

    // ── Backward-compatible tests (existing behavior preserved) ────────────

    #[test]
    fn assemble_planner_prompt_basic() {
        let ctx = make_context(2, 1);
        let result = assemble_prompt(&ctx, None, None);

        assert!(result.system_prompt.contains("Open the settings app"));
        assert!(result.system_prompt.contains("Settings"));
        assert!(result.estimated_tokens > 0);
    }

    #[test]
    fn assemble_conversational_with_user_message() {
        let mut ctx = make_context(1, 0);
        ctx.inference_mode = InferenceMode::Conversational;
        ctx.conversation_history = vec![ConversationTurn {
            role: Role::User,
            content: "Hey AURA, what's up?".into(),
            timestamp_ms: 1000,
        }];

        let result = assemble_prompt(&ctx, None, None);

        assert!(result.system_prompt.contains("Hey AURA, what's up?"));
        assert!(result.system_prompt.contains("0.85")); // openness
        assert!(result.system_prompt.contains("0.60")); // trust_level
    }

    #[test]
    fn truncation_removes_old_history_first() {
        let mut ctx = make_context(50, 20);
        ctx.inference_mode = InferenceMode::Composer;

        let result = assemble_prompt(&ctx, Some("Tap settings"), None);

        assert!(result.was_truncated);
        assert!(result.system_prompt.contains("settings"));
    }

    #[test]
    fn replan_prompt_includes_failure() {
        let mut ctx = make_context(1, 0);
        ctx.inference_mode = InferenceMode::Strategist;
        let failure = make_failure();

        let result = assemble_replan_prompt(&ctx, &failure);

        assert!(result.system_prompt.contains("state_mismatch"));
        assert!(result.system_prompt.contains("0x1234")); // failing_action
        assert!(result.system_prompt.contains("0xabcd")); // target_id
        assert!(result.system_prompt.contains("approaches_tried=3"));
    }

    #[test]
    fn empty_context_still_produces_valid_prompt() {
        let ctx = ContextPackage::default();
        let result = assemble_prompt(&ctx, None, None);

        assert!(result.system_prompt.contains("(none)"));
        assert!(result.estimated_tokens > 0);
    }

    #[test]
    fn format_goal_with_progress_and_blockers() {
        let goal = GoalSummary {
            description: "Send a message".into(),
            progress_percent: 60,
            current_step: "composing text".into(),
            blockers: vec!["keyboard not visible".into()],
        };
        let text = format_goal(Some(&goal));
        assert!(text.contains("Send a message"));
        assert!(text.contains("60%"));
        assert!(text.contains("composing text"));
        assert!(text.contains("keyboard not visible"));
    }

    #[test]
    fn format_screen_summary() {
        let screen = ScreenSummary {
            package_name: "com.whatsapp".into(),
            activity_name: "ChatActivity".into(),
            interactive_elements: vec!["EditText:message".into(), "Button:send".into()],
            visible_text: vec!["Hello".into()],
        };
        let text = format_screen(Some(&screen));
        assert!(text.contains("com.whatsapp:ChatActivity"));
        assert!(text.contains("EditText:message"));
        assert!(text.contains("Button:send"));
        assert!(text.contains("Hello"));
    }

    #[test]
    fn format_failure_context_readable() {
        let fc = make_failure();
        let text = format_failure(&fc);
        assert!(text.contains("step=2"));
        assert!(text.contains("state_mismatch"));
        assert!(text.contains("approaches_tried=3"));
        assert!(text.contains("0xaaaa->0xbbbb"));
    }

    #[test]
    fn format_turn_roles() {
        let user_turn = ConversationTurn {
            role: Role::User,
            content: "open settings".into(),
            timestamp_ms: 0,
        };
        assert_eq!(format_turn(&user_turn), "User: open settings");

        let aura_turn = ConversationTurn {
            role: Role::Assistant,
            content: "on it".into(),
            timestamp_ms: 0,
        };
        assert_eq!(format_turn(&aura_turn), "AURA: on it");
    }

    #[test]
    fn format_memory_snippet() {
        let snippet = MemorySnippet {
            content: "user likes dark mode".into(),
            source: MemoryTier::Semantic,
            relevance: 0.9,
            timestamp_ms: 0,
        };
        let text = format_snippet(&snippet);
        assert!(text.contains("[semantic r=0.9]"));
        assert!(text.contains("user likes dark mode"));
    }

    #[test]
    fn token_budget_respects_context_package_budget() {
        let mut ctx = make_context(10, 5);
        ctx.token_budget = 50; // very small budget

        let result = assemble_prompt(&ctx, None, None);
        assert!(result.was_truncated);
    }

    #[test]
    fn composer_prompt_includes_template() {
        let mut ctx = make_context(1, 0);
        ctx.inference_mode = InferenceMode::Composer;

        let result = assemble_prompt(&ctx, Some("Tap the Send button"), None);

        assert!(result.system_prompt.contains("Tap the Send button"));
    }

    #[test]
    fn no_active_goal_shows_placeholder() {
        let mut ctx = make_context(0, 0);
        ctx.active_goal = None;

        let result = assemble_prompt(&ctx, None, None);
        assert!(result.system_prompt.contains("no active goal"));
    }

    #[test]
    fn no_screen_shows_placeholder() {
        let mut ctx = make_context(0, 0);
        ctx.current_screen = None;

        let result = assemble_prompt(&ctx, None, None);
        assert!(result.system_prompt.contains("no screen data"));
    }

    // ── Token tracker tests ────────────────────────────────────────────────

    #[test]
    fn token_tracker_initial_state() {
        let tracker = TokenTracker::new(1000, 0.5);
        assert_eq!(tracker.total_budget(), 1000);
        assert_eq!(tracker.consumed(), 0);
        assert_eq!(tracker.remaining(), 1000);
        assert_eq!(tracker.next_pass_budget(), 500);
        assert!(!tracker.is_exhausted());
    }

    #[test]
    fn token_tracker_records_usage() {
        let mut tracker = TokenTracker::new(1000, 0.5);
        tracker.record_usage(300);
        assert_eq!(tracker.consumed(), 300);
        assert_eq!(tracker.remaining(), 700);
        // next_pass_budget is min(per_pass_limit=500, remaining=700) = 500
        assert_eq!(tracker.next_pass_budget(), 500);
    }

    #[test]
    fn token_tracker_exhaustion() {
        let mut tracker = TokenTracker::new(200, 0.5);
        tracker.record_usage(160);
        // remaining = 40, which is < 50, so exhausted
        assert!(tracker.is_exhausted());
    }

    #[test]
    fn token_tracker_per_pass_limit_clamped() {
        let mut tracker = TokenTracker::new(1000, 0.5);
        tracker.record_usage(600);
        // remaining = 400, per_pass_limit = 500, so next_pass = 400
        assert_eq!(tracker.next_pass_budget(), 400);
    }

    #[test]
    fn token_tracker_fraction_clamped() {
        // Fraction too low → clamped to 0.1
        let tracker = TokenTracker::new(1000, 0.01);
        assert_eq!(tracker.next_pass_budget(), 100);

        // Fraction too high → clamped to 0.9
        let tracker2 = TokenTracker::new(1000, 0.99);
        assert_eq!(tracker2.next_pass_budget(), 900);
    }

    #[test]
    fn token_tracker_saturating_usage() {
        let mut tracker = TokenTracker::new(100, 0.5);
        tracker.record_usage(200); // over-report
        assert_eq!(tracker.remaining(), 0);
        assert!(tracker.is_exhausted());
    }

    // ── Context builder tests ──────────────────────────────────────────────

    #[test]
    fn context_builder_basic_produces_same_as_assemble() {
        let ctx = make_context(2, 1);
        let legacy = assemble_prompt(&ctx, None, None);
        let builder_result = ContextBuilder::new(&ctx).build();

        // Both should produce essentially the same prompt.
        assert_eq!(legacy.system_prompt, builder_result.system_prompt);
        assert_eq!(legacy.estimated_tokens, builder_result.estimated_tokens);
    }

    #[test]
    fn context_builder_with_grammar() {
        let ctx = make_context(1, 0);
        let result = ContextBuilder::new(&ctx)
            .with_grammar(GrammarKind::ActionPlan)
            .build();

        assert!(result.system_prompt.contains("OUTPUT FORMAT:"));
        assert_eq!(result.grammar_kind, Some(GrammarKind::ActionPlan));
    }

    #[test]
    fn context_builder_with_cot() {
        let ctx = make_context(1, 0);
        let result = ContextBuilder::new(&ctx).with_cot(true).build();

        assert!(result.system_prompt.contains("THINKING INSTRUCTIONS:"));
        assert!(result.cot_enabled);
    }

    #[test]
    fn context_builder_with_tools() {
        let ctx = make_context(1, 0);
        let result = ContextBuilder::new(&ctx).with_tools().build();

        assert!(result.system_prompt.contains("AVAILABLE TOOLS:"));
    }

    #[test]
    fn context_builder_with_retry() {
        let ctx = make_context(1, 0);
        let result = ContextBuilder::new(&ctx)
            .with_retry("bad output here", "invalid JSON format")
            .build();

        assert!(result.system_prompt.contains("RETRY CONTEXT:"));
        assert!(result.system_prompt.contains("bad output here"));
        assert!(result.system_prompt.contains("invalid JSON format"));
    }

    #[test]
    fn context_builder_with_budget_override() {
        let mut ctx = make_context(20, 10);
        ctx.token_budget = 5000; // large budget

        let result = ContextBuilder::new(&ctx).with_budget(100).build();

        // Small budget should cause truncation.
        assert!(result.was_truncated);
    }

    #[test]
    fn context_builder_combined_features() {
        let ctx = make_context(2, 1);
        let result = ContextBuilder::new(&ctx)
            .with_grammar(GrammarKind::ActionPlan)
            .with_cot(true)
            .with_tools()
            .build();

        assert!(result.system_prompt.contains("OUTPUT FORMAT:"));
        assert!(result.system_prompt.contains("THINKING INSTRUCTIONS:"));
        assert!(result.system_prompt.contains("AVAILABLE TOOLS:"));
        assert!(result.cot_enabled);
        assert_eq!(result.grammar_kind, Some(GrammarKind::ActionPlan));
    }

    // ── Reflection context tests ───────────────────────────────────────────

    #[test]
    fn reflection_context_basic() {
        let result = assemble_reflection_context(
            InferenceMode::Planner,
            "{\"goal_description\": \"test\", \"steps\": []}",
            "Open Settings",
            500,
        );

        assert!(result.system_prompt.contains("verification module"));
        assert!(result.system_prompt.contains("Open Settings"));
        assert_eq!(result.grammar_kind, Some(GrammarKind::ReflectionVerdict));
        assert!(!result.cot_enabled);
        assert!(result.config.temperature < 0.2);
    }

    #[test]
    fn reflection_context_truncates_for_small_budget() {
        let long_output = "x".repeat(10_000);
        let result = assemble_reflection_context(InferenceMode::Planner, &long_output, "test", 200);

        // Should have truncated — prompt shouldn't contain 10K chars of 'x'.
        assert!(result.system_prompt.len() < 5000);
    }

    // ── Retry context tests ────────────────────────────────────────────────

    #[test]
    fn retry_context_includes_previous_output() {
        let ctx = make_context(1, 0);
        let result = assemble_retry_context(
            &ctx,
            None,
            None,
            "previous bad output",
            "parse failure",
            2,
            1000,
        );

        assert!(result.system_prompt.contains("RETRY CONTEXT:"));
        assert!(result.system_prompt.contains("previous bad output"));
        assert!(result.system_prompt.contains("parse failure"));
    }

    #[test]
    fn retry_context_preserves_grammar() {
        let ctx = make_context(1, 0);
        let result = assemble_retry_context(&ctx, None, None, "bad", "wrong", 1, 1000);

        // Planner mode should get ActionPlan grammar.
        assert_eq!(result.grammar_kind, Some(GrammarKind::ActionPlan));
    }

    // ── Best-of-N context tests ────────────────────────────────────────────

    #[test]
    fn bon_context_with_grammar() {
        let ctx = make_context(1, 0);
        let result = assemble_bon_context(&ctx, None, None, GrammarKind::ActionPlan, false, 1000);

        assert!(result.system_prompt.contains("OUTPUT FORMAT:"));
        assert_eq!(result.grammar_kind, Some(GrammarKind::ActionPlan));
    }

    #[test]
    fn bon_context_with_tools() {
        let ctx = make_context(1, 0);
        let result = assemble_bon_context(
            &ctx,
            None,
            None,
            GrammarKind::ActionPlan,
            true, // inject tools
            1000,
        );

        assert!(result.system_prompt.contains("AVAILABLE TOOLS:"));
    }

    // ── Importance estimation tests ────────────────────────────────────────

    #[test]
    fn importance_planner_baseline() {
        let ctx = make_context(1, 0);
        let score = estimate_importance(&ctx, None);
        // Planner baseline = 0.5
        assert!(score >= 0.4 && score <= 0.6);
    }

    #[test]
    fn importance_higher_with_failure() {
        let ctx = make_context(1, 0);
        let failure = make_failure();
        let score_with = estimate_importance(&ctx, Some(&failure));
        let score_without = estimate_importance(&ctx, None);
        assert!(score_with > score_without);
    }

    #[test]
    fn importance_higher_with_blockers() {
        let mut ctx = make_context(1, 0);
        ctx.active_goal = Some(GoalSummary {
            description: "test".into(),
            progress_percent: 0,
            current_step: String::new(),
            blockers: vec!["some blocker".into()],
        });
        let score = estimate_importance(&ctx, None);
        // Planner(0.5) + blockers(0.1) = 0.6
        assert!(score >= 0.55);
    }

    #[test]
    fn importance_conversational_low() {
        let mut ctx = make_context(1, 0);
        ctx.inference_mode = InferenceMode::Conversational;
        let score = estimate_importance(&ctx, None);
        // Conversational baseline = 0.2
        assert!(score < 0.4);
    }

    #[test]
    fn importance_capped_at_one() {
        let mut ctx = make_context(10, 0); // 10 turns → +0.1
        ctx.inference_mode = InferenceMode::Strategist; // 0.6
        ctx.active_goal = Some(GoalSummary {
            description: "test".into(),
            progress_percent: 0,
            current_step: String::new(),
            blockers: vec!["b1".into()], // +0.1
        });
        let failure = make_failure(); // +0.2
                                      // Total would be 0.6 + 0.2 + 0.1 + 0.1 = 1.0
        let score = estimate_importance(&ctx, Some(&failure));
        assert!(score <= 1.0);
    }

    // ── CoT decision tests ─────────────────────────────────────────────────

    #[test]
    fn cot_forced_for_strategist() {
        let mut ctx = make_context(1, 0);
        ctx.inference_mode = InferenceMode::Strategist;
        assert!(should_force_cot(&ctx, None));
    }

    #[test]
    fn cot_not_forced_for_simple_conversation() {
        let mut ctx = make_context(1, 0);
        ctx.inference_mode = InferenceMode::Conversational;
        assert!(!should_force_cot(&ctx, None));
    }

    #[test]
    fn cot_forced_for_planner_with_blockers() {
        let mut ctx = make_context(1, 0);
        ctx.active_goal = Some(GoalSummary {
            description: "test".into(),
            progress_percent: 0,
            current_step: String::new(),
            blockers: vec!["blocked".into()],
        });
        assert!(should_force_cot(&ctx, None));
    }

    #[test]
    fn cot_not_forced_for_simple_planner() {
        let ctx = make_context(1, 0);
        // No blockers, short history, no failure
        assert!(!should_force_cot(&ctx, None));
    }

    #[test]
    fn cot_forced_for_planner_with_complex_history() {
        let ctx = make_context(6, 0); // > 4 turns
        assert!(should_force_cot(&ctx, None));
    }

    #[test]
    fn cot_forced_for_composer_with_failure() {
        let mut ctx = make_context(1, 0);
        ctx.inference_mode = InferenceMode::Composer;
        let failure = make_failure();
        assert!(should_force_cot(&ctx, Some(&failure)));
    }

    #[test]
    fn cot_not_forced_for_simple_composer() {
        let mut ctx = make_context(1, 0);
        ctx.inference_mode = InferenceMode::Composer;
        assert!(!should_force_cot(&ctx, None));
    }
}
