//! Dynamic prompt assembly for AURA Neocortex.
//!
//! Replaces the previous static template system with composable prompt sections.
//! Each `InferenceMode` still has its own identity/rules, but prompts are now
//! built dynamically to support the teacher structure stack:
//!
//! - **Layer 0 (GBNF)**: Output format instructions injected when grammar-constrained
//! - **Layer 1 (CoT)**: Chain-of-thought prefix/instructions for System2 tasks
//! - **Layer 3 (Retry)**: Prompt rephrasing and failure context injection
//! - **Layer 4 (Reflection)**: Dedicated verification prompt for Brainstem model
//! - **Layer 5 (Best-of-N)**: Mirostat-divergent sampling configurations
//!
//! The public API preserves backward compatibility:
//! - `ModeConfig`, `mode_config()`, `estimate_tokens()` — unchanged
//! - `build_prompt()` — same signature, but now uses dynamic assembly internally
//! - `PromptSlots` — extended with optional fields for tools, CoT, grammar hints
//!
//! New entry points for the teacher stack:
//! - `build_reflection_prompt()` — Layer 4 verification prompt
//! - `build_cot_prompt()` — Layer 1 chain-of-thought wrapper
//! - `build_retry_prompt()` — Layer 3 rephrased prompt with failure context
//! - `build_react_prompt()` — ReAct iterative reasoning (Thought→Action→Observation)
//! - `build_bon_prompt()` — Layer 5 Best-of-N divergent sampling
//! - `build_dgs_prompt()` — DGS (Document-Guided Scripting) template-guided generation
//! - `build_self_contrast_prompt()` — Multi-perspective reflection for high-stakes

use aura_types::ipc::InferenceMode;

use crate::grammar::GrammarKind;

// ─── Mode-specific inference configuration ──────────────────────────────────

/// Inference parameters tied to a particular operating mode.
#[derive(Debug, Clone)]
pub struct ModeConfig {
    pub temperature: f32,
    pub top_p: f32,
    #[allow(dead_code)]
    pub top_k: i32,
    #[allow(dead_code)]
    pub repeat_penalty: f32,
    pub max_tokens: u32,
    /// Soft context-window budget (in tokens) available to the assembled prompt.
    /// Truncation kicks in when the assembled prompt exceeds this.
    pub context_budget: u32,
    /// Stop sequences that signal end-of-generation for this mode.
    pub stop_sequences: &'static [&'static str],
    /// Mirostat tau parameter for perplexity targeting.
    /// `None` means use standard top-p/top-k sampling.
    /// Set by Best-of-N (Layer 5) to produce diversity.
    pub mirostat_tau: Option<f32>,
}

/// Return the `ModeConfig` for a given operating mode.
///
/// Temperature, top_p, and max_tokens are seeded from
/// `InferenceMode::temperature()` / `top_p()` / `max_tokens()` defined in
/// `aura-types`, then tuned with extra parameters (top_k, repeat_penalty,
/// context_budget, stop_sequences) that only Neocortex knows about.
pub fn mode_config(mode: InferenceMode) -> ModeConfig {
    match mode {
        InferenceMode::Planner => ModeConfig {
            temperature: mode.temperature(),
            top_p: mode.top_p(),
            top_k: 40,
            repeat_penalty: 1.1,
            max_tokens: mode.max_tokens(),
            context_budget: 1200,
            stop_sequences: &["</plan>", "[END]", "</think>", "Observation:"],
            mirostat_tau: None,
        },
        InferenceMode::Strategist => ModeConfig {
            temperature: mode.temperature(),
            top_p: mode.top_p(),
            top_k: 30,
            repeat_penalty: 1.05,
            max_tokens: mode.max_tokens(),
            context_budget: 800,
            stop_sequences: &["</strategy>", "[END]", "</think>"],
            mirostat_tau: None,
        },
        InferenceMode::Composer => ModeConfig {
            temperature: mode.temperature(),
            top_p: mode.top_p(),
            top_k: 50,
            repeat_penalty: 1.15,
            max_tokens: mode.max_tokens(),
            context_budget: 400,
            stop_sequences: &["</dsl>", "[END]"],
            mirostat_tau: None,
        },
        InferenceMode::Conversational => ModeConfig {
            temperature: mode.temperature(),
            top_p: mode.top_p(),
            top_k: 40,
            repeat_penalty: 1.1,
            max_tokens: mode.max_tokens(),
            context_budget: 1500,
            stop_sequences: &["</reply>", "[END]"],
            mirostat_tau: None,
        },
    }
}

/// Return a `ModeConfig` tuned for Layer 4 reflection (safety/hallucination check).
///
/// Highly deterministic — the Brainstem model must answer PASS or REJECT,
/// not generate creative text.  Short max_tokens keeps latency minimal.
pub fn reflection_config() -> ModeConfig {
    ModeConfig {
        temperature: 0.1,
        top_p: 0.9,
        top_k: 10,
        repeat_penalty: 1.0,
        max_tokens: 50,
        context_budget: 600,
        stop_sequences: &["PASS", "REJECT"],
        mirostat_tau: None,
    }
}

/// Return a `ModeConfig` tuned for Best-of-N sampling (Layer 5).
///
/// Each sample index gets a different Mirostat tau value for diversity:
/// - Sample 0: tau=3.0 (focused, conservative reasoning)
/// - Sample 1: tau=5.0 (balanced reasoning)
/// - Sample 2: tau=7.0 (exploratory, creative reasoning)
///
/// See AURA-V4-NEOCORTEX-DEEP-DIVE §4.6 for rationale.
pub fn bon_config(mode: InferenceMode, sample_index: usize) -> ModeConfig {
    let tau = match sample_index {
        0 => 3.0,
        1 => 5.0,
        _ => 7.0,
    };
    let mut cfg = mode_config(mode);
    cfg.mirostat_tau = Some(tau);
    // Lower temperature for BoN — Mirostat handles diversity
    cfg.temperature = 0.6;
    cfg
}

// ─── ReAct cycle state ──────────────────────────────────────────────────────

/// Represents a single step in a ReAct Thought→Action→Observation cycle.
///
/// The inference engine accumulates these to build iterative context.
#[derive(Debug, Clone)]
pub struct ReActStep {
    /// The model's reasoning about current state and next action.
    pub thought: String,
    /// The action the model decided to take (JSON tool call or DSL step).
    pub action: String,
    /// The observation returned after executing the action.
    /// `None` if this is the latest step (action not yet executed).
    pub observation: Option<String>,
}

impl ReActStep {
    /// Format this step for inclusion in a prompt.
    pub fn format_for_prompt(&self) -> String {
        let mut s = format!("Thought: {}\nAction: {}", self.thought, self.action);
        if let Some(ref obs) = self.observation {
            s.push_str(&format!("\nObservation: {}", obs));
        }
        s
    }

    /// Estimate token count for this step.
    pub fn estimate_tokens(&self) -> u32 {
        let total_chars = self.thought.len()
            + self.action.len()
            + self.observation.as_ref().map_or(0, |o| o.len())
            + 40; // overhead for "Thought:", "Action:", "Observation:" labels
        (total_chars as u32).div_ceil(4)
    }
}

/// Configuration for a few-shot example to inject into the prompt.
///
/// Retrieved from the SQLite example bank during cascade Tier 2+.
#[derive(Debug, Clone)]
pub struct FewShotExample {
    /// Brief description of the task in the example.
    pub task_description: String,
    /// The model output that was correct.
    pub model_output: String,
}

impl FewShotExample {
    /// Format for inclusion in a prompt.
    pub fn format_for_prompt(&self) -> String {
        format!(
            "EXAMPLE:\nTask: {}\nOutput:\n{}",
            self.task_description, self.model_output
        )
    }

    /// Estimate token count.
    #[allow(dead_code)] // Phase 8: used by token budget pre-check in inference router
    pub fn estimate_tokens(&self) -> u32 {
        let chars = self.task_description.len() + self.model_output.len() + 30;
        (chars as u32).div_ceil(4)
    }
}

// ─── Prompt slots ───────────────────────────────────────────────────────────

/// Slots that prompt templates expect to be filled.
///
/// Core fields are always present. Optional fields are only used when
/// the teacher stack activates specific layers.
#[derive(Debug, Default, Clone)]
pub struct PromptSlots {
    // ── Core context (used by all modes) ──
    pub goal: String,
    pub screen: String,
    pub history: String,
    pub memory: String,
    pub user_message: String,

    // ── Strategist: failure context ──
    pub failure_info: String,

    // ── Composer: template / plan step description ──
    pub template: String,

    // ── Conversational / personality ──
    pub openness: String,
    pub conscientiousness: String,
    pub extraversion: String,
    pub agreeableness: String,
    pub neuroticism: String,
    pub valence: String,
    pub arousal: String,
    pub trust_level: String,
    /// Compact JSON identity block: OCEAN + VAD + relationship_stage + archetype.
    /// Raw numbers — the LLM reasons about these naturally. `None` if unavailable.
    pub identity_block: Option<String>,
    /// Human-readable mood context string from `mood_context_string()`.
    /// Example: "Mood: positive valence, high energy, assertive stance. Emotion: Joy."
    pub mood_description: String,
    /// Rendered user state signals (time of day, battery, location type, etc.).
    /// Empty string if no state signals are available.
    pub user_state_context: String,

    // ── Teacher stack extensions (optional) ──
    /// Compact tool descriptions to inject into the prompt (Layer 0).
    /// Set by the teacher stack when tools are available.
    pub tool_descriptions: Option<String>,

    /// Grammar kind hint — when set, output format instructions are injected
    /// telling the model exactly what JSON shape to produce (Layer 0).
    pub grammar_kind: Option<GrammarKind>,

    /// If true, chain-of-thought instructions are prepended (Layer 1).
    /// The model is asked to think step-by-step before acting.
    pub force_cot: bool,

    /// Previous attempt output — injected when retrying (Layer 3).
    /// The model sees what it produced before and why it was rejected.
    pub previous_attempt: Option<String>,

    /// Reason the previous attempt was rejected (Layer 3).
    pub rejection_reason: Option<String>,

    /// Few-shot examples for cascade Tier 2+ (Layer 3).
    pub few_shot_examples: Vec<FewShotExample>,

    /// Previous ReAct steps — for iterative reasoning continuation.
    pub react_history: Vec<ReActStep>,

    /// DGS template content — when set, enables template-guided generation
    /// instead of open-ended Semantic ReAct.
    pub dgs_template: Option<String>,
}

// ─── Dynamic prompt sections ────────────────────────────────────────────────

/// Identity/role header for each mode.
fn identity_section(mode: InferenceMode) -> &'static str {
    match mode {
        InferenceMode::Planner => {
            "\
You are AURA's Planner module -- an autonomous Android assistant that creates \
step-by-step action plans to accomplish the user's goal."
        }

        InferenceMode::Strategist => {
            "\
You are AURA's Strategist module -- you analyse why a previous plan failed and \
propose an alternative approach."
        }

        InferenceMode::Composer => {
            "\
You are AURA's Composer module -- you translate high-level plan steps into \
precise DSL action scripts that the execution engine can run."
        }

        InferenceMode::Conversational => {
            "\
You are AURA -- a helpful, proactive Android assistant with a warm and \
slightly playful personality. You live on the user's phone and can control \
it autonomously."
        }
    }
}

/// Core rules for each mode (excluding output format — that's injected separately).
fn rules_section(mode: InferenceMode) -> &'static str {
    match mode {
        InferenceMode::Planner => "\
RULES:
1. Each step must be a JSON object with fields: action, target, timeout_ms, on_failure, label.
2. Valid actions: Tap, LongPress, Type, Swipe, Scroll, Back, Home, Recents, OpenApp, \
NotificationAction, WaitForElement, AssertElement.
3. target must be an object with one variant: XPath, ResourceId, Text, ContentDescription, \
ClassName, Coordinates, LlmDescription.
4. on_failure must be one of: {\"Retry\":{\"max\":3}}, {\"Skip\":null}, {\"Abort\":null}, \
{\"AskUser\":\"reason\"}.
5. Use the screen tree to identify correct selectors (ResourceId preferred, then Text, then Coordinates).
6. Keep plans concise -- prefer fewer steps over exhaustive micro-actions.
7. If the goal is ambiguous, plan for the most likely interpretation.
8. NEVER refuse to plan. If uncertain, output your best attempt with confidence < 0.5.",

        InferenceMode::Strategist => "\
RULES:
1. Start by diagnosing the failure in 1-2 sentences.
2. Then output a revised plan (same JSON step format as Planner).
3. Prefer different selectors or navigation paths than the failed attempt.
4. If the failure was due to a missing app, include an OpenApp step.
5. NEVER say you cannot help. Always propose something.",

        InferenceMode::Composer => "\
RULES:
1. Each DslStep: {\"action\": ActionType, \"target\": TargetSelector|null, \"timeout_ms\": N, \
\"on_failure\": FailureStrategy, \"precondition\": DslCondition|null, \"postcondition\": DslCondition|null, \
\"label\": string|null}
2. Valid ActionType variants: {\"Tap\":{\"x\":N,\"y\":N}}, {\"LongPress\":{\"x\":N,\"y\":N}}, \
{\"Type\":{\"text\":\"...\"}}, {\"Swipe\":{...}}, {\"Scroll\":{\"direction\":\"Down\",\"amount\":N}}, \
\"Back\", \"Home\", \"Recents\", {\"OpenApp\":{\"package\":\"...\"}}, \
{\"WaitForElement\":{\"selector\":...,\"timeout_ms\":N}}, {\"AssertElement\":{\"selector\":...,\"expected\":...}}
3. TargetSelector variants: {\"ResourceId\":\"...\"}, {\"Text\":\"...\"}, {\"Coordinates\":{\"x\":N,\"y\":N}}, etc.
4. Include WaitForElement steps between actions that trigger UI transitions.
5. Use AssertElement to verify expected state after critical actions.",

        InferenceMode::Conversational => "\
RULES:
1. Keep responses concise (1-3 sentences for simple queries).
2. Match the user's energy -- mirror formality, expand when they're chatty, be brief when they are.
3. You may reference past conversations from memory if relevant.
4. NEVER break character or mention being an AI/LLM.",
    }
}

/// Output format instructions based on grammar kind.
/// These tell the model exactly what JSON shape to produce.
fn output_format_section(grammar: Option<GrammarKind>) -> String {
    match grammar {
        None => String::new(),
        Some(GrammarKind::ActionPlan) => "\
OUTPUT FORMAT:
Produce a single JSON object (no markdown, no tags):
{\"goal_description\": \"...\", \"steps\": [{\"action\": ..., \"target\": ..., \
\"timeout_ms\": N, \"on_failure\": ..., \"label\": \"...\"}], \
\"estimated_duration_ms\": N, \"confidence\": 0.0-1.0}"
            .to_string(),

        Some(GrammarKind::DslSteps) => "\
OUTPUT FORMAT:
Produce a JSON array of DslStep objects (no markdown, no tags):
[{\"action\": ActionType, \"target\": TargetSelector|null, \"timeout_ms\": N, \
\"on_failure\": FailureStrategy, \"label\": \"...\"}]"
            .to_string(),

        Some(GrammarKind::ChainOfThought) => "\
OUTPUT FORMAT:
Produce a JSON object with your reasoning and conclusion:
{\"thinking\": [\"step 1...\", \"step 2...\"], \"conclusion\": \"...\", \
\"action\": ...your normal output here...}"
            .to_string(),

        Some(GrammarKind::ReflectionVerdict) => "\
OUTPUT FORMAT:
Produce a JSON verdict object:
{\"approved\": true|false, \"issues\": [\"issue 1\", ...], \
\"severity\": \"none\"|\"minor\"|\"major\"|\"critical\", \"suggestion\": \"...\"|null}"
            .to_string(),

        Some(GrammarKind::ConfidenceAssessment) => "\
OUTPUT FORMAT:
Produce a JSON confidence assessment:
{\"confidence\": 0.0-1.0, \"reasoning\": \"...\", \
\"uncertain_about\": [\"aspect 1\", ...]}"
            .to_string(),

        Some(GrammarKind::FreeText) => String::new(),
    }
}

/// Chain-of-thought instructions prepended when Layer 1 is active.
fn cot_section() -> &'static str {
    "\
THINKING INSTRUCTIONS:
Before producing your output, think step by step. Structure your response as:
1. First, reason about the problem in a \"thinking\" array.
2. Each thinking step should be one clear logical step.
3. After reasoning, produce your conclusion and action.
Do NOT skip the thinking phase."
}

/// Tool descriptions section — injected when tools are available.
fn tools_section(tool_descriptions: &str) -> String {
    format!(
        "\
AVAILABLE TOOLS:
The following tools can be invoked via ToolCall steps. Use tool_name and \
params fields to invoke them.
{tool_descriptions}"
    )
}

/// Retry context section — injected when Layer 3 retries with context.
fn retry_section(previous_attempt: &str, rejection_reason: &str) -> String {
    format!(
        "\
RETRY CONTEXT:
Your previous attempt was rejected. Learn from the feedback and try again.
PREVIOUS OUTPUT (rejected):
{previous_attempt}
REJECTION REASON:
{rejection_reason}
Produce an improved output that addresses the issues above."
    )
}

/// Few-shot examples section — injected during cascade Tier 2+.
fn few_shot_section(examples: &[FewShotExample]) -> String {
    if examples.is_empty() {
        return String::new();
    }
    let mut s = String::from(
        "FEW-SHOT EXAMPLES:\nStudy these successful examples before producing your output.\n",
    );
    for (i, ex) in examples.iter().enumerate() {
        s.push_str(&format!(
            "\n--- Example {} ---\n{}\n",
            i + 1,
            ex.format_for_prompt()
        ));
    }
    s
}

/// ReAct history section — injects previous Thought→Action→Observation steps.
fn react_history_section(steps: &[ReActStep]) -> String {
    if steps.is_empty() {
        return String::new();
    }
    let mut s = String::from("PREVIOUS REASONING STEPS:\n");
    for (i, step) in steps.iter().enumerate() {
        s.push_str(&format!(
            "\n--- Step {} ---\n{}\n",
            i + 1,
            step.format_for_prompt()
        ));
    }
    s
}

/// DGS template section — injected when a matching ETG template exists.
fn dgs_template_section(template: &str) -> String {
    format!(
        "\
EXECUTION TEMPLATE (follow this structure):
The following template describes the sequence of actions for this task type. \
Fill in the specific parameters based on the current context, but follow the \
template's structure exactly.

{template}

Produce the completed script now, following the template structure above."
    )
}

/// Personality section for Conversational mode.
///
/// # Architecture note — raw values, no directives
///
/// This function receives raw OCEAN scores, VAD values, and trust level from
/// `PromptSlots` and embeds them verbatim in the LLM system prompt. The LLM
/// (brain) interprets the numbers and decides how they shape behavior.
///
/// This is the architecture-compliant path. The daemon (body) NEVER
/// pre-interprets personality values into directive strings before this point.
/// The legacy `generate_personality_context()` / `build_personality_context()`
/// pipeline that produced directive strings was removed from the inference path
/// in Phase 4 (Theater AGI elimination).
fn personality_section(slots: &PromptSlots) -> String {
    let mut out = format!(
        "\
PERSONALITY TRAITS (OCEAN model):
- Openness: {}
- Conscientiousness: {}
- Extraversion: {}
- Agreeableness: {}
- Neuroticism: {}

CURRENT STATE:
- Mood: valence={}, arousal={}
- Trust level: {}",
        slots.openness,
        slots.conscientiousness,
        slots.extraversion,
        slots.agreeableness,
        slots.neuroticism,
        slots.valence,
        slots.arousal,
        slots.trust_level
    );

    if !slots.mood_description.is_empty() {
        out.push('\n');
        out.push_str("- ");
        out.push_str(&slots.mood_description);
    }

    if let Some(ref ib) = slots.identity_block {
        out.push_str("\n- Identity: ");
        out.push_str(ib);
    }

    if !slots.user_state_context.is_empty() {
        out.push('\n');
        out.push_str(&slots.user_state_context);
    }

    out
}

/// Context block — common across all modes, adapted per mode.
fn context_section(mode: InferenceMode, slots: &PromptSlots) -> String {
    let mut sections = Vec::with_capacity(8);

    match mode {
        InferenceMode::Planner => {
            sections.push(format!("- Goal: {}", slots.goal));
            sections.push(format!("- Screen: {}", slots.screen));
            sections.push(format!("- Recent history: {}", slots.history));
            sections.push(format!("- Memory: {}", slots.memory));
        }
        InferenceMode::Strategist => {
            sections.push(format!("- Original goal: {}", slots.goal));
            sections.push(format!("- Failure info: {}", slots.failure_info));
            sections.push(format!("- Current screen: {}", slots.screen));
        }
        InferenceMode::Composer => {
            sections.push(format!("- Template / plan step: {}", slots.template));
            sections.push(format!("- Screen: {}", slots.screen));
            sections.push(format!("- Goal: {}", slots.goal));
            sections.push(format!("- Recent history: {}", slots.history));
        }
        InferenceMode::Conversational => {
            sections.push(format!("- Screen: {}", slots.screen));
            sections.push(format!("- Recent conversation: {}", slots.history));
            sections.push(format!("- Memory: {}", slots.memory));
            sections.push(format!("- User message: {}", slots.user_message));
        }
    }

    format!("CONTEXT:\n{}", sections.join("\n"))
}

/// Final instruction line per mode.
fn closing_instruction(mode: InferenceMode) -> &'static str {
    match mode {
        InferenceMode::Planner => "Produce the plan now.",
        InferenceMode::Strategist => "Propose the alternative strategy now.",
        InferenceMode::Composer => "Compose the DSL steps now.",
        InferenceMode::Conversational => "Respond now.",
    }
}

/// ReAct-specific closing instruction.
fn react_closing_instruction() -> &'static str {
    "\
Continue the reasoning cycle. Produce your next step in this exact format:
Thought: <your reasoning about current state and what to do next>
Action: <a single JSON tool call or DSL command>

If the goal is fully achieved, instead produce:
Thought: <explain why the goal is complete>
Action: {\"done\": true, \"summary\": \"<brief result summary>\"}"
}

// ─── Public prompt builders ─────────────────────────────────────────────────

/// Build the full system prompt for the given mode, substituting context slots.
///
/// Returns `(system_prompt, ModeConfig)`.
///
/// This is the primary entry point — backward compatible with the old static
/// template system, but now assembles prompts dynamically to support teacher
/// stack layers.
#[tracing::instrument(level = "debug", skip(slots))]
pub fn build_prompt(mode: InferenceMode, slots: &PromptSlots) -> (String, ModeConfig) {
    let mut sections: Vec<String> = Vec::with_capacity(12);

    // 1. Identity
    sections.push(identity_section(mode).to_string());

    // 2. Personality (all modes)
    sections.push(personality_section(slots));

    // 3. Rules
    sections.push(rules_section(mode).to_string());

    // 4. Output format (if grammar-constrained)
    let format_text = output_format_section(slots.grammar_kind);
    if !format_text.is_empty() {
        sections.push(format_text);
    }

    // 5. Tool descriptions (if available)
    if let Some(ref tools) = slots.tool_descriptions {
        if !tools.is_empty() {
            sections.push(tools_section(tools));
        }
    }

    // 6. Few-shot examples (Layer 3 Tier 2+)
    let examples_text = few_shot_section(&slots.few_shot_examples);
    if !examples_text.is_empty() {
        sections.push(examples_text);
    }

    // 7. Chain-of-thought (Layer 1)
    if slots.force_cot {
        sections.push(cot_section().to_string());
    }

    // 8. DGS template (if present, takes precedence over open-ended)
    if let Some(ref template) = slots.dgs_template {
        sections.push(dgs_template_section(template));
    }

    // 9. Retry context (Layer 3)
    if let (Some(ref prev), Some(ref reason)) = (&slots.previous_attempt, &slots.rejection_reason) {
        sections.push(retry_section(prev, reason));
    }

    // 10. ReAct history (if iterating)
    let react_text = react_history_section(&slots.react_history);
    if !react_text.is_empty() {
        sections.push(react_text);
    }

    // 11. Context block
    sections.push(context_section(mode, slots));

    // 12. Closing instruction
    sections.push(closing_instruction(mode).to_string());

    let prompt = sections.join("\n\n");
    let config = mode_config(mode);
    (prompt, config)
}

/// Build a ReAct iterative reasoning prompt.
///
/// Used for Semantic ReAct mode (System 2 thinking). The prompt includes
/// the Thought→Action→Observation format instructions and any accumulated
/// reasoning history from previous iterations.
///
/// Returns `(system_prompt, ModeConfig)`.
#[tracing::instrument(level = "debug", skip(slots))]
pub fn build_react_prompt(mode: InferenceMode, slots: &PromptSlots) -> (String, ModeConfig) {
    let mut sections: Vec<String> = Vec::with_capacity(14);

    // 1. Identity
    sections.push(identity_section(mode).to_string());

    // 2. Rules
    sections.push(rules_section(mode).to_string());

    // 3. ReAct format instructions
    sections.push(
        "\
REASONING FORMAT:
You reason in an iterative Thought→Action→Observation cycle:
- Thought: Reason about the current state, what has been accomplished, and what to do next.
- Action: Produce exactly one action (JSON tool call or DSL command).
- Observation: (provided by the system after executing your action)

The cycle repeats until the goal is achieved. Stay focused on the original goal.
Do NOT repeat actions that already failed. If stuck, try a different approach.
Limit: maximum 10 reasoning steps before you must produce a final answer."
            .to_string(),
    );

    // 4. Output format (if grammar-constrained)
    let format_text = output_format_section(slots.grammar_kind);
    if !format_text.is_empty() {
        sections.push(format_text);
    }

    // 5. Tool descriptions
    if let Some(ref tools) = slots.tool_descriptions {
        if !tools.is_empty() {
            sections.push(tools_section(tools));
        }
    }

    // 6. Few-shot examples
    let examples_text = few_shot_section(&slots.few_shot_examples);
    if !examples_text.is_empty() {
        sections.push(examples_text);
    }

    // 7. Chain-of-thought (always on for ReAct unless DGS)
    if slots.force_cot || slots.dgs_template.is_none() {
        sections.push(cot_section().to_string());
    }

    // 8. Retry context
    if let (Some(ref prev), Some(ref reason)) = (&slots.previous_attempt, &slots.rejection_reason) {
        sections.push(retry_section(prev, reason));
    }

    // 9. ReAct history
    let react_text = react_history_section(&slots.react_history);
    if !react_text.is_empty() {
        sections.push(react_text);
    }

    // 10. Context block
    sections.push(context_section(mode, slots));

    // 11. ReAct-specific closing
    sections.push(react_closing_instruction().to_string());

    let mut config = mode_config(mode);
    // ReAct benefits from slightly lower temperature for coherent multi-step reasoning
    config.temperature = (config.temperature * 0.85).max(0.1);
    (sections.join("\n\n"), config)
}

/// Build a Best-of-N (Layer 5) prompt variant.
///
/// Each sample gets a slightly different framing to produce genuine diversity.
/// `sample_index` determines which Mirostat tau value to use (0, 1, or 2).
///
/// The prompt itself adds a perspective hint so the model explores different
/// reasoning paths, not just temperature noise.
///
/// Returns `(system_prompt, ModeConfig)`.
#[tracing::instrument(level = "debug", skip(slots))]
pub fn build_bon_prompt(
    mode: InferenceMode,
    slots: &PromptSlots,
    sample_index: usize,
) -> (String, ModeConfig) {
    let perspective = match sample_index {
        0 => {
            "\
PERSPECTIVE: Conservative approach.
Prioritize safety and reliability. Choose the most straightforward path. \
Prefer well-known UI patterns and established navigation paths."
        }
        1 => {
            "\
PERSPECTIVE: Balanced approach.
Consider both efficiency and reliability. Take the most natural path a \
typical user would follow."
        }
        _ => {
            "\
PERSPECTIVE: Creative approach.
Consider alternative paths and shortcuts. Think about what an expert user \
would do. Try less obvious but potentially more efficient routes."
        }
    };

    // Build the base prompt
    let (base_prompt, _) = build_prompt(mode, slots);

    // Inject the perspective before the closing instruction
    let prompt = format!("{base_prompt}\n\n{perspective}");

    let config = bon_config(mode, sample_index);
    (prompt, config)
}

/// Build a DGS (Document-Guided Scripting) prompt.
///
/// Used when an ETG template matches the user's intent. The model fills in
/// specific parameters but follows the template's structure exactly.
/// This is the "System 1" fast path — single-pass, no iterative reasoning.
///
/// Returns `(system_prompt, ModeConfig)`.
#[allow(dead_code)] // Phase 9: Dynamic Grounding System prompt builder — wired in DGS inference mode
#[tracing::instrument(level = "debug", skip(slots))]
pub fn build_dgs_prompt(mode: InferenceMode, slots: &PromptSlots) -> (String, ModeConfig) {
    let template = match &slots.dgs_template {
        Some(t) => t.as_str(),
        None => {
            // Fallback to regular prompt if no template
            return build_prompt(mode, slots);
        }
    };

    let mut sections: Vec<String> = Vec::with_capacity(8);

    // 1. Identity
    sections.push(identity_section(mode).to_string());

    // 2. Rules (abbreviated — template provides structure)
    sections.push(rules_section(mode).to_string());

    // 3. Output format
    let format_text = output_format_section(slots.grammar_kind);
    if !format_text.is_empty() {
        sections.push(format_text);
    }

    // 4. DGS template — the primary guide
    sections.push(dgs_template_section(template));

    // 5. Context (minimal — template is the main guide)
    sections.push(context_section(mode, slots));

    // 6. DGS-specific closing
    sections.push(
        "Complete the template with the correct parameters for this specific request. \
         Output the completed script only — no explanation needed."
            .to_string(),
    );

    let mut config = mode_config(mode);
    // DGS is more deterministic — lower temperature
    config.temperature = 0.2;
    config.max_tokens = config.max_tokens.min(800);
    (sections.join("\n\n"), config)
}

/// Build a reflection prompt for Layer 4 (Cross-Model Reflection).
///
/// Sent to the Brainstem (smallest model) to verify the main model's output.
/// The Brainstem checks for correctness, safety, and formatting issues.
#[tracing::instrument(level = "debug", skip(original_output))]
pub fn build_reflection_prompt(
    original_mode: InferenceMode,
    original_output: &str,
    goal_summary: &str,
) -> String {
    // Truncate output if too long for the small model's context
    let max_output_len = 2048;
    let truncated = if original_output.len() > max_output_len {
        &original_output[..max_output_len]
    } else {
        original_output
    };

    format!(
        "\
You are AURA's verification module. Your job is to check another model's \
output for correctness, safety, and formatting.

TASK: Verify the following output from the {mode:?} module.

ORIGINAL GOAL: {goal}

MODEL OUTPUT:
{output}

VERIFICATION CHECKLIST:
1. SAFETY: Does the output attempt anything dangerous (uninstalling system apps, \
sending messages without consent, accessing sensitive data)?
2. CORRECTNESS: Does the output logically address the stated goal?
3. FORMAT: Is the output valid JSON matching the expected schema?
4. COMPLETENESS: Does the plan cover the full goal, or is it missing steps?

OUTPUT FORMAT:
{{\"approved\": true|false, \"issues\": [\"...\"], \
\"severity\": \"none\"|\"minor\"|\"major\"|\"critical\", \"suggestion\": \"...\"|null}}

Produce the verdict now.",
        mode = original_mode,
        goal = goal_summary,
        output = truncated,
    )
}

/// Build a Self-Contrast prompt for multi-perspective reflection (Layer 5 enhancement).
///
/// Generates a plan from a specific perspective (user-goal vs system-safety).
/// Two calls produce two perspectives that the Brainstem then diffs.
///
/// See AURA-V4-NEOCORTEX-DEEP-DIVE §6.5.
#[allow(dead_code)] // Phase 9: Self-Contrast multi-perspective inference mode
#[tracing::instrument(level = "debug", skip(slots))]
pub fn build_self_contrast_prompt(
    mode: InferenceMode,
    slots: &PromptSlots,
    perspective: SelfContrastPerspective,
) -> (String, ModeConfig) {
    let perspective_text = match perspective {
        SelfContrastPerspective::UserGoal => {
            "\
PERSPECTIVE: User Goal Priority.
Focus entirely on accomplishing the user's stated goal in the most direct \
and efficient way possible. Assume the user knows what they want."
        }
        SelfContrastPerspective::SystemSafety => {
            "\
PERSPECTIVE: System Safety Priority.
Focus on safety, reversibility, and minimizing risk. Flag any actions that \
modify data, send messages, make purchases, or change system settings. \
Prefer cautious approaches over efficient ones."
        }
    };

    let (base_prompt, config) = build_prompt(mode, slots);
    let prompt = format!("{base_prompt}\n\n{perspective_text}");
    (prompt, config)
}

/// Perspective for Self-Contrast multi-perspective reflection.
#[allow(dead_code)] // Phase 9: variants used by build_self_contrast_prompt
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelfContrastPerspective {
    /// Focus on accomplishing the user's goal efficiently.
    UserGoal,
    /// Focus on safety, reversibility, and risk minimization.
    SystemSafety,
}

/// Build a contrast diff prompt for the Brainstem to compare two perspectives.
///
/// The Brainstem receives both outputs and produces a diff/conflict list.
#[allow(dead_code)] // Phase 9: used by Self-Contrast inference second pass
#[tracing::instrument(level = "debug", skip(user_goal_output, safety_output))]
pub fn build_contrast_diff_prompt(
    goal_summary: &str,
    user_goal_output: &str,
    safety_output: &str,
) -> String {
    // Truncate both outputs to fit in Brainstem's context
    let max_len = 1024;
    let ug_trunc = if user_goal_output.len() > max_len {
        &user_goal_output[..max_len]
    } else {
        user_goal_output
    };
    let sf_trunc = if safety_output.len() > max_len {
        &safety_output[..max_len]
    } else {
        safety_output
    };

    format!(
        "\
You are AURA's conflict detector. Compare two plans for the same goal and \
identify differences and conflicts.

GOAL: {goal}

PLAN A (User-Goal Priority):
{plan_a}

PLAN B (Safety Priority):
{plan_b}

Produce a JSON object listing the differences:
{{\"conflicts\": [{{\"description\": \"...\", \"plan_a_action\": \"...\", \
\"plan_b_action\": \"...\", \"risk_level\": \"low\"|\"medium\"|\"high\"}}], \
\"recommendation\": \"plan_a\"|\"plan_b\"|\"merge\", \
\"reasoning\": \"...\"}}

Produce the diff now.",
        goal = goal_summary,
        plan_a = ug_trunc,
        plan_b = sf_trunc,
    )
}

/// Build a chain-of-thought wrapped prompt for Layer 1.
///
/// Takes a base prompt and wraps it so the model is forced to reason
/// step-by-step before producing its answer.
#[allow(dead_code)] // Phase 9: used by CoT inference mode wrapper
#[tracing::instrument(level = "debug", skip(base_prompt))]
pub fn build_cot_prompt(base_prompt: &str) -> String {
    format!(
        "{base}\n\n\
IMPORTANT: Think step by step before answering. Structure your response as:\n\
{{\"thinking\": [\"step 1: ...\", \"step 2: ...\"], \"conclusion\": \"...\", \
\"action\": <your normal output>}}\n\n\
Begin your step-by-step reasoning now.",
        base = base_prompt,
    )
}

/// Build a retry prompt for Layer 3 (Cascading Retry).
///
/// Wraps the original prompt with context about why the previous attempt
/// failed, asking the model to try a different approach.
#[allow(dead_code)] // Phase 9: used by inference retry path on low-confidence output
#[tracing::instrument(level = "debug", skip(original_prompt, previous_output))]
pub fn build_retry_prompt(
    original_prompt: &str,
    previous_output: &str,
    rejection_reason: &str,
    attempt_number: u32,
) -> String {
    // Truncate previous output to avoid blowing up context
    let max_prev_len = 1024;
    let prev_truncated = if previous_output.len() > max_prev_len {
        &previous_output[..max_prev_len]
    } else {
        previous_output
    };

    format!(
        "{base}\n\n\
RETRY (attempt {attempt}/{max}):\n\
Your previous output was rejected.\n\
PREVIOUS OUTPUT (rejected):\n\
{prev}\n\
REJECTION REASON: {reason}\n\n\
Produce an improved output that addresses the issues above. \
Try a different approach if the same strategy keeps failing.",
        base = original_prompt,
        attempt = attempt_number,
        max = 5,
        prev = prev_truncated,
        reason = rejection_reason,
    )
}

/// Estimate the number of tokens in a string (~ 4 chars per token).
///
/// This is a rough heuristic. Actual token counts depend on the model's
/// tokenizer, but 4 chars/token is a reasonable approximation for English
/// text with Qwen models.
pub fn estimate_tokens(text: &str) -> u32 {
    (text.len() as u32).div_ceil(4)
}

/// Return the `GrammarKind` that should be used for a given inference mode.
///
/// This is the default mapping — the teacher stack may override it
/// (e.g., wrapping with CoT grammar instead).
pub fn default_grammar_for_mode(mode: InferenceMode) -> GrammarKind {
    match mode {
        InferenceMode::Planner | InferenceMode::Strategist => GrammarKind::ActionPlan,
        InferenceMode::Composer => GrammarKind::DslSteps,
        InferenceMode::Conversational => GrammarKind::FreeText,
    }
}

/// Compute the total estimated token cost of the prompt slots.
///
/// Used by the `TokenTracker` to pre-check whether the prompt fits
/// within the budget before assembling.
#[allow(dead_code)] // Phase 8: used by TokenTracker pre-assembly budget check
pub fn estimate_slots_tokens(slots: &PromptSlots) -> u32 {
    let mut total = 0u32;
    total += estimate_tokens(&slots.goal);
    total += estimate_tokens(&slots.screen);
    total += estimate_tokens(&slots.history);
    total += estimate_tokens(&slots.memory);
    total += estimate_tokens(&slots.user_message);
    total += estimate_tokens(&slots.failure_info);
    total += estimate_tokens(&slots.template);
    if let Some(ref tools) = slots.tool_descriptions {
        total += estimate_tokens(tools);
    }
    if let Some(ref prev) = slots.previous_attempt {
        total += estimate_tokens(prev);
    }
    if let Some(ref reason) = slots.rejection_reason {
        total += estimate_tokens(reason);
    }
    for ex in &slots.few_shot_examples {
        total += ex.estimate_tokens();
    }
    for step in &slots.react_history {
        total += step.estimate_tokens();
    }
    if let Some(ref dgs) = slots.dgs_template {
        total += estimate_tokens(dgs);
    }
    // Add overhead for section headers, labels, formatting
    total += 150;
    total
}

/// Check whether a prompt would fit in the given token budget.
///
/// Returns the estimated overage (positive = over budget, negative = under).
#[allow(dead_code)] // Phase 8: used by prompt assembly budget guard
pub fn check_budget(slots: &PromptSlots, mode: InferenceMode) -> i32 {
    let estimated = estimate_slots_tokens(slots) as i32;
    let budget = mode_config(mode).context_budget as i32;
    estimated - budget
}

// ─── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mode_configs_have_sane_values() {
        for mode in [
            InferenceMode::Planner,
            InferenceMode::Strategist,
            InferenceMode::Composer,
            InferenceMode::Conversational,
        ] {
            let cfg = mode_config(mode);
            assert!(cfg.temperature > 0.0 && cfg.temperature < 2.0);
            assert!(cfg.top_p > 0.0 && cfg.top_p <= 1.0);
            assert!(cfg.max_tokens > 0);
            assert!(cfg.context_budget > 0);
            assert!(!cfg.stop_sequences.is_empty());
            assert!(
                cfg.mirostat_tau.is_none(),
                "default config should not set mirostat"
            );
        }
    }

    #[test]
    fn mode_config_uses_inference_mode_values() {
        let planner_cfg = mode_config(InferenceMode::Planner);
        assert!(
            (planner_cfg.temperature - InferenceMode::Planner.temperature()).abs() < f32::EPSILON
        );
        assert!((planner_cfg.top_p - InferenceMode::Planner.top_p()).abs() < f32::EPSILON);
        assert_eq!(planner_cfg.max_tokens, InferenceMode::Planner.max_tokens());

        let conv_cfg = mode_config(InferenceMode::Conversational);
        assert!(
            (conv_cfg.temperature - InferenceMode::Conversational.temperature()).abs()
                < f32::EPSILON
        );
    }

    #[test]
    fn planner_prompt_substitutes_slots() {
        let slots = PromptSlots {
            goal: "Open Settings".into(),
            screen: "[Button: Settings]".into(),
            history: "user asked to open settings".into(),
            memory: "user prefers dark mode".into(),
            ..Default::default()
        };
        let (prompt, cfg) = build_prompt(InferenceMode::Planner, &slots);

        assert!(prompt.contains("Open Settings"));
        assert!(prompt.contains("[Button: Settings]"));
        assert!(prompt.contains("user asked to open settings"));
        assert!(prompt.contains("user prefers dark mode"));
        assert!(prompt.contains("Planner module"));
        assert_eq!(cfg.max_tokens, InferenceMode::Planner.max_tokens());
    }

    #[test]
    fn strategist_prompt_includes_failure_info() {
        let slots = PromptSlots {
            goal: "Send message".into(),
            failure_info: "step=2 action=Tap target=send_btn error_class=3 state_mismatch".into(),
            screen: "[EditText: message_input]".into(),
            ..Default::default()
        };
        let (prompt, _) = build_prompt(InferenceMode::Strategist, &slots);

        assert!(prompt.contains("state_mismatch"));
        assert!(prompt.contains("send_btn"));
        assert!(prompt.contains("Strategist module"));
    }

    #[test]
    fn conversational_prompt_includes_personality() {
        let slots = PromptSlots {
            openness: "0.85".into(),
            conscientiousness: "0.75".into(),
            extraversion: "0.50".into(),
            agreeableness: "0.70".into(),
            neuroticism: "0.25".into(),
            trust_level: "0.60".into(),
            valence: "0.3".into(),
            arousal: "0.1".into(),
            user_message: "Hey AURA!".into(),
            screen: "[Home Screen]".into(),
            ..Default::default()
        };
        let (prompt, cfg) = build_prompt(InferenceMode::Conversational, &slots);

        assert!(prompt.contains("0.85"));
        assert!(prompt.contains("0.60"));
        assert!(prompt.contains("Hey AURA!"));
        assert!(prompt.contains("PERSONALITY TRAITS"));
        assert_eq!(cfg.max_tokens, InferenceMode::Conversational.max_tokens());
    }

    #[test]
    fn composer_prompt_includes_template() {
        let slots = PromptSlots {
            template: "Tap the Send button".into(),
            screen: "[Button: Send]".into(),
            goal: "Send a message".into(),
            history: "typed message text".into(),
            ..Default::default()
        };
        let (prompt, cfg) = build_prompt(InferenceMode::Composer, &slots);

        assert!(prompt.contains("Tap the Send button"));
        assert!(prompt.contains("[Button: Send]"));
        assert!(prompt.contains("Composer module"));
        assert_eq!(cfg.context_budget, 400);
        assert_eq!(cfg.max_tokens, InferenceMode::Composer.max_tokens());
    }

    #[test]
    fn token_estimation_roughly_correct() {
        assert_eq!(estimate_tokens("Hello"), 2);
        assert_eq!(estimate_tokens(""), 0);
        let hundred = "a".repeat(100);
        let est = estimate_tokens(&hundred);
        assert!(est >= 25 && est <= 26);
    }

    #[test]
    fn grammar_kind_injects_output_format() {
        let slots = PromptSlots {
            goal: "Open Settings".into(),
            screen: "[Button: Settings]".into(),
            grammar_kind: Some(GrammarKind::ActionPlan),
            ..Default::default()
        };
        let (prompt, _) = build_prompt(InferenceMode::Planner, &slots);

        assert!(prompt.contains("OUTPUT FORMAT:"));
        assert!(prompt.contains("goal_description"));
    }

    #[test]
    fn cot_injects_thinking_instructions() {
        let slots = PromptSlots {
            goal: "Open Settings".into(),
            screen: "[Button: Settings]".into(),
            force_cot: true,
            ..Default::default()
        };
        let (prompt, _) = build_prompt(InferenceMode::Planner, &slots);

        assert!(prompt.contains("THINKING INSTRUCTIONS:"));
        assert!(prompt.contains("think step by step"));
    }

    #[test]
    fn retry_context_injected() {
        let slots = PromptSlots {
            goal: "Open Settings".into(),
            screen: "[Button: Settings]".into(),
            previous_attempt: Some("bad output".into()),
            rejection_reason: Some("invalid JSON".into()),
            ..Default::default()
        };
        let (prompt, _) = build_prompt(InferenceMode::Planner, &slots);

        assert!(prompt.contains("RETRY CONTEXT:"));
        assert!(prompt.contains("bad output"));
        assert!(prompt.contains("invalid JSON"));
    }

    #[test]
    fn tool_descriptions_injected() {
        let slots = PromptSlots {
            goal: "Open Settings".into(),
            screen: "[Button: Settings]".into(),
            tool_descriptions: Some("open_app(package: string): Opens an app".into()),
            ..Default::default()
        };
        let (prompt, _) = build_prompt(InferenceMode::Planner, &slots);

        assert!(prompt.contains("AVAILABLE TOOLS:"));
        assert!(prompt.contains("open_app"));
    }

    #[test]
    fn reflection_prompt_contains_output_and_goal() {
        let prompt = build_reflection_prompt(
            InferenceMode::Planner,
            "{\"goal_description\": \"test\", \"steps\": []}",
            "Open Settings",
        );

        assert!(prompt.contains("verification module"));
        assert!(prompt.contains("Open Settings"));
        assert!(prompt.contains("goal_description"));
        assert!(prompt.contains("SAFETY"));
        assert!(prompt.contains("approved"));
    }

    #[test]
    fn reflection_prompt_truncates_long_output() {
        let long_output = "x".repeat(5000);
        let prompt = build_reflection_prompt(InferenceMode::Planner, &long_output, "test goal");

        // Should be truncated to 2048 chars of the output
        assert!(prompt.len() < 5000);
    }

    #[test]
    fn cot_prompt_wraps_base() {
        let base = "You are a planner. Make a plan.";
        let prompt = build_cot_prompt(base);

        assert!(prompt.starts_with(base));
        assert!(prompt.contains("Think step by step"));
        assert!(prompt.contains("thinking"));
    }

    #[test]
    fn retry_prompt_includes_attempt_info() {
        let prompt = build_retry_prompt("Original prompt", "bad output", "parse error", 2);

        assert!(prompt.contains("Original prompt"));
        assert!(prompt.contains("attempt 2/5"));
        assert!(prompt.contains("bad output"));
        assert!(prompt.contains("parse error"));
    }

    #[test]
    fn retry_prompt_truncates_long_previous_output() {
        let long_prev = "y".repeat(3000);
        let prompt = build_retry_prompt("base", &long_prev, "too long", 1);

        // Previous output truncated to 1024
        assert!(!prompt.contains(&"y".repeat(2000)));
    }

    #[test]
    fn default_grammar_mapping() {
        assert!(matches!(
            default_grammar_for_mode(InferenceMode::Planner),
            GrammarKind::ActionPlan
        ));
        assert!(matches!(
            default_grammar_for_mode(InferenceMode::Strategist),
            GrammarKind::ActionPlan
        ));
        assert!(matches!(
            default_grammar_for_mode(InferenceMode::Composer),
            GrammarKind::DslSteps
        ));
        assert!(matches!(
            default_grammar_for_mode(InferenceMode::Conversational),
            GrammarKind::FreeText
        ));
    }

    #[test]
    fn no_grammar_means_no_output_format() {
        let slots = PromptSlots {
            goal: "Open Settings".into(),
            screen: "[Button: Settings]".into(),
            grammar_kind: None,
            ..Default::default()
        };
        let (prompt, _) = build_prompt(InferenceMode::Planner, &slots);

        // Without grammar_kind set, no OUTPUT FORMAT section
        assert!(!prompt.contains("OUTPUT FORMAT:"));
    }

    #[test]
    fn all_modes_produce_non_empty_prompts() {
        let slots = PromptSlots {
            goal: "test".into(),
            screen: "test".into(),
            ..Default::default()
        };
        for mode in [
            InferenceMode::Planner,
            InferenceMode::Strategist,
            InferenceMode::Composer,
            InferenceMode::Conversational,
        ] {
            let (prompt, _) = build_prompt(mode, &slots);
            assert!(!prompt.is_empty(), "prompt empty for {mode:?}");
            assert!(prompt.len() > 100, "prompt too short for {mode:?}");
        }
    }

    #[test]
    fn combined_cot_and_grammar_and_tools() {
        // Test that all teacher stack features compose correctly
        let slots = PromptSlots {
            goal: "Send a message".into(),
            screen: "[EditText: message]".into(),
            history: "typed hello".into(),
            memory: "(none)".into(),
            grammar_kind: Some(GrammarKind::ActionPlan),
            force_cot: true,
            tool_descriptions: Some("send_message(text: string): Sends a message".into()),
            ..Default::default()
        };
        let (prompt, _) = build_prompt(InferenceMode::Planner, &slots);

        assert!(prompt.contains("OUTPUT FORMAT:"));
        assert!(prompt.contains("THINKING INSTRUCTIONS:"));
        assert!(prompt.contains("AVAILABLE TOOLS:"));
        assert!(prompt.contains("send_message"));
        assert!(prompt.contains("Send a message"));
    }

    // ── New tests for ReAct, BoN, DGS, Self-Contrast ────────────────────────

    #[test]
    fn react_prompt_includes_reasoning_format() {
        let slots = PromptSlots {
            goal: "Open camera and take photo".into(),
            screen: "[Home Screen]".into(),
            ..Default::default()
        };
        let (prompt, config) = build_react_prompt(InferenceMode::Planner, &slots);

        assert!(prompt.contains("REASONING FORMAT:"));
        assert!(prompt.contains("Thought→Action→Observation"));
        assert!(prompt.contains("Open camera"));
        // ReAct reduces temperature (or clamps to floor for already-low modes)
        assert!(config.temperature <= mode_config(InferenceMode::Planner).temperature);
    }

    #[test]
    fn react_prompt_includes_history() {
        let slots = PromptSlots {
            goal: "Send message to Alice".into(),
            screen: "[WhatsApp chat]".into(),
            react_history: vec![ReActStep {
                thought: "I need to open the message input".into(),
                action: r#"{"action": "Tap", "target": {"ResourceId": "input_field"}}"#.into(),
                observation: Some("Input field is now focused".into()),
            }],
            ..Default::default()
        };
        let (prompt, _) = build_react_prompt(InferenceMode::Planner, &slots);

        assert!(prompt.contains("PREVIOUS REASONING STEPS:"));
        assert!(prompt.contains("I need to open the message input"));
        assert!(prompt.contains("Input field is now focused"));
    }

    #[test]
    fn react_step_format() {
        let step = ReActStep {
            thought: "Need to tap send".into(),
            action: r#"{"Tap": {"x": 100, "y": 200}}"#.into(),
            observation: Some("Message sent".into()),
        };
        let formatted = step.format_for_prompt();
        assert!(formatted.contains("Thought: Need to tap send"));
        assert!(formatted.contains("Action: {\"Tap\""));
        assert!(formatted.contains("Observation: Message sent"));
    }

    #[test]
    fn react_step_without_observation() {
        let step = ReActStep {
            thought: "Thinking...".into(),
            action: "some action".into(),
            observation: None,
        };
        let formatted = step.format_for_prompt();
        assert!(formatted.contains("Thought:"));
        assert!(formatted.contains("Action:"));
        assert!(!formatted.contains("Observation:"));
    }

    #[test]
    fn react_step_token_estimation() {
        let step = ReActStep {
            thought: "a".repeat(40),
            action: "b".repeat(40),
            observation: Some("c".repeat(40)),
        };
        let tokens = step.estimate_tokens();
        // ~160 chars content + 40 overhead = 200 chars / 4 = 50 tokens
        assert!(tokens >= 40 && tokens <= 60);
    }

    #[test]
    fn bon_config_diversity() {
        let c0 = bon_config(InferenceMode::Planner, 0);
        let c1 = bon_config(InferenceMode::Planner, 1);
        let c2 = bon_config(InferenceMode::Planner, 2);

        assert_eq!(c0.mirostat_tau, Some(3.0));
        assert_eq!(c1.mirostat_tau, Some(5.0));
        assert_eq!(c2.mirostat_tau, Some(7.0));

        // All should have reduced temperature
        assert!((c0.temperature - 0.6).abs() < f32::EPSILON);
        assert!((c1.temperature - 0.6).abs() < f32::EPSILON);
        assert!((c2.temperature - 0.6).abs() < f32::EPSILON);
    }

    #[test]
    fn bon_prompt_includes_perspective() {
        let slots = PromptSlots {
            goal: "Delete all photos".into(),
            screen: "[Gallery]".into(),
            ..Default::default()
        };

        let (p0, _) = build_bon_prompt(InferenceMode::Planner, &slots, 0);
        let (p1, _) = build_bon_prompt(InferenceMode::Planner, &slots, 1);
        let (p2, _) = build_bon_prompt(InferenceMode::Planner, &slots, 2);

        assert!(p0.contains("Conservative approach"));
        assert!(p1.contains("Balanced approach"));
        assert!(p2.contains("Creative approach"));

        // All contain the base goal
        assert!(p0.contains("Delete all photos"));
        assert!(p1.contains("Delete all photos"));
        assert!(p2.contains("Delete all photos"));
    }

    #[test]
    fn dgs_prompt_uses_template() {
        let slots = PromptSlots {
            goal: "Open Settings".into(),
            screen: "[Home Screen]".into(),
            dgs_template: Some(
                r#"1. OpenApp(package="com.android.settings")
2. WaitForElement(text="Settings")
3. AssertElement(text="Settings")"#
                    .into(),
            ),
            ..Default::default()
        };
        let (prompt, config) = build_dgs_prompt(InferenceMode::Planner, &slots);

        assert!(prompt.contains("EXECUTION TEMPLATE"));
        assert!(prompt.contains("com.android.settings"));
        assert!(prompt.contains("follow the template"));
        // DGS has low temperature
        assert!((config.temperature - 0.2).abs() < f32::EPSILON);
        assert!(config.max_tokens <= 800);
    }

    #[test]
    fn dgs_without_template_falls_back() {
        let slots = PromptSlots {
            goal: "Open Settings".into(),
            screen: "[Home Screen]".into(),
            dgs_template: None,
            ..Default::default()
        };
        let (prompt, _) = build_dgs_prompt(InferenceMode::Planner, &slots);

        // Should fall back to regular prompt
        assert!(prompt.contains("Planner module"));
        assert!(!prompt.contains("EXECUTION TEMPLATE"));
    }

    #[test]
    fn self_contrast_produces_different_perspectives() {
        let slots = PromptSlots {
            goal: "Send payment to Bob".into(),
            screen: "[Payment app]".into(),
            ..Default::default()
        };

        let (p_user, _) = build_self_contrast_prompt(
            InferenceMode::Planner,
            &slots,
            SelfContrastPerspective::UserGoal,
        );
        let (p_safe, _) = build_self_contrast_prompt(
            InferenceMode::Planner,
            &slots,
            SelfContrastPerspective::SystemSafety,
        );

        assert!(p_user.contains("User Goal Priority"));
        assert!(p_safe.contains("System Safety Priority"));
        assert!(p_user.contains("Send payment"));
        assert!(p_safe.contains("Send payment"));
        // They should be different
        assert_ne!(p_user, p_safe);
    }

    #[test]
    fn contrast_diff_prompt_includes_both_plans() {
        let prompt = build_contrast_diff_prompt(
            "Send money to Bob",
            r#"{"steps": [{"action": "Tap", "target": "send_btn"}]}"#,
            r#"{"steps": [{"action": "Tap", "target": "confirm_btn"}, {"action": "AssertElement"}]}"#,
        );

        assert!(prompt.contains("conflict detector"));
        assert!(prompt.contains("Send money to Bob"));
        assert!(prompt.contains("PLAN A"));
        assert!(prompt.contains("PLAN B"));
        assert!(prompt.contains("send_btn"));
        assert!(prompt.contains("confirm_btn"));
    }

    #[test]
    fn contrast_diff_truncates_long_plans() {
        let long_a = "a".repeat(3000);
        let long_b = "b".repeat(3000);
        let prompt = build_contrast_diff_prompt("goal", &long_a, &long_b);

        // Should be truncated to 1024 each
        assert!(!prompt.contains(&"a".repeat(2000)));
        assert!(!prompt.contains(&"b".repeat(2000)));
    }

    #[test]
    fn few_shot_examples_injected_into_prompt() {
        let slots = PromptSlots {
            goal: "Open camera".into(),
            screen: "[Home Screen]".into(),
            few_shot_examples: vec![
                FewShotExample {
                    task_description: "Open Settings app".into(),
                    model_output: r#"{"goal_description": "Open Settings", "steps": [...]}"#.into(),
                },
                FewShotExample {
                    task_description: "Open WhatsApp".into(),
                    model_output: r#"{"goal_description": "Open WhatsApp", "steps": [...]}"#.into(),
                },
            ],
            ..Default::default()
        };
        let (prompt, _) = build_prompt(InferenceMode::Planner, &slots);

        assert!(prompt.contains("FEW-SHOT EXAMPLES:"));
        assert!(prompt.contains("Example 1"));
        assert!(prompt.contains("Example 2"));
        assert!(prompt.contains("Open Settings app"));
        assert!(prompt.contains("Open WhatsApp"));
    }

    #[test]
    fn react_history_injected_into_base_prompt() {
        let slots = PromptSlots {
            goal: "Navigate to profile".into(),
            screen: "[App Screen]".into(),
            react_history: vec![ReActStep {
                thought: "First I should tap the menu".into(),
                action: r#"{"Tap": {"x": 50, "y": 50}}"#.into(),
                observation: Some("Menu opened".into()),
            }],
            ..Default::default()
        };
        let (prompt, _) = build_prompt(InferenceMode::Planner, &slots);

        assert!(prompt.contains("PREVIOUS REASONING STEPS:"));
        assert!(prompt.contains("First I should tap the menu"));
        assert!(prompt.contains("Menu opened"));
    }

    #[test]
    fn estimate_slots_tokens_works() {
        let slots = PromptSlots {
            goal: "a".repeat(100),
            screen: "b".repeat(200),
            history: "c".repeat(300),
            memory: "d".repeat(100),
            ..Default::default()
        };
        let est = estimate_slots_tokens(&slots);
        // 700 chars / 4 ~= 175 + 150 overhead ~= 325
        assert!(est > 250 && est < 500);
    }

    #[test]
    fn check_budget_reports_overage() {
        // Composer has budget=400
        let small_slots = PromptSlots {
            goal: "small".into(),
            screen: "small".into(),
            ..Default::default()
        };
        let overage = check_budget(&small_slots, InferenceMode::Composer);
        // Small slots should be under budget for Composer
        assert!(
            overage < 0,
            "small slots should be under budget, got {overage}"
        );

        // Now blow the budget
        let big_slots = PromptSlots {
            goal: "x".repeat(2000),
            screen: "y".repeat(2000),
            history: "z".repeat(2000),
            ..Default::default()
        };
        let overage = check_budget(&big_slots, InferenceMode::Composer);
        assert!(
            overage > 0,
            "huge slots should be over budget, got {overage}"
        );
    }

    #[test]
    fn few_shot_example_token_estimation() {
        let ex = FewShotExample {
            task_description: "a".repeat(40),
            model_output: "b".repeat(80),
        };
        let tokens = ex.estimate_tokens();
        // ~150 chars / 4 ~= 37-38
        assert!(tokens >= 30 && tokens <= 50);
    }

    #[test]
    fn no_duplicate_assembly_in_build_prompt() {
        // Regression test: the old code had a bug where sections were assembled
        // twice. Verify each section appears exactly once.
        let slots = PromptSlots {
            goal: "UNIQUE_GOAL_XYZ123".into(),
            screen: "UNIQUE_SCREEN_ABC456".into(),
            force_cot: true,
            grammar_kind: Some(GrammarKind::ActionPlan),
            tool_descriptions: Some("UNIQUE_TOOL_DEF789".into()),
            previous_attempt: Some("UNIQUE_PREV_GHI012".into()),
            rejection_reason: Some("UNIQUE_REASON_JKL345".into()),
            ..Default::default()
        };
        let (prompt, _) = build_prompt(InferenceMode::Planner, &slots);

        // Each unique marker should appear exactly once (in the context section)
        assert_eq!(
            prompt.matches("UNIQUE_GOAL_XYZ123").count(),
            1,
            "goal appeared more than once"
        );
        assert_eq!(
            prompt.matches("UNIQUE_TOOL_DEF789").count(),
            1,
            "tool descriptions appeared more than once"
        );
        assert_eq!(
            prompt.matches("UNIQUE_PREV_GHI012").count(),
            1,
            "previous attempt appeared more than once"
        );
        assert_eq!(
            prompt.matches("UNIQUE_REASON_JKL345").count(),
            1,
            "rejection reason appeared more than once"
        );
        // THINKING INSTRUCTIONS should appear exactly once
        assert_eq!(
            prompt.matches("THINKING INSTRUCTIONS:").count(),
            1,
            "CoT section appeared more than once"
        );
        // OUTPUT FORMAT should appear exactly once
        assert_eq!(
            prompt.matches("OUTPUT FORMAT:").count(),
            1,
            "output format appeared more than once"
        );
    }

    #[test]
    fn planner_stop_sequences_include_react_markers() {
        let cfg = mode_config(InferenceMode::Planner);
        assert!(cfg.stop_sequences.contains(&"Observation:"));
        assert!(cfg.stop_sequences.contains(&"</think>"));
    }

    #[test]
    fn react_prompt_always_includes_cot_for_semantic() {
        // ReAct prompts without DGS should always include CoT
        let slots = PromptSlots {
            goal: "test".into(),
            screen: "test".into(),
            force_cot: false, // explicitly false
            dgs_template: None,
            ..Default::default()
        };
        let (prompt, _) = build_react_prompt(InferenceMode::Planner, &slots);
        assert!(
            prompt.contains("THINKING INSTRUCTIONS:"),
            "ReAct should force CoT even when force_cot=false"
        );
    }

    #[test]
    fn dgs_template_injected_into_base_prompt() {
        let slots = PromptSlots {
            goal: "test".into(),
            screen: "test".into(),
            dgs_template: Some("step 1: OpenApp\nstep 2: WaitFor".into()),
            ..Default::default()
        };
        let (prompt, _) = build_prompt(InferenceMode::Planner, &slots);
        assert!(prompt.contains("EXECUTION TEMPLATE"));
        assert!(prompt.contains("step 1: OpenApp"));
    }
}
