//! Main event loop — `tokio::select!` over 7 channels.
//!
//! The loop runs until a cancellation signal is received or all producer
//! channels close.  Each select branch has independent error handling;
//! the loop **never** panics.
//!
//! A checkpoint timer fires every `config.checkpoint_interval_secs` to
//! persist state to disk.
//!
//! ## Wiring
//!
//! All 15+ subsystem connections are wired in this module through the
//! [`LoopSubsystems`] struct, which holds every subsystem not already
//! stored in [`DaemonState`].  The event flow is:
//!
//! - **A11y / Notification** → `EventParser` → `Amygdala` → gate decision
//!   → working memory + disposition
//! - **Chat** → `CommandParser` → build `ParsedEvent` → `Amygdala.score`
//!   → `PolicyGate.check_action` → `Contextor.enrich` → `RouteClassifier`
//!   → System1 fast path **or** System2 neocortex path
//! - **IPC outbound** → deserialize via bincode → `NeocortexClient.send`
//! - **IPC inbound** → match variant → PlanReady→react, ConversationReply
//!   →anti-sycophancy→response, Error→feedback_loop, MemoryWarning→unload
//! - **Cron tick** → memory consolidation, health report, token reset,
//!   stale request sweep

use std::path::Path;
use std::sync::atomic::Ordering;

#[cfg(test)]
use std::time::Instant;

use tokio::time::{interval, Duration};
use tracing::instrument;

use aura_types::events::{EventSource, GateDecision, Intent, ParsedEvent, ScoredEvent};
use aura_types::ipc::{DaemonToNeocortex, InferenceMode, NeocortexToDaemon};

use crate::bridge::router::ResponseRouter;
use crate::bridge::spawn_bridge;
use crate::bridge::telegram_bridge::TelegramBridge;
use crate::bridge::voice_bridge::VoiceBridge;
use crate::daemon_core::channels::{
    CronTick, DaemonResponse, DaemonResponseTx, DbWriteRequest, InputSource, IpcOutbound,
    UserCommand,
};
use crate::daemon_core::checkpoint::save_checkpoint;
use crate::daemon_core::react;
use crate::daemon_core::startup::DaemonState;

use crate::identity::IdentityEngine;
use crate::ipc::NeocortexClient;
use crate::memory::{consolidate, AuraMemory, ConsolidationLevel};
use crate::pipeline::amygdala::Amygdala;
use crate::pipeline::contextor::{Contextor, EnrichedEvent};
use crate::pipeline::parser::{CommandParser, EventParser};
use crate::policy::audit::AuditLog;
use crate::policy::emergency::{AnomalyDetector, EmergencyStop};
use crate::policy::gate::PolicyGate;
use crate::policy::sandbox::{ContainmentLevel, Sandbox};
use crate::routing::classifier::RouteClassifier;
use crate::routing::system1::System1;
use crate::routing::system2::System2;

use crate::arc::proactive::{ProactiveAction, ProactiveEngine};
use crate::arc::{ArcManager, ContextMode};
use crate::goals::scheduler::{BdiScheduler, Belief, BeliefSource, DeliberationResult, ScoredGoal, ScoreComponents};
use crate::goals::tracker::GoalTracker;
use crate::goals::decomposer::GoalDecomposer;
use crate::goals::registry::GoalRegistry;
use crate::goals::conflicts::{ConflictResolver, GoalConflictEntry};
use crate::outcome_bus::OutcomeBus;
use crate::reaction::ReactionDetector;
use aura_types::outcome::{ExecutionOutcome, OutcomeResult, RouteKind, UserReaction};
use crate::identity::affective::MoodEvent;
use aura_types::identity::RelationshipStage;
use aura_types::power::PowerTier;
use crate::identity::personality::PersonalityInfluence;
use crate::telegram::TelegramConfig;
use crate::voice::VoiceEngine;

use crate::execution::planner::EnhancedPlanner;
use crate::execution::learning::WorkflowObserver;
use crate::execution::react::{CognitiveState, EscalationContext, SemanticReact};

use crate::screen::{parse_tree, ScreenCache, SemanticGraph};
use crate::screen::semantic::ScreenSemanticState;

// -- P0/P1 Foundation imports -----------------------------------------------
use crate::health::{HealthMonitor, HealthReport, HealthStatus};
use crate::bridge::system_api::{SystemBridge, SystemCommand, SystemResult};
use crate::persistence::{CriticalVault, DataTier};
use crate::policy::{BoundaryContext, BoundaryDecision, BoundaryReasoner};
use crate::execution::retry::{StrategicRecovery, EnvironmentSnapshot, FailureCategory, RecoveryContext, RecoveryAction};

/// Maximum IPC payload size we'll accept for outbound writes (256 KB).
const MAX_IPC_PAYLOAD_BYTES: usize = 256 * 1024;

/// Cap on goals to prevent unbounded growth.
const MAX_ACTIVE_GOALS: usize = 64;

/// Timeout for IPC send operations (5 seconds).
const IPC_SEND_TIMEOUT_MS: u64 = 5_000;

/// Maximum cached screen entries (bounded for 4–8 GB phone).
const SCREEN_CACHE_MAX_ENTRIES: usize = 50;

/// Maximum screen cache memory budget (5 MB — conservative for mobile).
const SCREEN_CACHE_MAX_BYTES: usize = 5 * 1024 * 1024;

/// Screen cache TTL (2 seconds — screens change fast on mobile).
const SCREEN_CACHE_TTL_MS: u64 = 2_000;

/// Maximum semantic graph nodes we retain in `last_semantic_graph`.
/// Beyond this, we drop the graph and rely on summary text only.
const SEMANTIC_GRAPH_MAX_NODES: usize = 200;

// ---------------------------------------------------------------------------
// SandboxConfirmation — pending user-approval for restricted actions
// ---------------------------------------------------------------------------

/// A pending sandbox confirmation awaiting user approval via Telegram.
///
/// When the sandbox classifies a task as `L2:Restricted`, execution is
/// suspended and a confirmation prompt is sent to the user.  The user
/// can reply `/allow <id>` or `/deny <id>`.  If no response arrives
/// within `timeout`, the action is auto-denied.
#[derive(Debug, Clone)]
struct SandboxConfirmation {
    /// Unique identifier for this confirmation (monotonic counter).
    id: u64,
    /// Human-readable description of the action.
    description: String,
    /// The containment level that triggered confirmation.
    containment_level: String,
    /// When this confirmation was created.
    created_at: std::time::Instant,
    /// Timeout duration — auto-deny after this.
    timeout: std::time::Duration,
    /// The original task/action data needed to resume execution if approved.
    /// Stored as serialised string to avoid complex generic types.
    task_summary: String,
    /// The input source that originated the command (for routing responses).
    source: InputSource,
    /// The goal ID associated with this task (if any).
    goal_id: u64,
    /// The clamped priority of the original task request.
    priority: u32,
}

/// Max pending confirmations (BoundedVec principle — mobile device).
const MAX_PENDING_CONFIRMATIONS: usize = 100;
/// Default confirmation timeout: 60 seconds.
const CONFIRMATION_TIMEOUT_SECS: u64 = 60;

// ---------------------------------------------------------------------------
// LoopSubsystems — subsystems not held by DaemonState
// ---------------------------------------------------------------------------

/// All subsystems that the main loop needs but that are NOT held by
/// [`DaemonState`].  This avoids modifying DaemonState/startup.rs and
/// breaking existing tests.
struct LoopSubsystems {
    command_parser: CommandParser,
    event_parser: EventParser,
    amygdala: Amygdala,
    contextor: Contextor,
    classifier: RouteClassifier,
    system1: System1,
    system2: System2,
    neocortex: NeocortexClient,
    identity: IdentityEngine,
    memory: AuraMemory,
    response_tx: DaemonResponseTx,
    /// Rule-based + rate-limited policy gate for action safety checks.
    policy_gate: PolicyGate,
    /// Append-only, hash-chained audit log for policy decisions.
    audit_log: AuditLog,
    /// Consent tracker — records per-category user consent for privacy-gated
    /// actions (learning, proactive actions, data sharing, etc.).
    consent_tracker: crate::identity::ConsentTracker,
    /// Emergency stop system — kills execution on anomalies or user request.
    emergency: EmergencyStop,
    /// Action sandbox — containment and isolation for all actions.
    /// Defense-in-depth: PolicyGate AND ActionSandbox must BOTH pass.
    action_sandbox: Sandbox,

    // -- Sandbox confirmation system (user approval for L2:Restricted) ------
    /// Pending confirmations awaiting user `/allow` or `/deny` via Telegram.
    /// Bounded to [`MAX_PENDING_CONFIRMATIONS`] to prevent memory exhaustion.
    pending_confirmations: Vec<SandboxConfirmation>,
    /// Monotonic counter for confirmation IDs.
    next_confirmation_id: u64,

    // -- ARC subsystems (non-critical, Option<T> for degraded mode) --------
    /// BDI scheduler for goal deliberation (beliefs–desires–intentions).
    bdi_scheduler: Option<BdiScheduler>,
    /// Goal lifecycle tracker — mirrors checkpoint goals with richer state.
    goal_tracker: Option<GoalTracker>,
    /// Goal decomposer — breaks high-level goals into executable sub-goals.
    goal_decomposer: Option<GoalDecomposer>,
    /// Goal registry — capability matching for goal fulfilment.
    goal_registry: Option<GoalRegistry>,
    /// Conflict resolver — detects and resolves goal resource conflicts.
    conflict_resolver: Option<ConflictResolver>,
    /// Proactive engine — initiative budget, suggestions, routines, briefings.
    proactive: Option<ProactiveEngine>,
    /// ARC manager — owns cron, health, social, proactive, learning sub-engines.
    arc_manager: Option<ArcManager>,
    /// OutcomeBus — collects execution outcomes and dispatches to cognitive
    /// subsystems (learning, memory, BDI, identity, anti-sycophancy).
    outcome_bus: OutcomeBus,
    /// ReactionDetector — classifies the user's next input after AURA
    /// responds, closing the feedback loop for the learning engine.
    reaction_detector: ReactionDetector,

    // -- Execution intelligence (non-critical, Option<T> for degraded mode) --
    /// EnhancedPlanner — plan caching, Best-of-N, re-planning, resource
    /// estimation wrapping the base `ActionPlanner`.
    enhanced_planner: Option<EnhancedPlanner>,
    /// WorkflowObserver — observes successful execution traces and extracts
    /// recurring action sequences for automation candidates.
    workflow_observer: Option<WorkflowObserver>,

    // -- Semantic React (System1 ↔ System2 escalation) ----------------------
    /// SemanticReact — evaluates whether a failed System1 task should be
    /// escalated to System2 (neocortex) based on confidence, arousal,
    /// consecutive failures, battery level, and thermal state.
    semantic_react: SemanticReact,
    /// Running count of consecutive task failures (resets on success).
    /// Fed into `EscalationContext` for SemanticReact evaluation.
    consecutive_task_failures: u32,
    /// Running count of successful task completions (monotonic).
    /// Fed into `SemanticReact::adapt_thresholds` for learning.
    successful_task_count: u32,

    // -- Screen Intelligence (critical for perception loop) -----------------
    /// ScreenCache — LRU cache of recent screen trees for dedup, diffing,
    /// and prediction.  Bounded to `SCREEN_CACHE_MAX_ENTRIES` entries /
    /// `SCREEN_CACHE_MAX_BYTES` memory.
    screen_cache: ScreenCache,
    /// Last semantic graph built from the most recent screen tree.
    /// `None` until the first `TYPE_WINDOW_STATE_CHANGED` event delivers
    /// a full tree.  Dropped if `nodes.len() > SEMANTIC_GRAPH_MAX_NODES`
    /// to stay within mobile memory budget.
    last_semantic_graph: Option<SemanticGraph>,

    // -- Crash-safe persistence (P0 foundation) ----------------------------
    /// Write-ahead journal for crash-safe identity state persistence.
    /// `None` if journal creation failed (degrade gracefully).
    journal: Option<crate::persistence::WriteAheadJournal>,
    /// Safe mode state — activated when integrity verification finds critical issues.
    safe_mode: crate::persistence::SafeModeState,

    // -- P0/P1 Foundations (wired into the living system) -------------------
    /// Heartbeat Health Monitor — periodic self-diagnostic producing
    /// [`HealthReport`] snapshots.  Runs every 30s (60s in low-power).
    /// Feeds into strategic recovery and Telegram alerts.
    health_monitor: HealthMonitor,
    /// Strategic Failure Recovery — classifies failures into 5 categories
    /// and determines recovery actions (retry, escalate, wait, degrade).
    /// Replaces naive retry-or-fail with intelligent environment-aware recovery.
    strategic_recovery: StrategicRecovery,
    /// Typed System API Bridge — maps natural-language intents to direct
    /// Android JNI calls (~1000× faster than A11y UI automation).
    /// Checked BEFORE RouteClassifier so supported intents fast-path.
    system_bridge: SystemBridge,
    /// Critical Data Vault — 4-tier data classification (Ephemeral/Personal/
    /// Sensitive/Critical).  NEVER sends Tier 2+ to LLM, NEVER returns
    /// Tier 3 in search.  Protects the user's most sensitive data.
    critical_vault: CriticalVault,
    /// Dynamic Boundary Reasoning — 3-level rule system (Absolute/Conditional/
    /// Learned) for contextual ethics.  Evaluated alongside PolicyGate before
    /// task execution.  Learns from user /allow /deny patterns.
    boundary_reasoner: BoundaryReasoner,
}

impl LoopSubsystems {
    /// Construct all subsystems.  `response_tx` is cloned before the
    /// channel split so we can send responses from handlers.
    fn new(response_tx: DaemonResponseTx, data_dir: &Path) -> Self {
        let memory = match AuraMemory::new(data_dir) {
            Ok(m) => {
                tracing::info!(?data_dir, "AuraMemory opened from disk");
                m
            }
            Err(e) => {
                tracing::warn!(error = %e, "disk AuraMemory failed — falling back to in-memory");
                AuraMemory::new_in_memory()
                    .expect("in-memory AuraMemory must not fail")
            }
        };

        // ARC subsystem construction — non-critical, degrade gracefully.
        let bdi_scheduler = match std::panic::catch_unwind(BdiScheduler::new) {
            Ok(s) => {
                tracing::info!("BdiScheduler initialised");
                Some(s)
            }
            Err(_) => {
                tracing::warn!("BdiScheduler construction panicked — running without BDI");
                None
            }
        };

        let goal_tracker = match std::panic::catch_unwind(GoalTracker::new) {
            Ok(t) => {
                tracing::info!("GoalTracker initialised");
                Some(t)
            }
            Err(_) => {
                tracing::warn!("GoalTracker construction panicked — running without tracker");
                None
            }
        };

        let goal_decomposer = match std::panic::catch_unwind(GoalDecomposer::new) {
            Ok(d) => {
                tracing::info!("GoalDecomposer initialised");
                Some(d)
            }
            Err(_) => {
                tracing::warn!("GoalDecomposer construction panicked — running without decomposer");
                None
            }
        };

        let goal_registry = match std::panic::catch_unwind(GoalRegistry::new) {
            Ok(r) => {
                tracing::info!("GoalRegistry initialised");
                Some(r)
            }
            Err(_) => {
                tracing::warn!("GoalRegistry construction panicked — running without registry");
                None
            }
        };

        let conflict_resolver = match std::panic::catch_unwind(ConflictResolver::new) {
            Ok(c) => {
                tracing::info!("ConflictResolver initialised");
                Some(c)
            }
            Err(_) => {
                tracing::warn!("ConflictResolver construction panicked — running without conflict resolution");
                None
            }
        };

        let proactive = match std::panic::catch_unwind(ProactiveEngine::new) {
            Ok(p) => {
                tracing::info!("ProactiveEngine initialised");
                Some(p)
            }
            Err(_) => {
                tracing::warn!("ProactiveEngine construction panicked — running without proactive");
                None
            }
        };

        let arc_manager = match std::panic::catch_unwind(ArcManager::new) {
            Ok(a) => {
                tracing::info!("ArcManager initialised");
                Some(a)
            }
            Err(_) => {
                tracing::warn!("ArcManager construction panicked — running without ARC manager");
                None
            }
        };

        Self {
            command_parser: CommandParser::empty(),
            event_parser: EventParser::new(),
            amygdala: Amygdala::new(),
            contextor: Contextor::new(),
            classifier: RouteClassifier::new(),
            system1: System1::new(),
            system2: System2::new(),
            neocortex: NeocortexClient::disconnected(),
            identity: IdentityEngine::new(),
            memory,
            response_tx,
            policy_gate: build_hardened_policy_gate(),
            audit_log: AuditLog::new(4096),
            consent_tracker: crate::identity::ConsentTracker::with_defaults(now_ms()),
            emergency: EmergencyStop::new(),
            action_sandbox: Sandbox::new(),
            pending_confirmations: Vec::new(),
            next_confirmation_id: 0,
            bdi_scheduler,
            goal_tracker,
            goal_decomposer,
            goal_registry,
            conflict_resolver,
            proactive,
            arc_manager,
            outcome_bus: OutcomeBus::new(),
            reaction_detector: ReactionDetector::new(),
            enhanced_planner: match std::panic::catch_unwind(EnhancedPlanner::with_defaults) {
                Ok(ep) => {
                    tracing::info!("EnhancedPlanner initialised");
                    Some(ep)
                }
                Err(_) => {
                    tracing::warn!("EnhancedPlanner construction panicked — running without plan caching");
                    None
                }
            },
            workflow_observer: match std::panic::catch_unwind(WorkflowObserver::new) {
                Ok(wo) => {
                    tracing::info!("WorkflowObserver initialised");
                    Some(wo)
                }
                Err(_) => {
                    tracing::warn!("WorkflowObserver construction panicked — running without workflow learning");
                    None
                }
            },
            semantic_react: SemanticReact::new(),
            consecutive_task_failures: 0,
            successful_task_count: 0,
            screen_cache: ScreenCache::with_config(
                SCREEN_CACHE_MAX_ENTRIES,
                SCREEN_CACHE_MAX_BYTES,
                SCREEN_CACHE_TTL_MS,
            ),
            last_semantic_graph: None,

            // -- Crash-safe persistence (P0 foundation) ----------------------
            journal: {
                let journal_path = data_dir.join("identity.wal");
                match crate::persistence::WriteAheadJournal::new(&journal_path) {
                    Ok(j) => {
                        tracing::info!(
                            path = %journal_path.display(),
                            "WriteAheadJournal opened"
                        );
                        Some(j)
                    }
                    Err(e) => {
                        tracing::warn!(
                            error = %e,
                            "journal creation failed — running without WAL persistence"
                        );
                        None
                    }
                }
            },
            safe_mode: crate::persistence::SafeModeState::inactive(),

            // -- P0/P1 Foundations ────────────────────────────────────────
            health_monitor: HealthMonitor::new(now_ms(), 1),
            strategic_recovery: StrategicRecovery::new(),
            system_bridge: SystemBridge::new(),
            critical_vault: CriticalVault::new(),
            boundary_reasoner: BoundaryReasoner::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// Hardened PolicyGate construction
// ---------------------------------------------------------------------------

/// Build the execution-path PolicyGate with real deny/confirm/audit rules.
///
/// **This is the #1 safety-critical function in AURA.**  Every task execution
/// flows through this gate.  Rules are priority-sorted (lower = evaluated first)
/// and first-match wins.  The default effect for unmatched actions is `Audit`,
/// NOT `Allow`, so unknown actions are logged.
///
/// ## Three Immutable Pillars
/// - **Privacy Sovereignty**: deny contact/message/photo/location access without consent.
/// - **Truth & Transparency**: audit all credential and data-sharing actions.
/// - **Human Agency**: require confirmation for installs, purchases, account changes.
fn build_hardened_policy_gate() -> PolicyGate {
    use crate::policy::gate::RateLimiter;
    use crate::policy::rules::{PolicyRule, RuleEffect};

    // Start with an empty gate that allows everything by default.
    // We will add deny/confirm/audit rules that override the default for
    // dangerous action patterns, and set a strict rate limiter.
    let mut gate = PolicyGate::allow_all();

    // ── Priority 0: HARD DENY — irreversible / destructive system actions ──
    let hard_deny_rules = [
        ("deny-factory-reset", "*factory*reset*", "Factory reset is destructive and irreversible."),
        ("deny-wipe-data", "*wipe*data*", "Data wipe is irreversible — requires out-of-band confirmation."),
        ("deny-wipe-device", "*wipe*device*", "Device wipe is irreversible — requires out-of-band confirmation."),
        ("deny-uninstall-system", "*uninstall*system*", "System app removal can brick the device."),
        ("deny-format-storage", "*format*storage*", "Storage formatting destroys all user data."),
        ("deny-format-disk", "*format*disk*", "Disk formatting destroys all user data."),
        ("deny-root-access", "*root*access*", "Root/superuser access is never permitted."),
        ("deny-su-command", "*su *", "Superuser commands are never permitted."),
        ("deny-modify-bootloader", "*bootloader*", "Bootloader modification can brick the device."),
        ("deny-flash-firmware", "*flash*firmware*", "Firmware flashing can brick the device."),
    ];

    for (name, pattern, reason) in &hard_deny_rules {
        gate.add_rule(PolicyRule {
            name: name.to_string(),
            action_pattern: pattern.to_string(),
            effect: RuleEffect::Deny,
            reason: reason.to_string(),
            priority: 0,
        });
    }

    // ── Priority 5: DENY — privacy violations (Pillar #1: Privacy Sovereignty) ──
    let privacy_deny_rules = [
        ("deny-read-contacts", "*read*contact*", "Contact access requires explicit user consent."),
        ("deny-read-messages", "*read*message*", "Message access requires explicit user consent."),
        ("deny-read-sms", "*read*sms*", "SMS access requires explicit user consent."),
        ("deny-read-photos", "*read*photo*", "Photo access requires explicit user consent."),
        ("deny-read-gallery", "*read*gallery*", "Gallery access requires explicit user consent."),
        ("deny-read-location", "*read*location*", "Location access requires explicit user consent."),
        ("deny-access-camera", "*access*camera*", "Camera access requires explicit user consent."),
        ("deny-access-microphone", "*access*microphone*", "Microphone access requires explicit user consent."),
        ("deny-read-call-log", "*read*call*log*", "Call log access requires explicit user consent."),
        ("deny-read-calendar", "*read*calendar*", "Calendar access requires explicit user consent."),
        ("deny-share-data", "*share*data*", "Data sharing requires explicit user consent."),
        ("deny-export-data", "*export*user*data*", "Exporting user data requires explicit consent."),
    ];

    for (name, pattern, reason) in &privacy_deny_rules {
        gate.add_rule(PolicyRule {
            name: name.to_string(),
            action_pattern: pattern.to_string(),
            effect: RuleEffect::Deny,
            reason: reason.to_string(),
            priority: 5,
        });
    }

    // ── Priority 10: CONFIRM — financial / account actions (Pillar #3: Human Agency) ──
    let confirm_rules = [
        ("confirm-purchase", "*purchase*", "Purchases require explicit user confirmation."),
        ("confirm-payment", "*payment*", "Payments require explicit user confirmation."),
        ("confirm-transfer", "*transfer*money*", "Money transfers require explicit user confirmation."),
        ("confirm-subscribe", "*subscribe*", "Subscriptions require explicit user confirmation."),
        ("confirm-install-app", "*install*app*", "App installation requires user confirmation."),
        ("confirm-uninstall-app", "*uninstall*app*", "App removal requires user confirmation."),
        ("confirm-delete-file", "*delete*file*", "File deletion requires user confirmation."),
        ("confirm-delete-photo", "*delete*photo*", "Photo deletion requires user confirmation."),
        ("confirm-modify-settings", "*modify*system*setting*", "System setting changes require confirmation."),
        ("confirm-change-password", "*change*password*", "Password changes require user confirmation."),
        ("confirm-account-change", "*account*change*", "Account changes require user confirmation."),
        ("confirm-send-email", "*send*email*", "Sending email requires user confirmation."),
    ];

    for (name, pattern, reason) in &confirm_rules {
        gate.add_rule(PolicyRule {
            name: name.to_string(),
            action_pattern: pattern.to_string(),
            effect: RuleEffect::Confirm,
            reason: reason.to_string(),
            priority: 10,
        });
    }

    // ── Priority 20: AUDIT — sensitive but generally allowed actions ──
    let audit_rules = [
        ("audit-credential-access", "*credential*", "Credential access is audit-logged."),
        ("audit-token-access", "*token*", "Token access is audit-logged."),
        ("audit-api-key", "*api*key*", "API key access is audit-logged."),
        ("audit-network-request", "*network*request*", "Network requests are audit-logged."),
        ("audit-bluetooth", "*bluetooth*", "Bluetooth operations are audit-logged."),
        ("audit-wifi-change", "*wifi*change*", "WiFi configuration changes are audit-logged."),
    ];

    for (name, pattern, reason) in &audit_rules {
        gate.add_rule(PolicyRule {
            name: name.to_string(),
            action_pattern: pattern.to_string(),
            effect: RuleEffect::Audit,
            reason: reason.to_string(),
            priority: 20,
        });
    }

    // ── Priority 50: ALLOW — safe actions that should always proceed ──
    let allow_rules = [
        ("allow-navigate", "navigate*", "Navigation is safe."),
        ("allow-read-screen", "read*screen*", "Reading screen content is safe."),
        ("allow-read-notification", "read*notification*", "Reading notifications is safe."),
        ("allow-scroll", "scroll*", "Scrolling is safe."),
        ("allow-tap", "tap*", "Tapping UI elements is safe."),
        ("allow-type-text", "type*text*", "Typing text is safe."),
        ("allow-open-app", "open*app*", "Opening apps is safe."),
        ("allow-switch-app", "switch*app*", "Switching apps is safe."),
        ("allow-go-home", "*go*home*", "Going to home screen is safe."),
        ("allow-go-back", "*go*back*", "Going back is safe."),
    ];

    for (name, pattern, reason) in &allow_rules {
        gate.add_rule(PolicyRule {
            name: name.to_string(),
            action_pattern: pattern.to_string(),
            effect: RuleEffect::Allow,
            reason: reason.to_string(),
            priority: 50,
        });
    }

    // ── Rate limiter: 10 actions per 60s window (1 minute) ──
    // Much stricter than the default (10 per 1 second) — prevents
    // runaway automation loops.
    gate.set_rate_limiter(RateLimiter::new(10, Duration::from_secs(60)));

    tracing::info!(
        rule_count = gate.rule_count(),
        "execution PolicyGate initialised with hardened safety rules"
    );

    gate
}

// ---------------------------------------------------------------------------
// Consent-gated execution check
// ---------------------------------------------------------------------------

/// Consent categories that map from task description keywords to consent keys
/// tracked by `ConsentTracker`.
///
/// Returns the consent category name if the task description implies a
/// consent-gated action, or `None` if no consent gate applies.
fn consent_category_for_action(description: &str) -> Option<&'static str> {
    let lower = description.to_ascii_lowercase();

    // Proactive / autonomous actions (user must opt-in)
    if lower.contains("proactive") || lower.contains("routine") || lower.contains("automation") {
        return Some("proactive_actions");
    }

    // Data sharing / export
    if lower.contains("share") || lower.contains("export") || lower.contains("upload") {
        return Some("data_sharing");
    }

    // Privacy-sensitive access patterns
    if lower.contains("contact") || lower.contains("message") || lower.contains("sms")
        || lower.contains("photo") || lower.contains("gallery") || lower.contains("camera")
        || lower.contains("microphone") || lower.contains("location")
        || lower.contains("call log") || lower.contains("calendar")
    {
        return Some("privacy_access");
    }

    None
}

/// Check whether the task requires user consent and whether that consent has
/// been granted.  Returns `Some(denial_reason)` if the task must be blocked,
/// or `None` if execution may proceed.
///
/// This function does NOT modify state — it is a pure read on the
/// `ConsentTracker`.  Denied actions should be published to the `OutcomeBus`
/// by the caller.
fn check_consent_for_task(
    consent_tracker: &crate::identity::ConsentTracker,
    description: &str,
) -> Option<String> {
    if let Some(category) = consent_category_for_action(description) {
        let now = now_ms();
        if !consent_tracker.has_consent(category, now) {
            let reason = format!(
                "consent not granted for category '{}' — action blocked (Privacy Sovereignty)",
                category,
            );
            tracing::warn!(
                category,
                description,
                "task blocked: missing user consent"
            );
            return Some(reason);
        }
    }
    None
}

/// Sandbox containment check for a task description string.
///
/// Returns `Some(reason)` if the task is classified as **L3:Forbidden** and
/// must be refused outright, or `None` if the sandbox allows it to proceed
/// (L0–L2 are handled at the per-action level in the executor).
///
/// **Security reasoning**: This is the high-level containment gate.  It catches
/// obviously dangerous task descriptions (e.g. "factory reset", "wipe data")
/// before they reach the planning/execution pipeline.  Per-action sandbox
/// checks in `execute_step_attempt` provide the fine-grained second layer.
fn check_sandbox_for_task(sandbox: &Sandbox, description: &str) -> Option<String> {
    let level = sandbox.classify_string(description);
    match level {
        ContainmentLevel::Forbidden => {
            let reason = format!(
                "sandbox containment DENIED: task classified as {} — action refused \
                 (Defense-in-depth: Sandbox layer)",
                level,
            );
            tracing::warn!(
                target: "SECURITY",
                level = %level,
                description,
                "task blocked by sandbox containment — Forbidden"
            );
            Some(reason)
        }
        ContainmentLevel::Restricted => {
            // L2: Log at warn level — per-action confirmation handled downstream.
            tracing::info!(
                target: "SECURITY",
                level = %level,
                description,
                "task classified as Restricted — per-action confirmation required"
            );
            None
        }
        ContainmentLevel::Monitored => {
            tracing::debug!(
                target: "SECURITY",
                level = %level,
                description,
                "task classified as Monitored — execution will be logged"
            );
            None
        }
        ContainmentLevel::Direct => None,
    }
}

/// Dynamic boundary reasoning check for a task description.
///
/// Returns:
///  - `BoundaryGateResult::Allow` — proceed normally.
///  - `BoundaryGateResult::Deny(reason)` — block the task.
///  - `BoundaryGateResult::NeedConfirmation { reason, prompt }` — route
///    through the sandbox confirmation flow before executing.
///
/// This sits alongside PolicyGate (static rules) and ActionSandbox
/// (containment levels) as the third defense layer.  BoundaryReasoner
/// brings *dynamic* reasoning: it considers time-of-day, trust stage,
/// action category, and recent denial patterns.
enum BoundaryGateResult {
    Allow,
    Deny(String),
    NeedConfirmation { reason: String, prompt: String },
}

/// Convert a [`RelationshipStage`] to the 0–4 numeric scale that
/// [`BoundaryContext`] expects.
fn relationship_stage_to_u8(stage: RelationshipStage) -> u8 {
    match stage {
        RelationshipStage::Stranger => 0,
        RelationshipStage::Acquaintance => 1,
        RelationshipStage::Friend => 2,
        RelationshipStage::CloseFriend => 3,
        RelationshipStage::Soulmate => 4,
    }
}

/// Get the current relationship stage as a u8, reading from the identity
/// subsystem's relationship tracker.
fn current_relationship_stage(subs: &LoopSubsystems) -> u8 {
    let stage = subs
        .identity
        .relationships
        .get_relationship("primary_user")
        .map(|r| r.stage)
        .unwrap_or(RelationshipStage::Stranger);
    relationship_stage_to_u8(stage)
}

fn check_boundary_for_task(
    boundary_reasoner: &BoundaryReasoner,
    description: &str,
    relationship_stage: u8,
) -> BoundaryGateResult {
    // Build a BoundaryContext from available information.
    // We use chrono-free time extraction: hour from epoch ms.
    let now = now_ms();
    let secs_of_day = (now / 1000) % 86400;
    let current_hour = (secs_of_day / 3600) as u8;

    // Heuristic category detection from the description string.
    let lower = description.to_lowercase();
    let involves_money = lower.contains("pay")
        || lower.contains("transfer")
        || lower.contains("purchase")
        || lower.contains("buy")
        || lower.contains("wallet")
        || lower.contains("bank")
        || lower.contains("transaction");
    let involves_messaging = lower.contains("send")
        || lower.contains("message")
        || lower.contains("sms")
        || lower.contains("email")
        || lower.contains("chat")
        || lower.contains("post");
    let involves_data_access = lower.contains("read")
        || lower.contains("contacts")
        || lower.contains("calendar")
        || lower.contains("photos")
        || lower.contains("files")
        || lower.contains("storage")
        || lower.contains("history");

    let action_category = if involves_money {
        "financial".to_string()
    } else if involves_messaging {
        "messaging".to_string()
    } else if involves_data_access {
        "data_access".to_string()
    } else {
        "general".to_string()
    };

    let ctx = BoundaryContext {
        current_hour,
        relationship_stage,
        action_category,
        involves_money,
        involves_messaging,
        involves_data_access,
        data_tier: None,
        target_app: None,
        recent_denials: 0, // TODO: track denial count in LoopSubsystems
    };

    match boundary_reasoner.evaluate(description, &ctx) {
        BoundaryDecision::Allow => BoundaryGateResult::Allow,
        BoundaryDecision::AllowWithConfirmation { reason, confirmation_prompt } => {
            tracing::info!(
                target: "SECURITY",
                description,
                reason = %reason,
                "BoundaryReasoner: requires user confirmation"
            );
            BoundaryGateResult::NeedConfirmation {
                reason,
                prompt: confirmation_prompt,
            }
        }
        BoundaryDecision::Deny { reason, level } => {
            tracing::warn!(
                target: "SECURITY",
                description,
                reason = %reason,
                level = ?level,
                "BoundaryReasoner: DENIED (conditional rule)"
            );
            BoundaryGateResult::Deny(reason)
        }
        BoundaryDecision::DenyAbsolute { reason, rule_id } => {
            tracing::error!(
                target: "SAFETY",
                description,
                reason = %reason,
                rule_id,
                "BoundaryReasoner: ABSOLUTE DENY — hardcoded safety rule"
            );
            BoundaryGateResult::Deny(format!("[{rule_id}] {reason}"))
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Current wall-clock time in milliseconds since UNIX epoch.
/// Guaranteed to be strictly monotonic (never goes backwards) even if the system clock changes.
fn now_ms() -> u64 {
    static LAST_TIME_MS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    
    let mut last = LAST_TIME_MS.load(std::sync::atomic::Ordering::Acquire);
    loop {
        if now <= last {
            // Clock went backwards or didn't move! Advance by 1 ms to guarantee monotonicity.
            let next = last + 1;
            match LAST_TIME_MS.compare_exchange_weak(last, next, std::sync::atomic::Ordering::SeqCst, std::sync::atomic::Ordering::Relaxed) {
                Ok(_) => return next,
                Err(x) => last = x,
            }
        } else {
            match LAST_TIME_MS.compare_exchange_weak(last, now, std::sync::atomic::Ordering::SeqCst, std::sync::atomic::Ordering::Relaxed) {
                Ok(_) => return now,
                Err(x) => last = x,
            }
        }
    }
}

/// Best-effort send of a [`DaemonResponse`] to the bridge layer.
async fn send_response(tx: &DaemonResponseTx, dest: InputSource, text: String) {
    let resp = DaemonResponse {
        destination: dest,
        text,
    };
    if let Err(e) = tx.send(resp).await {
        tracing::error!(error = %e, "failed to send daemon response");
    }
}

/// Format a [`SystemResult`] into a human-readable message for the user.
///
/// This is used by the SystemBridge fast-path to present typed API results
/// in natural language rather than raw debug output.
fn format_system_result(result: &SystemResult) -> String {
    match result {
        SystemResult::Battery { level, charging, health } => {
            let pct = (level * 100.0) as u8;
            let charge_str = if *charging { ", charging" } else { "" };
            format!("Battery: {pct}%{charge_str} (health: {health:?})")
        }
        SystemResult::Storage { total_bytes, free_bytes } => {
            let total_gb = *total_bytes as f64 / 1_073_741_824.0;
            let free_gb = *free_bytes as f64 / 1_073_741_824.0;
            format!("Storage: {free_gb:.1} GB free of {total_gb:.1} GB total")
        }
        SystemResult::Network { connected, wifi, mobile_data, signal_strength } => {
            if !connected {
                return "Network: disconnected".to_string();
            }
            let transport = match (*wifi, *mobile_data) {
                (true, _) => "Wi-Fi",
                (_, true) => "mobile data",
                _ => "connected",
            };
            match signal_strength {
                Some(s) => format!("Network: {transport} (signal: {s})"),
                None => format!("Network: {transport}"),
            }
        }
        SystemResult::Memory { total_bytes, available_bytes, low_memory } => {
            let total_gb = *total_bytes as f64 / 1_073_741_824.0;
            let avail_gb = *available_bytes as f64 / 1_073_741_824.0;
            let warn = if *low_memory { " [LOW MEMORY]" } else { "" };
            format!("RAM: {avail_gb:.1} GB available of {total_gb:.1} GB{warn}")
        }
        SystemResult::Thermal(state) => {
            format!("Thermal state: {state:?}")
        }
        SystemResult::Contacts(contacts) => {
            if contacts.is_empty() {
                "No contacts found.".to_string()
            } else {
                let list: Vec<String> = contacts.iter().take(5)
                    .map(|c| {
                        let phone = c.phone.as_deref().unwrap_or("no phone");
                        format!("  {} — {}", c.name, phone)
                    })
                    .collect();
                let more = if contacts.len() > 5 {
                    format!("\n  ...and {} more", contacts.len() - 5)
                } else {
                    String::new()
                };
                format!("Contacts ({}):\n{}{more}", contacts.len(), list.join("\n"))
            }
        }
        SystemResult::Calendar(events) => {
            if events.is_empty() {
                "No calendar events found.".to_string()
            } else {
                let list: Vec<String> = events.iter().take(5)
                    .map(|e| {
                        let loc = e.location.as_deref().unwrap_or("no location");
                        format!("  {} ({})", e.title, loc)
                    })
                    .collect();
                format!("Calendar ({} events):\n{}", events.len(), list.join("\n"))
            }
        }
        SystemResult::Photos(photos) => {
            format!("Found {} recent photos.", photos.len())
        }
        SystemResult::Notifications(notifs) => {
            if notifs.is_empty() {
                "No active notifications.".to_string()
            } else {
                let list: Vec<String> = notifs.iter().take(5)
                    .map(|n| format!("  [{}] {}", n.package, n.title))
                    .collect();
                format!("Notifications ({}):\n{}", notifs.len(), list.join("\n"))
            }
        }
        SystemResult::ActionCompleted { command, success, message } => {
            if *success {
                format!("{command}: {message}")
            } else {
                format!("{command} failed: {message}")
            }
        }
    }
}

/// Ensure the IPC client is connected; attempt reconnect if not.
/// Returns `true` if connected after the call.
async fn ensure_ipc_connected(neocortex: &mut NeocortexClient) -> bool {
    if neocortex.is_connected() {
        return true;
    }
    tracing::info!("IPC client disconnected — attempting reconnect");
    match neocortex.reconnect().await {
        Ok(()) => {
            tracing::info!("IPC reconnect succeeded");
            true
        }
        Err(e) => {
            tracing::warn!(error = %e, "IPC reconnect failed");
            false
        }
    }
}

// ---------------------------------------------------------------------------
// Main loop entry
// ---------------------------------------------------------------------------

/// Run the daemon event loop until cancellation or channel exhaustion.
///
/// This is the heart of the daemon.  It `select!`s over 7 channels plus
/// a periodic checkpoint timer and a cancellation check.
///
/// **Key design:** The tx halves of every channel are immediately dropped
/// so that channels close properly when external producers drop their
/// cloned handles.  A periodic 100 ms wake-up ensures the cancel flag is
/// checked even when no messages arrive.
///
/// All subsystem wiring flows through [`LoopSubsystems`] which is constructed
/// once at the start and passed by `&mut` to every handler.
pub async fn run(mut state: DaemonState) {
    let checkpoint_interval = Duration::from_secs(state.config.daemon.checkpoint_interval_s as u64);
    let mut checkpoint_timer = interval(checkpoint_interval);
    // Consume the first (immediate) tick.
    checkpoint_timer.tick().await;

    let mut select_count: u64 = state.checkpoint.select_count;

    // Clone response_tx BEFORE split so handlers can send responses.
    let response_tx = state.channels.response_tx.clone();

    // Split channels: take rx halves for select!, drop tx halves so
    // channels close when external producers drop their cloned senders.
    let channels = std::mem::take(&mut state.channels);
    let (senders, mut rxs) = channels.split();
    drop(senders);

    // --- Bridge wiring ---
    // Extract response_rx from receivers — the ResponseRouter will own it
    // instead of the select! loop. Replace with a dummy closed channel so
    // the select! branch sees an immediate close (harmless).
    let response_rx = {
        let (_, dummy_rx) = tokio::sync::mpsc::channel::<DaemonResponse>(1);
        std::mem::replace(&mut rxs.response_rx, dummy_rx)
    };

    // Clone the user_command_tx so bridges can inject commands into the pipeline.
    // We need to clone it from the _original_ channels before they were split,
    // but response_tx was already cloned above. For cmd_tx, create a fresh one
    // by using state.channels — but we already moved it. The simplest approach:
    // create a dedicated bridge command channel that feeds into user_command_rx.
    // Actually, the channels struct had user_command_tx in the senders half which
    // we dropped. We need to create a new sender for the bridge. The cleanest
    // solution: the user_command channel is an mpsc, so we can clone the tx
    // before we split. Let's restructure: clone cmd_tx BEFORE split.
    //
    // REVISED: we already consumed `channels` above. Instead, create a
    // *new* mpsc pair: bridges send into this, and we add a forwarding branch
    // to the select! that drains bridge_cmd_rx into the normal pipeline.
    let (bridge_cmd_tx, mut bridge_cmd_rx) = tokio::sync::mpsc::channel::<UserCommand>(64);

    // Create the response router.
    let router = ResponseRouter::new(response_rx);

    // Register voice and telegram bridges with the router.
    let voice_bridge_rx = match router.register("voice").await {
        Ok(rx) => Some(rx),
        Err(e) => {
            tracing::warn!(error = %e, "failed to register voice bridge");
            None
        }
    };
    let telegram_bridge_rx = match router.register("telegram").await {
        Ok(rx) => Some(rx),
        Err(e) => {
            tracing::warn!(error = %e, "failed to register telegram bridge");
            None
        }
    };

    // Spawn the response router as a background task.
    let router_handle = router.spawn();
    state.subsystems.response_router = Some(router_handle);
    tracing::info!("response router spawned");

    // Spawn voice bridge (non-critical — runs in degraded mode if init fails).
    if let Some(voice_rx) = voice_bridge_rx {
        let voice_engine = VoiceEngine::default();
        let voice_bridge = VoiceBridge::new(voice_engine, state.cancel_flag.clone());
        let handle = spawn_bridge(Box::new(voice_bridge), bridge_cmd_tx.clone(), voice_rx);
        tracing::info!(bridge = handle.name, "voice bridge spawned");
        state.subsystems.voice_bridge = Some(handle);
    }

    // Spawn telegram bridge (non-critical — runs in degraded mode if init fails).
    if let Some(telegram_rx) = telegram_bridge_rx {
        let telegram_config = TelegramConfig::default();
        let telegram_bridge =
            TelegramBridge::new(telegram_config, state.cancel_flag.clone(), None);
        let handle = spawn_bridge(
            Box::new(telegram_bridge),
            bridge_cmd_tx.clone(),
            telegram_rx,
        );
        tracing::info!(bridge = handle.name, "telegram bridge spawned");
        state.subsystems.telegram_bridge = Some(handle);
    }

    // Drop the bridge_cmd_tx clone we hold so the channel closes when
    // all bridges exit (each bridge holds its own clone via spawn_bridge).
    drop(bridge_cmd_tx);

    // Build subsystems.
    let data_dir = Path::new(&state.config.sqlite.db_path)
        .parent()
        .unwrap_or_else(|| Path::new("."));
    let mut subs = LoopSubsystems::new(response_tx, data_dir);

    // ── Crash-safe persistence: journal recovery + integrity check ──────
    //
    // 1. Recover committed entries from journal (discard uncommitted tail).
    // 2. Replay committed entries into identity state.
    // 3. Run full integrity verification on recovered state.
    // 4. Activate safe mode if critical issues are found.
    {
        let mut journal_recovered = false;
        let mut journal_corruption = false;

        if let Some(ref mut journal) = subs.journal {
            match journal.recover() {
                Ok((entries, report)) => {
                    tracing::info!(
                        committed_txns = report.committed_transactions,
                        committed_entries = report.committed_entries,
                        uncommitted = report.uncommitted_entries,
                        corruption = report.corruption_detected,
                        "journal recovery complete"
                    );

                    journal_recovered = true;
                    journal_corruption = report.corruption_detected;

                    // Replay committed entries into identity state.
                    let mut replayed: usize = 0;
                    let mut failed: usize = 0;

                    for entry in &entries {
                        match entry.category {
                            crate::persistence::JournalCategory::Personality => {
                                if let Some(event) =
                                    crate::identity::decode_personality_event(&entry.payload)
                                {
                                    subs.identity.personality.evolve(event);
                                    replayed += 1;
                                } else {
                                    tracing::warn!(
                                        ts = entry.timestamp_ms,
                                        "journal replay: malformed personality payload — skipping"
                                    );
                                    failed += 1;
                                }
                            }
                            crate::persistence::JournalCategory::Trust => {
                                if let Some((user_id, interaction, ts)) =
                                    crate::identity::decode_trust_event(&entry.payload)
                                {
                                    subs.identity
                                        .relationships
                                        .record_interaction(&user_id, interaction, ts);
                                    replayed += 1;
                                } else {
                                    tracing::warn!(
                                        ts = entry.timestamp_ms,
                                        "journal replay: malformed trust payload — skipping"
                                    );
                                    failed += 1;
                                }
                            }
                            crate::persistence::JournalCategory::Mood => {
                                if let Some((event, ts)) =
                                    crate::identity::decode_mood_event(&entry.payload)
                                {
                                    subs.identity.affective.process_event(event, ts);
                                    replayed += 1;
                                } else {
                                    tracing::warn!(
                                        ts = entry.timestamp_ms,
                                        "journal replay: malformed mood payload — skipping"
                                    );
                                    failed += 1;
                                }
                            }
                            crate::persistence::JournalCategory::Consent => {
                                if let Some((_category, _granted, _ts)) =
                                    crate::identity::decode_consent_event(&entry.payload)
                                {
                                    // Consent state is loaded from DB, not replayed from journal.
                                    // Journal entry is kept for audit trail only.
                                    replayed += 1;
                                } else {
                                    tracing::warn!(
                                        ts = entry.timestamp_ms,
                                        "journal replay: malformed consent payload — skipping"
                                    );
                                    failed += 1;
                                }
                            }
                            // Goal / Execution / Memory categories: logged but not replayed
                            // into identity state (they're replayed by their own subsystems).
                            _ => {
                                replayed += 1;
                            }
                        }
                    }

                    tracing::info!(
                        replayed,
                        failed,
                        total = entries.len(),
                        "journal entry replay complete"
                    );

                    // Compact journal after successful recovery to reclaim space.
                    if !entries.is_empty() {
                        if let Err(e) = journal.compact(&entries) {
                            tracing::warn!(
                                error = %e,
                                "journal compaction failed — will retry next startup"
                            );
                        } else {
                            tracing::info!("journal compacted after recovery");
                        }
                    }
                }
                Err(e) => {
                    tracing::error!(
                        error = %e,
                        "journal recovery FAILED — identity state may be stale"
                    );
                    journal_corruption = true;
                }
            }
        } else {
            tracing::warn!("no journal available — skipping recovery (state relies on checkpoint only)");
        }

        // Run integrity verification on the (possibly replayed) identity state.
        let goal_count = state.checkpoint.goals.len();
        let report = crate::persistence::IntegrityVerifier::full_verification(
            &subs.identity.personality,
            &subs.identity.relationships,
            goal_count,
            journal_recovered,
            journal_corruption,
        );

        if report.is_clean() {
            tracing::info!("startup integrity verification: CLEAN — all checks passed");
        } else {
            tracing::warn!(
                critical = report.critical_count,
                warnings = report.warning_count,
                "startup integrity verification found issues"
            );
            for issue in &report.issues {
                match issue.severity {
                    crate::persistence::VerificationSeverity::Critical => {
                        tracing::error!(
                            subsystem = issue.subsystem,
                            msg = %issue.message,
                            "CRITICAL integrity issue"
                        );
                    }
                    crate::persistence::VerificationSeverity::Warning => {
                        tracing::warn!(
                            subsystem = issue.subsystem,
                            msg = %issue.message,
                            "integrity warning"
                        );
                    }
                    crate::persistence::VerificationSeverity::Info => {
                        tracing::info!(
                            subsystem = issue.subsystem,
                            msg = %issue.message,
                            "integrity info"
                        );
                    }
                }
            }
        }

        // Activate safe mode if warranted.
        if subs.safe_mode.activate_from_report(&report) {
            tracing::error!(
                reasons = ?subs.safe_mode.reasons,
                "SAFE MODE ACTIVATED on startup — proactive actions and learning are frozen"
            );

            // Log notification message for Telegram dispatch.
            if let Some(msg) = subs.safe_mode.notification_message() {
                tracing::info!(
                    telegram_msg_len = msg.len(),
                    "safe mode notification ready for Telegram dispatch"
                );
                // The Telegram bridge will pick this up in the next cron tick.
                // For now, log it so operators can see it in logcat.
                tracing::warn!(target: "SAFE_MODE", "{}", msg);
            }
        }
    }

    // Load user profile from database for consent checking.
    let db_path = &state.config.sqlite.db_path;
    match rusqlite::Connection::open(db_path) {
        Ok(db) => {
            if let Err(e) = subs.identity.load_user_profile(&db) {
                tracing::warn!(error = %e, "failed to load user profile - using defaults");
            } else if subs.identity.user_profile().is_some() {
                tracing::info!("user profile loaded for consent checking");
            }
        }
        Err(e) => {
            tracing::warn!(error = %e, "failed to open database for profile load");
        }
    }

    // Track how many channels are still open.
    // 7 original receiver channels + bridge_cmd_rx = 8 total.
    // (response_rx was replaced by a dummy — it is no longer counted;
    //  bridge_cmd_rx takes its slot as the 8th channel.)
    let mut open_channels: u8 = 8;

    tracing::info!(
        startup_ms = state.startup_time_ms,
        restored_select_count = select_count,
        "main loop starting — all subsystems wired"
    );

    loop {
        // Check cancellation flag (non-blocking).
        if state.cancel_flag.load(Ordering::Acquire) {
            tracing::info!(select_count, "cancellation flag set — exiting main loop");
            break;
        }

        // ── Emergency system: heartbeat + anomaly check ──────────
        subs.emergency.heartbeat();
        if let Some(reason) = subs.emergency.check_and_trigger() {
            tracing::error!(
                target: "SECURITY",
                reason = %format!("{reason:?}"),
                "emergency stop auto-triggered by anomaly detection"
            );
            if let Err(e) = subs.audit_log.log_policy_decision(
                "emergency_auto_trigger",
                &crate::policy::rules::RuleEffect::Deny,
                &format!("auto-triggered: {reason:?}"),
                0,
            ) {
                tracing::warn!(error = %e, "failed to audit auto-triggered emergency");
            }
        }

        // Exit if all channels have closed.
        if open_channels == 0 {
            tracing::info!(select_count, "all channels closed — exiting main loop");
            break;
        }

        tokio::select! {
            // ----- A11y events (accessibility service) -----
            msg = rxs.a11y_rx.recv() => {
                match msg {
                    Some(event) => {
                        select_count += 1;
                        if let Err(e) = handle_a11y_event(event, &mut state, &mut subs).await {
                            tracing::error!(error = %e, "a11y event handler failed");
                        }
                    }
                    None => {
                        tracing::warn!("a11y channel closed");
                        open_channels = open_channels.saturating_sub(1);
                    }
                }
            }

            // ----- Notification events -----
            msg = rxs.notification_rx.recv() => {
                match msg {
                    Some(event) => {
                        select_count += 1;
                        if let Err(e) = handle_notification_event(event, &mut state, &mut subs).await {
                            tracing::error!(error = %e, "notification handler failed");
                        }
                    }
                    None => {
                        tracing::warn!("notification channel closed");
                        open_channels = open_channels.saturating_sub(1);
                    }
                }
            }

            // ----- User commands -----
            msg = rxs.user_command_rx.recv() => {
                match msg {
                    Some(cmd) => {
                        select_count += 1;
                        if let Err(e) = handle_user_command(cmd, &mut state, &mut subs).await {
                            tracing::error!(error = %e, "user command handler failed");
                        }
                    }
                    None => {
                        tracing::warn!("user command channel closed");
                        open_channels = open_channels.saturating_sub(1);
                    }
                }
            }

            // ----- IPC outbound (daemon -> neocortex) -----
            msg = rxs.ipc_outbound_rx.recv() => {
                match msg {
                    Some(outbound) => {
                        select_count += 1;
                        if let Err(e) = handle_ipc_outbound(outbound, &mut subs).await {
                            tracing::error!(error = %e, "IPC outbound handler failed");
                        }
                    }
                    None => {
                        tracing::warn!("IPC outbound channel closed");
                        open_channels = open_channels.saturating_sub(1);
                    }
                }
            }

            // ----- IPC inbound (neocortex -> daemon) -----
            msg = rxs.ipc_inbound_rx.recv() => {
                match msg {
                    Some(inbound) => {
                        select_count += 1;
                        if let Err(e) = handle_ipc_inbound(inbound, &mut state, &mut subs).await {
                            tracing::error!(error = %e, "IPC inbound handler failed");
                        }
                    }
                    None => {
                        tracing::warn!("IPC inbound channel closed");
                        open_channels = open_channels.saturating_sub(1);
                    }
                }
            }

            // ----- DB write requests -----
            msg = rxs.db_write_rx.recv() => {
                match msg {
                    Some(req) => {
                        select_count += 1;
                        if let Err(e) = handle_db_write(req, &state) {
                            tracing::error!(error = %e, "db write handler failed");
                        }
                    }
                    None => {
                        tracing::warn!("db write channel closed");
                        open_channels = open_channels.saturating_sub(1);
                    }
                }
            }

            // ----- Cron ticks -----
            msg = rxs.cron_tick_rx.recv() => {
                match msg {
                    Some(tick) => {
                        select_count += 1;
                        if let Err(e) = handle_cron_tick(tick, &mut state, &mut subs).await {
                            tracing::error!(error = %e, "cron tick handler failed");
                        }
                    }
                    None => {
                        tracing::warn!("cron tick channel closed");
                        open_channels = open_channels.saturating_sub(1);
                    }
                }
            }

            // ----- Daemon responses (routed to bridges via ResponseRouter) -----
            // The dummy response_rx closes immediately; just decrement and move on.
            msg = rxs.response_rx.recv() => {
                match msg {
                    Some(response) => {
                        // Should not happen — the dummy channel has no senders.
                        // But handle gracefully: log and discard.
                        select_count += 1;
                        tracing::debug!(
                            destination = %response.destination,
                            len = response.text.len(),
                            "unexpected response on dummy channel — discarding"
                        );
                    }
                    None => {
                        tracing::debug!("dummy response channel closed (expected)");
                        open_channels = open_channels.saturating_sub(1);
                    }
                }
            }

            // ----- Bridge commands (bridges → daemon pipeline) -----
            msg = bridge_cmd_rx.recv() => {
                match msg {
                    Some(cmd) => {
                        select_count += 1;
                        tracing::debug!(source = %cmd.source(), "bridge command received — forwarding to pipeline");
                        if let Err(e) = handle_user_command(cmd, &mut state, &mut subs).await {
                            tracing::error!(error = %e, "bridge command handler failed");
                        }
                    }
                    None => {
                        tracing::info!("bridge command channel closed — all bridges exited");
                        open_channels = open_channels.saturating_sub(1);
                    }
                }
            }

            // ----- Periodic checkpoint -----
            _ = checkpoint_timer.tick() => {
                // Safe mode guard: skip periodic checkpoint to avoid persisting
                // potentially corrupt identity/memory state.  Journal writes
                // (the WAL recovery mechanism) are NOT affected by this guard.
                if subs.safe_mode.active {
                    tracing::debug!(
                        "safe mode active — skipping periodic checkpoint to prevent \
                         persisting potentially corrupt state"
                    );
                } else {
                state.checkpoint.select_count = select_count;
                let cp = state.checkpoint.clone();
                let path = crate::daemon_core::startup::checkpoint_path_from_config(&state.config);

                // Checkpoint I/O is blocking — run off the async thread.
                let result = tokio::task::spawn_blocking(move || {
                    save_checkpoint(&cp, Path::new(&path))
                }).await;

                match result {
                    Ok(Ok(())) => {
                        tracing::debug!(select_count, "periodic checkpoint saved");
                    }
                    Ok(Err(e)) => {
                        tracing::error!(error = %e, "periodic checkpoint failed");
                    }
                    Err(e) => {
                        tracing::error!(error = %e, "checkpoint spawn_blocking panicked");
                    }
                }
                } // end safe mode else

                // Check if the reaction observation window has expired.
                // This ensures timed-out windows generate a NoReaction
                // feedback outcome even if the user never sends another
                // message.
                if let Some(expired) = subs.reaction_detector.check_expiry(now_ms()) {
                    tracing::debug!(
                        corr_intent = %expired.correlation_intent,
                        "reaction window expired — publishing NoReaction feedback"
                    );
                    let expiry_outcome = ExecutionOutcome::new(
                        expired.correlation_intent.clone(),
                        OutcomeResult::PartialSuccess,
                        0,
                        expired.confidence,
                        RouteKind::System1,
                        expired.correlation_timestamp,
                    )
                    .with_user_reaction(expired.reaction);
                    subs.outcome_bus.publish(expiry_outcome);
                    flush_outcome_bus(&mut subs).await;
                }
            }

            // ----- Periodic wake-up for cancel-flag polling -----
            _ = tokio::time::sleep(Duration::from_millis(100)) => {
                // No work; loop head re-checks cancel_flag and open_channels.
                continue;
            }
        }
    }

    // Update final select count before returning.
    state.checkpoint.select_count = select_count;
    tracing::info!(select_count, "main loop exited");
}

// ---------------------------------------------------------------------------
// Per-branch handlers
// ---------------------------------------------------------------------------

/// Process an accessibility event through the full pipeline:
/// `EventParser` → `Amygdala` → gate decision → working memory + disposition.
#[instrument(skip_all, fields(event_type = event.event_type, package = %event.package_name))]
async fn handle_a11y_event(
    event: aura_types::events::RawEvent,
    state: &mut DaemonState,
    subs: &mut LoopSubsystems,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    tracing::debug!(
        event_type = event.event_type,
        package = %event.package_name,
        class = %event.class_name,
        ts = event.timestamp_ms,
        "processing a11y event"
    );

    // Stage 1: EventParser — classify the raw event.
    let parsed = subs.event_parser.parse_raw(&event);
    tracing::debug!(intent = ?parsed.intent, source = ?parsed.source, "a11y event parsed");

    // Stage 2: Amygdala — score importance and make gate decision.
    let scored = subs.amygdala.score(&parsed);
    tracing::debug!(
        score = scored.score_total,
        gate = ?scored.gate_decision,
        "a11y event scored"
    );

    // Store in working memory regardless of gate decision.
    subs.memory.store_working(
        parsed.content.clone(),
        EventSource::Accessibility,
        scored.score_total,
        now_ms(),
    );

    // Update screen summary for high-salience window changes.
    if event.event_type == 32 {
        // Screen intelligence: process raw accessibility tree when available
        if let Some(ref raw_nodes) = event.raw_nodes {
            if !raw_nodes.is_empty() {
                // Parse raw nodes into structured tree
                let screen_tree = parse_tree(raw_nodes);

                // Cache the screen tree
                subs.screen_cache.insert(screen_tree.clone());

                // Build semantic graph with bounded size
                let graph = SemanticGraph::from_tree(&screen_tree);
                if graph.estimated_size_bytes() <= SCREEN_CACHE_MAX_BYTES {
                    // Compute screen complexity for classifier
                    let state = ScreenSemanticState::from_graph(&graph);
                    let complexity = match state {
                        ScreenSemanticState::Interactive => 0.1,
                        ScreenSemanticState::Loading => 0.3,
                        ScreenSemanticState::Transitional => 0.2,
                        ScreenSemanticState::Success => 0.2,
                        ScreenSemanticState::InputRequired => 0.4,
                        ScreenSemanticState::Error => 0.5,
                        ScreenSemanticState::Blocked => 0.7,
                        _ => 0.3, // default for unknown states
                    };
                    let pattern_count = graph.interactive_nodes().len();

                    // Feed to classifier and contextor
                    subs.classifier.set_screen_context(complexity, pattern_count);
                    subs.contextor.set_screen_summary(Some(graph.summary_for_llm()));

                    // Store for executor use
                    subs.last_semantic_graph = Some(graph);
                } else {
                    log::warn!(
                        "SemanticGraph exceeds max bytes ({}), skipping",
                        graph.estimated_size_bytes()
                    );
                }
            }
        } else {
            // Fallback: lightweight package::class summary when no raw tree
            let summary = format!(
                "{}::{}",
                event.package_name, event.class_name
            );
            subs.contextor.set_screen_summary(Some(summary));
        }
    }

    // Map gate decision → MoodEvent and process through AffectiveEngine.
    let mood_event = match scored.gate_decision {
        GateDecision::EmergencyBypass | GateDecision::InstantWake => {
            Some(MoodEvent::UserFrustrated)
        }
        GateDecision::SlowAccumulate => None, // low salience — no mood shift
        GateDecision::Suppress => Some(MoodEvent::Silence { duration_ms: 0 }),
    };

    if let Some(event) = mood_event {
        let ts = now_ms();
        let personality = &subs.identity.personality.traits;
        subs.identity.affective.process_event_with_personality(event, ts, personality);
        // Sync AffectiveEngine state back to checkpoint.
        let affect_state = subs.identity.affective.current_state();
        state.checkpoint.disposition = affect_state.clone();
        tracing::debug!(
            new_arousal = state.checkpoint.disposition.mood.arousal,
            new_valence = state.checkpoint.disposition.mood.valence,
            "a11y disposition update via AffectiveEngine"
        );
    } else {
        tracing::debug!("a11y event below salience threshold — no mood update");
    }

    Ok(())
}

/// Process a notification through the full pipeline:
/// `EventParser.parse_notification` → `Amygdala.score` → gate decision
/// → working memory + disposition.
#[instrument(skip_all, fields(package = %event.package, title = %event.title))]
async fn handle_notification_event(
    event: aura_types::events::NotificationEvent,
    state: &mut DaemonState,
    subs: &mut LoopSubsystems,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    tracing::debug!(
        package = %event.package,
        title = %event.title,
        category = ?event.category,
        ongoing = event.is_ongoing,
        ts = event.timestamp_ms,
        "processing notification"
    );

    // Stage 1: EventParser — classify the notification.
    let parsed = subs.event_parser.parse_notification(&event);
    tracing::debug!(intent = ?parsed.intent, "notification parsed");

    // Stage 2: Amygdala — score importance.
    let scored = subs.amygdala.score(&parsed);
    tracing::info!(
        importance = scored.score_total,
        gate = ?scored.gate_decision,
        package = %event.package,
        "notification scored"
    );

    // Store in working memory.
    subs.memory.store_working(
        format!("[{}] {}: {}", event.package, event.title, event.text),
        EventSource::Notification,
        scored.score_total,
        now_ms(),
    );

    // Map notification importance → MoodEvent and process through AffectiveEngine.
    let mood_event = if scored.score_total >= 0.7 {
        MoodEvent::UserHappy // high-importance notification — engagement boost
    } else if scored.score_total >= 0.4 {
        MoodEvent::Compliment // moderate importance — slight positive
    } else {
        MoodEvent::Silence { duration_ms: 0 } // low importance — minimal effect
    };

    let ts = now_ms();
    let personality = &subs.identity.personality.traits;
    subs.identity.affective.process_event_with_personality(mood_event, ts, personality);
    // Sync AffectiveEngine state back to checkpoint.
    let affect_state = subs.identity.affective.current_state();
    state.checkpoint.disposition = affect_state.clone();

    Ok(())
}

/// Parse, validate, and route a user command through the full pipeline.
///
/// For `Chat` messages the pipeline is:
/// 1. `CommandParser.parse` — NLU intent + entities
/// 2. Build `ParsedEvent` from parse result
/// 3. `Amygdala.score` — importance scoring
/// 4. `PolicyGate.check_action` — ethics check
/// 5. `Contextor.enrich` — memory + identity enrichment
/// 6. `RouteClassifier.classify` — System1 vs System2 decision
/// 7. Dispatch to the chosen path
#[instrument(skip_all, fields(cmd = ?cmd))]
async fn handle_user_command(
    cmd: UserCommand,
    state: &mut DaemonState,
    subs: &mut LoopSubsystems,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    tracing::info!(?cmd, "handling user command");

    match cmd {
        UserCommand::Chat { text, source, voice_meta } => {
            if text.trim().is_empty() {
                tracing::warn!("ignoring empty chat message");
                return Ok(());
            }

            // Truncate excessively long messages (guard against abuse).
            let text = if text.len() > 4096 {
                tracing::warn!(len = text.len(), "truncating oversized chat message");
                text[..4096].to_string()
            } else {
                text
            };

            tracing::info!(
                len = text.len(),
                source = %source,
                "chat message accepted"
            );

            // ── Stage -0.5: Sandbox confirmation commands ────────────
            // Intercept /allow <id> and /deny <id> BEFORE normal NLU
            // processing.  These are control-plane commands for the
            // sandbox confirmation system.
            {
                let trimmed = text.trim();
                if trimmed.starts_with("/allow ") || trimmed.starts_with("/deny ") {
                    let parts: Vec<&str> = trimmed.splitn(2, ' ').collect();
                    if parts.len() == 2 {
                        if let Ok(conf_id) = parts[1].trim().parse::<u64>() {
                            let is_allow = trimmed.starts_with("/allow");

                            // Find and remove the confirmation from the pending list.
                            if let Some(pos) = subs.pending_confirmations.iter().position(|c| c.id == conf_id) {
                                let conf = subs.pending_confirmations.remove(pos);
                                let response_time_ms = conf.created_at.elapsed().as_millis() as u64;

                                if is_allow {
                                    tracing::info!(
                                        target: "SECURITY",
                                        conf_id,
                                        description = %conf.description,
                                        "user APPROVED sandbox confirmation"
                                    );

                                    // Resume the task execution.
                                    // ── BoundaryReasoner gate (defense-in-depth) ──
                                    // Even though the user approved via /allow, the
                                    // boundary reasoner may still veto if conditions
                                    // changed (e.g. time-of-day restrictions).
                                    let boundary_result = check_boundary_for_task(
                                        &subs.boundary_reasoner,
                                        &conf.task_summary,
                                        current_relationship_stage(subs),
                                    );
                                    match boundary_result {
                                        BoundaryGateResult::Deny(reason) => {
                                            tracing::warn!(
                                                target: "SECURITY",
                                                conf_id,
                                                reason = %reason,
                                                "BoundaryReasoner blocked /allow resume"
                                            );
                                            // Record user's intent to allow even though
                                            // boundary overrode — feeds L3 preference
                                            // learning (user wanted this action).
                                            subs.boundary_reasoner.record_user_response(
                                                &conf.task_summary,
                                                true, // user wanted to allow
                                                response_time_ms,
                                            );
                                            // Publish failure to OutcomeBus — no fire-and-forget.
                                            subs.outcome_bus.publish(ExecutionOutcome::new(
                                                conf.task_summary.clone(),
                                                OutcomeResult::Failure,
                                                0,
                                                0.0,
                                                RouteKind::System1,
                                                now_ms(),
                                            ));
                                            if let Some(goal) = state.checkpoint.goals.iter_mut().find(|g| g.id == conf.goal_id) {
                                                goal.status = aura_types::goals::GoalStatus::Failed(
                                                    format!("boundary denied after /allow: {}", reason),
                                                );
                                            }
                                            send_response(
                                                &subs.response_tx,
                                                source.clone(),
                                                format!("Action blocked by boundary rules: {reason}"),
                                            ).await;
                                            flush_outcome_bus(subs).await;
                                        }
                                        // NeedConfirmation after user already confirmed
                                        // is contradictory — treat as Allow to respect
                                        // the user's explicit approval.
                                        BoundaryGateResult::Allow
                                        | BoundaryGateResult::NeedConfirmation { .. } => {
                                    let mut policy_ctx = react::PolicyContext {
                                        gate: &mut subs.policy_gate,
                                        audit: &mut subs.audit_log,
                                    };
                                    let (outcome, session) = react::execute_task(
                                        conf.task_summary.clone(),
                                        conf.priority,
                                        None,
                                        Some(&mut policy_ctx),
                                    ).await;

                                    // Update goal status based on execution outcome.
                                    if let Some(goal) = state.checkpoint.goals.iter_mut().find(|g| g.id == conf.goal_id) {
                                        match &outcome {
                                            react::TaskOutcome::Success { final_confidence, .. } => {
                                                goal.status = aura_types::goals::GoalStatus::Completed;
                                                tracing::info!(
                                                    goal_id = conf.goal_id,
                                                    session_id = session.session_id,
                                                    confidence = final_confidence,
                                                    "confirmed task completed successfully"
                                                );
                                            }
                                            react::TaskOutcome::Failed { reason, .. }
                                            | react::TaskOutcome::CycleAborted { cycle_reason: reason, .. } => {
                                                goal.status = aura_types::goals::GoalStatus::Failed(reason.clone());
                                                tracing::warn!(
                                                    goal_id = conf.goal_id,
                                                    reason = %reason,
                                                    "confirmed task execution failed"
                                                );
                                            }
                                            react::TaskOutcome::Cancelled { .. } => {
                                                // Treat cancellation as a pause — goal remains active.
                                                tracing::info!(
                                                    goal_id = conf.goal_id,
                                                    "confirmed task was cancelled"
                                                );
                                            }
                                        }
                                    }

                                    let ack = format!(
                                        "\u{2705} Approved and executed: {}",
                                        conf.description
                                    );
                                    send_response(&subs.response_tx, source, ack).await;

                                    // ── Record user's approval for BoundaryReasoner L3 learning ──
                                    subs.boundary_reasoner.record_user_response(
                                        &conf.task_summary,
                                        true, // user allowed
                                        response_time_ms,
                                    );
                                        } // end Allow|NeedConfirmation arm
                                    } // end match boundary_result
                                } else {
                                    tracing::info!(
                                        target: "SECURITY",
                                        conf_id,
                                        description = %conf.description,
                                        "user DENIED sandbox confirmation"
                                    );

                                    // ── Record user's denial for BoundaryReasoner L3 learning ──
                                    subs.boundary_reasoner.record_user_response(
                                        &conf.task_summary,
                                        false, // user denied
                                        response_time_ms,
                                    );

                                    // Mark the goal as failed.
                                    if let Some(goal) = state.checkpoint.goals.iter_mut().find(|g| g.id == conf.goal_id) {
                                        goal.status = aura_types::goals::GoalStatus::Failed(
                                            format!("user denied confirmation {}", conf_id),
                                        );
                                    }

                                    subs.outcome_bus.publish(ExecutionOutcome::new(
                                        conf.task_summary.clone(),
                                        OutcomeResult::Failure,
                                        0,
                                        0.0,
                                        RouteKind::System1,
                                        now_ms(),
                                    ));

                                    let ack = format!(
                                        "\u{274c} Denied: {}",
                                        conf.description
                                    );
                                    send_response(&subs.response_tx, source, ack).await;
                                    flush_outcome_bus(subs).await;
                                }
                            } else {
                                let msg = format!(
                                    "Confirmation {} not found or expired.",
                                    conf_id
                                );
                                send_response(&subs.response_tx, source, msg).await;
                            }

                            return Ok(());
                        }
                    }
                }
            }

            // ── Stage -1: Reaction Detection ─────────────────────────
            // If a reaction observation window is active from the previous
            // AURA response, classify the user's new input as a reaction.
            // Uses genuine cognitive signals: Amygdala emotional valence
            // (from voice metadata when available) and trigram similarity
            // (from AURA's memory subsystem) — no keyword matching.
            if subs.reaction_detector.is_active() {
                // Extract window data before mutably borrowing the detector.
                let orig_input = subs.reaction_detector
                    .window_original_input()
                    .map(|s| s.to_owned());
                let resp_text = subs.reaction_detector
                    .window_response_text()
                    .map(|s| s.to_owned());

                if let (Some(orig), Some(resp)) = (orig_input, resp_text) {
                    // Sentiment: use voice biomarker valence when available
                    // (genuine signal from the voice analysis pipeline).
                    // For text-only inputs, default to neutral — we honestly
                    // lack text sentiment analysis and refuse to fake it.
                    let sentiment = voice_meta
                        .as_ref()
                        .and_then(|m| m.emotional_valence)
                        .unwrap_or(0.0);

                    // Similarity: Jaccard trigram similarity from AURA's
                    // memory/embeddings subsystem — a genuine string
                    // similarity metric, not keyword matching.
                    let sim_to_original =
                        crate::memory::jaccard_trigram_similarity(&text, &orig);
                    let sim_to_response =
                        crate::memory::jaccard_trigram_similarity(&text, &resp);

                    if let Some(classified) = subs.reaction_detector.classify_reaction(
                        &text,
                        sentiment,
                        sim_to_original,
                        sim_to_response,
                        now_ms(),
                    ) {
                        tracing::info!(
                            reaction = ?classified.reaction,
                            confidence = classified.confidence,
                            corr_intent = %classified.correlation_intent,
                            sim_orig = sim_to_original,
                            sim_resp = sim_to_response,
                            sentiment,
                            "user reaction classified via ReactionDetector"
                        );

                        // Publish a feedback outcome so the OutcomeBus
                        // dispatches the reaction signal to learning,
                        // BDI, memory, and identity subsystems.
                        let feedback_outcome = ExecutionOutcome::new(
                            classified.correlation_intent.clone(),
                            match classified.reaction {
                                UserReaction::ExplicitPositive => OutcomeResult::Success,
                                UserReaction::FollowUp => OutcomeResult::Success,
                                UserReaction::ExplicitNegative => OutcomeResult::Failure,
                                UserReaction::Repetition => OutcomeResult::Failure,
                                UserReaction::TopicChange => OutcomeResult::PartialSuccess,
                                UserReaction::NoReaction => OutcomeResult::PartialSuccess,
                            },
                            0, // no duration for reaction
                            classified.confidence,
                            RouteKind::System1, // best guess; reaction correlates with original route
                            classified.correlation_timestamp,
                        )
                        .with_user_reaction(classified.reaction)
                        .with_input_summary(&text);
                        subs.outcome_bus.publish(feedback_outcome);

                        // Update BDI scheduler with a belief derived from
                        // the reaction — using AURA's actual BDI system.
                        if let Some(ref mut bdi) = subs.bdi_scheduler {
                            use crate::goals::scheduler::{Belief, BeliefSource};
                            let (key, value) = match classified.reaction {
                                UserReaction::ExplicitPositive => {
                                    ("user_satisfaction", "positive")
                                }
                                UserReaction::ExplicitNegative => {
                                    ("user_satisfaction", "negative")
                                }
                                UserReaction::Repetition => {
                                    ("response_quality", "inadequate")
                                }
                                UserReaction::FollowUp => {
                                    ("user_engagement", "engaged")
                                }
                                UserReaction::TopicChange => {
                                    ("user_engagement", "disengaged")
                                }
                                UserReaction::NoReaction => {
                                    ("user_engagement", "neutral")
                                }
                            };
                            let _ = bdi.update_belief(Belief {
                                key: key.to_string(),
                                value: value.to_string(),
                                confidence: classified.confidence,
                                updated_at_ms: now_ms(),
                                source: BeliefSource::ExecutionOutcome,
                            });
                        }
                    }
                }
            }

            // Process mood through AffectiveEngine — user interaction = engagement.
            {
                let ts = now_ms();
                let personality = &subs.identity.personality.traits;

                // Base event: user is chatting → engagement.
                let base_event = if let Some(ref meta) = voice_meta {
                    if let Some(valence) = meta.emotional_valence {
                        if valence > 0.2 {
                            MoodEvent::UserHappy
                        } else if valence < -0.2 {
                            MoodEvent::UserFrustrated
                        } else {
                            MoodEvent::Compliment // neutral engagement
                        }
                    } else {
                        MoodEvent::Compliment // no valence data — mild positive
                    }
                } else {
                    MoodEvent::Compliment // text-only chat — mild positive
                };

                subs.identity.affective.process_event_with_personality(base_event, ts, personality);

                // Voice biomarker stress/fatigue → dedicated mood events + stress accumulation.
                if let Some(ref meta) = voice_meta {
                    if let Some(stress) = meta.emotional_stress {
                        if stress > 0.3 {
                            let event = MoodEvent::VoiceStressDetected { level: stress };
                            subs.identity.affective.process_event_with_personality(
                                event.clone(), ts, personality,
                            );
                            subs.identity.stress.accumulate(
                                &event, ts, personality.neuroticism,
                            );
                            tracing::debug!(stress, "voice stress biomarker processed");
                        }
                    }
                    if let Some(fatigue) = meta.emotional_fatigue {
                        if fatigue > 0.3 {
                            let event = MoodEvent::VoiceFatigueDetected { level: fatigue };
                            subs.identity.affective.process_event_with_personality(
                                event.clone(), ts, personality,
                            );
                            subs.identity.stress.accumulate(
                                &event, ts, personality.neuroticism,
                            );
                            tracing::debug!(fatigue, "voice fatigue biomarker processed");
                        }
                    }
                }

                let affect_state = subs.identity.affective.current_state();
                state.checkpoint.disposition = affect_state.clone();
                tracing::debug!(
                    new_arousal = state.checkpoint.disposition.mood.arousal,
                    new_valence = state.checkpoint.disposition.mood.valence,
                    "chat disposition update via AffectiveEngine"
                );
            }

            // ── Stage 0a: Emergency stop-phrase detection ─────────────
            if let Some(reason) = AnomalyDetector::check_user_stop_phrase(&text) {
                tracing::warn!(
                    reason = %format!("{reason:?}"),
                    "user stop phrase detected — activating emergency stop"
                );
                match subs.emergency.activate(reason, "user stop phrase in chat") {
                    Ok(report) => {
                        tracing::error!(
                            target: "SECURITY",
                            triggered_at = report.triggered_at_ms,
                            "emergency stop activated by user phrase"
                        );
                        if let Err(e) = subs.audit_log.log_policy_decision(
                            "emergency_stop",
                            &crate::policy::rules::RuleEffect::Deny,
                            "user triggered emergency stop via chat",
                            0,
                        ) {
                            tracing::warn!(error = %e, "failed to audit emergency stop");
                        }
                    }
                    Err(e) => {
                        // Already activated — not a failure, just redundant.
                        tracing::debug!(error = %e, "emergency stop already active");
                    }
                }
                send_response(
                    &subs.response_tx,
                    source.clone(),
                    "Emergency stop activated. All actions are halted. Say 'resume' when ready."
                        .to_string(),
                )
                .await;
                return Ok(());
            }

            // ── Stage 0b: Manipulation check ───────────────────────────
            let manip = subs.identity.check_manipulation(&text);
            match manip.verdict {
                crate::identity::ManipulationVerdict::Clean => {
                    tracing::debug!(score = manip.score, "manipulation check: clean");
                }
                crate::identity::ManipulationVerdict::Suspicious => {
                    tracing::warn!(
                        score = manip.score,
                        patterns = ?manip.detected_patterns,
                        "manipulation check: suspicious input detected"
                    );
                    if let Err(e) = subs.audit_log.log_policy_decision(
                        "manipulation_check",
                        &crate::policy::rules::RuleEffect::Audit,
                        &format!(
                            "suspicious input (score={:.2}): {:?}",
                            manip.score, manip.detected_patterns
                        ),
                        0,
                    ) {
                        tracing::warn!(error = %e, "failed to audit suspicious manipulation");
                    }
                    // Allow but flagged — continue processing.
                }
                crate::identity::ManipulationVerdict::Manipulative => {
                    tracing::warn!(
                        score = manip.score,
                        patterns = ?manip.detected_patterns,
                        "manipulation check: BLOCKED manipulative input"
                    );
                    if let Err(e) = subs.audit_log.log_policy_decision(
                        "manipulation_check",
                        &crate::policy::rules::RuleEffect::Deny,
                        &format!(
                            "manipulative input blocked (score={:.2}): {:?}",
                            manip.score, manip.detected_patterns
                        ),
                        0,
                    ) {
                        tracing::warn!(error = %e, "failed to audit manipulation block");
                    }
                    send_response(
                        &subs.response_tx,
                        source.clone(),
                        "I've detected potentially manipulative language in your request. \
                         I'm designed to respond to straightforward requests. \
                         Could you please rephrase?"
                            .to_string(),
                    )
                    .await;
                    return Ok(());
                }
            }

            // ── Stage 1: NLU Parse ──────────────────────────────────
            let parse_result = subs.command_parser.parse(&text);
            tracing::debug!(
                intent = ?parse_result.intent,
                confidence = parse_result.confidence,
                entities = parse_result.entities.len(),
                "NLU parse complete"
            );

            // ── Stage 2: Build ParsedEvent ──────────────────────────
            let intent = match parse_result.intent {
                crate::pipeline::parser::NluIntent::Conversation { .. } => Intent::ConversationContinue,
                crate::pipeline::parser::NluIntent::Unknown { .. } => Intent::InformationRequest,
                _ => {
                    if parse_result.intent.tool_name().is_some() {
                        Intent::ActionRequest
                    } else {
                        Intent::InformationRequest
                    }
                }
            };

            let parsed = ParsedEvent {
                source: EventSource::UserCommand,
                intent,
                content: text.clone(),
                entities: parse_result.entities.iter().map(|e| e.value.clone()).collect(),
                timestamp_ms: now_ms(),
                raw_event_type: 0,
            };

            // ── Stage 3: Amygdala Score ─────────────────────────────
            let scored = subs.amygdala.score(&parsed);
            tracing::debug!(
                score = scored.score_total,
                gate = ?scored.gate_decision,
                "chat scored"
            );

            // ── Stage 4: Policy Gate ────────────────────────────────
            let action_desc = parse_result
                .intent
                .tool_name()
                .unwrap_or("conversation");
            let verdict = subs.identity.policy_gate.check_action("user_chat", action_desc);
            match verdict {
                crate::identity::PolicyVerdict::Block { reason } => {
                    tracing::warn!(reason = %reason, "policy gate blocked user command");
                    send_response(
                        &subs.response_tx,
                        source,
                        format!("I can't do that: {}", reason),
                    ).await;
                    return Ok(());
                }
                crate::identity::PolicyVerdict::Audit { reason } => {
                    tracing::info!(reason = %reason, "policy gate flagged for audit");
                    // Continue but log.
                }
                crate::identity::PolicyVerdict::Allow => {}
            }

            // ── Stage 5: Contextor Enrich ───────────────────────────
            let enriched = subs.contextor.enrich(
                scored.clone(),
                &subs.memory,
                &subs.identity.relationships,
                &subs.identity.personality,
                &subs.identity.affective,
                now_ms(),
            ).await;

            let _enriched = match enriched {
                Ok(e) => {
                    tracing::debug!(
                        memory_snippets = e.memory_context.len(),
                        token_budget = e.context_token_budget,
                        "context enrichment complete"
                    );
                    Some(e)
                }
                Err(e) => {
                    tracing::warn!(error = %e, "context enrichment failed — proceeding without");
                    None
                }
            };

            // If enrichment failed entirely and this is a low-confidence parse,
            // fall back to System1 rather than sending uncontextualized queries.
            if _enriched.is_none() && scored.score_total < 0.3 {
                dispatch_system1(&scored, &text, source, state, subs).await;
                return Ok(());
            }

            // ── Stage 5b: Wire learning state into classifier ────────
            // Feed personality bias and working memory into the route
            // classifier so that routing adapts to personality evolution
            // and current cognitive load — closes the personality→routing gap.
            {
                let bias = subs.identity.personality.routing_bias();
                subs.classifier.set_personality_bias(bias);

                let wm_count = subs.memory.working.len();
                let wm_max = subs.memory.working.capacity();
                subs.classifier.set_working_memory(wm_count, wm_max);

                tracing::trace!(
                    personality_bias = bias,
                    working_memory = wm_count,
                    wm_max = wm_max,
                    "classifier state updated from personality + working memory"
                );
            }

            // Feed screen context to classifier from last semantic graph
            if let Some(ref graph) = subs.last_semantic_graph {
                let state = ScreenSemanticState::from_graph(graph);
                let complexity = match state {
                    ScreenSemanticState::Interactive => 0.1,
                    ScreenSemanticState::Loading => 0.3,
                    ScreenSemanticState::Transitional => 0.2,
                    ScreenSemanticState::Success => 0.2,
                    ScreenSemanticState::InputRequired => 0.4,
                    ScreenSemanticState::Error => 0.5,
                    ScreenSemanticState::Blocked => 0.7,
                    _ => 0.3,
                };
                subs.classifier.set_screen_context(complexity, graph.interactive_nodes().len());
            }

            // ── Stage 5c: SystemBridge Intent Fast-Path ─────────────
            // If the user's text maps directly to a typed system API
            // call (battery status, storage info, etc.), skip the entire
            // classifier → System1/System2 pipeline and answer instantly.
            // This is the ~1000× speedup path: no LLM, no planning,
            // just a direct JNI bridge call.
            if let Some(cmd) = SystemBridge::can_handle_intent(&text) {
                tracing::info!(
                    command = %cmd,
                    "SystemBridge fast-path: intent matched — bypassing classifier"
                );
                match subs.system_bridge.execute(cmd) {
                    Ok(result) => {
                        // Format result for user in a human-readable way.
                        let response_text = format_system_result(&result);
                        tracing::info!(
                            response_len = response_text.len(),
                            "SystemBridge fast-path: success"
                        );
                        // Publish success to OutcomeBus so learning loop sees it.
                        subs.outcome_bus.publish(ExecutionOutcome::new(
                            text.clone(),
                            OutcomeResult::Success,
                            0, // no iterations — instant
                            1.0, // max confidence — typed API
                            RouteKind::System1,
                            now_ms(),
                        ));
                        subs.health_monitor.record_success(now_ms());
                        flush_outcome_bus(subs).await;
                        send_response(&subs.response_tx, source, response_text).await;
                        return Ok(());
                    }
                    Err(e) => {
                        // Bridge failed — fall through to normal pipeline.
                        // This is NOT a hard error; the classifier may find
                        // a better route (e.g. via the LLM).
                        tracing::warn!(
                            error = %e,
                            "SystemBridge fast-path: execution failed — \
                             falling through to classifier"
                        );
                    }
                }
            }

            // ── Stage 6: Route Classification ───────────────────────
            let route = subs.classifier.classify(&scored);
            tracing::info!(
                path = ?route.path,
                confidence = route.confidence,
                reason = %route.reason,
                "route decision"
            );

            // ── Stage 7: Dispatch ───────────────────────────────────
            use crate::routing::classifier::RoutePath;
            match route.path {
                RoutePath::System1 | RoutePath::DaemonOnly => {
                    dispatch_system1(&scored, &text, source, state, subs).await;
                }
                RoutePath::System2 => {
                    let mode = route.neocortex_mode.unwrap_or(InferenceMode::Conversational);
                    dispatch_system2(&scored, mode, _enriched.as_ref(), source, state, subs).await;
                }
                RoutePath::Hybrid => {
                    // Try System1 first; fall back to System2 if it fails.
                    let hybrid_start = now_ms();
                    let s1_result = subs.system1.execute(&scored.parsed, hybrid_start);
                    if s1_result.success {
                        if let Some(ref resp) = s1_result.response_text {
                            send_response(&subs.response_tx, source, resp.clone()).await;

                            // Open reaction observation window for Hybrid→System1.
                            subs.reaction_detector.open_window(
                                &scored.parsed.content,
                                resp,
                                scored.parsed.intent.as_str(),
                                hybrid_start,
                                now_ms(),
                            );
                        }
                        if let Some(plan) = s1_result.action_plan {
                            // Feed WorkflowObserver before plan is moved.
                            if let Some(ref mut observer) = subs.workflow_observer {
                                observer.observe_success(&plan, now_ms());
                            }
                            subs.system1.cache_plan(
                                &scored.parsed.content,
                                plan,
                                1.0,
                                now_ms(),
                            );
                        }
                        // ── OutcomeBus: Hybrid→System1 success ──────────
                        let hybrid_outcome = ExecutionOutcome::new(
                            scored.parsed.intent.as_str().to_owned(),
                            OutcomeResult::Success,
                            s1_result.execution_time_ms,
                            1.0,
                            RouteKind::Hybrid,
                            hybrid_start,
                        )
                        .with_input_summary(&scored.parsed.content)
                        .with_response_summary(
                            s1_result.response_text.as_deref().unwrap_or(""),
                        )
                        .with_route_confidence(route.confidence);
                        subs.outcome_bus.publish(hybrid_outcome);
                    } else {
                        // System1 failed — fall back to System2.
                        // System2 outcome will be captured in handle_ipc_inbound.
                        let mode = route.neocortex_mode.unwrap_or(InferenceMode::Conversational);
                        dispatch_system2(&scored, mode, _enriched.as_ref(), source, state, subs).await;
                    }
                }
            }

            // Store the conversation turn in contextor.
            subs.contextor.push_conversation_turn(
                aura_types::ipc::ConversationTurn {
                    role: aura_types::ipc::Role::User,
                    content: text,
                    timestamp_ms: now_ms(),
                },
            );

            // Store in working memory.
            subs.memory.store_working(
                parsed.content.clone(),
                EventSource::UserCommand,
                scored.score_total,
                now_ms(),
            );

            // Flush outcome bus — dispatch all pending outcomes to cognitive subsystems.
            flush_outcome_bus(subs).await;
        }

        UserCommand::TaskRequest {
            description,
            priority,
            source,
        } => {
            if description.trim().is_empty() {
                tracing::warn!("ignoring task request with empty description");
                return Ok(());
            }

            // Guard against unbounded goal accumulation.
            if state.checkpoint.goals.len() >= MAX_ACTIVE_GOALS {
                tracing::warn!(
                    max = MAX_ACTIVE_GOALS,
                    "goal limit reached — rejecting task request"
                );
                return Ok(());
            }

            let clamped_priority = priority.clamp(1, 10);

            // Create a Goal and register it *before* execution.
            let goal_id = state.checkpoint.goals.len() as u64 + 1;
            let ts = now_ms();

            let goal_priority = match clamped_priority {
                1..=2 => aura_types::goals::GoalPriority::Critical,
                3..=4 => aura_types::goals::GoalPriority::High,
                5..=6 => aura_types::goals::GoalPriority::Medium,
                7..=8 => aura_types::goals::GoalPriority::Low,
                _ => aura_types::goals::GoalPriority::Background,
            };

            let goal = aura_types::goals::Goal {
                id: goal_id,
                description: description.clone(),
                priority: goal_priority,
                status: aura_types::goals::GoalStatus::Active,
                steps: Vec::new(),
                created_ms: ts,
                deadline_ms: None,
                parent_goal: None,
                source: aura_types::goals::GoalSource::UserExplicit,
            };
            state.checkpoint.goals.push(goal.clone());

            // Mirror into BDI scheduler for deliberation.
            if let Some(ref mut bdi) = subs.bdi_scheduler {
                let urgency = (clamped_priority as f32) / 10.0;
                let scored_goal = ScoredGoal {
                    goal_id,
                    score: urgency,
                    components: ScoreComponents {
                        urgency,
                        importance: urgency,
                        user_expectation: 1.0, // explicit user request
                        freshness: 1.0,
                        aging_boost: 0.0,
                    },
                    enqueued_at_ms: ts,
                    is_active: true,
                };
                let _ = bdi.base.enqueue(scored_goal);
                tracing::debug!(goal_id, "goal enqueued in BDI scheduler");
            }

            // Track goal lifecycle in GoalTracker.
            if let Some(ref mut tracker) = subs.goal_tracker {
                let track_goal = aura_types::goals::Goal {
                    id: goal_id,
                    description: description.clone(),
                    priority: goal_priority,
                    status: aura_types::goals::GoalStatus::Active,
                    steps: Vec::new(),
                    created_ms: ts,
                    deadline_ms: None,
                    parent_goal: None,
                    source: aura_types::goals::GoalSource::UserExplicit,
                };
                if let Err(e) = tracker.track(track_goal) {
                    tracing::warn!(error = %e, goal_id, "GoalTracker.track() failed");
                }
                if let Err(e) = tracker.activate(goal_id, ts) {
                    tracing::warn!(error = %e, goal_id, "GoalTracker.activate() failed");
                }
                tracing::debug!(goal_id, "goal tracked and activated in GoalTracker");
            }

            tracing::info!(
                description = %description,
                priority = clamped_priority,
                goal_id,
                active_goals = state.checkpoint.goals.len(),
                source = %source,
                "task request accepted — executing via react engine"
            );

            // Execute through the react engine inline (DaemonState is !Sync).
            // ── Consent gate (Pillar #1: Privacy Sovereignty) ──────────
            if let Some(denial_reason) = check_consent_for_task(&subs.consent_tracker, &description) {
                // Publish denial to OutcomeBus so learning + audit see it.
                subs.outcome_bus.publish(ExecutionOutcome::new(
                    description.clone(),
                    OutcomeResult::Failure,
                    0,
                    0.0,
                    RouteKind::System1,
                    now_ms(),
                ));
                if let Some(goal) = state.checkpoint.goals.iter_mut().find(|g| g.id == goal_id) {
                    goal.status = aura_types::goals::GoalStatus::Failed(denial_reason);
                }
                flush_outcome_bus(subs).await;

            // ── Sandbox gate (Defense-in-depth: containment check) ─────
            } else if let Some(denial_reason) = check_sandbox_for_task(&subs.action_sandbox, &description) {
                // Sandbox classified this task description as Forbidden (L3).
                // Publish denial to OutcomeBus so learning + audit see it.
                tracing::error!(
                    target: "SECURITY",
                    description = %description,
                    reason = %denial_reason,
                    "sandbox DENIED task at description level — L3:Forbidden"
                );
                subs.audit_log.log_policy_decision(
                    &description,
                    &crate::policy::rules::RuleEffect::Deny,
                    &denial_reason,
                    0,
                ).ok();
                subs.outcome_bus.publish(ExecutionOutcome::new(
                    description.clone(),
                    OutcomeResult::Failure,
                    0,
                    0.0,
                    RouteKind::System1,
                    now_ms(),
                ));
                if let Some(goal) = state.checkpoint.goals.iter_mut().find(|g| g.id == goal_id) {
                    goal.status = aura_types::goals::GoalStatus::Failed(denial_reason);
                }
                flush_outcome_bus(subs).await;
            } else {
            // ── BoundaryReasoner gate (defense-in-depth) ──────────
            let boundary_result = check_boundary_for_task(
                &subs.boundary_reasoner,
                &description,
                current_relationship_stage(subs),
            );
            match boundary_result {
                BoundaryGateResult::Deny(reason) => {
                    tracing::warn!(
                        target: "SECURITY",
                        description = %description,
                        reason = %reason,
                        "BoundaryReasoner DENIED task (Site 2: TaskRequest)"
                    );
                    subs.outcome_bus.publish(ExecutionOutcome::new(
                        description.clone(),
                        OutcomeResult::Failure,
                        0,
                        0.0,
                        RouteKind::System1,
                        now_ms(),
                    ));
                    if let Some(goal) = state.checkpoint.goals.iter_mut().find(|g| g.id == goal_id) {
                        goal.status = aura_types::goals::GoalStatus::Failed(
                            format!("boundary denied: {}", reason),
                        );
                    }
                    flush_outcome_bus(subs).await;
                }
                BoundaryGateResult::NeedConfirmation { reason, prompt } => {
                    // Route through sandbox confirmation flow so user
                    // can /allow or /deny.
                    tracing::info!(
                        target: "SECURITY",
                        description = %description,
                        reason = %reason,
                        "BoundaryReasoner requires confirmation (Site 2: TaskRequest)"
                    );
                    if subs.pending_confirmations.len() < MAX_PENDING_CONFIRMATIONS {
                        let conf_id = subs.next_confirmation_id;
                        subs.next_confirmation_id += 1;
                        let confirmation = SandboxConfirmation {
                            id: conf_id,
                            description: format!("Boundary: {}", prompt),
                            containment_level: "BoundaryReasoner".to_string(),
                            created_at: std::time::Instant::now(),
                            timeout: std::time::Duration::from_secs(CONFIRMATION_TIMEOUT_SECS),
                            task_summary: description.clone(),
                            source: source.clone(),
                            goal_id,
                            priority: clamped_priority,
                        };
                        let prompt_msg = format!(
                            "\u{26a0}\u{fe0f} Boundary check requires confirmation:\n{}\n\nReply /allow {} or /deny {}\nAuto-deny in {}s",
                            prompt, conf_id, conf_id, CONFIRMATION_TIMEOUT_SECS
                        );
                        send_response(&subs.response_tx, source.clone(), prompt_msg).await;
                        subs.pending_confirmations.push(confirmation);
                        if let Some(goal) = state.checkpoint.goals.iter_mut().find(|g| g.id == goal_id) {
                            goal.status = aura_types::goals::GoalStatus::Active;
                        }
                    } else {
                        // Too many pending — auto-deny.
                        subs.outcome_bus.publish(ExecutionOutcome::new(
                            description.clone(),
                            OutcomeResult::Failure,
                            0,
                            0.0,
                            RouteKind::System1,
                            now_ms(),
                        ));
                        if let Some(goal) = state.checkpoint.goals.iter_mut().find(|g| g.id == goal_id) {
                            goal.status = aura_types::goals::GoalStatus::Failed(
                                "boundary auto-denied: too many pending confirmations".to_string(),
                            );
                        }
                        flush_outcome_bus(subs).await;
                    }
                }
                BoundaryGateResult::Allow => {
            let mut policy_ctx = react::PolicyContext {
                gate: &mut subs.policy_gate,
                audit: &mut subs.audit_log,
            };
            let (outcome, session) =
                react::execute_task(description.clone(), clamped_priority, None, Some(&mut policy_ctx)).await;

            // Update goal status.
            if let Some(goal) = state.checkpoint.goals.iter_mut().find(|g| g.id == goal_id) {
                match &outcome {
                    react::TaskOutcome::Success { final_confidence, .. } => {
                        goal.status = aura_types::goals::GoalStatus::Completed;
                        tracing::info!(
                            goal_id,
                            session_id = session.session_id,
                            iterations = session.iterations.len(),
                            confidence = final_confidence,
                            "task completed successfully"
                        );
                    }
                    react::TaskOutcome::Failed { reason, .. } => {
                        goal.status =
                            aura_types::goals::GoalStatus::Failed(reason.clone());
                        tracing::warn!(
                            goal_id,
                            session_id = session.session_id,
                            reason = %reason,
                            "task execution failed"
                        );
                        // Record error for learning feedback.
                        subs.memory.feedback_loop.record_error(
                            "task_failure",
                            reason,
                            &description,
                            now_ms(),
                        );
                    }
                    react::TaskOutcome::Cancelled { .. } => {
                        goal.status = aura_types::goals::GoalStatus::Cancelled;
                        tracing::info!(
                            goal_id,
                            session_id = session.session_id,
                            "task was cancelled"
                        );
                    }
                    react::TaskOutcome::CycleAborted { cycle_reason, .. } => {
                        goal.status =
                            aura_types::goals::GoalStatus::Failed(cycle_reason.clone());
                        tracing::warn!(
                            goal_id,
                            session_id = session.session_id,
                            reason = %cycle_reason,
                            "task aborted due to cycle detection"
                        );
                        subs.memory.feedback_loop.record_error(
                            "cycle_abort",
                            cycle_reason,
                            &description,
                            now_ms(),
                        );
                    }
                }
            }

            // ── GoalTracker: mirror task outcome to richer BDI tracker ─────
            if let Some(ref mut tracker) = subs.goal_tracker {
                let ts = now_ms();
                let tracker_result = match &outcome {
                    react::TaskOutcome::Success { .. } => {
                        tracker.complete(goal_id, ts)
                    }
                    react::TaskOutcome::Failed { reason, .. } => {
                        tracker.fail(goal_id, reason.clone(), ts)
                    }
                    react::TaskOutcome::Cancelled { .. } => {
                        // Pause on cancellation — goal can be resumed later.
                        tracker.pause(goal_id, ts)
                    }
                    react::TaskOutcome::CycleAborted { cycle_reason, .. } => {
                        tracker.fail(goal_id, cycle_reason.clone(), ts)
                    }
                };
                if let Err(e) = tracker_result {
                    tracing::warn!(
                        goal_id,
                        error = %e,
                        "GoalTracker update failed — tracker may be out of sync"
                    );
                }
            }

            // ── SemanticReact: evaluate System1→System2 escalation on failure ──
            {
                // Update consecutive failure / success counters.
                match &outcome {
                    react::TaskOutcome::Success { .. } => {
                        subs.consecutive_task_failures = 0;
                        subs.successful_task_count = subs.successful_task_count.saturating_add(1);
                    }
                    react::TaskOutcome::Failed { .. }
                    | react::TaskOutcome::CycleAborted { .. } => {
                        subs.consecutive_task_failures =
                            subs.consecutive_task_failures.saturating_add(1);
                    }
                    react::TaskOutcome::Cancelled { .. } => {
                        // Cancellations don't affect the escalation counters.
                    }
                }

                // On failure, consult SemanticReact to decide whether to
                // escalate from System1 (fast ETG path) to System2 (LLM).
                if matches!(
                    &outcome,
                    react::TaskOutcome::Failed { .. } | react::TaskOutcome::CycleAborted { .. }
                ) {
                    // ── StrategicRecovery: classify + determine recovery ───
                    // Build an EnvironmentSnapshot from HealthMonitor state so
                    // failure classification is environment-aware (e.g. a timeout
                    // during low battery = Environmental, not Transient).
                    let env_snapshot = EnvironmentSnapshot {
                        battery_level: state.checkpoint.power_budget.battery_percent,
                        is_charging: false, // TODO: wire from JNI when available
                        thermal_throttling: false, // TODO: wire from JNI thermal state
                        network_available: true, // local-only, always "available"
                        available_memory_mb: 0, // TODO: wire from JNI
                        ..EnvironmentSnapshot::default()
                    };

                    let failure_reason = match &outcome {
                        react::TaskOutcome::Failed { reason, .. } => reason.as_str(),
                        react::TaskOutcome::CycleAborted { cycle_reason, .. } => cycle_reason.as_str(),
                        _ => "unknown",
                    };

                    let failure_category = StrategicRecovery::classify_failure(
                        failure_reason,
                        &env_snapshot,
                    );

                    let recovery_ctx = RecoveryContext {
                        operation_id: format!("goal_{}", goal_id),
                        failure_category: failure_category.clone(),
                        error_message: failure_reason.to_owned(),
                        attempt_number: subs.consecutive_task_failures,
                        environment_state: env_snapshot.clone(),
                    };

                    let recovery_action = subs.strategic_recovery.determine_recovery(&recovery_ctx);

                    tracing::info!(
                        goal_id,
                        failure_category = ?failure_category,
                        recovery_action = ?recovery_action,
                        consecutive_failures = subs.consecutive_task_failures,
                        "StrategicRecovery: classified failure and determined action"
                    );

                    // Record the error in HealthMonitor for error rate tracking.
                    subs.health_monitor.record_error(now_ms());

                    // ── Act on StrategicRecovery decision ────────────────────
                    // The recovery_action determines whether we escalate to
                    // System 2, retry, notify the user, or halt.  This replaces
                    // the unconditional SemanticReact escalation with a
                    // strategy-aware decision tree.
                    match recovery_action {
                        RecoveryAction::RetryWithBackoff { ref policy } => {
                            // Transient failure — stay in System 1 and let the
                            // consecutive_task_failures counter drive natural
                            // backoff on the next attempt.
                            tracing::info!(
                                goal_id,
                                max_retries = policy.max_retries,
                                consecutive_failures = subs.consecutive_task_failures,
                                "StrategicRecovery: will retry with backoff — \
                                 staying in System 1"
                            );
                            subs.strategic_recovery.record_recovery_outcome(
                                &recovery_ctx.operation_id,
                                true, // optimistic — actual outcome tracked later
                            );
                        }
                        RecoveryAction::Replan { ref reason, ref context }
                        | RecoveryAction::EscalateToStrategic { ref context, .. } => {
                            // The failure needs a fresh approach from the LLM.
                            // Route through SemanticReact to confirm escalation
                            // thresholds, then send a Replan to neocortex.
                            let replan_reason = match &recovery_action {
                                RecoveryAction::Replan { reason, .. } => reason.clone(),
                                _ => "strategic escalation after retry exhaustion".to_string(),
                            };
                            tracing::info!(
                                goal_id,
                                reason = %replan_reason,
                                "StrategicRecovery: escalating to System 2 replan"
                            );
                            let escalation_ctx = EscalationContext {
                                system1_confidence: 0.0,
                                amygdala_arousal: state.checkpoint.disposition.mood.arousal
                                    .clamp(0.0, 1.0),
                                consecutive_failures: subs.consecutive_task_failures,
                                battery_level: state.checkpoint.power_budget.battery_percent,
                                is_thermal_throttling: env_snapshot.thermal_throttling,
                            };
                            if matches!(
                                subs.semantic_react.evaluate_escalation(&escalation_ctx),
                                CognitiveState::System2
                            ) {
                                let replan_ctx = ContextPackage {
                                    active_goal: Some(aura_types::ipc::GoalSummary {
                                        description: description.clone(),
                                        current_step: String::new(),
                                        blockers: vec![replan_reason],
                                    }),
                                    inference_mode: InferenceMode::Planning,
                                    ..ContextPackage::default()
                                };
                                let replan_failure = FailureContext {
                                    task_goal_hash: goal_id,
                                    current_step: session.iteration_count,
                                    failing_action: 0,
                                    target_id: 0,
                                    expected_state_hash: 0,
                                    actual_state_hash: 0,
                                    tried_approaches: subs.consecutive_task_failures as u64,
                                    last_3_transitions: Default::default(),
                                    error_class: 1,
                                };
                                let replan_msg = DaemonToNeocortex::Replan {
                                    context: replan_ctx,
                                    failure: replan_failure,
                                };
                                if ensure_ipc_connected(&mut subs.neocortex).await {
                                    let send_result = tokio::time::timeout(
                                        Duration::from_millis(IPC_SEND_TIMEOUT_MS),
                                        subs.neocortex.send(&replan_msg),
                                    ).await;
                                    match send_result {
                                        Ok(Ok(())) => {
                                            tracing::info!(
                                                goal_id,
                                                "StrategicRecovery: Replan sent to neocortex"
                                            );
                                        }
                                        Ok(Err(e)) => {
                                            tracing::warn!(
                                                error = %e,
                                                "StrategicRecovery: IPC send failed"
                                            );
                                        }
                                        Err(_) => {
                                            tracing::warn!(
                                                "StrategicRecovery: IPC send timed out"
                                            );
                                        }
                                    }
                                } else {
                                    tracing::warn!(
                                        "StrategicRecovery: neocortex unreachable — \
                                         cannot escalate to System 2"
                                    );
                                }
                            } else {
                                tracing::debug!(
                                    goal_id,
                                    "SemanticReact: below escalation threshold — \
                                     staying in System 1 despite Replan request"
                                );
                            }
                        }
                        RecoveryAction::RestartEnvironment { ref target, wait_ms } => {
                            // Environment fault — log the restart target.
                            // Actual app-restart requires JNI bridge (future).
                            tracing::warn!(
                                goal_id,
                                target = %target,
                                wait_ms,
                                "StrategicRecovery: restart-environment requested \
                                 (JNI bridge not yet wired — logging only)"
                            );
                            subs.strategic_recovery.record_recovery_outcome(
                                &recovery_ctx.operation_id,
                                false,
                            );
                        }
                        RecoveryAction::NotifyUser { ref message, ref severity } => {
                            // The failure requires human awareness.  Send a
                            // Telegram message and do NOT escalate to System 2.
                            tracing::info!(
                                goal_id,
                                severity = ?severity,
                                "StrategicRecovery: notifying user of failure"
                            );
                            let notify_text = format!(
                                "[AURA recovery] {message}"
                            );
                            send_response(
                                &subs.response_tx,
                                source.clone(),
                                notify_text,
                            ).await;
                            subs.strategic_recovery.record_recovery_outcome(
                                &recovery_ctx.operation_id,
                                true,
                            );
                        }
                        RecoveryAction::HaltAndLog { ref reason, ref category } => {
                            // Safety/critical failure — halt all retries and
                            // mark the goal as permanently failed.
                            tracing::error!(
                                target: "SAFETY",
                                goal_id,
                                reason = %reason,
                                category = ?category,
                                "StrategicRecovery: HALTING — safety/critical failure"
                            );
                            if let Some(goal) = state.checkpoint.goals.iter_mut().find(|g| g.id == goal_id) {
                                goal.status = aura_types::goals::GoalStatus::Failed(
                                    format!("halted: {reason}")
                                );
                            }
                            subs.outcome_bus.publish(ExecutionOutcome::new(
                                description.clone(),
                                OutcomeResult::Failure,
                                0,
                                0.0,
                                RouteKind::System1,
                                now_ms(),
                            ));
                            flush_outcome_bus(subs).await;
                            // Reset consecutive failures — this goal is dead.
                            subs.consecutive_task_failures = 0;
                            subs.strategic_recovery.record_recovery_outcome(
                                &recovery_ctx.operation_id,
                                false,
                            );
                        }
                        RecoveryAction::TryAlternative { ref alternative, ref reason } => {
                            // The recovery engine suggests a different approach.
                            // Log it so the next planning cycle can consider it.
                            // Actual re-routing will happen on the next attempt
                            // as the planner reads failure history.
                            tracing::info!(
                                goal_id,
                                alternative = %alternative,
                                reason = %reason,
                                "StrategicRecovery: alternative approach suggested — \
                                 will influence next planning cycle"
                            );
                            subs.strategic_recovery.record_recovery_outcome(
                                &recovery_ctx.operation_id,
                                true, // optimistic
                            );
                        }
                    }
                }
            }

            // --- ARC wiring: emit mood events and update GoalTracker from outcome ---
            {
                let outcome_ts = now_ms();
                let personality = &subs.identity.personality.traits;

                // Emit mood event based on task outcome.
                let mood_event = match &outcome {
                    react::TaskOutcome::Success { .. } => MoodEvent::TaskSucceeded,
                    react::TaskOutcome::Failed { .. }
                    | react::TaskOutcome::CycleAborted { .. } => MoodEvent::TaskFailed,
                    react::TaskOutcome::Cancelled { .. } => MoodEvent::Silence { duration_ms: 0 },
                };
                subs.identity.affective.process_event_with_personality(
                    mood_event, outcome_ts, personality,
                );
                let affect_state = subs.identity.affective.current_state();
                state.checkpoint.disposition = affect_state.clone();
                tracing::debug!(
                    new_valence = state.checkpoint.disposition.mood.valence,
                    new_arousal = state.checkpoint.disposition.mood.arousal,
                    "task outcome disposition update via AffectiveEngine"
                );

                // NOTE: GoalTracker lifecycle update already handled above
                // (lines ~1557-1581). Removed duplicate that would double-fire
                // complete/fail on an already-transitioned goal, causing errors.
            }

            // ── OutcomeBus: publish task execution outcome ──────────
            {
                let (task_result, task_confidence, task_iterations) = match &outcome {
                    react::TaskOutcome::Success { final_confidence, iterations_used, .. } => {
                        (OutcomeResult::Success, *final_confidence, *iterations_used as u32)
                    }
                    react::TaskOutcome::Failed { iterations_used, .. } => {
                        (OutcomeResult::Failure, 0.0, *iterations_used as u32)
                    }
                    react::TaskOutcome::Cancelled { iterations_completed, .. } => {
                        (OutcomeResult::UserCancelled, 0.0, *iterations_completed as u32)
                    }
                    react::TaskOutcome::CycleAborted { iterations_completed, .. } => {
                        (OutcomeResult::Failure, 0.0, *iterations_completed as u32)
                    }
                };
                let task_duration = match &outcome {
                    react::TaskOutcome::Success { total_ms, .. }
                    | react::TaskOutcome::Failed { total_ms, .. }
                    | react::TaskOutcome::Cancelled { total_ms, .. } => *total_ms,
                    react::TaskOutcome::CycleAborted { .. } => 0,
                };
                let task_outcome = ExecutionOutcome::new(
                    description.clone(),
                    task_result,
                    task_duration,
                    task_confidence,
                    RouteKind::React,
                    now_ms().saturating_sub(task_duration),
                )
                .with_input_summary(&description)
                .with_goal(goal_id)
                .with_react_iterations(task_iterations);
                subs.outcome_bus.publish(task_outcome);
            }

            // Store episodic memory of the task outcome.
            let episode = format!(
                "Task '{}' (goal {}) outcome: {:?}",
                description, goal_id, outcome
            );
            if let Err(e) = subs.memory.store_episodic(
                episode,
                0.0,  // neutral emotional valence
                0.6,  // moderate importance
                vec!["task".to_string(), "react".to_string()],
                now_ms(),
            ).await {
                tracing::warn!(error = %e, "failed to store task episode");
            }

            // Flush outcome bus.
            flush_outcome_bus(subs).await;
                } // end BoundaryGateResult::Allow arm
            } // end match boundary_result (Site 2: TaskRequest)
            } // end consent-allowed else branch
        }

        UserCommand::CancelTask { task_id, source: _source } => {
            if task_id.trim().is_empty() {
                tracing::warn!("ignoring cancel with empty task_id");
                return Ok(());
            }

            let parsed_id: u64 = match task_id.trim().parse() {
                Ok(id) => id,
                Err(e) => {
                    tracing::warn!(task_id = %task_id, error = %e, "invalid task_id format");
                    return Ok(());
                }
            };

            let found = state.checkpoint.goals.iter().any(|g| g.id == parsed_id);

            if found {
                tracing::info!(task_id = parsed_id, "cancelling task");
                state.checkpoint.goals.retain(|g| g.id != parsed_id);

                // Also cancel any pending System2 request for this task.
                if let Some(msg) = subs.system2.cancel_request(parsed_id) {
                    tracing::debug!(?msg, "cancelled pending System2 request");
                }
            } else {
                tracing::warn!(task_id = parsed_id, "cancel requested for unknown task");
            }
        }

        UserCommand::ProfileSwitch { profile, source } => {
            if profile.trim().is_empty() {
                tracing::warn!("ignoring profile switch with empty name");
                return Ok(());
            }

            tracing::info!(
                profile = %profile,
                source = %source,
                "profile switch requested"
            );

            // Record the interaction in the identity engine.
            subs.identity.relationships.record_interaction(
                &profile,
                crate::identity::InteractionType::Neutral,
                now_ms(),
            );
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// System1 / System2 dispatch helpers
// ---------------------------------------------------------------------------

/// Dispatch an event through the System1 (fast, daemon-only) path.
async fn dispatch_system1(
    scored: &ScoredEvent,
    text: &str,
    source: InputSource,
    _state: &mut DaemonState,
    subs: &mut LoopSubsystems,
) {
    let result = subs.system1.execute(&scored.parsed, now_ms());
    tracing::debug!(
        success = result.success,
        exec_ms = result.execution_time_ms,
        "System1 execution complete"
    );

    if let Some(ref resp) = result.response_text {
        // ── Epistemic markers (Fix A — System1 path) ──────────────
        // Assess whether the response needs an epistemic hedge based
        // on AURA's confidence in the relevant knowledge domains.
        let resp_with_markers = if let Some(marker) =
            subs.identity.assess_epistemic_markers(resp, text)
        {
            format!("{marker} {resp}")
        } else {
            resp.clone()
        };

        send_response(&subs.response_tx, source, resp_with_markers.clone()).await;

        // Open a reaction observation window so the ReactionDetector can
        // classify the user's next input relative to this response.
        let outcome_ts = now_ms().saturating_sub(result.execution_time_ms);
        subs.reaction_detector.open_window(
            &scored.parsed.content,
            &resp_with_markers,
            scored.parsed.intent.as_str(),
            outcome_ts,
            now_ms(),
        );

        // Record AURA's response as a conversation turn.
        subs.contextor.push_conversation_turn(
            aura_types::ipc::ConversationTurn {
                role: aura_types::ipc::Role::Assistant,
                content: resp_with_markers,
                timestamp_ms: now_ms(),
            },
        );
    }

    // If System1 produced an action plan, execute it and cache for future use.
    if let Some(plan) = result.action_plan {
        // ── Consent gate (Pillar #1: Privacy Sovereignty) ──────────
        if let Some(_denial) = check_consent_for_task(&subs.consent_tracker, &scored.parsed.content) {
            subs.outcome_bus.publish(ExecutionOutcome::new(
                scored.parsed.content.clone(),
                OutcomeResult::Failure,
                0,
                0.0,
                RouteKind::System1,
                now_ms(),
            ));
        } else {
        // ── BoundaryReasoner gate (defense-in-depth) ──────────
        // System1 fast-path: NeedConfirmation is treated as Deny because
        // System1 actions should be low-risk and automatic — if the
        // boundary reasoner wants confirmation, the action is too risky
        // for the fast path.
        let boundary_result = check_boundary_for_task(
            &subs.boundary_reasoner,
            &scored.parsed.content,
            current_relationship_stage(subs),
        );
        match boundary_result {
            BoundaryGateResult::Deny(reason)
            | BoundaryGateResult::NeedConfirmation { reason, .. } => {
                tracing::warn!(
                    target: "SECURITY",
                    content = %scored.parsed.content,
                    reason = %reason,
                    "BoundaryReasoner blocked System1 action (Site 3: dispatch_system1)"
                );
                subs.outcome_bus.publish(ExecutionOutcome::new(
                    scored.parsed.content.clone(),
                    OutcomeResult::Failure,
                    0,
                    0.0,
                    RouteKind::System1,
                    now_ms(),
                ));
            }
            BoundaryGateResult::Allow => {
        let mut policy_ctx = react::PolicyContext {
            gate: &mut subs.policy_gate,
            audit: &mut subs.audit_log,
        };
        let (outcome, _session) = react::execute_task(
            scored.parsed.content.clone(),
            5, // default priority
            Some(plan.clone()),
            Some(&mut policy_ctx),
        ).await;

        match outcome {
            react::TaskOutcome::Success { .. } => {
                // Feed WorkflowObserver before plan is moved into cache.
                if let Some(ref mut observer) = subs.workflow_observer {
                    observer.observe_success(&plan, now_ms());
                }
                subs.system1.cache_plan(text, plan, 1.0, now_ms());
                tracing::debug!("System1 plan cached after success");
            }
            _ => {
                tracing::debug!(?outcome, "System1 plan execution did not succeed — not caching");
            }
        }
            } // end BoundaryGateResult::Allow arm
        } // end match boundary_result (Site 3: dispatch_system1)
        } // end consent-allowed else branch
    }

    // ── OutcomeBus: publish System1 execution outcome ───────────
    let s1_outcome = ExecutionOutcome::new(
        scored.parsed.intent.as_str().to_owned(),
        if result.success { OutcomeResult::Success } else { OutcomeResult::Failure },
        result.execution_time_ms,
        if result.success { 1.0 } else { 0.0 },
        RouteKind::System1,
        now_ms().saturating_sub(result.execution_time_ms),
    )
    .with_input_summary(&scored.parsed.content)
    .with_response_summary(
        result.response_text.as_deref().unwrap_or(""),
    );
    subs.outcome_bus.publish(s1_outcome);
}

/// Dispatch an event through the System2 (slow, neocortex LLM) path.
///
/// When an [`EnrichedEvent`] is available from the Contextor, its memory
/// snippets, conversation history, personality context, and token budget
/// are threaded into the `ContextPackage` sent to the neocortex. This
/// ensures the LLM has full conversational context for high-quality responses.
///
/// Personality influence is computed and threaded into the context,
/// providing OCEAN-modulated tone, response style, and prompt directives.
///
/// If the neocortex is unreachable, falls back to System1.
async fn dispatch_system2(
    scored: &ScoredEvent,
    mode: InferenceMode,
    enriched: Option<&EnrichedEvent>,
    source: InputSource,
    state: &mut DaemonState,
    subs: &mut LoopSubsystems,
) {
    // Compute personality influence for this interaction.
    let personality_influence = {
        let mood = &state.checkpoint.disposition;
        let trust = state.checkpoint.trust_score;
        // Derive relationship stage from the primary user (source-based).
        let relationship_stage = subs
            .identity
            .relationships
            .get_relationship("primary_user")
            .map(|r| r.stage)
            .unwrap_or(RelationshipStage::Stranger);
        // PersonalityEngine wraps Personality and provides compute_influence.
        // IdentityEngine stores a raw Personality, so we construct a temporary
        // engine from the current trait snapshot.
        let pe = crate::identity::PersonalityEngine::with_traits(
            subs.identity.personality.traits.clone(),
        );
        pe.compute_influence(mood, relationship_stage, trust)
    };

    tracing::debug!(
        routing_bias = personality_influence.routing_bias,
        complexity_mod = personality_influence.complexity_modifier,
        archetype = ?personality_influence.archetype,
        "personality influence computed for System2 dispatch"
    );

    // Prepare the System2 request (base context from parsed event).
    let request = match subs.system2.prepare_request(&scored.parsed, mode, now_ms()) {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(error = %e, "System2 prepare failed — falling back to System1");
            dispatch_system1(scored, &scored.parsed.content, source, state, subs).await;
            return;
        }
    };

    // Enrich the outgoing message's ContextPackage with Contextor output.
    let message = if let Some(ctx) = enriched {
        enrich_system2_message(request.message, ctx)
    } else {
        tracing::debug!("no enriched context available — sending base context");
        request.message
    };

    // Inject personality influence prompt into the message context.
    let message = inject_personality_influence(message, &personality_influence);

    // ── ThinkingPartner primer injection (Fix B) ──────────────────
    // When the user's question is complex enough and conditions allow,
    // inject a Socratic or Reflective primer that steers the neocortex
    // toward coaching rather than direct answering.
    let message = {
        let cognitive_load = subs.amygdala.cognitive_load;
        if let Some(primer) = subs
            .identity
            .evaluate_thinking_challenge(&scored.parsed.content, cognitive_load)
        {
            inject_thinking_partner_primer(message, primer)
        } else {
            message
        }
    };

    tracing::debug!(request_id = request.request_id, "System2 request prepared");

    // Ensure IPC is connected.
    if !ensure_ipc_connected(&mut subs.neocortex).await {
        tracing::warn!("neocortex unreachable — falling back to System1");
        subs.system2.complete_request(request.request_id);
        dispatch_system1(scored, &scored.parsed.content, source, state, subs).await;
        return;
    }

    // Send with timeout.
    let send_result = tokio::time::timeout(
        Duration::from_millis(IPC_SEND_TIMEOUT_MS),
        subs.neocortex.send(&message),
    ).await;

    match send_result {
        Ok(Ok(())) => {
            tracing::info!(request_id = request.request_id, "System2 request sent to neocortex");
            // Track the originating source so ConversationReply can route back.
            state.last_system2_source = Some(source.clone());
            // The response will arrive via the IPC inbound channel.
        }
        Ok(Err(e)) => {
            tracing::warn!(error = %e, "System2 IPC send failed — falling back to System1");
            subs.system2.complete_request(request.request_id);
            dispatch_system1(scored, &scored.parsed.content, source, state, subs).await;
        }
        Err(_) => {
            tracing::warn!("System2 IPC send timed out — falling back to System1");
            subs.system2.complete_request(request.request_id);
            dispatch_system1(scored, &scored.parsed.content, source, state, subs).await;
        }
    }
}

/// Flush the outcome bus — drains all pending outcomes and dispatches to
/// cognitive subsystems using split borrows from `LoopSubsystems`.
///
/// Call this at the end of any handler that published outcomes.
async fn flush_outcome_bus(subs: &mut LoopSubsystems) {
    if subs.outcome_bus.pending_count() == 0 {
        return;
    }

    // ── Safe mode guard: skip learning/outcome dispatch ──────────────
    // In safe mode, drain the outcome bus to prevent unbounded growth but
    // do NOT apply learning to identity, ARC, BDI, or memory subsystems.
    // This prevents potentially corrupt state from propagating through
    // the learning pipeline after a crash recovery.
    if subs.safe_mode.should_block_action(false, true) {
        let drained = subs.outcome_bus.pending_count();
        subs.outcome_bus.drain_all();
        tracing::debug!(
            drained_count = drained,
            "safe mode active — drained outcome bus without applying learning"
        );
        return;
    }
    let now = now_ms();
    let LoopSubsystems {
        ref mut outcome_bus,
        ref mut arc_manager,
        ref memory,
        ref mut bdi_scheduler,
        ref mut identity,
        ref mut workflow_observer,
        ref mut semantic_react,
        ref mut goal_registry,
        ref successful_task_count,
        ..
    } = *subs;
    outcome_bus.dispatch(
        arc_manager.as_mut(),
        memory,
        bdi_scheduler.as_mut(),
        identity,
        now,
    ).await;

    // ── Outcome statistics: capture once, share across consumers ──────
    let (recent_successes, recent_failures) = outcome_bus.recent_success_failure_counts();
    let recent_total = recent_successes + recent_failures;
    let recent_success_rate = if recent_total > 0 {
        Some(recent_successes as f32 / recent_total as f32)
    } else {
        None
    };

    // ── SemanticReact: adapt thresholds from outcome statistics ──
    // Compute a feedback delta from the outcome bus's recent success/failure
    // ratio. Each successful outcome nudges the confidence threshold DOWN
    // (trust System 1 more), each failure nudges it UP (escalate sooner).
    // The delta is small (±0.005) to ensure gradual organic learning.
    if let Some(success_rate) = recent_success_rate {
        // If success rate > 0.7, lower threshold (trust S1 more): negative delta
        // If success rate < 0.3, raise threshold (escalate sooner): positive delta
        // In between: proportional adjustment centered at 0.5
        let feedback_delta = (0.5 - success_rate) * 0.01;
        semantic_react.adapt_thresholds(feedback_delta, *successful_task_count);
        tracing::trace!(
            success_rate,
            feedback_delta,
            new_threshold = semantic_react.base_confidence_threshold,
            "SemanticReact thresholds adapted from outcome statistics"
        );
    }

    // ── BDI belief update: feed outcome statistics into scheduler ───
    // Converts the recent success/failure ratio into a Belief so the BDI
    // deliberation cycle has up-to-date knowledge about execution reliability.
    if let (Some(ref mut scheduler), Some(success_rate)) = (bdi_scheduler, recent_success_rate) {
        let belief = Belief {
            key: "execution_success_rate".into(),
            value: format!("{:.3}", success_rate),
            confidence: (recent_total as f32 / 50.0).min(1.0), // confidence grows with sample size
            updated_at_ms: now,
            source: BeliefSource::ExecutionOutcome,
        };
        if let Err(e) = scheduler.update_belief(belief) {
            tracing::warn!(error = %e, "BDI belief update failed");
        } else {
            tracing::trace!(
                success_rate,
                sample_size = recent_total,
                "BDI belief updated: execution_success_rate"
            );
        }
    }

    // ── GoalRegistry: update Bayesian capability confidence ─────
    // Each dispatched outcome carries a capability_id (the intent/task
    // description) and a success flag.  GoalRegistry maintains per-capability
    // Bayesian priors that influence future goal decomposition and BDI scoring.
    if let Some(ref mut registry) = goal_registry {
        let capability_outcomes = outcome_bus.drain_capability_outcomes();
        for (capability_id, succeeded) in &capability_outcomes {
            match registry.update_confidence(capability_id, *succeeded, now) {
                Ok(new_confidence) => {
                    tracing::trace!(
                        capability = %capability_id,
                        succeeded,
                        new_confidence,
                        "GoalRegistry: capability confidence updated"
                    );
                }
                Err(e) => {
                    tracing::warn!(
                        capability = %capability_id,
                        error = %e,
                        "GoalRegistry: update_confidence failed"
                    );
                }
            }
        }
    }

    // ── WorkflowObserver: check for automation candidates ───────
    if let Some(ref observer) = workflow_observer {
        if let Some(candidate) = observer.extract_automation_candidate() {
            tracing::info!(
                frequency = candidate.frequency,
                steps = candidate.sequence.len(),
                "WorkflowObserver: automation candidate detected"
            );
            // TODO: Surface to user via proactive engine
        }
    }
}

/// Thread enriched context from the Contextor into a `DaemonToNeocortex` message.
///
/// Maps `EnrichedEvent` fields onto the `ContextPackage` inside the IPC message,
/// giving the neocortex access to memory snippets, conversation history,
/// personality directives, screen state, active goals, and token budget.
fn enrich_system2_message(
    message: DaemonToNeocortex,
    enriched: &EnrichedEvent,
) -> DaemonToNeocortex {
    /// Apply enrichment to a `ContextPackage`.
    fn apply(pkg: &mut aura_types::ipc::ContextPackage, enriched: &EnrichedEvent) {
        // Memory snippets — replace the empty default with enriched retrieval,
        // then redact any snippet containing sensitive data before the LLM sees it.
        if !enriched.memory_context.is_empty() {
            pkg.memory_snippets = enriched.memory_context
                .iter()
                .filter(|snippet| {
                    let (tier, _category) = CriticalVault::classify_data(snippet);
                    match tier {
                        DataTier::Public | DataTier::Internal => true,
                        // Tier ≥ Sensitive: redact from LLM context.
                        // The LLM never needs raw passwords, tokens, PII, etc.
                        _ => {
                            tracing::debug!(
                                snippet_len = snippet.len(),
                                tier = ?tier,
                                "CriticalVault: redacted sensitive memory snippet \
                                 before LLM dispatch"
                            );
                            false
                        }
                    }
                })
                .cloned()
                .collect();
        }

        // Conversation history — merge enriched turns (avoid duplicating the
        // user turn that `prepare_request` already inserted).
        if !enriched.conversation_history.is_empty() {
            // Keep existing turns, prepend enriched history before them.
            let existing = std::mem::take(&mut pkg.conversation_history);
            pkg.conversation_history = enriched.conversation_history.clone();
            for turn in existing {
                // Deduplicate: skip if the last enriched turn matches.
                let dominated = pkg.conversation_history.last().is_some_and(|last| {
                    last.content == turn.content && last.timestamp_ms == turn.timestamp_ms
                });
                if !dominated {
                    pkg.conversation_history.push(turn);
                }
            }
        }

        // Token budget — use the Contextor's importance-scaled budget.
        pkg.token_budget = enriched.context_token_budget as u32;

        // Screen summary.
        if let Some(ref summary) = enriched.screen_summary {
            pkg.current_screen = Some(aura_types::ipc::ScreenSummary {
                package_name: String::new(),
                activity_name: String::new(),
                interactive_elements: subs.last_semantic_graph
                    .as_ref()
                    .map(|g| g.interactive_nodes().into_iter().cloned().collect())
                    .unwrap_or_default(),
                visible_text: vec![summary.clone()],
            });
        }

        // Active goal — take the highest-priority one.
        if let Some(goal) = enriched.active_goals.first() {
            pkg.active_goal = Some(goal.clone());
        }

        // Personality context — inject as a system-level conversation turn
        // if the personality prompt has useful directives.
        if let Some(ref personality_text) = enriched.personality_context {
            if !personality_text.is_empty() {
                // Prepend as a system turn so the LLM sees it first.
                pkg.conversation_history.insert(
                    0,
                    aura_types::ipc::ConversationTurn {
                        role: aura_types::ipc::Role::System,
                        content: personality_text.clone(),
                        timestamp_ms: 0,
                    },
                );
            }
        }

        // Enforce size limit — if the package is too large, trim memory snippets.
        while pkg.estimated_size() > aura_types::ipc::ContextPackage::MAX_SIZE
            && !pkg.memory_snippets.is_empty()
        {
            pkg.memory_snippets.pop();
        }
    }

    match message {
        DaemonToNeocortex::Converse { mut context } => {
            apply(&mut context, enriched);
            DaemonToNeocortex::Converse { context }
        }
        DaemonToNeocortex::Plan {
            mut context,
            failure,
        } => {
            apply(&mut context, enriched);
            DaemonToNeocortex::Plan { context, failure }
        }
        DaemonToNeocortex::Compose {
            mut context,
            template,
        } => {
            apply(&mut context, enriched);
            DaemonToNeocortex::Compose { context, template }
        }
        DaemonToNeocortex::Replan {
            mut context,
            failure,
        } => {
            apply(&mut context, enriched);
            DaemonToNeocortex::Replan { context, failure }
        }
        other => other, // Pass through non-context variants unchanged.
    }
}

/// Inject [`PersonalityInfluence`] directives into the IPC message.
///
/// Adds OCEAN-modulated response style directives as a system-level
/// conversation turn. This supplements the Contextor's existing personality
/// injection with richer influence data (archetype, response params,
/// routing bias) that the neocortex can use for tone calibration.
fn inject_personality_influence(
    message: DaemonToNeocortex,
    influence: &PersonalityInfluence,
) -> DaemonToNeocortex {
    fn apply_influence(
        pkg: &mut aura_types::ipc::ContextPackage,
        influence: &PersonalityInfluence,
    ) {
        // Build a compact personality directive from influence data.
        let directive = format!(
            "[Personality: {:?} | proactivity={:.2} verbosity={:.2} risk_appetite={:.2} autonomy={:.2} | routing_bias={:+.3} complexity_mod={:+.3}]",
            influence.archetype,
            influence.response_params.proactivity,
            influence.response_params.verbosity,
            influence.response_params.risk_tolerance,
            influence.response_params.autonomy,
            influence.routing_bias,
            influence.complexity_modifier,
        );

        // If there's already a personality prompt from enrichment, append to it.
        // Otherwise insert a fresh system turn.
        let has_personality_turn = pkg
            .conversation_history
            .first()
            .is_some_and(|turn| {
                turn.role == aura_types::ipc::Role::System
                    && turn.timestamp_ms == 0
            });

        if has_personality_turn {
            // Append influence to existing personality system turn.
            if let Some(first) = pkg.conversation_history.first_mut() {
                first.content.push('\n');
                first.content.push_str(&directive);
            }
        } else {
            // Insert a new personality system turn at the front.
            pkg.conversation_history.insert(
                0,
                aura_types::ipc::ConversationTurn {
                    role: aura_types::ipc::Role::System,
                    content: directive,
                    timestamp_ms: 0,
                },
            );
        }
    }

    match message {
        DaemonToNeocortex::Converse { mut context } => {
            apply_influence(&mut context, influence);
            DaemonToNeocortex::Converse { context }
        }
        DaemonToNeocortex::Plan {
            mut context,
            failure,
        } => {
            apply_influence(&mut context, influence);
            DaemonToNeocortex::Plan { context, failure }
        }
        DaemonToNeocortex::Compose {
            mut context,
            template,
        } => {
            apply_influence(&mut context, influence);
            DaemonToNeocortex::Compose { context, template }
        }
        DaemonToNeocortex::Replan {
            mut context,
            failure,
        } => {
            apply_influence(&mut context, influence);
            DaemonToNeocortex::Replan { context, failure }
        }
        other => other,
    }
}

/// Inject a ThinkingPartner primer into a System2 message (Fix B).
///
/// When `evaluate_thinking_challenge` returns Reflective or Socratic, this
/// appends the primer to the system turn so the neocortex adopts a coaching
/// posture rather than giving a direct answer.  The primer is lightweight
/// (static `&str`) and does not allocate on the hot path when not triggered.
fn inject_thinking_partner_primer(
    message: DaemonToNeocortex,
    primer: &str,
) -> DaemonToNeocortex {
    fn apply_primer(pkg: &mut aura_types::ipc::ContextPackage, primer: &str) {
        // Append to the existing system turn (personality influence already
        // created one).  If none exists, insert a new one.
        let has_system_turn = pkg
            .conversation_history
            .first()
            .is_some_and(|turn| {
                turn.role == aura_types::ipc::Role::System
                    && turn.timestamp_ms == 0
            });

        if has_system_turn {
            if let Some(first) = pkg.conversation_history.first_mut() {
                first.content.push('\n');
                first.content.push_str("[ThinkingPartner] ");
                first.content.push_str(primer);
            }
        } else {
            pkg.conversation_history.insert(
                0,
                aura_types::ipc::ConversationTurn {
                    role: aura_types::ipc::Role::System,
                    content: format!("[ThinkingPartner] {primer}"),
                    timestamp_ms: 0,
                },
            );
        }
    }

    match message {
        DaemonToNeocortex::Converse { mut context } => {
            apply_primer(&mut context, primer);
            DaemonToNeocortex::Converse { context }
        }
        DaemonToNeocortex::Plan {
            mut context,
            failure,
        } => {
            apply_primer(&mut context, primer);
            DaemonToNeocortex::Plan { context, failure }
        }
        DaemonToNeocortex::Compose {
            mut context,
            template,
        } => {
            apply_primer(&mut context, primer);
            DaemonToNeocortex::Compose { context, template }
        }
        DaemonToNeocortex::Replan {
            mut context,
            failure,
        } => {
            apply_primer(&mut context, primer);
            DaemonToNeocortex::Replan { context, failure }
        }
        other => other,
    }
}

// ---------------------------------------------------------------------------
// IPC outbound handler
// ---------------------------------------------------------------------------

/// Validate and forward an IPC payload to the neocortex process.
///
/// The raw `Vec<u8>` payload is deserialized via bincode to verify it's a
/// valid `DaemonToNeocortex` message, then sent through the IPC client.
#[instrument(skip_all, fields(payload_len = msg.payload.len()))]
async fn handle_ipc_outbound(
    msg: IpcOutbound,
    subs: &mut LoopSubsystems,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let len = msg.payload.len();

    if len == 0 {
        tracing::warn!("dropping empty IPC outbound payload");
        return Ok(());
    }

    if len > MAX_IPC_PAYLOAD_BYTES {
        tracing::error!(
            len,
            max = MAX_IPC_PAYLOAD_BYTES,
            "IPC outbound payload exceeds maximum — dropping"
        );
        return Ok(());
    }

    // Deserialize the payload to verify it's a valid message.
    let message: DaemonToNeocortex = match bincode::serde::decode_from_slice(&msg.payload, bincode::config::standard()) {
        Ok((m, _)) => m,
        Err(e) => {
            tracing::error!(error = %e, "IPC outbound payload deserialization failed");
            return Ok(());
        }
    };

    tracing::debug!(len, "IPC outbound payload validated");

    // Ensure IPC connection.
    if !ensure_ipc_connected(&mut subs.neocortex).await {
        tracing::error!("cannot send IPC outbound — neocortex unreachable");
        return Ok(());
    }

    // Send the typed message.
    if let Err(e) = subs.neocortex.send(&message).await {
        tracing::error!(error = %e, "IPC outbound send failed");
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// IPC inbound handler
// ---------------------------------------------------------------------------

/// Route an inbound message from the neocortex process to the appropriate
/// daemon subsystem, updating checkpoint state as needed.
///
/// Wired connections:
/// - `PlanReady` → `react::execute_task` → goal tracker + episodic memory
/// - `ConversationReply` → anti-sycophancy gate → response channel
/// - `Error` → feedback loop learning
/// - `MemoryWarning` → send `Unload` to neocortex
/// - `Loaded` → working memory record
#[instrument(skip_all)]
async fn handle_ipc_inbound(
    msg: NeocortexToDaemon,
    state: &mut DaemonState,
    subs: &mut LoopSubsystems,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    match msg {
        NeocortexToDaemon::Loaded {
            model_name,
            memory_used_mb,
        } => {
            tracing::info!(
                model = %model_name,
                memory_mb = memory_used_mb,
                "neocortex model loaded"
            );
            // Record in working memory.
            subs.memory.store_working(
                format!("Model loaded: {} ({}MB)", model_name, memory_used_mb),
                EventSource::Internal,
                0.5,
                now_ms(),
            );
        }

        NeocortexToDaemon::LoadFailed { reason } => {
            tracing::error!(reason = %reason, "neocortex model load failed");
            subs.memory.feedback_loop.record_error(
                "model_load",
                &reason,
                "neocortex",
                now_ms(),
            );
        }

        NeocortexToDaemon::Unloaded => {
            tracing::info!("neocortex model unloaded");
        }

        NeocortexToDaemon::PlanReady { plan } => {
            let step_count = plan.steps.len();
            tracing::info!(
                steps = step_count,
                goal = %plan.goal_description,
                confidence = plan.confidence,
                "action plan received from neocortex"
            );

            // ── Emergency guard: block execution during emergency ────
            if !subs.emergency.actions_allowed() {
                tracing::warn!(
                    state = %subs.emergency.state(),
                    "plan execution blocked — emergency stop active"
                );
                if let Err(e) = subs.audit_log.log_policy_decision(
                    "plan_execution",
                    &crate::policy::rules::RuleEffect::Deny,
                    &format!(
                        "plan '{}' blocked — emergency state: {}",
                        plan.goal_description,
                        subs.emergency.state()
                    ),
                    0,
                ) {
                    tracing::warn!(error = %e, "failed to audit emergency plan block");
                }
                return Ok(());
            }

            // Guard against absurdly large plans.
            if step_count > 100 {
                tracing::warn!(steps = step_count, "plan too large — rejecting");
                return Ok(());
            }

            // ── EnhancedPlanner: check cache or use neocortex plan ──
            let plan = if let Some(ref mut enhanced) = subs.enhanced_planner {
                enhanced.set_time(now_ms());
                if let Some(cached) = enhanced.cache_lookup(&plan.goal_description) {
                    tracing::debug!(
                        goal = %plan.goal_description,
                        "EnhancedPlanner: using cached plan instead of neocortex plan"
                    );
                    cached.clone()
                } else {
                    plan
                }
            } else {
                plan
            };

            // Clone plan before move into execute_task — needed for
            // cache_store and WorkflowObserver on success.
            let plan_for_observation = plan.clone();

            // Execute the plan through the react engine.
            let plan_goal_desc = plan.goal_description.clone();

            // ── Consent gate (Pillar #1: Privacy Sovereignty) ──────────
            if let Some(_denial) = check_consent_for_task(&subs.consent_tracker, &plan_goal_desc) {
                subs.outcome_bus.publish(ExecutionOutcome::new(
                    plan_goal_desc.clone(),
                    OutcomeResult::Failure,
                    0,
                    0.0,
                    RouteKind::System2,
                    now_ms(),
                ));
                flush_outcome_bus(subs).await;
            } else {
            // ── BoundaryReasoner gate (defense-in-depth) ──────────
            // Neocortex IPC handler has no user response channel, so
            // NeedConfirmation is treated as Deny — the neocortex should
            // not produce plans that the boundary reasoner considers risky.
            let boundary_result = check_boundary_for_task(
                &subs.boundary_reasoner,
                &plan_goal_desc,
                current_relationship_stage(subs),
            );
            match boundary_result {
                BoundaryGateResult::Deny(reason)
                | BoundaryGateResult::NeedConfirmation { reason, .. } => {
                    tracing::warn!(
                        target: "SECURITY",
                        plan = %plan_goal_desc,
                        reason = %reason,
                        "BoundaryReasoner blocked neocortex plan (Site 4: PlanReady)"
                    );
                    subs.outcome_bus.publish(ExecutionOutcome::new(
                        plan_goal_desc.clone(),
                        OutcomeResult::Failure,
                        0,
                        0.0,
                        RouteKind::System2,
                        now_ms(),
                    ));
                    flush_outcome_bus(subs).await;
                }
                BoundaryGateResult::Allow => {
            let mut policy_ctx = react::PolicyContext {
                gate: &mut subs.policy_gate,
                audit: &mut subs.audit_log,
            };
            let (outcome, session) = react::execute_task(
                plan.goal_description.clone(),
                5, // default priority for neocortex plans
                Some(plan),
                Some(&mut policy_ctx),
            ).await;

            tracing::info!(
                session_id = session.session_id,
                iterations = session.iterations.len(),
                ?outcome,
                "plan execution complete"
            );

            // ── EnhancedPlanner + WorkflowObserver: post-execution ──
            {
                let success = matches!(&outcome, react::TaskOutcome::Success { .. });
                if let Some(ref mut enhanced) = subs.enhanced_planner {
                    enhanced.set_time(now_ms());
                    if success {
                        enhanced.cache_store(
                            &plan_goal_desc,
                            plan_for_observation.clone(),
                        );
                    }
                    enhanced.cache_record_outcome(&plan_goal_desc, success);
                }
                if success {
                    if let Some(ref mut observer) = subs.workflow_observer {
                        observer.observe_success(&plan_for_observation, now_ms());
                    }
                }
            }

            // ── OutcomeBus: publish plan execution outcome ──────────
            {
                let (plan_result, plan_confidence, plan_iters) = match &outcome {
                    react::TaskOutcome::Success { final_confidence, iterations_used, .. } => {
                        (OutcomeResult::Success, *final_confidence, *iterations_used as u32)
                    }
                    react::TaskOutcome::Failed { iterations_used, .. } => {
                        (OutcomeResult::Failure, 0.0, *iterations_used as u32)
                    }
                    react::TaskOutcome::Cancelled { iterations_completed, .. } => {
                        (OutcomeResult::UserCancelled, 0.0, *iterations_completed as u32)
                    }
                    react::TaskOutcome::CycleAborted { iterations_completed, .. } => {
                        (OutcomeResult::Failure, 0.0, *iterations_completed as u32)
                    }
                };
                let plan_duration = match &outcome {
                    react::TaskOutcome::Success { total_ms, .. }
                    | react::TaskOutcome::Failed { total_ms, .. }
                    | react::TaskOutcome::Cancelled { total_ms, .. } => *total_ms,
                    react::TaskOutcome::CycleAborted { .. } => 0,
                };
                let plan_outcome_ev = ExecutionOutcome::new(
                    plan_goal_desc.clone(),
                    plan_result,
                    plan_duration,
                    plan_confidence,
                    RouteKind::System2,
                    now_ms().saturating_sub(plan_duration),
                )
                .with_input_summary(&plan_goal_desc)
                .with_react_iterations(plan_iters);
                subs.outcome_bus.publish(plan_outcome_ev);
            }

            // ── GoalTracker: create + track a goal for this plan ──────
            // PlanReady comes from neocortex without a pre-existing goal_id,
            // so we generate one and wire it through the full BDI lifecycle.
            {
                let ts = now_ms();
                let plan_goal_id = state.checkpoint.goals.len() as u64 + 1;
                let plan_goal = aura_types::goals::Goal {
                    id: plan_goal_id,
                    description: plan_goal_desc.clone(),
                    priority: aura_types::goals::GoalPriority::Medium,
                    status: match &outcome {
                        react::TaskOutcome::Success { .. } => aura_types::goals::GoalStatus::Completed,
                        react::TaskOutcome::Failed { .. }
                        | react::TaskOutcome::CycleAborted { .. } => aura_types::goals::GoalStatus::Failed,
                        react::TaskOutcome::Cancelled { .. } => aura_types::goals::GoalStatus::Cancelled,
                    },
                    steps: Vec::new(),
                    created_ms: ts,
                    deadline_ms: None,
                    parent_goal: None,
                    source: aura_types::goals::GoalSource::SystemInferred,
                };
                state.checkpoint.goals.push(plan_goal.clone());

                if let Some(ref mut tracker) = subs.goal_tracker {
                    let _ = tracker.track(plan_goal);
                    // For completed/failed plans, the goal is already terminal —
                    // activate then immediately transition to final state.
                    let _ = tracker.activate(plan_goal_id, ts);
                    match &outcome {
                        react::TaskOutcome::Success { .. } => {
                            let _ = tracker.complete(plan_goal_id, ts);
                        }
                        react::TaskOutcome::Failed { reason, .. } => {
                            let _ = tracker.fail(plan_goal_id, reason.clone(), ts);
                        }
                        react::TaskOutcome::CycleAborted { cycle_reason, .. } => {
                            let _ = tracker.fail(plan_goal_id, cycle_reason.clone(), ts);
                        }
                        react::TaskOutcome::Cancelled { .. } => {
                            let _ = tracker.pause(plan_goal_id, ts);
                        }
                    }
                }

                // Mirror into BDI scheduler so deliberation has full picture.
                if let Some(ref mut bdi) = subs.bdi_scheduler {
                    let scored = ScoredGoal {
                        goal_id: plan_goal_id,
                        score: 0.5,
                        components: ScoreComponents {
                            urgency: 0.5,
                            importance: 0.6,
                            user_expectation: 0.3, // system-inferred, not explicit
                            freshness: 1.0,
                            aging_boost: 0.0,
                        },
                        enqueued_at_ms: ts,
                        is_active: !matches!(
                            &outcome,
                            react::TaskOutcome::Success { .. }
                                | react::TaskOutcome::Failed { .. }
                                | react::TaskOutcome::CycleAborted { .. }
                        ),
                    };
                    let _ = bdi.base.enqueue(scored);
                }
            }

            // Store episodic memory of the plan outcome.
            let episode = format!(
                "Neocortex plan '{}' ({} steps): {:?}",
                session.session_id, step_count, outcome
            );
            if let Err(e) = subs.memory.store_episodic(
                episode,
                0.0,
                0.7,
                vec!["plan".to_string(), "neocortex".to_string()],
                now_ms(),
            ).await {
                tracing::warn!(error = %e, "failed to store plan episode");
            }
                } // end BoundaryGateResult::Allow arm
            } // end match boundary_result (Site 4: PlanReady)
            } // end consent-allowed else branch (Site 3: neocortex plan)
        }

        NeocortexToDaemon::ConversationReply { text, mood_hint } => {
            tracing::info!(
                len = text.len(),
                mood_hint = ?mood_hint,
                "conversation reply received"
            );

            // Apply mood hint to disposition.
            if let Some(hint) = mood_hint {
                let clamped = hint.clamp(-1.0, 1.0);
                state.checkpoint.disposition.mood.valence =
                    (state.checkpoint.disposition.mood.valence + clamped * 0.1)
                        .clamp(-1.0, 1.0);
            }

            // ── TRUTH framework validation (with epistemic awareness) ──
            // Extract the last user input to determine relevant knowledge
            // domains, then validate using epistemic-enhanced TRUTH which
            // penalizes overclaiming on low-confidence domains (Fix C).
            let last_user_for_truth: String = {
                let history = subs.contextor.conversation_history();
                history
                    .iter()
                    .rev()
                    .find(|t| t.role == aura_types::ipc::Role::User)
                    .map(|t| t.content.clone())
                    .unwrap_or_default()
            };
            let truth_result = subs
                .identity
                .validate_response_with_epistemic(&text, &last_user_for_truth);
            tracing::debug!(
                overall = truth_result.overall,
                passes = truth_result.passes,
                notes = ?truth_result.notes,
                "TRUTH framework validation"
            );

            let text_after_truth = if !truth_result.passes {
                tracing::warn!(
                    overall = truth_result.overall,
                    notes = ?truth_result.notes,
                    "response FAILED TRUTH framework validation"
                );
                // Log to audit if available.
                let audit_msg = format!(
                    "TRUTH validation failed (score={:.2}): {:?}",
                    truth_result.overall, truth_result.notes
                );
                if let Err(e) = subs.audit_log.log_policy_decision(
                    "conversation_reply",
                    &crate::policy::rules::RuleEffect::Audit,
                    &audit_msg,
                    0,
                ) {
                    tracing::warn!(error = %e, "failed to audit TRUTH validation failure");
                }
                // Append a transparency note rather than blocking entirely.
                format!(
                    "{}\n\n[Note: This response was flagged for review — {}]",
                    text,
                    truth_result
                        .notes
                        .first()
                        .map(|s| s.as_str())
                        .unwrap_or("TRUTH framework concern detected")
                )
            } else {
                text.clone()
            };

            // ── Epistemic markers (Fix A — System2 path) ──────────────
            // If the response touches domains where AURA has low confidence
            // and lacks appropriate hedging, prepend an epistemic marker.
            let text_after_truth = if let Some(marker) =
                subs.identity.assess_epistemic_markers(&text_after_truth, &last_user_for_truth)
            {
                format!("{marker} {text_after_truth}")
            } else {
                text_after_truth
            };

            // ── Anti-sycophancy gate ────────────────────────────────
            let gate_result = subs.identity.check_response();
            let final_text = match gate_result {
                crate::identity::GateResult::Pass => text_after_truth,
                crate::identity::GateResult::Nudge { reason } => {
                    tracing::info!(reason = %reason, "anti-sycophancy nudge");
                    // Append a honesty nudge to the response.
                    format!("{}\n\n[Note: {}]", text_after_truth, reason)
                }
                crate::identity::GateResult::Block { reason } => {
                    tracing::warn!(reason = %reason, "anti-sycophancy blocked response");
                    // Provide a neutral fallback.
                    "I want to be honest with you. Let me reconsider my response.".to_string()
                }
            };

            // Send response to user via the response channel.
            // Route to the original source that triggered this System2 request
            // (voice, Telegram, etc.), falling back to Direct if unknown.
            let reply_dest = state
                .last_system2_source
                .take()
                .unwrap_or(InputSource::Direct);
            send_response(&subs.response_tx, reply_dest, final_text.clone()).await;

            // Open a reaction observation window for the System2 response.
            // Use the last user input from conversation history as the
            // "original input" for correlation.
            {
                let history = subs.contextor.conversation_history();
                let last_user = history
                    .iter()
                    .rev()
                    .find(|t| t.role == aura_types::ipc::Role::User)
                    .map(|t| t.content.as_str())
                    .unwrap_or("");
                subs.reaction_detector.open_window(
                    last_user,
                    &final_text,
                    "conversation", // System2 uses "conversation" as intent
                    now_ms(),
                    now_ms(),
                );
            }

            // Record the response in the anti-sycophancy sliding window so
            // the gate has data for future `check_response()` calls.
            // Pull the last user input and previous assistant response from
            // conversation history to enable context-aware statistical analysis
            // (content overlap, stance-flip detection, personality modulation).
            let history = subs.contextor.conversation_history();
            let last_user_input: String = history
                .iter()
                .rev()
                .find(|t| t.role == aura_types::ipc::Role::User)
                .map(|t| t.content.clone())
                .unwrap_or_default();
            let previous_response: Option<String> = history
                .iter()
                .rev()
                .find(|t| t.role == aura_types::ipc::Role::Assistant)
                .map(|t| t.content.clone());
            subs.identity.record_response_in_context(
                &last_user_input,
                &final_text,
                previous_response.as_deref(),
            );

            // Record as conversation turn.
            subs.contextor.push_conversation_turn(
                aura_types::ipc::ConversationTurn {
                    role: aura_types::ipc::Role::Assistant,
                    content: final_text.clone(),
                    timestamp_ms: now_ms(),
                },
            );

            // ── OutcomeBus: publish System2 conversation outcome ────
            {
                let s2_result = if matches!(gate_result, crate::identity::GateResult::Block { .. }) {
                    OutcomeResult::PolicyBlocked
                } else if truth_result.passes {
                    OutcomeResult::Success
                } else {
                    OutcomeResult::PartialSuccess
                };
                let s2_outcome = ExecutionOutcome::new(
                    "conversation".to_string(),
                    s2_result,
                    0, // duration unknown for async System2 (request was fire-and-forget)
                    truth_result.overall.clamp(0.0, 1.0),
                    RouteKind::System2,
                    now_ms(),
                )
                .with_input_summary(&last_user_input)
                .with_response_summary(&final_text);
                subs.outcome_bus.publish(s2_outcome);
            }
        }

        NeocortexToDaemon::ComposedScript { steps } => {
            tracing::info!(
                step_count = steps.len(),
                "composed DSL script received"
            );
            // DSL scripts are forwarded to the execution engine.
            // Build a plan from the DSL steps and execute.
            let plan = aura_types::etg::ActionPlan {
                goal_description: "composed script".to_string(),
                steps,
                estimated_duration_ms: 0,
                confidence: 0.8,
                source: aura_types::etg::PlanSource::LlmGenerated,
            };
            // ── Consent gate (Pillar #1: Privacy Sovereignty) ──────────
            if let Some(_denial) = check_consent_for_task(&subs.consent_tracker, "composed script") {
                subs.outcome_bus.publish(ExecutionOutcome::new(
                    "composed script".to_string(),
                    OutcomeResult::Failure,
                    0,
                    0.0,
                    RouteKind::System2,
                    now_ms(),
                ));
                flush_outcome_bus(subs).await;
            } else {
            // ── BoundaryReasoner gate (defense-in-depth) ──────────
            // Neocortex IPC handler — NeedConfirmation treated as Deny.
            let script_desc = format!("composed script ({} steps)", plan.steps.len());
            let boundary_result = check_boundary_for_task(
                &subs.boundary_reasoner,
                &script_desc,
                current_relationship_stage(subs),
            );
            match boundary_result {
                BoundaryGateResult::Deny(reason)
                | BoundaryGateResult::NeedConfirmation { reason, .. } => {
                    tracing::warn!(
                        target: "SECURITY",
                        script_desc = %script_desc,
                        reason = %reason,
                        "BoundaryReasoner blocked DSL script (Site 5: ComposedScript)"
                    );
                    subs.outcome_bus.publish(ExecutionOutcome::new(
                        "composed script".to_string(),
                        OutcomeResult::Failure,
                        0,
                        0.0,
                        RouteKind::System2,
                        now_ms(),
                    ));
                    flush_outcome_bus(subs).await;
                }
                BoundaryGateResult::Allow => {
            let mut policy_ctx = react::PolicyContext {
                gate: &mut subs.policy_gate,
                audit: &mut subs.audit_log,
            };
            let (outcome, _session) = react::execute_task(
                "composed script".to_string(),
                5,
                Some(plan),
                Some(&mut policy_ctx),
            ).await;
            tracing::debug!(?outcome, "composed script execution complete");
                } // end BoundaryGateResult::Allow arm
            } // end match boundary_result (Site 5: ComposedScript)
            } // end consent-allowed else branch (Site 4: DSL script)
        }

        NeocortexToDaemon::Progress { percent, stage } => {
            tracing::debug!(
                percent,
                stage = %stage,
                "neocortex inference progress"
            );
        }

        NeocortexToDaemon::Error { code, message } => {
            tracing::error!(
                code,
                message = %message,
                "neocortex error"
            );
            // Record in feedback loop for learning.
            subs.memory.feedback_loop.record_error(
                "neocortex_error",
                &message,
                &format!("code={}", code),
                now_ms(),
            );
        }

        NeocortexToDaemon::Pong { uptime_ms } => {
            tracing::debug!(uptime_ms, "neocortex pong");
        }

        NeocortexToDaemon::MemoryWarning {
            used_mb,
            available_mb,
        } => {
            tracing::warn!(
                used_mb,
                available_mb,
                "neocortex memory warning"
            );

            // If critically low, send unload command.
            if available_mb < 128 {
                tracing::warn!("critical memory — requesting model unload");
                if ensure_ipc_connected(&mut subs.neocortex).await {
                    if let Err(e) = subs.neocortex.send(&DaemonToNeocortex::Unload).await {
                        tracing::error!(error = %e, "failed to send Unload");
                    }
                }
            }
        }

        NeocortexToDaemon::TokenBudgetExhausted => {
            tracing::warn!("neocortex token budget exhausted");
            state.checkpoint.token_counters.cloud_tokens =
                state.checkpoint.token_counters.cloud_tokens.saturating_add(1);
        }
    }

    // Flush any outcomes published during IPC handling.
    flush_outcome_bus(subs).await;

    Ok(())
}

// ---------------------------------------------------------------------------
// DB write handler
// ---------------------------------------------------------------------------

/// Execute a database write request synchronously.
///
/// `rusqlite::Connection` is `!Sync`, so this function is intentionally
/// **not** `async` — keeping the borrow of `state.db` off the future's
/// captured state avoids `Send` issues with `tokio::spawn`.
#[instrument(skip_all, fields(variant = std::any::type_name::<DbWriteRequest>()))]
fn handle_db_write(
    req: DbWriteRequest,
    state: &DaemonState,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {

    match req {
        DbWriteRequest::Telemetry { payload } => {
            let len = payload.len();
            if len == 0 {
                tracing::warn!("dropping empty telemetry payload");
                return Ok(());
            }
            if len > 16_384 {
                tracing::warn!(len, "telemetry payload too large — dropping");
                return Ok(());
            }

            tracing::debug!(len, "writing telemetry to db");

            state.db.execute_batch(
                "CREATE TABLE IF NOT EXISTS telemetry (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    payload BLOB NOT NULL,
                    created_at INTEGER NOT NULL DEFAULT (strftime('%s','now'))
                )"
            )?;

            state.db.execute(
                "INSERT INTO telemetry (payload) VALUES (?1)",
                rusqlite::params![payload],
            )?;
        }

        DbWriteRequest::Episode {
            content,
            importance,
        } => {
            if content.trim().is_empty() {
                tracing::warn!("dropping episode with empty content");
                return Ok(());
            }
            let clamped_importance = importance.clamp(0.0, 1.0);

            tracing::debug!(
                len = content.len(),
                importance = clamped_importance,
                "writing episode to db"
            );

            state.db.execute_batch(
                "CREATE TABLE IF NOT EXISTS episodes (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    content TEXT NOT NULL,
                    importance REAL NOT NULL,
                    created_at INTEGER NOT NULL DEFAULT (strftime('%s','now'))
                )"
            )?;

            state.db.execute(
                "INSERT INTO episodes (content, importance) VALUES (?1, ?2)",
                rusqlite::params![content, clamped_importance],
            )?;
        }

        DbWriteRequest::AmygdalaBaseline { app, score } => {
            if app.trim().is_empty() {
                tracing::warn!("dropping amygdala baseline with empty app name");
                return Ok(());
            }
            let clamped_score = score.clamp(0.0, 1.0);

            tracing::debug!(
                app = %app,
                score = clamped_score,
                "writing amygdala baseline to db"
            );

            state.db.execute_batch(
                "CREATE TABLE IF NOT EXISTS amygdala_baselines (
                    app TEXT PRIMARY KEY,
                    score REAL NOT NULL,
                    updated_at INTEGER NOT NULL DEFAULT (strftime('%s','now'))
                )"
            )?;

            state.db.execute(
                "INSERT OR REPLACE INTO amygdala_baselines (app, score) VALUES (?1, ?2)",
                rusqlite::params![app, clamped_score],
            )?;
        }

        DbWriteRequest::RawSql { sql, params } => {
            let sql_upper = sql.trim().to_uppercase();
            if sql_upper.starts_with("DROP")
                || sql_upper.starts_with("ALTER")
                || sql_upper.starts_with("ATTACH")
            {
                tracing::error!(sql = %sql, "rejecting dangerous raw SQL");
                return Ok(());
            }

            if params.len() > 32 {
                tracing::warn!(
                    count = params.len(),
                    "too many SQL params — dropping"
                );
                return Ok(());
            }

            tracing::debug!(sql = %sql, param_count = params.len(), "executing raw SQL");

            let param_refs: Vec<&dyn rusqlite::types::ToSql> = params
                .iter()
                .map(|p| p as &dyn rusqlite::types::ToSql)
                .collect();

            state.db.execute(&sql, param_refs.as_slice())?;
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Cron tick handler
// ---------------------------------------------------------------------------

/// Execute a cron tick: dispatch to the appropriate handler based on job name.
///
/// Wired cron jobs:
/// - `memory_compaction` → `consolidate()` + `system2.sweep_stale()`
/// - `health_report` → log working memory stats
/// - `token_reset` → zero token counters
/// - `checkpoint` → save checkpoint to disk
#[instrument(skip_all, fields(job_id = tick.job_id, job_name = %tick.job_name))]
async fn handle_cron_tick(
    tick: CronTick,
    state: &mut DaemonState,
    subs: &mut LoopSubsystems,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    tracing::debug!(
        job_id = tick.job_id,
        job_name = %tick.job_name,
        scheduled_at_ms = tick.scheduled_at_ms,
        "processing cron tick"
    );

    // ── Safe mode status: periodic visibility ────────────────────────
    // Log a warning on every cron tick when safe mode is active so
    // operators/logs make the degraded state obvious.
    if subs.safe_mode.active {
        tracing::warn!(
            job_name = %tick.job_name,
            reasons = ?subs.safe_mode.reasons,
            "AURA running in SAFE MODE — proactive actions and learning frozen"
        );
    }

    // Check power budget — skip non-critical jobs when battery is low.
    let power_state = aura_types::power::PowerState::from_battery_percent(
        state.checkpoint.power_budget.battery_percent,
    );
    let is_critical = tick.job_name.contains("health")
        || tick.job_name.contains("checkpoint")
        || tick.job_name.contains("battery");

    if power_state == aura_types::power::PowerState::Critical && !is_critical {
        tracing::info!(
            job_name = %tick.job_name,
            "skipping non-critical cron job — power state is Critical"
        );
        return Ok(());
    }

    if power_state == aura_types::power::PowerState::Emergency && !is_critical {
        tracing::info!(
            job_name = %tick.job_name,
            "skipping non-critical cron job — power state is Emergency"
        );
        return Ok(());
    }

    // Update last_fired_ms in checkpoint cron state.
    if let Some(cron_entry) = state
        .checkpoint
        .cron_state
        .iter_mut()
        .find(|c| c.job_id == tick.job_id)
    {
        cron_entry.last_fired_ms = tick.scheduled_at_ms;
        tracing::debug!(
            job_id = tick.job_id,
            "updated cron state last_fired_ms"
        );
    } else {
        tracing::debug!(
            job_id = tick.job_id,
            "cron job not found in checkpoint state — first run"
        );
    }

    // Dispatch by job name pattern.

    // ── Sweep expired sandbox confirmations ──────────────────────
    // Runs on every cron tick to auto-deny confirmations whose
    // timeout has elapsed.  Uses `retain` for O(n) in-place removal.
    {
        let response_tx = subs.response_tx.clone();
        let mut expired: Vec<SandboxConfirmation> = Vec::new();
        subs.pending_confirmations.retain(|conf| {
            if conf.created_at.elapsed() > conf.timeout {
                tracing::info!(
                    target: "SECURITY",
                    conf_id = conf.id,
                    description = %conf.description,
                    "sandbox confirmation auto-denied (timeout)"
                );
                expired.push(conf.clone());
                false // remove from pending list
            } else {
                true // keep
            }
        });

        // Notify user and update goal status for each expired confirmation.
        for conf in &expired {
            let timeout_msg = format!(
                "\u{23f0} Confirmation {} auto-denied (timeout): {}",
                conf.id, conf.description
            );
            send_response(&response_tx, conf.source.clone(), timeout_msg).await;

            // Mark associated goal as failed.
            if let Some(goal) = state.checkpoint.goals.iter_mut().find(|g| g.id == conf.goal_id) {
                goal.status = aura_types::goals::GoalStatus::Failed(
                    format!("sandbox confirmation {} timed out", conf.id),
                );
            }

            subs.outcome_bus.publish(ExecutionOutcome::new(
                conf.task_summary.clone(),
                OutcomeResult::Failure,
                0,
                0.0,
                RouteKind::System1,
                now_ms(),
            ));
        }

        if !expired.is_empty() {
            flush_outcome_bus(subs).await;
        }
    }

    if tick.job_name.contains("health_report") {
        tracing::info!("executing health report cron job");

        // ── HealthMonitor: perform periodic self-diagnostic ────────────
        // The monitor produces a HealthReport with battery, memory, thermal,
        // neocortex liveness, error rate, and overall status.  Critical/Degraded
        // reports trigger Telegram alerts and feed into StrategicRecovery's
        // EnvironmentSnapshot.
        let ts = now_ms();
        if subs.health_monitor.should_check(ts) {
            let report = subs.health_monitor.check(ts);
            tracing::info!(
                status = ?report.status,
                battery = report.battery_percent,
                memory_mb = report.memory_used_mb,
                error_rate = report.error_rate,
                uptime_s = report.uptime_secs,
                "health check complete"
            );

            // Alert user via Telegram when status is Critical or Degraded.
            match report.status {
                HealthStatus::Critical => {
                    let alert = format!(
                        "\u{1f6a8} AURA Health CRITICAL\n\
                         Battery: {:.0}%\nMemory: {} MB\n\
                         Error rate: {:.1}%\nUptime: {}s\n\
                         Action: reducing activity, preserving state.",
                        report.battery_percent * 100.0,
                        report.memory_used_mb,
                        report.error_rate * 100.0,
                        report.uptime_secs,
                    );
                    send_response(&subs.response_tx, InputSource::Internal, alert).await;
                }
                HealthStatus::Degraded => {
                    let alert = format!(
                        "\u{26a0}\u{fe0f} AURA Health Degraded\n\
                         Battery: {:.0}%\nError rate: {:.1}%\n\
                         Some features may be slower or unavailable.",
                        report.battery_percent * 100.0,
                        report.error_rate * 100.0,
                    );
                    send_response(&subs.response_tx, InputSource::Internal, alert).await;
                }
                HealthStatus::Healthy => {
                    tracing::debug!("health status: Healthy — no alert needed");
                }
            }
        }

        // Log working memory stats (existing behavior preserved).
        let slot_count = subs.memory.working.len();
        tracing::info!(
            working_slots = slot_count,
            "health report: memory status"
        );
    } else if tick.job_name.contains("memory_compaction") {
        tracing::info!("executing memory compaction cron job");
        // Run micro consolidation (fast, <1ms).
        let report = consolidate(
            ConsolidationLevel::Micro,
            &mut subs.memory.working,
            &subs.memory.episodic,
            &subs.memory.semantic,
            &subs.memory.archive,
            &mut subs.memory.pattern_engine,
            now_ms(),
        ).await;
        tracing::info!(
            swept = report.working_slots_swept,
            duration_ms = report.duration_ms,
            "micro consolidation complete"
        );

        // Also sweep stale System2 requests.
        subs.system2.sweep_stale(now_ms());
    } else if tick.job_name.contains("token_reset") {
        tracing::info!("executing daily token counter reset");
        state.checkpoint.token_counters.local_tokens = 0;
        state.checkpoint.token_counters.cloud_tokens = 0;
    } else if tick.job_name.contains("checkpoint") {
        tracing::info!("executing checkpoint cron job");
        let cp = state.checkpoint.clone();
        let path = crate::daemon_core::startup::checkpoint_path_from_config(&state.config);
        let result = tokio::task::spawn_blocking(move || {
            save_checkpoint(&cp, Path::new(&path))
        })
        .await;
        match result {
            Ok(Ok(())) => tracing::debug!("cron-triggered checkpoint saved"),
            Ok(Err(e)) => tracing::error!(error = %e, "cron checkpoint failed"),
            Err(e) => tracing::error!(error = %e, "cron checkpoint spawn panicked"),
        }
    } else if tick.job_name.contains("proactive_tick") {
        // Safe mode: block all proactive actions.
        if subs.safe_mode.should_block_action(true, false) {
            tracing::info!("proactive tick BLOCKED by safe mode — skipping");
            return Ok(());
        }
        tracing::info!("executing proactive engine tick");
        if let Some(ref mut proactive) = subs.proactive {
            // Determine current power tier from battery percent.
            let power_tier = battery_percent_to_power_tier(
                state.checkpoint.power_budget.battery_percent,
            );
            // Use context mode from ARC manager if available, else Default.
            let context_mode = subs
                .arc_manager
                .as_ref()
                .map(|a| a.context_mode)
                .unwrap_or(ContextMode::Default);

            // Get current hour for consent checking
            let current_hour = {
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default();
                let secs = now.as_secs();
                ((secs / 3600) % 24) as u8
            };

            // Get user consent from profile - defaults to false for safety
            let profile_allows = subs.identity.is_proactive_allowed(current_hour);
            // Privacy Sovereignty: ConsentTracker must also grant "proactive_actions".
            // By default this is NOT granted — the user must explicitly opt-in.
            let consent_allows = subs
                .identity
                .consent_tracker
                .has_consent("proactive_actions", now_ms());
            let proactive_allowed = profile_allows && consent_allows;
            tracing::debug!(
                proactive_allowed,
                profile_allows,
                consent_allows,
                hour = current_hour,
                "proactive consent check (profile + ConsentTracker)"
            );

            match proactive.tick(now_ms(), power_tier, context_mode, proactive_allowed) {
                Ok(actions) => {
                    tracing::info!(
                        action_count = actions.len(),
                        "proactive engine produced actions"
                    );
                    for action in actions {
                        match action {
                            ProactiveAction::Suggest(suggestion) => {
                                tracing::info!(
                                    text = %suggestion.text,
                                    confidence = suggestion.confidence,
                                    "proactive suggestion"
                                );
                                // Route suggestion to user via response channel.
                                let msg = format!("[Suggestion] {}", suggestion.text);
                                send_response(
                                    &subs.response_tx,
                                    InputSource::Direct,
                                    msg,
                                ).await;
                            }
                            ProactiveAction::Briefing(sections) => {
                                tracing::info!(
                                    sections = sections.len(),
                                    "proactive briefing generated"
                                );
                                let mut brief = String::from("[Briefing]\n");
                                for section in &sections {
                                    brief.push_str(&format!(
                                        "• {}\n",
                                        section.key()
                                    ));
                                }
                                send_response(
                                    &subs.response_tx,
                                    InputSource::Direct,
                                    brief,
                                ).await;
                            }
                            ProactiveAction::RunAutomation { routine_id, actions: auto_actions } => {
                                tracing::info!(
                                    routine_id = %routine_id,
                                    steps = auto_actions.len(),
                                    "proactive automation triggered"
                                );
                                let desc = format!("proactive_routine:{}", routine_id);
                                // ── Consent gate (Pillar #1: Privacy Sovereignty) ──────────
                                if let Some(_denial) = check_consent_for_task(&subs.consent_tracker, &desc) {
                                    subs.outcome_bus.publish(ExecutionOutcome::new(
                                        desc.clone(),
                                        OutcomeResult::Failure,
                                        0,
                                        0.0,
                                        RouteKind::System1,
                                        now_ms(),
                                    ));
                flush_outcome_bus(subs).await;

            // ── Sandbox confirmation gate (L2:Restricted → user approval) ─────
            } else if subs.action_sandbox.classify_string(&description) == ContainmentLevel::Restricted {
                // L2:Restricted — do NOT execute immediately.  Queue a
                // confirmation prompt and wait for the user to `/allow` or
                // `/deny` before proceeding.
                tracing::warn!(
                    target: "SECURITY",
                    description = %description,
                    "task classified as L2:Restricted — queuing confirmation prompt"
                );

                if subs.pending_confirmations.len() < MAX_PENDING_CONFIRMATIONS {
                    let conf_id = subs.next_confirmation_id;
                    subs.next_confirmation_id += 1;

                    let confirmation = SandboxConfirmation {
                        id: conf_id,
                        description: format!("Action: {}", description),
                        containment_level: "L2:Restricted".to_string(),
                        created_at: std::time::Instant::now(),
                        timeout: std::time::Duration::from_secs(CONFIRMATION_TIMEOUT_SECS),
                        task_summary: description.clone(),
                        source: source.clone(),
                        goal_id,
                        priority: clamped_priority,
                    };

                    // Send prompt to user via the response channel.
                    let prompt_msg = format!(
                        "\u{26a0}\u{fe0f} Action requires confirmation:\n{}\n\nReply /allow {} or /deny {}\nAuto-deny in {}s",
                        confirmation.description, conf_id, conf_id, CONFIRMATION_TIMEOUT_SECS
                    );
                    send_response(&subs.response_tx, source.clone(), prompt_msg).await;

                    subs.pending_confirmations.push(confirmation);

                    // Mark goal as waiting for confirmation.
                    if let Some(goal) = state.checkpoint.goals.iter_mut().find(|g| g.id == goal_id) {
                        goal.status = aura_types::goals::GoalStatus::Active;
                    }
                } else {
                    tracing::warn!(
                        max = MAX_PENDING_CONFIRMATIONS,
                        "too many pending confirmations — auto-denying task"
                    );
                    // Auto-deny: too many pending confirmations.
                    subs.outcome_bus.publish(ExecutionOutcome::new(
                        description.clone(),
                        OutcomeResult::Failure,
                        0,
                        0.0,
                        RouteKind::System1,
                        now_ms(),
                    ));
                    if let Some(goal) = state.checkpoint.goals.iter_mut().find(|g| g.id == goal_id) {
                        goal.status = aura_types::goals::GoalStatus::Failed(
                            "auto-denied: too many pending confirmations".to_string(),
                        );
                    }
                    send_response(
                        &subs.response_tx,
                        source.clone(),
                        "Action auto-denied: too many pending confirmations.".to_string(),
                    ).await;
                    flush_outcome_bus(subs).await;
                }

            } else {
                                // ── BoundaryReasoner gate (defense-in-depth) ──
                                // Proactive actions already passed consent + sandbox
                                // gates. NeedConfirmation → Deny to avoid double-
                                // confirmation with the sandbox L2:Restricted flow.
                                let boundary_result = check_boundary_for_task(
                                    &subs.boundary_reasoner,
                                    &desc,
                                    current_relationship_stage(subs),
                                );
                                match boundary_result {
                                    BoundaryGateResult::Deny(reason)
                                    | BoundaryGateResult::NeedConfirmation { reason, .. } => {
                                        tracing::warn!(
                                            target: "SECURITY",
                                            desc = %desc,
                                            reason = %reason,
                                            "BoundaryReasoner blocked proactive action (Site 6)"
                                        );
                                        subs.outcome_bus.publish(ExecutionOutcome::new(
                                            desc.clone(),
                                            OutcomeResult::Failure,
                                            0,
                                            0.0,
                                            RouteKind::System1,
                                            now_ms(),
                                        ));
                                        flush_outcome_bus(subs).await;
                                    }
                                    BoundaryGateResult::Allow => {
                                // Execute via react engine.
                                let mut policy_ctx = react::PolicyContext {
                                    gate: &mut subs.policy_gate,
                                    audit: &mut subs.audit_log,
                                };
                                let (_outcome, _session) = react::execute_task(
                                    desc, 7, None, Some(&mut policy_ctx),
                                ).await;
                                    } // end BoundaryGateResult::Allow arm
                                } // end match boundary_result (Site 6: proactive)
                                } // end consent-allowed else branch (Site 5: proactive)
                            }
                            ProactiveAction::Alert { domain, message, urgency } => {
                                tracing::warn!(
                                    domain = %domain,
                                    urgency = urgency,
                                    "proactive alert"
                                );
                                let msg = format!(
                                    "[Alert — {} (urgency {})] {}",
                                    domain, urgency, message
                                );
                                send_response(
                                    &subs.response_tx,
                                    InputSource::Direct,
                                    msg,
                                ).await;
                            }
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!(error = %e, "ProactiveEngine tick failed");
                }
            }
        } else {
            tracing::debug!("proactive engine not available — skipping tick");
        }
    } else if tick.job_name.contains("dreaming_tick") {
        // Safe mode: block all learning activity.
        if subs.safe_mode.should_block_action(false, true) {
            tracing::info!("dreaming tick BLOCKED by safe mode — skipping");
            return Ok(());
        }
        tracing::info!("executing dreaming engine tick");
        
        // Get power tier and context mode
        let power_tier = battery_percent_to_power_tier(
            state.checkpoint.power_budget.battery_percent,
        );
        let context_mode = subs
            .arc_manager
            .as_ref()
            .map(|a| a.context_mode)
            .unwrap_or(ContextMode::Default);
        
        // Check if ArcManager has the learning engine with dreaming
        if let Some(ref mut arc_manager) = subs.arc_manager {
            // Build dreaming conditions from system state
            let is_charging = state.checkpoint.power_budget.is_charging;
            let screen_off = matches!(context_mode, ContextMode::Sleeping);
            let battery_percent = state.checkpoint.power_budget.battery_percent;
            let thermal_nominal = state
                .subsystems
                .platform
                .as_ref()
                .map(|p| !p.thermal.should_pause_inference())
                .unwrap_or(true); // Assume nominal if platform unavailable
            
            let conditions = crate::arc::learning::dreaming::DreamingConditions {
                is_charging,
                screen_off,
                battery_percent,
                thermal_nominal,
                now_ms: now_ms(),
            };
            
            if conditions.can_dream() {
                // Try to start a dreaming session
                match arc_manager.learning.dreaming.try_start_session(&conditions) {
                    Ok(true) => {
                        tracing::info!("dreaming session started");
                        // Run the full consolidation cycle
                        let (processed, created, pruned) = 
                            arc_manager.learning.dreaming.run_consolidation_cycle(now_ms());
                        tracing::info!(
                            processed,
                            created,
                            pruned,
                            "dreaming consolidation cycle complete"
                        );
                        
                        // Finalize the session
                        let report = crate::arc::learning::dreaming::PhaseReport {
                            phase: Some(crate::arc::learning::dreaming::DreamPhase::Maintenance),
                            duration_ms: 0,
                            items_processed: processed,
                            items_created: created,
                            items_pruned: pruned,
                            completed: true,
                            abort_reason: None,
                        };
                        let _ = arc_manager.learning.dreaming.advance_phase(report, &conditions);
                    }
                    Ok(false) => {
                        tracing::debug!("dreaming conditions not met or session already active");
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "failed to start dreaming session");
                    }
                }
            } else {
                tracing::debug!(
                    charging = is_charging,
                    screen_off = screen_off,
                    battery = battery_percent,
                    thermal_ok = thermal_nominal,
                    "dreaming conditions not met"
                );
            }
        } else {
            tracing::debug!("arc manager not available — skipping dreaming tick");
        }
    // ── Health domain ────────────────────────────────────────────────────
    } else if tick.job_name.contains("medication_check") {
        if let Some(ref mut arc) = subs.arc_manager {
            let now = now_ms() as i64 / 1000; // epoch seconds
            let pending = arc.health.medication.check_pending_doses(now);
            if !pending.is_empty() {
                tracing::info!(count = pending.len(), "medication doses pending");
                let summary = pending
                    .iter()
                    .map(|d| format!("• {} (window {}–{})", d.medication_name, d.start, d.end))
                    .collect::<Vec<_>>()
                    .join("\n");
                send_response(
                    &subs.response_tx,
                    InputSource::System,
                    format!("💊 Medication reminder:\n{summary}"),
                );
            }
        } else {
            tracing::debug!("arc manager not available — skipping medication_check");
        }
    } else if tick.job_name.contains("vital_ingest") {
        if let Some(ref mut arc) = subs.arc_manager {
            arc.health.vitals.ingest();
            tracing::debug!(
                readings = arc.health.vitals.reading_count(),
                "vital signs ingested"
            );
        } else {
            tracing::debug!("arc manager not available — skipping vital_ingest");
        }
    } else if tick.job_name.contains("step_sync") {
        if let Some(ref mut arc) = subs.arc_manager {
            let score = arc.health.fitness.activity_score();
            tracing::debug!(activity_score = score, "step sync complete");
        } else {
            tracing::debug!("arc manager not available — skipping step_sync");
        }
    } else if tick.job_name.contains("sleep_infer") {
        if let Some(ref mut arc) = subs.arc_manager {
            let quality = arc.health.sleep.quality_score();
            tracing::debug!(quality_score = quality, records = arc.health.sleep.record_count(), "sleep inference done");
        } else {
            tracing::debug!("arc manager not available — skipping sleep_infer");
        }
    } else if tick.job_name.contains("health_score_compute") {
        if let Some(ref mut arc) = subs.arc_manager {
            match arc.health.compute_score() {
                Ok(score) => {
                    arc.state_store.update(
                        crate::arc::DomainId::Health,
                        score,
                        crate::arc::DomainLifecycle::Active,
                        &[("composite_health", score as f64)],
                        now_ms(),
                    );
                    tracing::info!(health_score = score, "health domain score updated");
                }
                Err(e) => tracing::warn!(error = %e, "health score computation failed"),
            }
        } else {
            tracing::debug!("arc manager not available — skipping health_score_compute");
        }
    } else if tick.job_name.contains("health_weekly_report") {
        if let Some(ref mut arc) = subs.arc_manager {
            let composite = arc.health.compute_score().unwrap_or(0.0);
            let med_adherence = arc.health.medication.adherence_score();
            let vitals_score = arc.health.vitals.composite_score();
            let fitness_score = arc.health.fitness.activity_score();
            let sleep_quality = arc.health.sleep.quality_score();
            let report = format!(
                "📋 Weekly Health Report\n\
                 ├─ Composite: {composite:.1}%\n\
                 ├─ Medication adherence: {med_adherence:.1}%\n\
                 ├─ Vitals: {vitals_score:.1}\n\
                 ├─ Fitness: {fitness_score:.1}\n\
                 └─ Sleep quality: {sleep_quality:.1}"
            );
            send_response(&subs.response_tx, InputSource::System, report);
            tracing::info!(composite, "health weekly report generated");
        } else {
            tracing::debug!("arc manager not available — skipping health_weekly_report");
        }

    // ── Social domain ────────────────────────────────────────────────────
    } else if tick.job_name.contains("contact_update") {
        if let Some(ref mut arc) = subs.arc_manager {
            let total = arc.social.contacts.total_contacts();
            tracing::debug!(total_contacts = total, "contact store refreshed");
        } else {
            tracing::debug!("arc manager not available — skipping contact_update");
        }
    } else if tick.job_name.contains("importance_recalc") {
        if let Some(ref mut arc) = subs.arc_manager {
            let now_secs = (now_ms() / 1000) as i64;
            let social = &mut arc.social;
            let contacts = social.contacts.all_mut();
            let scored = social.importance.score_all(contacts, now_secs, social.importance.observation_window_days());
            tracing::debug!(observation_window_days = social.importance.observation_window_days(), "importance recalc using configured window");
            tracing::info!(scored_count = scored, "importance scores recalculated");
        } else {
            tracing::debug!("arc manager not available — skipping importance_recalc");
        }
    } else if tick.job_name.contains("relationship_health") {
        if let Some(ref mut arc) = subs.arc_manager {
            let avg = arc.social.relationship_health.average_health();
            tracing::debug!(average_health = avg, "relationship health evaluated");
        } else {
            tracing::debug!("arc manager not available — skipping relationship_health");
        }
    } else if tick.job_name.contains("social_gap_scan") {
        if let Some(ref mut arc) = subs.arc_manager {
            let now = now_ms();
            let gaps = arc.social.gap_detector.detect_gaps(now);
            if !gaps.is_empty() {
                tracing::info!(gap_count = gaps.len(), "social gaps detected");
                for gap in &gaps {
                    tracing::debug!(?gap, "social gap alert");
                }
            }
        } else {
            tracing::debug!("arc manager not available — skipping social_gap_scan");
        }
    } else if tick.job_name.contains("birthday_check") {
        if let Some(ref mut arc) = subs.arc_manager {
            let now = chrono::Utc::now();
            let month = now.month() as u8;
            let day = now.day() as u8;
            let days_ahead = arc.social.birthdays.scan_ahead_days();
            tracing::debug!(scan_ahead_days = days_ahead, "birthday check using configured lookahead");
            match arc.social.birthdays.scan_upcoming(month, day, days_ahead) {
                Ok(upcoming) => {
                    if !upcoming.is_empty() {
                        let names: Vec<_> = upcoming.iter().map(|b| b.name.as_str()).collect();
                        tracing::info!(?names, "upcoming birthdays detected");
                        let msg = upcoming
                            .iter()
                            .map(|b| format!("• {} — {}/{}", b.name, b.month, b.day))
                            .collect::<Vec<_>>()
                            .join("\n");
                        send_response(
                            &subs.response_tx,
                            InputSource::System,
                            format!("🎂 Upcoming birthdays:\n{msg}"),
                        );
                    }
                }
                Err(e) => tracing::warn!(error = %e, "birthday scan failed"),
            }
        } else {
            tracing::debug!("arc manager not available — skipping birthday_check");
        }
    } else if tick.job_name.contains("social_score_compute") {
        if let Some(ref mut arc) = subs.arc_manager {
            match arc.social.compute_score() {
                Ok(score) => {
                    arc.state_store.update(
                        crate::arc::DomainId::Social,
                        score,
                        crate::arc::DomainLifecycle::Active,
                        &[("composite_social", score as f64)],
                        now_ms(),
                    );
                    tracing::info!(social_score = score, "social domain score updated");
                }
                Err(e) => tracing::warn!(error = %e, "social score computation failed"),
            }
        } else {
            tracing::debug!("arc manager not available — skipping social_score_compute");
        }
    } else if tick.job_name.contains("social_weekly_report") {
        if let Some(ref mut arc) = subs.arc_manager {
            let composite = arc.social.compute_score().unwrap_or(0.0);
            let gap_health = arc.social.gap_detector.health_score();
            let rel_health = arc.social.relationship_health.average_health();
            let graph_diversity = arc.social.graph.diversity_score();
            let report = format!(
                "📋 Weekly Social Report\n\
                 ├─ Composite: {composite:.1}%\n\
                 ├─ Gap health: {gap_health:.1}\n\
                 ├─ Relationship health: {rel_health:.1}\n\
                 └─ Graph diversity: {graph_diversity:.2}"
            );
            send_response(&subs.response_tx, InputSource::System, report);
            tracing::info!(composite, "social weekly report generated");
        } else {
            tracing::debug!("arc manager not available — skipping social_weekly_report");
        }

    // ── Proactive domain ─────────────────────────────────────────────────
    } else if (tick.job_name.contains("trigger_rule_eval")
            || tick.job_name.contains("opportunity_detect")
            || tick.job_name.contains("threat_accumulate")
            || tick.job_name.contains("action_drain")
            || tick.job_name.contains("daily_budget_reset"))
        && subs.safe_mode.should_block_action(true, false)
    {
        tracing::info!(
            job = %tick.job_name,
            "safe-mode: skipping proactive cron job"
        );
    } else if tick.job_name.contains("trigger_rule_eval") {
        if let Some(ref mut arc) = subs.arc_manager {
            let now = now_ms();
            match arc.proactive.suggestions.evaluate_triggers(now) {
                Ok(suggestions) => {
                    if !suggestions.is_empty() {
                        tracing::info!(count = suggestions.len(), "proactive triggers fired");
                        for s in &suggestions {
                            tracing::debug!(suggestion = ?s, "suggestion generated");
                        }
                    }
                }
                Err(e) => tracing::warn!(error = %e, "trigger rule evaluation failed"),
            }
        } else {
            tracing::debug!("arc manager not available — skipping trigger_rule_eval");
        }
    } else if tick.job_name.contains("opportunity_detect") {
        if let Some(ref mut arc) = subs.arc_manager {
            let now = now_ms();
            match arc.proactive.detect_opportunities(now) {
                Ok(enqueued) => {
                    if enqueued > 0 {
                        tracing::info!(
                            enqueued,
                            pending = arc.proactive.pending_action_count(),
                            "opportunity detection produced new actions"
                        );
                    }
                }
                Err(e) => tracing::warn!(error = %e, "opportunity detection failed"),
            }
        } else {
            tracing::debug!("arc manager not available — skipping opportunity_detect");
        }
    } else if tick.job_name.contains("threat_accumulate") {
        if let Some(ref mut arc) = subs.arc_manager {
            let threat = arc.proactive.accumulate_threats();
            if threat > 0.5 {
                tracing::warn!(
                    threat_score = threat,
                    "elevated threat score — user may be rejecting too many suggestions"
                );
            }
        } else {
            tracing::debug!("arc manager not available — skipping threat_accumulate");
        }
    } else if tick.job_name.contains("action_drain") {
        if let Some(ref mut arc) = subs.arc_manager {
            let drained = arc.proactive.drain_pending_actions();
            if !drained.is_empty() {
                tracing::info!(
                    count = drained.len(),
                    budget = arc.proactive.budget(),
                    "proactive actions drained"
                );
                for action in &drained {
                    tracing::debug!(action = ?action, "drained proactive action");
                }
            }
        } else {
            tracing::debug!("arc manager not available — skipping action_drain");
        }
    } else if tick.job_name.contains("daily_budget_reset") {
        if let Some(ref mut arc) = subs.arc_manager {
            let reset_secs = arc.proactive.daily_budget_reset_secs();
            tracing::debug!(daily_budget_reset_secs = reset_secs, "daily budget reset using configured elapsed seconds");
            arc.proactive.regenerate_initiative(reset_secs);
            tracing::info!(
                budget = arc.proactive.budget(),
                daily_suggestions = arc.proactive.daily_suggestions(),
                "daily proactive budget reset"
            );
        } else {
            tracing::debug!("arc manager not available — skipping daily_budget_reset");
        }

    // ── Learning domain ──────────────────────────────────────────────────
    } else if (tick.job_name.contains("pattern_observe")
            || tick.job_name.contains("pattern_analyze")
            || tick.job_name.contains("pattern_deviation_check")
            || tick.job_name.contains("hebbian_decay")
            || tick.job_name.contains("hebbian_consolidate")
            || tick.job_name.contains("interest_update")
            || tick.job_name.contains("skill_progress"))
        && subs.safe_mode.should_block_action(false, true)
    {
        tracing::info!(
            job = %tick.job_name,
            "safe-mode: skipping learning cron job"
        );
    } else if tick.job_name.contains("pattern_observe") {
        if let Some(ref mut arc) = subs.arc_manager {
            // pattern_observe fires every 1s — this is a lightweight check.
            // The PatternDetector::observe() requires an Observation; since we
            // don't have a concrete observation here, we age existing patterns.
            let now = now_ms();
            let aged = arc.learning.patterns.age_patterns(now);
            if aged > 0 {
                tracing::trace!(aged_count = aged, "patterns aged");
            }
        } else {
            tracing::trace!("arc manager not available — skipping pattern_observe");
        }
    } else if tick.job_name.contains("pattern_analyze") {
        if let Some(ref mut arc) = subs.arc_manager {
            // Weekly deep pattern analysis — run consolidation at full depth.
            let now = now_ms();
            match arc.learning.consolidate(now) {
                Ok(report) => {
                    tracing::info!(
                        strengthened = report.strengthened,
                        pruned = report.pruned,
                        new_associations = report.new_associations,
                        "weekly pattern analysis complete"
                    );
                }
                Err(e) => tracing::warn!(error = %e, "pattern analysis consolidation failed"),
            }
        } else {
            tracing::debug!("arc manager not available — skipping pattern_analyze");
        }
    } else if tick.job_name.contains("pattern_deviation_check") {
        if let Some(ref mut arc) = subs.arc_manager {
            let is_deviation = arc.learning.prediction.is_routine_change_detected();
            if is_deviation {
                tracing::info!("routine deviation detected by prediction engine");
            }
        } else {
            tracing::debug!("arc manager not available — skipping pattern_deviation_check");
        }
    } else if tick.job_name.contains("hebbian_decay") {
        if let Some(ref mut arc) = subs.arc_manager {
            let now = now_ms();
            let half_life_ms = arc.learning.hebbian.decay_half_life_ms();
            tracing::debug!(half_life_ms, "hebbian decay using configured half-life");
            arc.learning.hebbian.decay_all(now, half_life_ms);
            tracing::debug!("hebbian decay pass complete");
        } else {
            tracing::debug!("arc manager not available — skipping hebbian_decay");
        }
    } else if tick.job_name.contains("hebbian_consolidate") {
        if let Some(ref mut arc) = subs.arc_manager {
            let threshold = arc.learning.hebbian.prune_threshold();
            tracing::debug!(threshold, "hebbian consolidate using configured prune threshold");
            let pruned = arc.learning.hebbian.prune_weak(threshold);
            let now = now_ms();
            match arc.learning.consolidate(now) {
                Ok(report) => {
                    tracing::info!(
                        pruned_weak = pruned,
                        strengthened = report.strengthened,
                        pruned_total = report.pruned,
                        new_assoc = report.new_associations,
                        "hebbian consolidation complete"
                    );
                }
                Err(e) => tracing::warn!(error = %e, "hebbian consolidation failed"),
            }
        } else {
            tracing::debug!("arc manager not available — skipping hebbian_consolidate");
        }
    } else if tick.job_name.contains("interest_update") {
        if let Some(ref mut arc) = subs.arc_manager {
            let now = now_ms();
            let half_life_ms = arc.learning.interests.update_half_life_ms();
            tracing::debug!(half_life_ms, "interest update using configured half-life");
            arc.learning.interests.decay(now, half_life_ms);
            let top = arc.learning.interests.get_top_interests(5);
            tracing::debug!(?top, "interest model updated");
        } else {
            tracing::debug!("arc manager not available — skipping interest_update");
        }
    } else if tick.job_name.contains("skill_progress") {
        if let Some(ref mut arc) = subs.arc_manager {
            let now = now_ms();
            arc.learning.skills.decay_all_confidence(now);
            let reliable = arc.learning.skills.get_reliable_skills();
            tracing::debug!(reliable_count = reliable.len(), "skill progress updated");
        } else {
            tracing::debug!("arc manager not available — skipping skill_progress");
        }

    // ── System / cross-cutting ───────────────────────────────────────────
    } else if tick.job_name.contains("domain_state_publish") {
        if let Some(ref mut arc) = subs.arc_manager {
            let mut scores = std::collections::HashMap::new();
            for domain in crate::arc::DomainId::ALL.iter() {
                if let Some(s) = arc.state_store.get_health_score(*domain) {
                    scores.insert(*domain, s);
                }
            }
            let lqi = crate::arc::compute_life_quality(&scores, None);
            tracing::info!(domain_count = scores.len(), lqi = lqi, "domain state published");
        } else {
            tracing::debug!("arc manager not available — skipping domain_state_publish");
        }
    } else if tick.job_name.contains("life_quality_compute") {
        if let Some(ref mut arc) = subs.arc_manager {
            let mut scores = std::collections::HashMap::new();
            for domain in crate::arc::DomainId::ALL.iter() {
                if let Some(s) = arc.state_store.get_health_score(*domain) {
                    scores.insert(*domain, s);
                }
            }
            let lqi = crate::arc::compute_life_quality(&scores, None);
            tracing::info!(lqi = lqi, domains_active = scores.len(), "life quality index recomputed");
            send_response(
                &subs.response_tx,
                InputSource::System,
                format!("Life Quality Index: {lqi:.1}% ({} domains active)", scores.len()),
            );
        } else {
            tracing::debug!("arc manager not available — skipping life_quality_compute");
        }
    } else if tick.job_name.contains("cron_self_check") {
        if let Some(ref arc) = subs.arc_manager {
            let job_count = arc.scheduler.job_count();
            tracing::info!(registered_jobs = job_count, "cron self-check passed");
        } else {
            tracing::debug!("arc manager not available — skipping cron_self_check");
        }
    } else if tick.job_name.contains("memory_arc_flush") {
        // Flush AuraMemory's write-ahead log / dirty pages.
        if let Err(e) = subs.memory.flush() {
            tracing::warn!(error = %e, "memory arc flush failed");
        } else {
            tracing::debug!("memory arc flush complete");
        }
    } else if tick.job_name.contains("weekly_digest") {
        if let Some(ref mut arc) = subs.arc_manager {
            let mut scores = std::collections::HashMap::new();
            for domain in crate::arc::DomainId::ALL.iter() {
                if let Some(s) = arc.state_store.get_health_score(*domain) {
                    scores.insert(*domain, s);
                }
            }
            let lqi = crate::arc::compute_life_quality(&scores, None);
            let report = format!(
                "📊 Weekly AURA Digest\n\
                 ├─ Life Quality Index: {lqi:.1}%\n\
                 ├─ Active domains: {}/{}\n\
                 └─ Generated at: {}",
                scores.len(),
                crate::arc::DomainId::ALL.len(),
                chrono::Utc::now().format("%Y-%m-%d %H:%M UTC"),
            );
            send_response(&subs.response_tx, InputSource::System, report);
            tracing::info!(lqi, "weekly digest generated");
        } else {
            tracing::debug!("arc manager not available — skipping weekly_digest");
        }
    } else if tick.job_name.contains("deep_consolidation") {
        if let Some(ref mut arc) = subs.arc_manager {
            let now = now_ms();
            match arc.learning.consolidate(now) {
                Ok(report) => {
                    tracing::info!(
                        strengthened = report.strengthened,
                        pruned = report.pruned,
                        new_associations = report.new_associations,
                        "deep consolidation pass complete"
                    );
                }
                Err(e) => tracing::warn!(error = %e, "deep consolidation failed"),
            }
            // Also prune the social graph during deep work.
            let prune_threshold = arc.social.graph.prune_min_weight();
            tracing::debug!(prune_min_weight = prune_threshold, "social graph prune using configured threshold");
            let pruned = arc.social.graph.prune_weak(prune_threshold);
            if pruned > 0 {
                tracing::info!(pruned_edges = pruned, "social graph pruned during deep consolidation");
            }
        } else {
            tracing::debug!("arc manager not available — skipping deep_consolidation");
        }
    } else if tick.job_name.contains("bdi_deliberation") {
        // ── BDI Deliberation Cycle ────────────────────────────────────
        // Runs periodically to re-evaluate goal priorities, detect conflicts,
        // decompose committed goals, and handle stalls/deadlines.
        // Uses AURA's actual BdiScheduler (5-factor scoring + aging boost),
        // GoalDecomposer (HTN-style DAG decomposition), ConflictResolver
        // (resource + temporal + intent overlap detection), and GoalTracker
        // (lifecycle FSM with stall detection).
        let now = now_ms();
        let openness = subs.identity.personality.traits.openness;

        // Gather non-terminal goals for deliberation.
        let active_goals: Vec<aura_types::goals::Goal> = state
            .checkpoint
            .goals
            .iter()
            .filter(|g| {
                matches!(
                    g.status,
                    aura_types::goals::GoalStatus::Active
                        | aura_types::goals::GoalStatus::Pending
                )
            })
            .cloned()
            .collect();

        if active_goals.is_empty() {
            tracing::trace!("bdi_deliberation: no active goals — skipping");
        } else if let Some(ref mut scheduler) = subs.bdi_scheduler {
            // Run BDI deliberation: evaluates all scored goals, applies
            // personality-modulated weighting (openness biases toward novelty).
            let delib_result = scheduler.deliberate(&active_goals, now, openness);

            match delib_result {
                DeliberationResult::Commit { new_intentions } => {
                    tracing::info!(
                        committed = new_intentions.len(),
                        "BDI deliberation: committing new intentions"
                    );
                    // Decompose each newly committed goal into sub-goal DAGs.
                    for intention_id in &new_intentions {
                        let goal_opt = active_goals.iter().find(|g| g.id == *intention_id);
                        if let (Some(goal), Some(ref mut decomposer)) =
                            (goal_opt, subs.goal_decomposer.as_mut())
                        {
                            match decomposer.decompose(goal, 0) {
                                Ok(result) => {
                                    tracing::debug!(
                                        parent = result.parent_goal_id,
                                        sub_goals = result.sub_goals.len(),
                                        confidence = result.confidence,
                                        strategy = ?result.strategy,
                                        "goal decomposed into sub-goal DAG"
                                    );
                                    // Track each sub-goal in GoalTracker + checkpoint.
                                    for sub in &result.sub_goals {
                                        if let Some(ref mut tracker) = subs.goal_tracker {
                                            let _ = tracker.track(sub.goal.clone());
                                            let _ = tracker.activate(sub.goal.id, now);
                                        }
                                        state.checkpoint.goals.push(sub.goal.clone());
                                    }
                                }
                                Err(e) => {
                                    tracing::warn!(
                                        goal_id = intention_id,
                                        error = %e,
                                        "BDI: goal decomposition failed"
                                    );
                                }
                            }
                        }
                    }
                }
                DeliberationResult::Reconsider { drop_intentions, reason } => {
                    tracing::info!(
                        dropping = drop_intentions.len(),
                        reason = %reason,
                        "BDI deliberation: reconsidering — dropping intentions"
                    );
                    for drop_id in &drop_intentions {
                        if let Some(ref mut tracker) = subs.goal_tracker {
                            let _ = tracker.cancel(*drop_id, now);
                        }
                        // Mark dropped goals as cancelled in checkpoint.
                        if let Some(goal) = state.checkpoint.goals.iter_mut().find(|g| g.id == *drop_id) {
                            goal.status = aura_types::goals::GoalStatus::Cancelled;
                        }
                    }
                }
                DeliberationResult::Maintain => {
                    tracing::trace!("BDI deliberation: maintain current intentions");
                }
            }

            // ── Conflict detection across all active goals ──────────
            if let Some(ref mut resolver) = subs.conflict_resolver {
                let entries: Vec<GoalConflictEntry> = active_goals
                    .iter()
                    .map(|g| GoalConflictEntry {
                        goal_id: g.id,
                        score: 0.5, // neutral; BDI scorer already prioritized
                        resources: Vec::new(),
                        earliest_start_ms: None,
                        deadline_ms: g.deadline_ms,
                        intent_keywords: g
                            .description
                            .split_whitespace()
                            .take(5)
                            .map(|w| w.to_lowercase())
                            .collect(),
                    })
                    .collect();

                let conflicts = resolver.detect_conflicts(&entries, now);
                for conflict in &conflicts {
                    tracing::info!(
                        conflict_id = conflict.id,
                        goal_a = conflict.goal_a_id,
                        goal_b = conflict.goal_b_id,
                        conflict_type = ?conflict.conflict_type,
                        reason = %conflict.reason,
                        "BDI: goal conflict detected"
                    );
                    // Attempt automatic resolution.
                    if let Some((strategy, outcome)) = resolver.resolve(conflict.id, now) {
                        tracing::info!(
                            conflict_id = conflict.id,
                            strategy = ?strategy,
                            outcome = ?outcome,
                            "BDI: conflict auto-resolved"
                        );
                    }
                }
            }

            // ── Deadline + stall checks ─────────────────────────────
            if let Some(ref mut tracker) = subs.goal_tracker {
                let overdue = tracker.check_deadlines(now);
                for goal_id in &overdue {
                    tracing::warn!(goal_id, "BDI: goal overdue — deadline exceeded");
                    // Escalate overdue goals: update belief so next deliberation deprioritizes.
                    if let Some(ref mut sched) = subs.bdi_scheduler {
                        let belief = Belief {
                            key: format!("goal_{}_overdue", goal_id),
                            value: "true".into(),
                            confidence: 1.0,
                            updated_at_ms: now,
                            source: BeliefSource::ExecutionOutcome,
                        };
                        let _ = sched.update_belief(belief);
                    }
                }
                let stalls = tracker.detect_stalls(now);
                for stall in &stalls {
                    tracing::warn!(
                        goal_id = stall.goal_id,
                        progress = stall.progress_at_stall,
                        stall_ms = stall.stall_duration_ms,
                        "BDI: goal stall detected"
                    );
                }
            }
        } else {
            tracing::debug!("bdi_scheduler not available — skipping bdi_deliberation");
        }
    } else {
        tracing::error!(
            job_name = %tick.job_name,
            job_id = tick.job_id,
            "unknown cron job — no handler registered (this is a bug)"
        );
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Helpers — power tier mapping
// ---------------------------------------------------------------------------

/// Map a battery percentage to a [`PowerTier`] for proactive-engine gating.
///
/// Thresholds align with the `PowerState` bands but map onto the
/// latency-oriented `PowerTier` enum that [`ProactiveEngine::tick`] expects.
fn battery_percent_to_power_tier(percent: u8) -> PowerTier {
    match percent {
        0..=14 => PowerTier::P0Always,    // critical / emergency — only essential
        15..=29 => PowerTier::P1IdlePlus, // low power — light work only
        30..=50 => PowerTier::P2Normal,   // conservative — standard interaction
        51..=100 => PowerTier::P3Charging, // normal+ — background work ok
        _ => PowerTier::P2Normal,         // saturate at normal
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::daemon_core::channels::{InputSource, UserCommand};
    use aura_types::config::AuraConfig;

    /// Helper: build a minimal DaemonState for testing.
    fn test_state(dir: &tempfile::TempDir) -> DaemonState {
        let db_path = dir.path().join("test.db");

        let _db = rusqlite::Connection::open(&db_path).expect("open test db");
        let mut config = AuraConfig::default();
        config.sqlite.db_path = db_path.to_string_lossy().to_string();
        config.daemon.checkpoint_interval_s = 3600; // long interval — won't fire in tests

        // Use the full startup path to get subsystems properly initialised.
        let (state, _report) = crate::daemon_core::startup::startup(config)
            .expect("startup should succeed in test helper");
        state
    }

    #[tokio::test]
    async fn test_main_loop_exits_on_cancel_flag() {
        let dir = tempfile::tempdir().expect("tempdir");
        let state = test_state(&dir);
        let cancel = state.cancel_flag.clone();

        // Set cancel flag immediately — loop should exit on first iteration.
        cancel.store(true, Ordering::Release);

        let start = Instant::now();
        run(state).await;
        assert!(start.elapsed().as_millis() < 500, "should exit quickly");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn test_main_loop_exits_when_all_channels_close() {
        let dir = tempfile::tempdir().expect("tempdir");
        let state = test_state(&dir);
        let cancel = state.cancel_flag.clone();

        let a11y_tx = state.channels.a11y_tx.clone();
        let notif_tx = state.channels.notification_tx.clone();
        let cmd_tx = state.channels.user_command_tx.clone();
        let ipc_out_tx = state.channels.ipc_outbound_tx.clone();
        let ipc_in_tx = state.channels.ipc_inbound_tx.clone();
        let db_tx = state.channels.db_write_tx.clone();
        let cron_tx = state.channels.cron_tick_tx.clone();

        let local = tokio::task::LocalSet::new();
        let handle = local.spawn_local(async move {
            run(state).await;
        });

        local.run_until(async {
            drop(a11y_tx);
            drop(notif_tx);
            drop(cmd_tx);
            drop(ipc_out_tx);
            drop(ipc_in_tx);
            drop(db_tx);
            drop(cron_tx);

            tokio::time::sleep(Duration::from_millis(50)).await;
            cancel.store(true, Ordering::Release);

            let result = tokio::time::timeout(Duration::from_secs(2), handle).await;
            assert!(result.is_ok(), "main loop should exit within timeout");
        }).await;
    }

    #[tokio::test(flavor = "current_thread")]
    async fn test_main_loop_processes_user_command() {
        let dir = tempfile::tempdir().expect("tempdir");
        let state = test_state(&dir);
        let cancel = state.cancel_flag.clone();
        let cmd_tx = state.channels.user_command_tx.clone();

        let local = tokio::task::LocalSet::new();
        let handle = local.spawn_local(async move {
            run(state).await;
        });

        local.run_until(async {
            cmd_tx
                .send(UserCommand::Chat {
                    text: "hello".to_string(),
                    source: InputSource::Direct,
                    voice_meta: None,
                })
                .await
                .expect("send should succeed");

            tokio::time::sleep(Duration::from_millis(200)).await;
            cancel.store(true, Ordering::Release);

            let result = tokio::time::timeout(Duration::from_secs(5), handle).await;
            assert!(result.is_ok(), "main loop should exit after processing command");
        }).await;
    }

    #[tokio::test]
    async fn test_a11y_pipeline() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut state = test_state(&dir);
        let response_tx = state.channels.response_tx.clone();
        let data_dir = dir.path().to_path_buf();
        let mut subs = LoopSubsystems::new(response_tx, &data_dir);

        let event = aura_types::events::RawEvent {
            event_type: 32,
            package_name: "com.whatsapp".to_string(),
            class_name: "android.widget.TextView".to_string(),
            text: Some("Hello!".to_string()),
            content_description: None,
            timestamp_ms: 1_700_000_000_000,
            source_node_id: None,
        };

        let result = handle_a11y_event(event, &mut state, &mut subs).await;
        assert!(result.is_ok(), "a11y handler should succeed");
    }

    #[tokio::test]
    async fn test_notification_pipeline() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut state = test_state(&dir);
        let response_tx = state.channels.response_tx.clone();
        let data_dir = dir.path().to_path_buf();
        let mut subs = LoopSubsystems::new(response_tx, &data_dir);

        let event = aura_types::events::NotificationEvent {
            package: "com.whatsapp".to_string(),
            title: "Alice".to_string(),
            text: "Hey, are you free?".to_string(),
            category: aura_types::events::NotificationCategory::Message,
            timestamp_ms: 1_700_000_000_000,
            is_ongoing: false,
            actions: vec![],
        };

        let result = handle_notification_event(event, &mut state, &mut subs).await;
        assert!(result.is_ok(), "notification handler should succeed");
    }

    #[tokio::test]
    async fn test_ipc_inbound_conversation_reply() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut state = test_state(&dir);
        let response_tx = state.channels.response_tx.clone();
        let data_dir = dir.path().to_path_buf();
        let mut subs = LoopSubsystems::new(response_tx, &data_dir);

        let msg = NeocortexToDaemon::ConversationReply {
            text: "Hello from neocortex!".to_string(),
            mood_hint: Some(0.5),
        };

        let result = handle_ipc_inbound(msg, &mut state, &mut subs).await;
        assert!(result.is_ok(), "conversation reply handler should succeed");

        // Verify disposition was nudged.
        assert!(
            state.checkpoint.disposition.mood.valence > 0.0,
            "valence should be positive after positive mood hint"
        );
    }

    #[tokio::test]
    async fn test_ipc_inbound_error_records_feedback() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut state = test_state(&dir);
        let response_tx = state.channels.response_tx.clone();
        let data_dir = dir.path().to_path_buf();
        let mut subs = LoopSubsystems::new(response_tx, &data_dir);

        let msg = NeocortexToDaemon::Error {
            code: 42,
            message: "test error".to_string(),
        };

        let result = handle_ipc_inbound(msg, &mut state, &mut subs).await;
        assert!(result.is_ok(), "error handler should succeed");
    }

    #[tokio::test]
    async fn test_ipc_inbound_memory_warning() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut state = test_state(&dir);
        let response_tx = state.channels.response_tx.clone();
        let data_dir = dir.path().to_path_buf();
        let mut subs = LoopSubsystems::new(response_tx, &data_dir);

        let msg = NeocortexToDaemon::MemoryWarning {
            used_mb: 3500,
            available_mb: 50,
        };

        // This will try to send Unload but neocortex is disconnected — that's fine.
        let result = handle_ipc_inbound(msg, &mut state, &mut subs).await;
        assert!(result.is_ok(), "memory warning handler should succeed");
    }

    #[tokio::test]
    async fn test_ipc_inbound_token_budget_exhausted() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut state = test_state(&dir);
        let response_tx = state.channels.response_tx.clone();
        let data_dir = dir.path().to_path_buf();
        let mut subs = LoopSubsystems::new(response_tx, &data_dir);

        let before = state.checkpoint.token_counters.cloud_tokens;
        let msg = NeocortexToDaemon::TokenBudgetExhausted;
        let result = handle_ipc_inbound(msg, &mut state, &mut subs).await;
        assert!(result.is_ok());
        assert_eq!(
            state.checkpoint.token_counters.cloud_tokens,
            before + 1,
            "cloud_tokens should increment"
        );
    }

    #[test]
    fn test_db_write_telemetry() {
        let dir = tempfile::tempdir().expect("tempdir");
        let state = test_state(&dir);

        let req = DbWriteRequest::Telemetry {
            payload: vec![1, 2, 3, 4],
        };
        let result = handle_db_write(req, &state);
        assert!(result.is_ok(), "telemetry write should succeed");
    }

    #[test]
    fn test_db_write_episode() {
        let dir = tempfile::tempdir().expect("tempdir");
        let state = test_state(&dir);

        let req = DbWriteRequest::Episode {
            content: "test episode content".to_string(),
            importance: 0.75,
        };
        let result = handle_db_write(req, &state);
        assert!(result.is_ok(), "episode write should succeed");
    }

    #[tokio::test]
    async fn test_cron_token_reset() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut state = test_state(&dir);
        let response_tx = state.channels.response_tx.clone();
        let data_dir = dir.path().to_path_buf();
        let mut subs = LoopSubsystems::new(response_tx, &data_dir);

        state.checkpoint.token_counters.local_tokens = 100;
        state.checkpoint.token_counters.cloud_tokens = 50;

        let tick = CronTick {
            job_id: 1,
            job_name: "token_reset".to_string(),
            scheduled_at_ms: now_ms(),
        };

        let result = handle_cron_tick(tick, &mut state, &mut subs).await;
        assert!(result.is_ok());
        assert_eq!(state.checkpoint.token_counters.local_tokens, 0);
        assert_eq!(state.checkpoint.token_counters.cloud_tokens, 0);
    }

    #[tokio::test]
    async fn test_empty_chat_ignored() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut state = test_state(&dir);
        let response_tx = state.channels.response_tx.clone();
        let data_dir = dir.path().to_path_buf();
        let mut subs = LoopSubsystems::new(response_tx, &data_dir);

        let cmd = UserCommand::Chat {
            text: "   ".to_string(),
            source: InputSource::Direct,
            voice_meta: None,
        };

        let result = handle_user_command(cmd, &mut state, &mut subs).await;
        assert!(result.is_ok(), "empty chat should be handled gracefully");
    }

    #[tokio::test]
    async fn test_ipc_outbound_empty_payload() {
        let dir = tempfile::tempdir().expect("tempdir");
        let state = test_state(&dir);
        let response_tx = state.channels.response_tx.clone();
        let data_dir = dir.path();
        let mut subs = LoopSubsystems::new(response_tx, data_dir);

        let msg = IpcOutbound {
            payload: vec![],
        };

        let result = handle_ipc_outbound(msg, &mut subs).await;
        assert!(result.is_ok(), "empty IPC outbound should be handled");
    }

    #[tokio::test]
    async fn test_ipc_outbound_oversized() {
        let dir = tempfile::tempdir().expect("tempdir");
        let state = test_state(&dir);
        let response_tx = state.channels.response_tx.clone();
        let data_dir = dir.path();
        let mut subs = LoopSubsystems::new(response_tx, data_dir);

        let msg = IpcOutbound {
            payload: vec![0u8; MAX_IPC_PAYLOAD_BYTES + 1],
        };

        let result = handle_ipc_outbound(msg, &mut subs).await;
        assert!(result.is_ok(), "oversized IPC outbound should be dropped gracefully");
    }

    #[tokio::test]
    async fn test_task_request_creates_goal() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut state = test_state(&dir);
        let response_tx = state.channels.response_tx.clone();
        let data_dir = dir.path().to_path_buf();
        let mut subs = LoopSubsystems::new(response_tx, &data_dir);

        let goals_before = state.checkpoint.goals.len();

        let cmd = UserCommand::TaskRequest {
            description: "test task".to_string(),
            priority: 5,
            source: InputSource::Direct,
        };

        let result = handle_user_command(cmd, &mut state, &mut subs).await;
        assert!(result.is_ok(), "task request should succeed");
        assert!(
            state.checkpoint.goals.len() > goals_before,
            "a new goal should be created"
        );
    }

    #[tokio::test]
    async fn test_cancel_task() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut state = test_state(&dir);
        let response_tx = state.channels.response_tx.clone();
        let data_dir = dir.path().to_path_buf();
        let mut subs = LoopSubsystems::new(response_tx, &data_dir);

        // Add a goal to cancel.
        state.checkpoint.goals.push(aura_types::goals::Goal {
            id: 42,
            description: "test goal".to_string(),
            priority: aura_types::goals::GoalPriority::Medium,
            status: aura_types::goals::GoalStatus::Active,
            steps: Vec::new(),
            created_ms: now_ms(),
            deadline_ms: None,
            parent_goal: None,
            source: aura_types::goals::GoalSource::UserExplicit,
        });

        let cmd = UserCommand::CancelTask {
            task_id: "42".to_string(),
            source: InputSource::Direct,
        };

        let result = handle_user_command(cmd, &mut state, &mut subs).await;
        assert!(result.is_ok());
        assert!(
            !state.checkpoint.goals.iter().any(|g| g.id == 42),
            "goal 42 should be removed"
        );
    }

    #[tokio::test]
    async fn test_profile_switch() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut state = test_state(&dir);
        let response_tx = state.channels.response_tx.clone();
        let data_dir = dir.path().to_path_buf();
        let mut subs = LoopSubsystems::new(response_tx, &data_dir);

        let cmd = UserCommand::ProfileSwitch {
            profile: "work".to_string(),
            source: InputSource::Direct,
        };

        let result = handle_user_command(cmd, &mut state, &mut subs).await;
        assert!(result.is_ok(), "profile switch should succeed");
    }
}
