use serde::{Deserialize, Serialize};

/// Top-level configuration for the entire AURA system.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AuraConfig {
    pub daemon: DaemonConfig,
    pub amygdala: AmygdalaConfig,
    pub neocortex: NeocortexConfig,
    pub execution: ExecutionConfig,
    pub power: PowerConfig,
    pub identity: IdentityConfig,
    pub sqlite: SqliteConfig,
    #[serde(default)]
    pub routing: RoutingConfig,
    #[serde(default)]
    pub screen: ScreenConfig,
    #[serde(default)]
    pub etg: EtgConfig,
    #[serde(default)]
    pub goals: GoalsConfig,
    #[serde(default)]
    pub cron: CronConfig,
    #[serde(default)]
    pub telegram: TelegramConfig,
    #[serde(default)]
    pub voice: VoiceConfig,
    #[serde(default)]
    pub proactive: ProactiveConfig,
    #[serde(default)]
    pub policy: PolicyConfig,
    #[serde(default)]
    pub onboarding: OnboardingConfig,
    #[serde(default)]
    pub token_budget: TokenBudgetConfig,
}

/// Daemon process configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DaemonConfig {
    /// Seconds between state checkpoint writes.
    pub checkpoint_interval_s: u32,
    /// RSS memory warning threshold (MB).
    pub rss_warning_mb: u32,
    /// RSS memory hard ceiling (MB) — daemon will shed load above this.
    pub rss_ceiling_mb: u32,
    /// AURA version string (e.g. "4.0.0-alpha.1").
    #[serde(default = "default_daemon_version")]
    pub version: String,
    /// Log level: "trace" | "debug" | "info" | "warn" | "error".
    #[serde(default = "default_daemon_log_level")]
    pub log_level: String,
    /// Root data directory for AURA files.
    #[serde(default = "default_daemon_data_dir")]
    pub data_dir: String,
}

fn default_daemon_version() -> String {
    "4.0.0-alpha.1".to_string()
}
fn default_daemon_log_level() -> String {
    "info".to_string()
}
fn default_daemon_data_dir() -> String {
    "/data/data/com.aura/files".to_string()
}

impl Default for DaemonConfig {
    fn default() -> Self {
        Self {
            checkpoint_interval_s: 300,
            rss_warning_mb: 28,
            rss_ceiling_mb: 30,
            version: "4.0.0-alpha.1".to_string(),
            log_level: "info".to_string(),
            data_dir: "/data/data/com.aura/files".to_string(),
        }
    }
}

/// Amygdala (event scoring / gating) configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AmygdalaConfig {
    /// Score threshold for InstantWake gate decision.
    pub instant_threshold: f32,
    /// Weight for lexical keyword scoring.
    pub weight_lex: f32,
    /// Weight for source-based scoring.
    pub weight_src: f32,
    /// Weight for temporal relevance scoring.
    pub weight_time: f32,
    /// Weight for anomaly scoring.
    pub weight_anom: f32,
    /// Size of the deduplication ring buffer for notification storms.
    pub storm_dedup_size: u32,
    /// Minimum interval between same-source events (ms).
    pub storm_rate_limit_ms: u32,
    /// Number of events needed during cold start before normal gating.
    pub cold_start_events: u32,
    /// Hours of cold start learning period.
    pub cold_start_hours: u32,
}

impl Default for AmygdalaConfig {
    fn default() -> Self {
        Self {
            instant_threshold: 0.65,
            weight_lex: 0.40,
            weight_src: 0.25,
            weight_time: 0.20,
            weight_anom: 0.15,
            storm_dedup_size: 50,
            storm_rate_limit_ms: 30_000,
            cold_start_events: 200,
            cold_start_hours: 72,
        }
    }
}

impl AmygdalaConfig {
    /// Verify that the scoring weights sum to 1.0 (within tolerance).
    #[must_use]
    pub fn weights_valid(&self) -> bool {
        let sum = self.weight_lex + self.weight_src + self.weight_time + self.weight_anom;
        (sum - 1.0).abs() < 0.01
    }
}

/// Neocortex (LLM) process configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NeocortexConfig {
    /// Default context window size (tokens).
    pub default_n_ctx: u32,
    /// Number of CPU threads for inference.
    pub n_threads: u32,
    /// Max memory for model loading (MB).
    pub max_memory_mb: u32,
    /// Timeout for a single inference call (ms).
    pub inference_timeout_ms: u32,
    /// Path to model files directory.
    pub model_dir: String,
    /// Default model identifier — used for display, logging, and as a hint
    /// to `ModelScanner` when selecting which GGUF file to prefer.
    /// Users can override this in `aura.config.toml`; the compiled default
    /// is Qwen3-8B (Full8B tier, Q4_K_M quantization).
    #[serde(default = "default_model_name")]
    pub default_model_name: String,
    /// Absolute path to the default GGUF model file.
    /// Resolved at startup; `~` is expanded to the user home directory.
    /// If empty or the file does not exist, `ModelScanner` falls back to
    /// scanning `model_dir` and selecting by RAM estimate.
    /// Default: `~/aura/models/qwen3-8b-q4_k_m.gguf`
    #[serde(default = "default_model_path")]
    pub default_model_path: String,
    /// Native context window for the default model (tokens).
    /// Set to Qwen3-8B's full 32 K context; lower this on memory-constrained
    /// devices or override per-request via `ModelParams::n_ctx`.
    #[serde(default = "default_model_context_size")]
    pub default_model_context_size: u32,
}

fn default_model_name() -> String {
    "Qwen3-8B-Q4_K_M".to_string()
}
fn default_model_path() -> String {
    "~/aura/models/qwen3-8b-q4_k_m.gguf".to_string()
}
fn default_model_context_size() -> u32 {
    32_768
}

impl Default for NeocortexConfig {
    fn default() -> Self {
        Self {
            default_n_ctx: 4096,
            n_threads: 4,
            max_memory_mb: 2048,
            inference_timeout_ms: 60_000,
            model_dir: "/data/local/tmp/aura/models".to_string(),
            default_model_name: default_model_name(),
            default_model_path: default_model_path(),
            default_model_context_size: default_model_context_size(),
        }
    }
}

/// Execution engine configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionConfig {
    /// Max steps in Normal safety mode.
    pub max_steps_normal: u32,
    /// Max steps in Safety mode.
    pub max_steps_safety: u32,
    /// Max steps in Power mode.
    pub max_steps_power: u32,
    /// Max actions per minute rate limit.
    pub rate_limit_actions_per_min: u32,
    /// Minimum delay between actions (ms) — human-like pacing.
    pub delay_min_ms: u32,
    /// Maximum delay between actions (ms).
    pub delay_max_ms: u32,
}

impl Default for ExecutionConfig {
    fn default() -> Self {
        Self {
            max_steps_normal: 200,
            max_steps_safety: 50,
            max_steps_power: 500,
            rate_limit_actions_per_min: 60,
            delay_min_ms: 150,
            delay_max_ms: 500,
        }
    }
}

/// Power management configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PowerConfig {
    /// Daily token budget for LLM inference.
    pub daily_token_budget: u32,
    /// Battery level to enter conservative mode.
    pub conservative_threshold: u8,
    /// Battery level to enter low-power mode.
    pub low_power_threshold: u8,
    /// Battery level to enter critical mode.
    pub critical_threshold: u8,
    /// Battery level to enter emergency mode.
    pub emergency_threshold: u8,
}

impl Default for PowerConfig {
    fn default() -> Self {
        Self {
            daily_token_budget: 50_000,
            conservative_threshold: 50,
            low_power_threshold: 30,
            critical_threshold: 15,
            emergency_threshold: 5,
        }
    }
}

/// Identity / personality configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IdentityConfig {
    /// Minimum time between mood updates (ms).
    pub mood_cooldown_ms: u64,
    /// Maximum mood shift per update.
    pub max_mood_delta: f32,
    /// Trust hysteresis gap for relationship stage transitions.
    pub trust_hysteresis: f32,
}

impl Default for IdentityConfig {
    fn default() -> Self {
        Self {
            mood_cooldown_ms: 60_000,
            max_mood_delta: 0.2,
            trust_hysteresis: 0.05,
        }
    }
}

/// SQLite storage configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SqliteConfig {
    /// Path to the SQLite database file.
    pub db_path: String,
    /// WAL mode journal size limit (bytes).
    pub wal_size_limit: u64,
    /// Max number of episodic memories before consolidation.
    pub max_episodes: u32,
    /// Max number of semantic entries.
    pub max_semantic_entries: u32,
}

impl Default for SqliteConfig {
    fn default() -> Self {
        Self {
            db_path: "/data/data/com.aura/databases/aura.db".to_string(),
            wal_size_limit: 4 * 1024 * 1024, // 4MB
            max_episodes: 10_000,
            max_semantic_entries: 5_000,
        }
    }
}

// =========================================================================
//  New subsystem config structs (added for v4 config expansion)
// =========================================================================

/// Routing subsystem configuration — controls how events are dispatched
/// to different processing tiers (fast-path vs neocortex).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoutingConfig {
    /// Score threshold for routing to the complex (neocortex) path.
    #[serde(default = "default_complexity_threshold")]
    pub complexity_threshold: f32,
    /// Hysteresis gap to prevent route flapping near the threshold.
    #[serde(default = "default_hysteresis_gap")]
    pub hysteresis_gap: f32,
    /// Number of working memory slots available for routing decisions.
    #[serde(default = "default_working_memory_slots")]
    pub working_memory_slots: u32,
    /// Weight for complexity factor in routing score (must sum to 1.0 with others).
    #[serde(default = "default_weight_complexity")]
    pub weight_complexity: f32,
    /// Weight for importance factor.
    #[serde(default = "default_weight_importance")]
    pub weight_importance: f32,
    /// Weight for urgency factor.
    #[serde(default = "default_weight_urgency")]
    pub weight_urgency: f32,
    /// Weight for memory load factor.
    #[serde(default = "default_weight_memory_load")]
    pub weight_memory_load: f32,
}

fn default_complexity_threshold() -> f32 {
    0.50
}
fn default_hysteresis_gap() -> f32 {
    0.15
}
fn default_working_memory_slots() -> u32 {
    7
}
fn default_weight_complexity() -> f32 {
    0.40
}
fn default_weight_importance() -> f32 {
    0.25
}
fn default_weight_urgency() -> f32 {
    0.20
}
fn default_weight_memory_load() -> f32 {
    0.15
}

impl Default for RoutingConfig {
    fn default() -> Self {
        Self {
            complexity_threshold: 0.50,
            hysteresis_gap: 0.15,
            working_memory_slots: 7,
            weight_complexity: 0.40,
            weight_importance: 0.25,
            weight_urgency: 0.20,
            weight_memory_load: 0.15,
        }
    }
}

impl RoutingConfig {
    /// Verify that the four routing weights sum to 1.0 (within tolerance).
    /// Mirrors `AmygdalaConfig::weights_valid()` to prevent silent score corruption.
    /// See: AURA-V4-BATCH3-FOUNDATION-AUDIT §1.2 — "RoutingConfig has NO weights_valid()".
    #[must_use]
    pub fn weights_valid(&self) -> bool {
        let sum = self.weight_complexity
            + self.weight_importance
            + self.weight_urgency
            + self.weight_memory_load;
        (sum - 1.0).abs() < 0.01
    }
}

/// Screen capture / accessibility tree configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScreenConfig {
    /// Maximum depth for accessibility tree traversal.
    #[serde(default = "default_max_tree_depth")]
    pub max_tree_depth: u32,
    /// Timeout for tree snapshot (ms).
    #[serde(default = "default_snapshot_timeout_ms")]
    pub snapshot_timeout_ms: u32,
    /// Maximum nodes to serialize per snapshot.
    #[serde(default = "default_max_nodes")]
    pub max_nodes: u32,
    /// Enable screen content hashing for change detection.
    #[serde(default = "default_enable_hash_diff")]
    pub enable_hash_diff: bool,
}

fn default_max_tree_depth() -> u32 {
    15
}
fn default_snapshot_timeout_ms() -> u32 {
    2000
}
fn default_max_nodes() -> u32 {
    500
}
fn default_enable_hash_diff() -> bool {
    true
}

impl Default for ScreenConfig {
    fn default() -> Self {
        Self {
            max_tree_depth: 15,
            snapshot_timeout_ms: 2000,
            max_nodes: 500,
            enable_hash_diff: true,
        }
    }
}

/// Execution Task Graph (ETG) configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EtgConfig {
    /// Maximum depth of the task graph before aborting.
    #[serde(default = "default_max_graph_depth")]
    pub max_graph_depth: u32,
    /// Timeout for a single ETG step (ms).
    #[serde(default = "default_step_timeout_ms")]
    pub step_timeout_ms: u32,
    /// Enable rollback on step failure.
    #[serde(default = "default_enable_rollback")]
    pub enable_rollback: bool,
    /// Maximum retry attempts per step.
    #[serde(default = "default_max_step_retries")]
    pub max_step_retries: u32,
}

fn default_max_graph_depth() -> u32 {
    10
}
fn default_step_timeout_ms() -> u32 {
    5000
}
fn default_enable_rollback() -> bool {
    true
}
fn default_max_step_retries() -> u32 {
    2
}

impl Default for EtgConfig {
    fn default() -> Self {
        Self {
            max_graph_depth: 10,
            step_timeout_ms: 5000,
            enable_rollback: true,
            max_step_retries: 2,
        }
    }
}

/// Goal management configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GoalsConfig {
    /// Maximum concurrent active goals.
    #[serde(default = "default_goals_max_active")]
    pub max_active: u32,
    /// Default goal deadline (seconds, 0 = no deadline).
    #[serde(default)]
    pub default_deadline_s: u32,
    /// Maximum sub-goal decomposition depth.
    #[serde(default = "default_max_decomposition")]
    pub max_decomposition: u32,
    /// Maximum retry attempts per goal before marking failed.
    #[serde(default = "default_goals_max_retries")]
    pub max_retries: u32,
    /// Scheduler tick interval (ms).
    #[serde(default = "default_scheduler_tick_ms")]
    pub scheduler_tick_ms: u32,
}

fn default_goals_max_active() -> u32 {
    5
}
fn default_max_decomposition() -> u32 {
    5
}
fn default_goals_max_retries() -> u32 {
    3
}
fn default_scheduler_tick_ms() -> u32 {
    5000
}

impl Default for GoalsConfig {
    fn default() -> Self {
        Self {
            max_active: 5,
            default_deadline_s: 0,
            max_decomposition: 5,
            max_retries: 3,
            scheduler_tick_ms: 5000,
        }
    }
}

/// Cron / scheduled tasks configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CronConfig {
    /// Enable the built-in cron scheduler.
    #[serde(default = "default_cron_enabled")]
    pub enabled: bool,
    /// Minimum interval between cron ticks (ms).
    #[serde(default = "default_cron_min_interval_ms")]
    pub min_interval_ms: u32,
    /// Maximum queued cron jobs before shedding.
    #[serde(default = "default_cron_max_queue_size")]
    pub max_queue_size: u32,
}

fn default_cron_enabled() -> bool {
    true
}
fn default_cron_min_interval_ms() -> u32 {
    60_000
}
fn default_cron_max_queue_size() -> u32 {
    50
}

impl Default for CronConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            min_interval_ms: 60_000,
            max_queue_size: 50,
        }
    }
}

/// Telegram bot integration configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TelegramConfig {
    /// Enable Telegram bot integration.
    #[serde(default)]
    pub enabled: bool,
    /// Bot token (prefer AURA_TELEGRAM_TOKEN env var for security).
    #[serde(default)]
    pub bot_token: String,
    /// Allowed chat IDs (empty = deny all).
    /// Bounded: max MAX_TELEGRAM_ALLOWED_CHAT_IDS items enforced at load site.
    #[serde(default)]
    pub allowed_chat_ids: Vec<i64>,
    /// Polling interval (ms).
    #[serde(default = "default_telegram_poll_interval_ms")]
    pub poll_interval_ms: u32,
}

fn default_telegram_poll_interval_ms() -> u32 {
    2000
}

impl Default for TelegramConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            bot_token: String::new(),
            allowed_chat_ids: Vec::new(),
            poll_interval_ms: 2000,
        }
    }
}

/// Max allowed chat IDs in [`TelegramConfig`].
pub const MAX_TELEGRAM_ALLOWED_CHAT_IDS: usize = 32;

/// Voice input/output (TTS/STT) configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VoiceConfig {
    /// Enable voice input/output.
    #[serde(default)]
    pub enabled: bool,
    /// Wake word detection sensitivity (0.0 = off, 1.0 = max).
    #[serde(default = "default_wake_sensitivity")]
    pub wake_sensitivity: f32,
    /// TTS engine: "system" | "local" | "none".
    #[serde(default = "default_tts_engine")]
    pub tts_engine: String,
    /// STT engine: "system" | "whisper_local" | "none".
    #[serde(default = "default_stt_engine")]
    pub stt_engine: String,
    /// Max recording duration (ms).
    #[serde(default = "default_max_record_ms")]
    pub max_record_ms: u32,
}

fn default_wake_sensitivity() -> f32 {
    0.5
}
fn default_tts_engine() -> String {
    "system".to_string()
}
fn default_stt_engine() -> String {
    "system".to_string()
}
fn default_max_record_ms() -> u32 {
    30_000
}

impl Default for VoiceConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            wake_sensitivity: 0.5,
            tts_engine: "system".to_string(),
            stt_engine: "system".to_string(),
            max_record_ms: 30_000,
        }
    }
}

/// Proactive behavior configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProactiveConfig {
    /// Enable proactive suggestions and actions.
    #[serde(default = "default_proactive_enabled")]
    pub enabled: bool,
    /// Minimum confidence to surface a proactive suggestion.
    #[serde(default = "default_proactive_min_confidence")]
    pub min_confidence: f32,
    /// Cooldown between proactive nudges (ms).
    #[serde(default = "default_proactive_cooldown_ms")]
    pub cooldown_ms: u64,
    /// Maximum proactive actions per hour.
    #[serde(default = "default_proactive_max_per_hour")]
    pub max_per_hour: u32,
    /// Require user confirmation for proactive actions.
    #[serde(default = "default_proactive_require_confirmation")]
    pub require_confirmation: bool,
}

fn default_proactive_enabled() -> bool {
    true
}
fn default_proactive_min_confidence() -> f32 {
    0.70
}
fn default_proactive_cooldown_ms() -> u64 {
    300_000
}
fn default_proactive_max_per_hour() -> u32 {
    10
}
fn default_proactive_require_confirmation() -> bool {
    true
}

impl Default for ProactiveConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            min_confidence: 0.70,
            cooldown_ms: 300_000,
            max_per_hour: 10,
            require_confirmation: true,
        }
    }
}

/// PolicyGate configuration — loaded from safety.toml overlay.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicyConfig {
    /// Default effect when no rule matches: "allow" | "deny" | "audit" | "confirm".
    #[serde(default = "default_policy_effect")]
    pub default_effect: String,
    /// Whether to log all policy decisions (verbose mode).
    #[serde(default)]
    pub log_all_decisions: bool,
    /// Maximum rules evaluated per event (safety cap).
    #[serde(default = "default_max_rules_per_event")]
    pub max_rules_per_event: u32,
    /// Ordered list of policy rules (first match wins).
    /// Bounded: max MAX_POLICY_RULES items enforced at load site.
    #[serde(default)]
    pub rules: Vec<PolicyRuleConfig>,
}

fn default_policy_effect() -> String {
    "allow".to_string()
}
fn default_max_rules_per_event() -> u32 {
    100
}

impl Default for PolicyConfig {
    fn default() -> Self {
        Self {
            default_effect: "allow".to_string(),
            log_all_decisions: false,
            max_rules_per_event: 100,
            rules: Vec::new(),
        }
    }
}

/// Max policy rules in [`PolicyConfig`].
pub const MAX_POLICY_RULES: usize = 256;

/// A single PolicyGate rule — action glob pattern mapped to an effect.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicyRuleConfig {
    /// Human-readable rule name.
    pub name: String,
    /// Glob pattern matching the action string (e.g. "*factory*reset*").
    pub action: String,
    /// Effect: "allow" | "deny" | "audit" | "confirm".
    pub effect: String,
    /// Explanation surfaced to user on deny/confirm.
    pub reason: String,
    /// Priority — lower number = evaluated first (0 = highest priority).
    pub priority: u32,
}

/// Token budget configuration for daemon-side LLM context window overflow prevention.
///
/// These values are consumed by `TokenBudgetManager` in `aura-daemon`.
/// All fields have sensible defaults for the Qwen3-8B model used on-device.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenBudgetConfig {
    /// Maximum tokens allowed per agentic session before forced compaction.
    #[serde(default = "default_token_budget_session_limit")]
    pub session_limit: u32,
    /// Tokens reserved for the LLM response — excluded from the planning budget.
    #[serde(default = "default_token_budget_response_reserve")]
    pub response_reserve: u32,
    /// Fraction of session_limit at which `BudgetStatus::Warning` fires
    /// and advisory summarization is recommended (0.0–1.0).
    #[serde(default = "default_token_budget_compaction_threshold")]
    pub compaction_threshold: f32,
    /// Fraction of session_limit at which `BudgetStatus::Critical` fires
    /// and mandatory summarization is enforced (0.0–1.0).
    #[serde(default = "default_token_budget_force_compaction_threshold")]
    pub force_compaction_threshold: f32,
}

fn default_token_budget_session_limit() -> u32 {
    2048
}
fn default_token_budget_response_reserve() -> u32 {
    512
}
fn default_token_budget_compaction_threshold() -> f32 {
    0.75
}
fn default_token_budget_force_compaction_threshold() -> f32 {
    0.90
}

impl Default for TokenBudgetConfig {
    fn default() -> Self {
        Self {
            session_limit: 2048,
            response_reserve: 512,
            compaction_threshold: 0.75,
            force_compaction_threshold: 0.90,
        }
    }
}

/// Onboarding / first-run configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OnboardingConfig {
    /// Allow the user to skip onboarding entirely.
    #[serde(default = "default_onboarding_skip_allowed")]
    pub skip_allowed: bool,
    /// Number of personality calibration questions (5–10).
    #[serde(default = "default_calibration_questions")]
    pub calibration_questions: u8,
    /// Enable device benchmarking during calibration.
    #[serde(default = "default_benchmark_enabled")]
    pub benchmark_enabled: bool,
    /// Maximum onboarding duration before auto-save and pause (seconds).
    #[serde(default = "default_onboarding_timeout_s")]
    pub timeout_s: u32,
    /// Welcome-back system: days of daily tips after onboarding.
    #[serde(default = "default_welcome_daily_days")]
    pub welcome_daily_days: u8,
    /// Welcome-back system: weeks of weekly highlights.
    #[serde(default = "default_welcome_weekly_weeks")]
    pub welcome_weekly_weeks: u8,
}

fn default_onboarding_skip_allowed() -> bool {
    true
}
fn default_calibration_questions() -> u8 {
    7
}
fn default_benchmark_enabled() -> bool {
    true
}
fn default_onboarding_timeout_s() -> u32 {
    600
}
fn default_welcome_daily_days() -> u8 {
    7
}
fn default_welcome_weekly_weeks() -> u8 {
    3
}

impl Default for OnboardingConfig {
    fn default() -> Self {
        Self {
            skip_allowed: true,
            calibration_questions: 7,
            benchmark_enabled: true,
            timeout_s: 600,
            welcome_daily_days: 7,
            welcome_weekly_weeks: 3,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_aura_config_defaults() {
        let config = AuraConfig::default();
        assert_eq!(config.daemon.checkpoint_interval_s, 300);
        assert_eq!(config.daemon.rss_warning_mb, 28);
        assert_eq!(config.daemon.rss_ceiling_mb, 30);
        assert!((config.amygdala.instant_threshold - 0.65).abs() < f32::EPSILON);
        assert_eq!(config.execution.max_steps_normal, 200);
        assert_eq!(config.execution.max_steps_safety, 50);
        assert_eq!(config.execution.max_steps_power, 500);
        assert_eq!(config.execution.rate_limit_actions_per_min, 60);
        assert_eq!(config.execution.delay_min_ms, 150);
        assert_eq!(config.execution.delay_max_ms, 500);
    }

    #[test]
    fn test_amygdala_weights_sum_to_one() {
        let config = AmygdalaConfig::default();
        assert!(config.weights_valid());

        let bad = AmygdalaConfig {
            weight_lex: 0.50,
            ..AmygdalaConfig::default()
        };
        assert!(!bad.weights_valid());
    }

    #[test]
    fn test_config_serialization_roundtrip() {
        let config = AuraConfig::default();
        let json = serde_json::to_string(&config).unwrap();
        let deser: AuraConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(deser.daemon.checkpoint_interval_s, 300);
        assert_eq!(
            deser.execution.rate_limit_actions_per_min,
            config.execution.rate_limit_actions_per_min
        );
    }

    #[test]
    fn test_sqlite_config_defaults() {
        let config = SqliteConfig::default();
        assert_eq!(config.wal_size_limit, 4 * 1024 * 1024);
        assert_eq!(config.max_episodes, 10_000);
    }

    #[test]
    fn test_routing_weights_sum_to_one() {
        let config = RoutingConfig::default();
        assert!(config.weights_valid());
    }

    #[test]
    fn test_routing_weights_invalid_detected() {
        let bad = RoutingConfig {
            weight_complexity: 0.80,
            ..RoutingConfig::default()
        };
        assert!(!bad.weights_valid());
    }

    #[test]
    fn test_neocortex_default_model_is_qwen3() {
        let cfg = NeocortexConfig::default();
        assert!(
            cfg.default_model_name.to_lowercase().contains("qwen3"),
            "default model name must reference Qwen3, got: {}",
            cfg.default_model_name
        );
        assert!(
            cfg.default_model_path.contains("qwen3"),
            "default model path must reference qwen3 GGUF, got: {}",
            cfg.default_model_path
        );
        assert!(
            cfg.default_model_path.ends_with(".gguf"),
            "default model path must be a .gguf file"
        );
        assert_eq!(
            cfg.default_model_context_size, 32_768,
            "Qwen3-8B default context must be 32768 tokens"
        );
    }
}
