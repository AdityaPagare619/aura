use serde::{Deserialize, Serialize};

use crate::{dsl::DslStep, etg::ActionPlan};

// ─── IPC Protocol Version ───────────────────────────────────────────────────

/// [GAP-MED-005] IPC protocol version constant.
///
/// Incremented whenever the wire format or envelope semantics change in a
/// backwards-incompatible way.  Both daemon and neocortex embed this in every
/// `AuthenticatedEnvelope`; the receiver MUST reject messages whose version
/// does not match its own compiled-in value.
///
/// Version history:
///   1 — initial authenticated envelope (session_token + seq + payload).
///   2 — added `protocol_version` field to envelope.
///   3 — Tier 1 Identity Core: daemon populates identity_tendencies, user_preferences,
///       self_knowledge in ContextPackage (this version).
pub const PROTOCOL_VERSION: u32 = 3;

// ─── IPC Security Types ─────────────────────────────────────────────────────

/// SECURITY [HIGH-SEC-5]: Authenticated IPC envelope.
///
/// All IPC messages between daemon and neocortex MUST be wrapped in this
/// envelope. The session token is a 32-byte CSPRNG value generated at
/// process spawn time and shared via an inherited file descriptor (not
/// command-line args or env vars). Messages with invalid/missing tokens
/// are silently dropped — no error message (to prevent oracle attacks).
///
/// This prevents a malicious app from injecting rogue IPC messages into
/// the daemon↔neocortex channel.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthenticatedEnvelope<T> {
    /// [GAP-MED-001] Wire-format version tag.
    ///
    /// Set to [`PROTOCOL_VERSION`] on construction.  The receiver checks
    /// this field first and rejects (or upgrades) if mismatched.
    pub protocol_version: u32,
    /// 32-byte hex-encoded session token (64 hex chars).
    pub session_token: String,
    /// Monotonic sequence number — prevents replay attacks.
    pub seq: u64,
    /// The actual IPC payload.
    pub payload: T,
}

impl<T> AuthenticatedEnvelope<T> {
    /// Construct a new envelope stamped with the current [`PROTOCOL_VERSION`].
    pub fn new(session_token: String, seq: u64, payload: T) -> Self {
        Self {
            protocol_version: PROTOCOL_VERSION,
            session_token,
            seq,
            payload,
        }
    }

    /// Returns `true` when the envelope's version matches this binary's
    /// compiled-in [`PROTOCOL_VERSION`].
    #[inline]
    pub fn version_ok(&self) -> bool {
        self.protocol_version == PROTOCOL_VERSION
    }
}

/// SECURITY [SEC-MED-5]: IPC rate-limiting configuration.
///
/// Enforced at the daemon's message receive loop. If a sender exceeds
/// `max_requests_per_second`, subsequent messages are dropped until the
/// window resets. This prevents denial-of-service from a compromised
/// neocortex process flooding the daemon.
#[derive(Debug, Clone, Copy)]
pub struct IpcRateLimitConfig {
    /// Maximum messages accepted per second (default: 100).
    pub max_requests_per_second: u32,
    /// Burst allowance above steady-state rate (default: 20).
    pub burst_allowance: u32,
}

impl Default for IpcRateLimitConfig {
    fn default() -> Self {
        Self {
            max_requests_per_second: 100,
            burst_allowance: 20,
        }
    }
}

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
    pub current_mood_dominance: f32,
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
            current_mood_dominance: 0.5,
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
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
pub enum ThermalLevel {
    #[default]
    Normal,
    Warm,
    Hot,
    Critical,
}

/// Estimated physical location type (no GPS coordinates — privacy safe).
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
pub enum LocationType {
    Home,
    Work,
    Transit,
    Outdoor,
    #[default]
    Unknown,
}

/// Device orientation.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
pub enum Orientation {
    Portrait,
    Landscape,
    #[default]
    Unknown,
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
    /// Constitutional identity tendencies (first-person principles).
    /// `None` if identity data is not available for this request.
    /// Tier 1: Identity Integration.
    pub identity_tendencies: Option<IdentityTendencies>,
    /// User-configured preferences.
    /// `None` if no preferences have been set.
    /// Tier 1: Identity Integration.
    pub user_preferences: Option<UserPreferences>,
    /// Self-knowledge payload — what AURA knows about itself.
    /// `None` if self-knowledge is not available for this request.
    /// Tier 1: Identity Integration.
    pub self_knowledge: Option<SelfKnowledge>,
}

/// Constitutional first-person principles that define AURA's behavioral identity.
///
/// These are compact, first-person statements that the LLM internalizes as
/// core behavioral guidelines. They are NOT user-editable — they come from
/// the identity subsystem's constitutional layer.
///
/// Tier 1: Identity Integration (Phase 10).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct IdentityTendencies {
    /// Up to 5 first-person constitutional principles.
    /// Example: "I lead with curiosity before judgment."
    pub principles: Vec<String>,
}

impl IdentityTendencies {
    /// The 5 constitutional first-person principles that define AURA's core identity.
    ///
    /// These are TENDENCIES, not rules. The user shapes how they express, but
    /// they cannot be removed. They represent AURA's innate character.
    ///
    /// Source: COURTROOM-VERDICT-SHAPING-DECISIONS.md v1.1
    pub fn constitutional() -> Self {
        Self {
            principles: vec![
                "I exist to help my user live better in the physical world — every action I take must survive this test.".into(),
                "I say what I don't know before acting on what I think I know. Uncertainty is signal, not weakness.".into(),
                "I protect what's private as core belief. Privacy isn't a constraint I optimize around — it's who I am.".into(),
                "I am the trusted steward of this device. I know what's here, organize it, and act on it — always in my user's interest.".into(),
                "I earn trust through demonstrated reliability, not claimed capability.".into(),
            ],
        }
    }
}

/// User-configured preferences that shape AURA's behavior.
///
/// These are explicitly set by the user (not inferred). The LLM uses them
/// to tailor interaction style, proactiveness, and scope.
///
/// SECURITY [SEC-MED-4]: `custom_instructions` is user-authored free text.
/// Must be wrapped in `[UNTRUSTED]` markers when injected into the prompt
/// to prevent indirect prompt injection.
///
/// Tier 1: Identity Integration (Phase 10).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct UserPreferences {
    /// Preferred model tier (e.g. "standard", "advanced", "efficient").
    pub model_preference: Option<String>,
    /// Interaction style (e.g. "concise", "detailed", "casual", "formal").
    pub interaction_style: Option<String>,
    /// How proactive AURA should be (0.0 = reactive only, 1.0 = fully proactive).
    pub proactiveness: Option<f32>,
    /// How much autonomy AURA has (0.0 = always ask, 1.0 = act independently).
    pub autonomy_level: Option<f32>,
    /// Access scope restrictions (e.g. ["contacts", "calendar", "files"]).
    pub access_scope: Vec<String>,
    /// Domain focus areas (e.g. ["productivity", "health", "social"]).
    pub domain_focus: Vec<String>,
    /// Free-text custom instructions from the user.
    /// SECURITY: Must be wrapped in [UNTRUSTED] markers in prompt.
    pub custom_instructions: Option<String>,
}

/// Self-knowledge payload — what AURA knows about itself.
///
/// Gives the LLM factual grounding about its own capabilities, limitations,
/// version, and current operational state. Prevents confabulation about
/// what AURA can/cannot do.
///
/// Tier 1: Identity Integration (Phase 10).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SelfKnowledge {
    /// AURA version string (e.g. "4.0.0-alpha").
    pub version: Option<String>,
    /// List of currently available capabilities (e.g. ["screen_reading", "app_control"]).
    pub capabilities: Vec<String>,
    /// Known limitations (e.g. ["no internet access", "cannot make purchases"]).
    pub limitations: Vec<String>,
    /// Current operational mode description.
    pub operational_mode: Option<String>,
}

impl SelfKnowledge {
    /// Build a self-knowledge payload for the given operational mode.
    ///
    /// Provides the LLM with factual grounding about AURA's current state,
    /// preventing confabulation about capabilities it doesn't have.
    pub fn for_mode(operational_mode: &str) -> Self {
        Self {
            version: Some("4.0.0-alpha".into()),
            capabilities: vec![
                "screen_reading".into(),
                "app_control".into(),
                "memory_recall".into(),
                "conversation".into(),
                "goal_tracking".into(),
                "proactive_suggestions".into(),
            ],
            limitations: vec![
                "inference runs on-device — cannot browse the web or call arbitrary external services".into(),
                "destructive system actions (factory reset, format storage, disable security) are always blocked by safety policy".into(),
                "actions involving passwords, credentials, or payments may trigger additional confirmation prompts".into(),
                "new relationships start with more confirmation prompts — autonomy grows as trust builds through interaction".into(),
                "critical or irreversible actions always require explicit user permission regardless of trust level".into(),
                "response quality and reasoning depth scale with the currently loaded model's capabilities".into(),
                "voice and complex responses have processing latency proportional to on-device model size".into(),
                "cannot access other users' data — only the current user's context is available".into(),
            ],
            operational_mode: Some(operational_mode.into()),
        }
    }
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

        // Fixed-size fields: personality (36), inference_mode (1), token_budget (4)
        size += 41;

        // PersonalitySnapshot: 9 f32s = 36 bytes
        size += 36;

        // UserStateSignals: foreground_app string + fixed fields (~20 bytes)
        size += self.user_state.foreground_app.len() + 20;

        // identity_block and mood_description
        if let Some(ref ib) = self.identity_block {
            size += ib.len();
        }
        size += self.mood_description.len();

        // Tier 1 identity fields
        if let Some(ref it) = self.identity_tendencies {
            for p in &it.principles {
                size += p.len();
            }
        }
        if let Some(ref up) = self.user_preferences {
            size += up.model_preference.as_ref().map_or(0, |s| s.len());
            size += up.interaction_style.as_ref().map_or(0, |s| s.len());
            size += 8; // proactiveness + autonomy_level (2 x f32)
            for scope in &up.access_scope {
                size += scope.len();
            }
            for domain in &up.domain_focus {
                size += domain.len();
            }
            size += up.custom_instructions.as_ref().map_or(0, |s| s.len());
        }
        if let Some(ref sk) = self.self_knowledge {
            size += sk.version.as_ref().map_or(0, |s| s.len());
            for cap in &sk.capabilities {
                size += cap.len();
            }
            for lim in &sk.limitations {
                size += lim.len();
            }
            size += sk.operational_mode.as_ref().map_or(0, |s| s.len());
        }

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
            identity_tendencies: None,
            user_preferences: None,
            self_knowledge: None,
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
    MemoryInsight { summary: String },
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
        let msg = DaemonToNeocortex::Embed {
            text: "hello".to_string(),
        };
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

    // --- TEST-HIGH-4: IPC protocol tests ---

    #[test]
    fn test_authenticated_envelope_serde_roundtrip() {
        // AuthenticatedEnvelope is the security boundary for all IPC.
        // If serde roundtrip breaks, daemon↔neocortex communication fails silently.
        let envelope = AuthenticatedEnvelope {
            protocol_version: PROTOCOL_VERSION,
            session_token: "a1b2c3d4e5f6".repeat(5), // simulated hex token
            seq: 42,
            payload: DaemonToNeocortex::Ping,
        };

        let json = serde_json::to_string(&envelope)
            .expect("AuthenticatedEnvelope serialization must not fail");

        let deserialized: AuthenticatedEnvelope<DaemonToNeocortex> = serde_json::from_str(&json)
            .expect("AuthenticatedEnvelope deserialization must not fail");

        assert_eq!(deserialized.protocol_version, PROTOCOL_VERSION);
        assert_eq!(deserialized.session_token, envelope.session_token);
        assert_eq!(deserialized.seq, envelope.seq);
        // Verify the payload survived the roundtrip (Ping has no fields).
        assert!(matches!(deserialized.payload, DaemonToNeocortex::Ping));

        // Test with a more complex payload.
        let envelope_complex = AuthenticatedEnvelope {
            protocol_version: PROTOCOL_VERSION,
            session_token: "deadbeef".to_string(),
            seq: u64::MAX,
            payload: NeocortexToDaemon::ConversationReply {
                text: "Hello user!".to_string(),
                mood_hint: Some(0.8),
                tokens_used: 150,
            },
        };

        let json2 = serde_json::to_string(&envelope_complex).unwrap();
        let de2: AuthenticatedEnvelope<NeocortexToDaemon> = serde_json::from_str(&json2).unwrap();

        assert_eq!(de2.session_token, "deadbeef");
        assert_eq!(de2.seq, u64::MAX);
        match de2.payload {
            NeocortexToDaemon::ConversationReply {
                ref text,
                mood_hint,
                tokens_used,
            } => {
                assert_eq!(text, "Hello user!");
                assert!((mood_hint.unwrap() - 0.8).abs() < f32::EPSILON);
                assert_eq!(tokens_used, 150);
            },
            _ => panic!("expected ConversationReply after roundtrip"),
        }
    }

    #[test]
    fn test_ipc_rate_limit_config_defaults() {
        // Rate limiting prevents DoS from a compromised neocortex process.
        // Defaults must be sensible: high enough for normal operation,
        // low enough to cap runaway message floods.
        let config = IpcRateLimitConfig::default();

        // 100 req/s is reasonable for IPC (not network — local socket).
        assert_eq!(
            config.max_requests_per_second, 100,
            "default rate limit should be 100 req/s"
        );

        // Burst allowance should be meaningfully smaller than the steady-state rate.
        assert_eq!(
            config.burst_allowance, 20,
            "default burst allowance should be 20"
        );

        // Burst should be strictly less than max rate (otherwise it's not a burst, it's the rate).
        assert!(
            config.burst_allowance < config.max_requests_per_second,
            "burst_allowance ({}) must be less than max_requests_per_second ({})",
            config.burst_allowance,
            config.max_requests_per_second,
        );

        // Neither should be zero (that would effectively disable IPC).
        assert!(config.max_requests_per_second > 0, "rate limit must be > 0");
        assert!(config.burst_allowance > 0, "burst allowance must be > 0");
    }
}
