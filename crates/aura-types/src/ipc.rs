use serde::{Deserialize, Serialize};

use crate::dsl::DslStep;
use crate::etg::ActionPlan;

// ─── Sub-types used throughout IPC ──────────────────────────────────────────

/// Model quantisation / size tier.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum ModelTier {
    /// ~1.5B params — reflexive brainstem tasks.
    Brainstem1_5B,
    /// ~4B params — standard planning.
    Standard4B,
    /// ~8B params — full reasoning.
    Full8B,
}

/// Parameters for model loading.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelParams {
    pub n_ctx: u32,
    pub n_threads: u32,
    pub model_tier: ModelTier,
}

impl Default for ModelParams {
    fn default() -> Self {
        Self {
            n_ctx: 4096,
            n_threads: 4,
            model_tier: ModelTier::Standard4B,
        }
    }
}

/// Role in a conversation.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum Role {
    User,
    Assistant,
    System,
}

/// A single turn in a conversation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationTurn {
    pub role: Role,
    pub content: String,
    pub timestamp_ms: u64,
}

/// Memory tier classification.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum MemoryTier {
    Working,
    Episodic,
    Semantic,
    Archive,
}

/// A snippet of memory included in context.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemorySnippet {
    pub content: String,
    pub source: MemoryTier,
    pub relevance: f32,
    pub timestamp_ms: u64,
}

/// Summary of the current screen state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScreenSummary {
    pub package_name: String,
    pub activity_name: String,
    /// Bounded: max MAX_SCREEN_INTERACTIVE_ELEMENTS items enforced at collection site.
    pub interactive_elements: Vec<String>,
    /// Bounded: max MAX_SCREEN_VISIBLE_TEXT items enforced at collection site.
    pub visible_text: Vec<String>,
}

/// Max interactive elements in a [`ScreenSummary`] snapshot.
pub const MAX_SCREEN_INTERACTIVE_ELEMENTS: usize = 128;
/// Max visible text lines in a [`ScreenSummary`] snapshot.
pub const MAX_SCREEN_VISIBLE_TEXT: usize = 64;

/// Summary of active goal progress.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GoalSummary {
    pub description: String,
    pub progress_percent: u8,
    pub current_step: String,
    /// Bounded: max MAX_GOAL_BLOCKERS items enforced at collection site.
    pub blockers: Vec<String>,
}

/// Max blockers in a [`GoalSummary`] snapshot.
pub const MAX_GOAL_BLOCKERS: usize = 8;

/// OCEAN personality snapshot sent with context.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersonalitySnapshot {
    pub openness: f32,
    pub conscientiousness: f32,
    pub extraversion: f32,
    pub agreeableness: f32,
    pub neuroticism: f32,
    pub current_mood_valence: f32,
    pub current_mood_arousal: f32,
    pub trust_level: f32,
}

impl Default for PersonalitySnapshot {
    fn default() -> Self {
        Self {
            openness: 0.85,
            conscientiousness: 0.75,
            extraversion: 0.50,
            agreeableness: 0.70,
            neuroticism: 0.25,
            current_mood_valence: 0.0,
            current_mood_arousal: 0.0,
            trust_level: 0.0,
        }
    }
}

/// Current user activity state (simple legacy enum, retained for compatibility).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum UserState {
    Active,
    Idle,
    Sleeping,
    Driving,
    InMeeting,
    Unknown,
}

/// Time of day bucket derived from wall-clock hour.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum TimeOfDay {
    EarlyMorning, // 04–08
    Morning,      // 08–12
    Afternoon,    // 12–17
    Evening,      // 17–21
    Night,        // 21–04
}

impl Default for TimeOfDay {
    fn default() -> Self {
        TimeOfDay::Morning
    }
}

/// Device thermal load.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum ThermalLevel {
    Normal,
    Warm,
    Hot,
    Critical,
}

impl Default for ThermalLevel {
    fn default() -> Self {
        ThermalLevel::Normal
    }
}

/// Estimated physical location type (no GPS coordinates — privacy safe).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum LocationType {
    Home,
    Work,
    Transit,
    Outdoor,
    Unknown,
}

impl Default for LocationType {
    fn default() -> Self {
        LocationType::Unknown
    }
}

/// Device orientation.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum Orientation {
    Portrait,
    Landscape,
    Unknown,
}

impl Default for Orientation {
    fn default() -> Self {
        Orientation::Unknown
    }
}

/// Rich user and device state signals sent with every context package.
///
/// All fields are raw sensor readings — no pre-composed text. The LLM
/// receives these as numbers/enums and reasons about them naturally.
/// JNI calls that fail on non-Android hosts silently return defaults.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserStateSignals {
    pub time_of_day: TimeOfDay,
    /// Battery level 0–100. 255 = unknown.
    pub battery_level: u8,
    pub is_charging: bool,
    pub thermal_state: ThermalLevel,
    pub network_available: bool,
    /// Foreground app package name, empty if unknown.
    pub foreground_app: String,
    pub estimated_location_type: LocationType,
    pub device_orientation: Orientation,
    pub is_screen_on: bool,
    /// Total steps today from pedometer, 0 if unavailable.
    pub step_count_today: u32,
}

impl Default for UserStateSignals {
    fn default() -> Self {
        Self {
            time_of_day: TimeOfDay::default(),
            battery_level: 255,
            is_charging: false,
            thermal_state: ThermalLevel::default(),
            network_available: true,
            foreground_app: String::new(),
            estimated_location_type: LocationType::default(),
            device_orientation: Orientation::default(),
            is_screen_on: true,
            step_count_today: 0,
        }
    }
}

/// Inference mode — each implies different generation parameters.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum InferenceMode {
    /// Structured action planning: low temp, high precision.
    Planner,
    /// Strategic reasoning with broader exploration.
    Strategist,
    /// Script/DSL composition.
    Composer,
    /// Natural conversation: higher temp, creative.
    Conversational,
}

impl InferenceMode {
    /// Recommended temperature for this mode.
    #[must_use]
    pub fn temperature(&self) -> f32 {
        match self {
            InferenceMode::Planner => 0.1,
            InferenceMode::Strategist => 0.4,
            InferenceMode::Composer => 0.2,
            InferenceMode::Conversational => 0.7,
        }
    }

    /// Recommended top_p for this mode.
    #[must_use]
    pub fn top_p(&self) -> f32 {
        match self {
            InferenceMode::Planner => 0.9,
            InferenceMode::Strategist => 0.95,
            InferenceMode::Composer => 0.9,
            InferenceMode::Conversational => 0.95,
        }
    }

    /// Recommended max_tokens for this mode.
    #[must_use]
    pub fn max_tokens(&self) -> u32 {
        match self {
            InferenceMode::Planner => 2048,
            InferenceMode::Strategist => 4096,
            InferenceMode::Composer => 1024,
            InferenceMode::Conversational => 512,
        }
    }
}

/// A single state transition pair (16 bytes).
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct TransitionPair {
    pub from_hash: u64,
    pub to_hash: u64,
}

/// Compact failure context (96 bytes target) sent for re-planning.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct FailureContext {
    pub task_goal_hash: u64,
    pub current_step: u32,
    pub failing_action: u64,
    pub target_id: u64,
    pub expected_state_hash: u64,
    pub actual_state_hash: u64,
    pub tried_approaches: u64,
    pub last_3_transitions: [TransitionPair; 3],
    pub error_class: u8,
}

/// Full context package sent to neocortex for inference.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextPackage {
    /// Bounded: max MAX_CONVERSATION_HISTORY items enforced at collection site.
    pub conversation_history: Vec<ConversationTurn>,
    /// Bounded: max MAX_MEMORY_SNIPPETS items enforced at collection site.
    pub memory_snippets: Vec<MemorySnippet>,
    pub current_screen: Option<ScreenSummary>,
    pub active_goal: Option<GoalSummary>,
    pub personality: PersonalitySnapshot,
    pub user_state: UserStateSignals,
    pub inference_mode: InferenceMode,
    pub token_budget: u32,
    /// Compact JSON identity block: OCEAN + VAD + relationship_stage + archetype.
    /// Raw numbers only — the LLM reasons about these naturally.
    /// `None` if identity data is not available for this request.
    pub identity_block: Option<String>,
    /// Human-readable mood context string.
    /// Example: "Mood: positive valence, high energy, assertive stance. Emotion: Joy."
    pub mood_description: String,
}

impl ContextPackage {
    /// Maximum allowed context package size (64 KB).
    pub const MAX_SIZE: usize = 65_536;
    /// Max conversation turns in a [`ContextPackage`].
    pub const MAX_CONVERSATION_HISTORY: usize = 64;
    /// Max memory snippets in a [`ContextPackage`].
    pub const MAX_MEMORY_SNIPPETS: usize = 32;

    /// Estimate the serialized size of this context package.
    /// Used to enforce the ≤64KB constraint before sending over IPC.
    #[must_use]
    pub fn estimated_size(&self) -> usize {
        let mut size = 0usize;

        // Conversation history
        for turn in &self.conversation_history {
            size += turn.content.len() + 16; // content + role + timestamp overhead
        }

        // Memory snippets
        for snippet in &self.memory_snippets {
            size += snippet.content.len() + 16; // content + tier + relevance + timestamp
        }

        // Screen summary
        if let Some(ref screen) = self.current_screen {
            size += screen.package_name.len() + screen.activity_name.len();
            for elem in &screen.interactive_elements {
                size += elem.len();
            }
            for text in &screen.visible_text {
                size += text.len();
            }
        }

        // Goal summary
        if let Some(ref goal) = self.active_goal {
            size += goal.description.len() + goal.current_step.len();
            for blocker in &goal.blockers {
                size += blocker.len();
            }
        }

        // Fixed-size fields: personality (32), inference_mode (1), token_budget (4)
        size += 37;

        // PersonalitySnapshot: 8 f32s = 32 bytes
        size += 32;

        // UserStateSignals: foreground_app string + fixed fields (~20 bytes)
        size += self.user_state.foreground_app.len() + 20;

        // identity_block and mood_description
        if let Some(ref ib) = self.identity_block {
            size += ib.len();
        }
        size += self.mood_description.len();

        size
    }
}

impl Default for ContextPackage {
    fn default() -> Self {
        Self {
            conversation_history: Vec::new(),
            memory_snippets: Vec::new(),
            current_screen: None,
            active_goal: None,
            personality: PersonalitySnapshot::default(),
            user_state: UserStateSignals::default(),
            inference_mode: InferenceMode::Conversational,
            token_budget: 2048,
            identity_block: None,
            mood_description: String::new(),
        }
    }
}

// ─── Proactive trigger types ─────────────────────────────────────────────────

/// A typed proactive trigger detected by the daemon.
///
/// Each variant carries only raw, factual data — never pre-composed text.
/// The LLM (neocortex) receives this as part of a [`DaemonToNeocortex::ProactiveContext`]
/// message and is responsible for generating all user-facing language.
///
/// Architecture law: **Daemon encodes structured facts. LLM encodes language.**
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ProactiveTrigger {
    /// A user goal has been stalled (no progress recorded) for N days.
    GoalStalled {
        goal_id: u64,
        title: String,
        stalled_days: u32,
    },
    /// A user goal has passed its deadline by N days.
    GoalOverdue {
        goal_id: u64,
        title: String,
        overdue_days: u32,
    },
    /// A social relationship has gone silent past the configured threshold.
    ///
    /// `contact_name` should be resolved from the contacts store before
    /// constructing this trigger; falling back to `"contact:<id>"` is
    /// acceptable until a contact-name resolution service is wired in.
    SocialGap {
        contact_name: String,
        days_since_contact: u32,
    },
    /// A health or system metric has crossed a threshold warranting attention.
    HealthAlert {
        metric: String,
        value: f32,
        threshold: f32,
    },
    /// Memory consolidation identified a recurring pattern worth surfacing.
    MemoryInsight {
        summary: String,
    },
    /// A configured trigger rule fired (routine deviation, contextual rule, etc.).
    TriggerRuleFired {
        rule_name: String,
        description: String,
    },
}

// ─── Daemon → Neocortex messages ────────────────────────────────────────────

/// Messages sent from the daemon process to the neocortex (LLM) process.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum DaemonToNeocortex {
    /// Load a model from disk.
    Load {
        model_path: String,
        params: ModelParams,
    },
    /// Graceful unload (finish current work first).
    Unload,
    /// Immediate unload (drop everything).
    UnloadImmediate,
    /// Plan actions for a goal.
    Plan {
        context: ContextPackage,
        failure: Option<FailureContext>,
    },
    /// Re-plan after a failure with failure context.
    Replan {
        context: ContextPackage,
        failure: FailureContext,
    },
    /// Have a conversation with the user.
    Converse { context: ContextPackage },
    /// Compose a DSL script from a template.
    Compose {
        context: ContextPackage,
        template: String,
    },
    /// Cancel current inference.
    Cancel,
    /// Health check.
    Ping,
    /// Request to embed text into a neural vector.
    Embed { text: String },
    /// One step of a ReAct loop: send action result + screen state for LLM decision.
    ReActStep {
        /// The tool/action that was just executed.
        tool_name: String,
        /// The raw observation string from executing the action.
        // TODO(types): Phase 4 wire-point — typed payload enum pending IPC finalization
        observation: String,
        /// Current screen description captured after the action.
        screen_description: String,
        /// The original goal being pursued.
        goal: String,
        /// How many steps have been taken so far.
        step_index: u32,
        /// Maximum steps allowed for this task.
        max_steps: u32,
    },
    /// A proactive opportunity detected by the daemon.
    ///
    /// The daemon sends typed structured data; the LLM generates the natural-language
    /// message for the user. **Never send format strings here — send typed facts.**
    ///
    /// The neocortex must respond with [`NeocortexToDaemon::ConversationReply`].
    ProactiveContext {
        /// The type of proactive trigger that fired, carrying only raw factual data.
        trigger: ProactiveTrigger,
        /// Full user context for the LLM to reason about (OCEAN, VAD, memories, etc.).
        context: ContextPackage,
    },
    /// Ask the LLM to generalize/summarize a prompt. Used during the memory
    /// consolidation (dreaming) phase. Rust never reasons — only the LLM may
    /// synthesize meaning from raw episodes.
    ///
    /// The neocortex must respond with [`NeocortexToDaemon::Summary`].
    Summarize {
        /// The full prompt to send to the LLM, including the raw episode text.
        prompt: String,
    },
    /// Ask the LLM to score a candidate plan (0.0–1.0).
    ///
    /// The neocortex must respond with [`NeocortexToDaemon::PlanScore`].
    ScorePlan { plan: ActionPlan },
    /// Ask the LLM to classify a failure string into a [`FailureCategory`]-
    /// equivalent label.
    ///
    /// The neocortex must respond with [`NeocortexToDaemon::FailureClassification`].
    ClassifyFailure {
        /// The raw error string produced by the failing operation.
        error: String,
        /// Serialised environment / context hints for the LLM.
        context: String,
    },
}

// ─── Neocortex → Daemon messages ────────────────────────────────────────────

/// Messages sent from the neocortex (LLM) process back to the daemon.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum NeocortexToDaemon {
    /// Model loaded successfully.
    Loaded {
        model_name: String,
        memory_used_mb: u32,
    },
    /// Model failed to load.
    LoadFailed { reason: String },
    /// Model unloaded.
    Unloaded,
    /// Action plan ready.
    PlanReady {
        plan: ActionPlan,
        /// Tokens consumed generating this plan. 0 = not reported by neocortex.
        #[serde(default)]
        tokens_used: u32,
    },
    /// Conversational reply.
    ConversationReply {
        text: String,
        mood_hint: Option<f32>,
        /// Tokens consumed generating this reply. 0 = not reported by neocortex.
        #[serde(default)]
        tokens_used: u32,
    },
    /// Composed DSL script.
    ComposedScript { steps: Vec<DslStep> },
    /// Progress update during inference.
    Progress { percent: u8, stage: String },
    /// Error during inference.
    Error { code: u16, message: String },
    /// Health check response.
    Pong { uptime_ms: u64 },
    /// Memory pressure warning.
    MemoryWarning { used_mb: u32, available_mb: u32 },
    /// Token budget exhausted during inference.
    TokenBudgetExhausted,
    /// Response to an Embed request.
    Embedding { vector: Vec<f32> },
    /// LLM decision after processing a ReAct step.
    ReActDecision {
        /// Whether the LLM considers the goal complete.
        done: bool,
        /// Reasoning / explanation from the LLM.
        reasoning: String,
        /// Next action to take, if not done.
        // TODO(types): Phase 4 wire-point — typed payload enum pending IPC finalization
        next_action: Option<String>,
        /// Tokens consumed generating this decision. 0 = not reported by neocortex.
        #[serde(default)]
        tokens_used: u32,
    },
    /// Response to a [`DaemonToNeocortex::Summarize`] request.
    ///
    /// Contains the LLM-generated generalization/insight derived from raw episodes.
    Summary {
        /// The synthesized text from the LLM.
        text: String,
        /// Tokens consumed generating this summary. 0 = not reported by neocortex.
        #[serde(default)]
        tokens_used: u32,
    },
    /// Response to a [`DaemonToNeocortex::ScorePlan`] request.
    ///
    /// Contains a score in the range \[0.0, 1.0\] for the candidate plan.
    PlanScore {
        /// Quality score for the candidate plan. Higher is better.
        score: f32,
    },
    /// Response to a [`DaemonToNeocortex::ClassifyFailure`] request.
    ///
    /// Contains the LLM-assigned failure category label matching
    /// `FailureCategory` variant names: `"Transient"`, `"Strategic"`,
    /// `"Environmental"`, `"Capability"`, or `"Safety"`.
    FailureClassification {
        /// Failure category name as a string (matches `FailureCategory` variant).
        category: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_inference_mode_constants() {
        assert!((InferenceMode::Planner.temperature() - 0.1).abs() < f32::EPSILON);
        assert_eq!(InferenceMode::Strategist.max_tokens(), 4096);
        assert!((InferenceMode::Conversational.top_p() - 0.95).abs() < f32::EPSILON);
        assert_eq!(InferenceMode::Composer.max_tokens(), 1024);
    }

    #[test]
    fn test_context_package_estimated_size() {
        let ctx = ContextPackage::default();
        let size = ctx.estimated_size();
        // Empty package should be small (just fixed overhead)
        assert!(size < 200);

        // Build a larger context
        let mut ctx = ContextPackage::default();
        ctx.conversation_history.push(ConversationTurn {
            role: Role::User,
            content: "a".repeat(1000),
            timestamp_ms: 0,
        });
        ctx.memory_snippets.push(MemorySnippet {
            content: "b".repeat(2000),
            source: MemoryTier::Episodic,
            relevance: 0.9,
            timestamp_ms: 0,
        });
        let size = ctx.estimated_size();
        assert!(size > 3000);
        assert!(size < ContextPackage::MAX_SIZE);
    }

    #[test]
    fn test_model_params_default() {
        let params = ModelParams::default();
        assert_eq!(params.n_ctx, 4096);
        assert_eq!(params.n_threads, 4);
        assert_eq!(params.model_tier, ModelTier::Standard4B);
    }

    #[test]
    fn test_failure_context_size() {
        let fc = FailureContext {
            task_goal_hash: 0,
            current_step: 0,
            failing_action: 0,
            target_id: 0,
            expected_state_hash: 0,
            actual_state_hash: 0,
            tried_approaches: 0,
            last_3_transitions: [TransitionPair {
                from_hash: 0,
                to_hash: 0,
            }; 3],
            error_class: 0,
        };
        let size = std::mem::size_of_val(&fc);
        assert!(size <= 120, "FailureContext too large: {} bytes", size);
    }

    #[test]
    fn test_daemon_to_neocortex_ping() {
        let msg = DaemonToNeocortex::Ping;
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("Ping"));
    }

    #[test]
    fn test_daemon_to_neocortex_embed() {
        let msg = DaemonToNeocortex::Embed { text: "hello".to_string() };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("Embed"));
        assert!(json.contains("hello"));
    }

    #[test]
    fn test_personality_snapshot_default() {
        let p = PersonalitySnapshot::default();
        assert!((p.openness - 0.85).abs() < f32::EPSILON);
        assert!((p.trust_level - 0.0).abs() < f32::EPSILON);
    }
}
