use serde::{Deserialize, Serialize};

use crate::dsl::DslStep;
use crate::etg::ActionPlan;

// ─── Sub-types used throughout IPC ──────────────────────────────────────────

/// Model quantisation / size tier.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
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
    pub interactive_elements: Vec<String>,
    pub visible_text: Vec<String>,
}

/// Summary of active goal progress.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GoalSummary {
    pub description: String,
    pub progress_percent: u8,
    pub current_step: String,
    pub blockers: Vec<String>,
}

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

/// Current user activity state.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum UserState {
    Active,
    Idle,
    Sleeping,
    Driving,
    InMeeting,
    Unknown,
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
    pub conversation_history: Vec<ConversationTurn>,
    pub memory_snippets: Vec<MemorySnippet>,
    pub current_screen: Option<ScreenSummary>,
    pub active_goal: Option<GoalSummary>,
    pub personality: PersonalitySnapshot,
    pub user_state: UserState,
    pub inference_mode: InferenceMode,
    pub token_budget: u32,
}

impl ContextPackage {
    /// Maximum allowed context package size (64 KB).
    pub const MAX_SIZE: usize = 65_536;

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

        // Fixed-size fields: personality (32), user_state (1), inference_mode (1), token_budget (4)
        size += 38;

        // PersonalitySnapshot: 8 f32s = 32 bytes
        size += 32;

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
            user_state: UserState::Unknown,
            inference_mode: InferenceMode::Conversational,
            token_budget: 2048,
        }
    }
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
    PlanReady { plan: ActionPlan },
    /// Conversational reply.
    ConversationReply {
        text: String,
        mood_hint: Option<f32>,
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
    fn test_personality_snapshot_default() {
        let p = PersonalitySnapshot::default();
        assert!((p.openness - 0.85).abs() < f32::EPSILON);
        assert!((p.trust_level - 0.0).abs() < f32::EPSILON);
    }
}
