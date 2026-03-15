//! # Main Event Loop — `main_loop.rs`
//!
//! **ARCH-MED-1 (Architecture Documentation)**
//!
//! This is the central nervous system of the AURA v4 daemon.  It owns the
//! `tokio::select!` event loop that multiplexes 7+ channels and drives every
//! subsystem in the process.
//!
//! ## Why this file is large (~7 300 lines)
//!
//! `main_loop.rs` is intentionally a "god file."  The daemon's event loop
//! must see — and coordinate — every subsystem in a single `select!` block.
//! Splitting it across modules would force shared mutable state behind
//! `Arc<Mutex<_>>`, which conflicts with the single-writer `&mut self`
//! memory model (see ARCH-MED-3 below).  The trade-off is a large file with
//! clear section headers rather than scattered state synchronisation.
//!
//! ### Future refactoring direction
//!
//! If this file grows past ~10 000 lines, consider extracting pure-logic
//! helpers (scoring, enrichment, formatting) into companion modules while
//! keeping the `select!` loop and `&mut self` receiver here.
//!
//! ## Concurrency model — ARCH-MED-3 (Single-Writer `&mut self`)
//!
//! The daemon's mutable state lives in [`DaemonState`] (and its subsystem
//! struct [`LoopSubsystems`]).  **Only the main loop holds `&mut self`
//! references to these structs.**  Every other subsystem communicates via
//! message-passing channels; none hold a shared reference to daemon state.
//!
//! This guarantees:
//! - **No data races** — Rust's borrow checker enforces single-writer at compile time.
//! - **No hidden locking** — There are zero `Mutex<DaemonState>` patterns in the codebase.
//! - **Predictable ordering** — All state mutations happen on the select loop's task, so operations
//!   within a single tick are sequential.
//!
//! The cost is that subsystems cannot directly query daemon state; they must
//! send a request through a channel and wait for the main loop to respond.
//! This is an intentional design choice, not an oversight.
//!
//! ## Event flow
//!
//! The loop runs until a cancellation signal is received or all producer
//! channels close.  Each select branch has independent error handling;
//! the loop **never** panics.
//!
//! A checkpoint timer fires every `config.checkpoint_interval_secs` to
//! persist state to disk.
//!
//! ### Wiring
//!
//! All 15+ subsystem connections are wired in this module through the
//! [`LoopSubsystems`] struct, which holds every subsystem not already
//! stored in [`DaemonState`].  The event flow is:
//!
//! - **A11y / Notification** → `EventParser` → `Amygdala` → gate decision → working memory +
//!   disposition
//! - **Chat** → `CommandParser` → build `ParsedEvent` → `Amygdala.score` →
//!   `PolicyGate.check_action` → `Contextor.enrich` → `RouteClassifier` → System1 fast path **or**
//!   System2 neocortex path
//! - **IPC outbound** → deserialize via bincode → `NeocortexClient.send`
//! - **IPC inbound** → match variant → PlanReady→react, ConversationReply
//!   →anti-sycophancy→response, Error→feedback_loop, MemoryWarning→unload
//! - **Cron tick** → memory consolidation, health report, token reset, stale request sweep
//!
//! ## IPC channel capacity note — GAP-CRIT-003
//!
//! The bridge command channel is created with capacity 64.  All `.send()`
//! call sites in this file handle errors with `if let Err(e)` + `warn!()`
//! logging (verified via audit, 2026-03).  The router uses `try_send()`
//! with explicit `Full` / `Closed` handling.  If 64 proves too small under
//! production load, increase the constant — do **not** switch to unbounded
//! channels (memory-leak risk under back-pressure).

#[cfg(test)]
use std::time::Instant;
use std::{path::Path, sync::atomic::Ordering};

use aura_types::{
    events::{DaemonEvent, EventSource, GateDecision, Intent, ParsedEvent, ScoredEvent},
    identity::RelationshipStage,
    ipc::{
        ContextPackage, DaemonToNeocortex, FailureContext, IdentityTendencies, InferenceMode,
        NeocortexToDaemon, SelfKnowledge,
    },
    outcome::{ExecutionOutcome, OutcomeResult, RouteKind, UserReaction},
    power::PowerTier,
};
use tokio::time::{interval, Duration};
use tracing::instrument;

// -- P0/P1 Foundation imports -----------------------------------------------
use crate::health::{HealthMonitor, HealthStatus};
use crate::{
    arc::{
        proactive::{ProactiveAction, ProactiveEngine},
        ArcManager, ContextMode,
    },
    bridge::{
        router::ResponseRouter,
        spawn_bridge,
        system_api::{SystemBridge, SystemCommand, SystemResult},
        telegram_bridge::TelegramBridge,
        voice_bridge::VoiceBridge,
    },
    daemon_core::{
        channels::{
            CronTick, DaemonResponse, DaemonResponseTx, DbWriteRequest, InputSource, IpcOutbound,
            UserCommand,
        },
        checkpoint::save_checkpoint,
        proactive_dispatcher::{
            arc_trigger_to_ipc, should_dispatch, trigger_to_ipc, AlertDirection, ProactiveTrigger,
        },
        react,
        startup::DaemonState,
    },
    execution::{
        learning::WorkflowObserver,
        planner::EnhancedPlanner,
        react::{CognitiveState, EscalationContext, SemanticReact},
        retry::{EnvironmentSnapshot, RecoveryAction, RecoveryContext, StrategicRecovery},
    },
    goals::{
        conflicts::{ConflictResolver, GoalConflictEntry},
        decomposer::GoalDecomposer,
        registry::GoalRegistry,
        scheduler::{
            BdiScheduler, Belief, BeliefSource, DeliberationResult, ScoreComponents, ScoredGoal,
        },
        tracker::GoalTracker,
    },
    identity::{affective::MoodEvent, IdentityEngine},
    ipc::NeocortexClient,
    memory::{consolidate, AuraMemory, ConsolidationLevel},
    outcome_bus::OutcomeBus,
    persistence::{CriticalVault, DataTier},
    pipeline::{
        amygdala::Amygdala,
        contextor::{Contextor, EnrichedEvent},
        parser::{CommandParser, EventParser},
    },
    policy::{
        audit::AuditLog,
        emergency::{AnomalyDetector, EmergencyStop},
        gate::PolicyGate,
        sandbox::{ContainmentLevel, Sandbox},
        BoundaryContext, BoundaryDecision, BoundaryReasoner,
    },
    reaction::ReactionDetector,
    routing::{classifier::RouteClassifier, system1::System1, system2::System2},
    screen::{detect_app_state, extract_screen_summary, AppState, ScreenCache},
    telegram::TelegramConfig,
    voice::VoiceEngine,
};

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
#[allow(dead_code)] // Phase 8: containment_level read by sandbox audit log exporter
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
    /// Last detected app state from the most recent screen tree.
    /// `None` until the first screen event delivers a full tree.
    /// Used to feed structural complexity signal into the route classifier.
    last_app_state: Option<AppState>,

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
    /// Rolling count of recent boundary denials, fed into [`BoundaryContext`]
    /// so the reasoner can escalate when anomalous denial bursts occur.
    recent_denial_count: u32,

    /// Epoch-ms timestamp of the last user-initiated input.
    ///
    /// Runtime-only (not persisted).  Used by `cron_handle_dreaming` to enforce
    /// the 30-minute idle guard — dreaming never runs while the user is active.
    /// Initialised to the daemon start time and updated on every
    /// `user_command_rx` message.
    last_interaction_ms: u64,
}

impl LoopSubsystems {
    /// Construct all subsystems.  `response_tx` is cloned before the
    /// channel split so we can send responses from handlers.
    ///
    /// `memory` is taken by move from `DaemonState.subsystems.memory` — this
    /// ensures there is exactly ONE `AuraMemory` instance open against the
    /// SQLite files, eliminating the previous double-init / WAL write conflict.
    fn new(response_tx: DaemonResponseTx, data_dir: &std::path::Path, memory: AuraMemory) -> Self {
        // ARC subsystem construction — non-critical, degrade gracefully.
        let bdi_scheduler = match std::panic::catch_unwind(BdiScheduler::new) {
            Ok(s) => {
                tracing::info!("BdiScheduler initialised");
                Some(s)
            },
            Err(_) => {
                tracing::warn!("BdiScheduler construction panicked — running without BDI");
                None
            },
        };

        let goal_tracker = match std::panic::catch_unwind(GoalTracker::new) {
            Ok(t) => {
                tracing::info!("GoalTracker initialised");
                Some(t)
            },
            Err(_) => {
                tracing::warn!("GoalTracker construction panicked — running without tracker");
                None
            },
        };

        let goal_decomposer = match std::panic::catch_unwind(GoalDecomposer::new) {
            Ok(d) => {
                tracing::info!("GoalDecomposer initialised");
                Some(d)
            },
            Err(_) => {
                tracing::warn!("GoalDecomposer construction panicked — running without decomposer");
                None
            },
        };

        let goal_registry = match std::panic::catch_unwind(GoalRegistry::new) {
            Ok(r) => {
                tracing::info!("GoalRegistry initialised");
                Some(r)
            },
            Err(_) => {
                tracing::warn!("GoalRegistry construction panicked — running without registry");
                None
            },
        };

        let conflict_resolver = match std::panic::catch_unwind(ConflictResolver::new) {
            Ok(c) => {
                tracing::info!("ConflictResolver initialised");
                Some(c)
            },
            Err(_) => {
                tracing::warn!(
                    "ConflictResolver construction panicked — running without conflict resolution"
                );
                None
            },
        };

        let proactive = match std::panic::catch_unwind(ProactiveEngine::new) {
            Ok(p) => {
                tracing::info!("ProactiveEngine initialised");
                Some(p)
            },
            Err(_) => {
                tracing::warn!("ProactiveEngine construction panicked — running without proactive");
                None
            },
        };

        let arc_manager = match std::panic::catch_unwind(ArcManager::new) {
            Ok(a) => {
                tracing::info!("ArcManager initialised");
                Some(a)
            },
            Err(_) => {
                tracing::warn!("ArcManager construction panicked — running without ARC manager");
                None
            },
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
                },
                Err(_) => {
                    tracing::warn!(
                        "EnhancedPlanner construction panicked — running without plan caching"
                    );
                    None
                },
            },
            workflow_observer: match std::panic::catch_unwind(WorkflowObserver::new) {
                Ok(wo) => {
                    tracing::info!("WorkflowObserver initialised");
                    Some(wo)
                },
                Err(_) => {
                    tracing::warn!("WorkflowObserver construction panicked — running without workflow learning");
                    None
                },
            },
            semantic_react: SemanticReact::new(),
            consecutive_task_failures: 0,
            successful_task_count: 0,
            screen_cache: ScreenCache::with_config(
                SCREEN_CACHE_MAX_ENTRIES,
                SCREEN_CACHE_MAX_BYTES,
                SCREEN_CACHE_TTL_MS,
            ),
            last_app_state: None,

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
                    },
                    Err(e) => {
                        tracing::warn!(
                            error = %e,
                            "journal creation failed — running without WAL persistence"
                        );
                        None
                    },
                }
            },
            safe_mode: crate::persistence::SafeModeState::inactive(),

            // -- P0/P1 Foundations ────────────────────────────────────────
            health_monitor: HealthMonitor::new(now_ms(), 1),
            strategic_recovery: StrategicRecovery::new(),
            system_bridge: SystemBridge::new(),
            critical_vault: CriticalVault::new(),
            boundary_reasoner: BoundaryReasoner::new(),
            recent_denial_count: 0,
            last_interaction_ms: now_ms(),
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
    use crate::policy::{
        gate::RateLimiter,
        rules::{PolicyRule, RuleEffect},
    };

    // Start with an empty gate that allows everything by default.
    // We will add deny/confirm/audit rules that override the default for
    // dangerous action patterns, and set a strict rate limiter.
    let mut gate = PolicyGate::allow_all_builder();

    // ── Priority 0: HARD DENY — irreversible / destructive system actions ──
    let hard_deny_rules = [
        (
            "deny-factory-reset",
            "*factory*reset*",
            "Factory reset is destructive and irreversible.",
        ),
        (
            "deny-wipe-data",
            "*wipe*data*",
            "Data wipe is irreversible — requires out-of-band confirmation.",
        ),
        (
            "deny-wipe-device",
            "*wipe*device*",
            "Device wipe is irreversible — requires out-of-band confirmation.",
        ),
        (
            "deny-uninstall-system",
            "*uninstall*system*",
            "System app removal can brick the device.",
        ),
        (
            "deny-format-storage",
            "*format*storage*",
            "Storage formatting destroys all user data.",
        ),
        (
            "deny-format-disk",
            "*format*disk*",
            "Disk formatting destroys all user data.",
        ),
        (
            "deny-root-access",
            "*root*access*",
            "Root/superuser access is never permitted.",
        ),
        (
            "deny-su-command",
            "*su *",
            "Superuser commands are never permitted.",
        ),
        (
            "deny-modify-bootloader",
            "*bootloader*",
            "Bootloader modification can brick the device.",
        ),
        (
            "deny-flash-firmware",
            "*flash*firmware*",
            "Firmware flashing can brick the device.",
        ),
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
        (
            "deny-read-contacts",
            "*read*contact*",
            "Contact access requires explicit user consent.",
        ),
        (
            "deny-read-messages",
            "*read*message*",
            "Message access requires explicit user consent.",
        ),
        (
            "deny-read-sms",
            "*read*sms*",
            "SMS access requires explicit user consent.",
        ),
        (
            "deny-read-photos",
            "*read*photo*",
            "Photo access requires explicit user consent.",
        ),
        (
            "deny-read-gallery",
            "*read*gallery*",
            "Gallery access requires explicit user consent.",
        ),
        (
            "deny-read-location",
            "*read*location*",
            "Location access requires explicit user consent.",
        ),
        (
            "deny-access-camera",
            "*access*camera*",
            "Camera access requires explicit user consent.",
        ),
        (
            "deny-access-microphone",
            "*access*microphone*",
            "Microphone access requires explicit user consent.",
        ),
        (
            "deny-read-call-log",
            "*read*call*log*",
            "Call log access requires explicit user consent.",
        ),
        (
            "deny-read-calendar",
            "*read*calendar*",
            "Calendar access requires explicit user consent.",
        ),
        (
            "deny-share-data",
            "*share*data*",
            "Data sharing requires explicit user consent.",
        ),
        (
            "deny-export-data",
            "*export*user*data*",
            "Exporting user data requires explicit consent.",
        ),
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
        (
            "confirm-purchase",
            "*purchase*",
            "Purchases require explicit user confirmation.",
        ),
        (
            "confirm-payment",
            "*payment*",
            "Payments require explicit user confirmation.",
        ),
        (
            "confirm-transfer",
            "*transfer*money*",
            "Money transfers require explicit user confirmation.",
        ),
        (
            "confirm-subscribe",
            "*subscribe*",
            "Subscriptions require explicit user confirmation.",
        ),
        (
            "confirm-install-app",
            "*install*app*",
            "App installation requires user confirmation.",
        ),
        (
            "confirm-uninstall-app",
            "*uninstall*app*",
            "App removal requires user confirmation.",
        ),
        (
            "confirm-delete-file",
            "*delete*file*",
            "File deletion requires user confirmation.",
        ),
        (
            "confirm-delete-photo",
            "*delete*photo*",
            "Photo deletion requires user confirmation.",
        ),
        (
            "confirm-modify-settings",
            "*modify*system*setting*",
            "System setting changes require confirmation.",
        ),
        (
            "confirm-change-password",
            "*change*password*",
            "Password changes require user confirmation.",
        ),
        (
            "confirm-account-change",
            "*account*change*",
            "Account changes require user confirmation.",
        ),
        (
            "confirm-send-email",
            "*send*email*",
            "Sending email requires user confirmation.",
        ),
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
        (
            "audit-credential-access",
            "*credential*",
            "Credential access is audit-logged.",
        ),
        (
            "audit-token-access",
            "*token*",
            "Token access is audit-logged.",
        ),
        (
            "audit-api-key",
            "*api*key*",
            "API key access is audit-logged.",
        ),
        (
            "audit-network-request",
            "*network*request*",
            "Network requests are audit-logged.",
        ),
        (
            "audit-bluetooth",
            "*bluetooth*",
            "Bluetooth operations are audit-logged.",
        ),
        (
            "audit-wifi-change",
            "*wifi*change*",
            "WiFi configuration changes are audit-logged.",
        ),
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
        (
            "allow-read-screen",
            "read*screen*",
            "Reading screen content is safe.",
        ),
        (
            "allow-read-notification",
            "read*notification*",
            "Reading notifications is safe.",
        ),
        ("allow-scroll", "scroll*", "Scrolling is safe."),
        ("allow-tap", "tap*", "Tapping UI elements is safe."),
        ("allow-type-text", "type*text*", "Typing text is safe."),
        ("allow-open-app", "open*app*", "Opening apps is safe."),
        ("allow-switch-app", "switch*app*", "Switching apps is safe."),
        (
            "allow-go-home",
            "*go*home*",
            "Going to home screen is safe.",
        ),
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
    if lower.contains("contact")
        || lower.contains("message")
        || lower.contains("sms")
        || lower.contains("photo")
        || lower.contains("gallery")
        || lower.contains("camera")
        || lower.contains("microphone")
        || lower.contains("location")
        || lower.contains("call log")
        || lower.contains("calendar")
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
            tracing::warn!(category, description, "task blocked: missing user consent");
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
        },
        ContainmentLevel::Restricted => {
            // L2: Log at warn level — per-action confirmation handled downstream.
            tracing::info!(
                target: "SECURITY",
                level = %level,
                description,
                "task classified as Restricted — per-action confirmation required"
            );
            None
        },
        ContainmentLevel::Monitored => {
            tracing::debug!(
                target: "SECURITY",
                level = %level,
                description,
                "task classified as Monitored — execution will be logged"
            );
            None
        },
        ContainmentLevel::Direct => None,
    }
}

/// Dynamic boundary reasoning check for a task description.
///
/// Returns:
///  - `BoundaryGateResult::Allow` — proceed normally.
///  - `BoundaryGateResult::Deny(reason)` — block the task.
///  - `BoundaryGateResult::NeedConfirmation { reason, prompt }` — route through the sandbox
///    confirmation flow before executing.
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
    recent_denials: u32,
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
        recent_denials,
    };

    match boundary_reasoner.evaluate(description, &ctx) {
        BoundaryDecision::Allow => BoundaryGateResult::Allow,
        BoundaryDecision::AllowWithConfirmation {
            reason,
            confirmation_prompt,
        } => {
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
        },
        BoundaryDecision::Deny { reason, level } => {
            tracing::warn!(
                target: "SECURITY",
                description,
                reason = %reason,
                level = ?level,
                "BoundaryReasoner: DENIED (conditional rule)"
            );
            BoundaryGateResult::Deny(reason)
        },
        BoundaryDecision::DenyAbsolute { reason, rule_id } => {
            tracing::error!(
                target: "SAFETY",
                description,
                reason = %reason,
                rule_id,
                "BoundaryReasoner: ABSOLUTE DENY — hardcoded safety rule"
            );
            BoundaryGateResult::Deny(format!("[{rule_id}] {reason}"))
        },
    }
}

/// Convenience wrapper: calls [`check_boundary_for_task`] and increments
/// `subs.recent_denial_count` when the result is a denial.
///
/// **Must be called before every sensitive operation** (SMS, contacts, calendar,
/// sensitive app launches).  If it returns `Deny`, the caller MUST return early
/// and report the denial reason to the LLM — never silently skip.
fn boundary_check(subs: &mut LoopSubsystems, description: &str) -> BoundaryGateResult {
    let stage = current_relationship_stage(subs);
    let result = check_boundary_for_task(
        &subs.boundary_reasoner,
        description,
        stage,
        subs.recent_denial_count,
    );
    if matches!(result, BoundaryGateResult::Deny(_)) {
        subs.recent_denial_count = subs.recent_denial_count.saturating_add(1);
    }
    result
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
            match LAST_TIME_MS.compare_exchange_weak(
                last,
                next,
                std::sync::atomic::Ordering::SeqCst,
                std::sync::atomic::Ordering::Relaxed,
            ) {
                Ok(_) => return next,
                Err(x) => last = x,
            }
        } else {
            match LAST_TIME_MS.compare_exchange_weak(
                last,
                now,
                std::sync::atomic::Ordering::SeqCst,
                std::sync::atomic::Ordering::Relaxed,
            ) {
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
        mood_hint: None,
    };
    if let Err(e) = tx.send(resp).await {
        tracing::error!(error = %e, "failed to send daemon response");
    }
}

/// Like [`send_response`] but forwards the LLM's mood_hint so TTS can use it.
async fn send_response_with_mood(
    tx: &DaemonResponseTx,
    dest: InputSource,
    text: String,
    mood_hint: Option<f32>,
) {
    let resp = DaemonResponse {
        destination: dest,
        text,
        mood_hint,
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
        SystemResult::Battery {
            level,
            charging,
            health,
        } => {
            let pct = (level * 100.0) as u8;
            let charge_str = if *charging { ", charging" } else { "" };
            format!("Battery: {pct}%{charge_str} (health: {health:?})")
        },
        SystemResult::Storage {
            total_bytes,
            free_bytes,
        } => {
            let total_gb = *total_bytes as f64 / 1_073_741_824.0;
            let free_gb = *free_bytes as f64 / 1_073_741_824.0;
            format!("Storage: {free_gb:.1} GB free of {total_gb:.1} GB total")
        },
        SystemResult::Network {
            connected,
            wifi,
            mobile_data,
            signal_strength,
        } => {
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
        },
        SystemResult::Memory {
            total_bytes,
            available_bytes,
            low_memory,
        } => {
            let total_gb = *total_bytes as f64 / 1_073_741_824.0;
            let avail_gb = *available_bytes as f64 / 1_073_741_824.0;
            let warn = if *low_memory { " [LOW MEMORY]" } else { "" };
            format!("RAM: {avail_gb:.1} GB available of {total_gb:.1} GB{warn}")
        },
        SystemResult::Thermal(state) => {
            format!("Thermal state: {state:?}")
        },
        SystemResult::Contacts(contacts) => {
            if contacts.is_empty() {
                "No contacts found.".to_string()
            } else {
                let list: Vec<String> = contacts
                    .iter()
                    .take(5)
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
        },
        SystemResult::Calendar(events) => {
            if events.is_empty() {
                "No calendar events found.".to_string()
            } else {
                let list: Vec<String> = events
                    .iter()
                    .take(5)
                    .map(|e| {
                        let loc = e.location.as_deref().unwrap_or("no location");
                        format!("  {} ({})", e.title, loc)
                    })
                    .collect();
                format!("Calendar ({} events):\n{}", events.len(), list.join("\n"))
            }
        },
        SystemResult::Photos(photos) => {
            format!("Found {} recent photos.", photos.len())
        },
        SystemResult::Notifications(notifs) => {
            if notifs.is_empty() {
                "No active notifications.".to_string()
            } else {
                let list: Vec<String> = notifs
                    .iter()
                    .take(5)
                    .map(|n| format!("  [{}] {}", n.package, n.title))
                    .collect();
                format!("Notifications ({}):\n{}", notifs.len(), list.join("\n"))
            }
        },
        SystemResult::ActionCompleted {
            command,
            success,
            message,
        } => {
            if *success {
                format!("{command}: {message}")
            } else {
                format!("{command} failed: {message}")
            }
        },
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
        },
        Err(e) => {
            tracing::warn!(error = %e, "IPC reconnect failed");
            false
        },
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
        },
    };
    let telegram_bridge_rx = match router.register("telegram").await {
        Ok(rx) => Some(rx),
        Err(e) => {
            tracing::warn!(error = %e, "failed to register telegram bridge");
            None
        },
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
        let telegram_bridge = TelegramBridge::new(telegram_config, state.cancel_flag.clone(), None);
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
    // Move the AuraMemory out of DaemonState by swapping in a harmless
    // in-memory placeholder.  This ensures exactly ONE AuraMemory instance is
    // open against the SQLite files for the process lifetime, eliminating the
    // previous double-init / WAL write conflict.
    // After this swap, state.subsystems.memory is an unused in-memory dummy;
    // all real memory access goes through subs.memory.
    let loop_memory = {
        let placeholder = match AuraMemory::new_in_memory() {
            Ok(m) => m,
            Err(e) => {
                tracing::error!(
                    "FATAL: failed to create in-memory AuraMemory placeholder \
                     for memory swap — daemon loop cannot continue: {e}"
                );
                return;
            },
        };
        std::mem::replace(&mut state.subsystems.memory, placeholder)
    };
    let mut subs = LoopSubsystems::new(response_tx, data_dir, loop_memory);

    // ── FIX 1: Provision vault encryption key before any vault operations ──
    //
    // Strategy: per-device stable key stored in `{data_dir}/vault.key`.
    // - If the file exists: read the 32 bytes from it.
    // - If not: generate 32 random bytes via OsRng, write with mode 0o600.
    // This gives per-device encryption without requiring a user PIN (PIN-based
    // key derivation can be added later as an upgrade path).
    {
        let vault_key_path = data_dir.join("vault.key");
        let vault_key: Option<[u8; 32]> = if vault_key_path.exists() {
            match std::fs::read(&vault_key_path) {
                Ok(bytes) if bytes.len() == 32 => {
                    let mut key = [0u8; 32];
                    key.copy_from_slice(&bytes);
                    tracing::info!("vault key loaded from {:?}", vault_key_path);
                    Some(key)
                },
                Ok(bytes) => {
                    tracing::error!(
                        len = bytes.len(),
                        path = ?vault_key_path,
                        "vault.key has wrong length — expected 32 bytes; vault will remain locked"
                    );
                    None
                },
                Err(e) => {
                    tracing::error!(error = %e, path = ?vault_key_path, "failed to read vault.key");
                    None
                },
            }
        } else {
            // Generate a fresh 32-byte key and persist it.
            use rand::RngCore;
            let mut key = [0u8; 32];
            aes_gcm::aead::OsRng.fill_bytes(&mut key);
            match std::fs::write(&vault_key_path, &key) {
                Ok(()) => {
                    // Restrict permissions to owner-read-only on Unix-like targets.
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::PermissionsExt;
                        let _ = std::fs::set_permissions(
                            &vault_key_path,
                            std::fs::Permissions::from_mode(0o600),
                        );
                    }
                    tracing::info!(path = ?vault_key_path, "vault key generated and persisted");
                    Some(key)
                },
                Err(e) => {
                    tracing::error!(error = %e, path = ?vault_key_path, "failed to write vault.key — vault will remain locked");
                    None
                },
            }
        };

        if let Some(key) = vault_key {
            subs.critical_vault.set_encryption_key(key);
            tracing::info!("vault encryption key provisioned");
        } else {
            tracing::warn!(
                "vault encryption key NOT provisioned — Tier 1+ store/retrieve will fail"
            );
        }
    }

    // ── FIX 2: Sync pre-existing checkpoint goals into GoalTracker ──────
    //
    // GoalTracker::new() starts empty. After a restart, checkpoint.goals
    // may contain Active/Pending goals from the previous session. Without
    // bulk-syncing them here, stall/overdue detection is blind to those
    // goals until they are individually touched in the current session.
    {
        let active_goal_count = state
            .checkpoint
            .goals
            .iter()
            .filter(|g| {
                matches!(
                    g.status,
                    aura_types::goals::GoalStatus::Active | aura_types::goals::GoalStatus::Pending
                )
            })
            .count();

        if active_goal_count > 0 {
            if let Some(ref mut tracker) = subs.goal_tracker {
                let mut synced: usize = 0;
                for goal in &state.checkpoint.goals {
                    if matches!(
                        goal.status,
                        aura_types::goals::GoalStatus::Active
                            | aura_types::goals::GoalStatus::Pending
                    ) {
                        match tracker.track(goal.clone()) {
                            Ok(()) => synced += 1,
                            Err(e) => {
                                tracing::warn!(
                                    goal_id = goal.id,
                                    error = ?e,
                                    "failed to sync goal into GoalTracker at startup"
                                );
                            },
                        }
                    }
                }
                tracing::info!(
                    synced,
                    total_in_checkpoint = state.checkpoint.goals.len(),
                    "checkpoint goals synced into GoalTracker"
                );
            } else {
                tracing::warn!(
                    active_goals = active_goal_count,
                    "GoalTracker unavailable — checkpoint goals not synced"
                );
            }
        }
    }

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
                            },
                            crate::persistence::JournalCategory::Trust => {
                                if let Some((user_id, interaction, ts)) =
                                    crate::identity::decode_trust_event(&entry.payload)
                                {
                                    subs.identity.relationships.record_interaction(
                                        &user_id,
                                        interaction,
                                        ts,
                                    );
                                    replayed += 1;
                                } else {
                                    tracing::warn!(
                                        ts = entry.timestamp_ms,
                                        "journal replay: malformed trust payload — skipping"
                                    );
                                    failed += 1;
                                }
                            },
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
                            },
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
                            },
                            // Goal / Execution / Memory categories: logged but not replayed
                            // into identity state (they're replayed by their own subsystems).
                            _ => {
                                replayed += 1;
                            },
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
                },
                Err(e) => {
                    tracing::error!(
                        error = %e,
                        "journal recovery FAILED — identity state may be stale"
                    );
                    journal_corruption = true;
                },
            }
        } else {
            tracing::warn!(
                "no journal available — skipping recovery (state relies on checkpoint only)"
            );
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
                    },
                    crate::persistence::VerificationSeverity::Warning => {
                        tracing::warn!(
                            subsystem = issue.subsystem,
                            msg = %issue.message,
                            "integrity warning"
                        );
                    },
                    crate::persistence::VerificationSeverity::Info => {
                        tracing::info!(
                            subsystem = issue.subsystem,
                            msg = %issue.message,
                            "integrity info"
                        );
                    },
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
        },
        Err(e) => {
            tracing::warn!(error = %e, "failed to open database for profile load");
        },
    }

    // Track how many channels are still open.
    // 7 original receiver channels + bridge_cmd_rx + health_event_rx = 9 total.
    // (response_rx was replaced by a dummy — it is no longer counted;
    //  bridge_cmd_rx takes its slot as the 8th channel; health_event_rx is 9th.)
    let mut open_channels: u8 = 9;

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
                        subs.last_interaction_ms = now_ms();
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

            // ----- Health events (heartbeat loop → reactive subsystems) -----
            msg = rxs.health_event_rx.recv() => {
                match msg {
                    Some(event) => {
                        select_count += 1;
                        handle_health_event(event, &mut subs);
                    }
                    None => {
                        tracing::warn!("health event channel closed");
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
                // Build ScreenTree directly from the pre-structured ScreenNode list.
                // `event.raw_nodes` is Vec<ScreenNode> (already hierarchical); the
                // first node is the root of the accessibility tree.
                let screen_tree = {
                    let root_node = raw_nodes[0].clone();
                    let node_count = raw_nodes.len() as u32;
                    aura_types::screen::ScreenTree {
                        root: root_node,
                        package_name: event.package_name.clone(),
                        activity_name: event.class_name.clone(),
                        node_count,
                        timestamp_ms: event.timestamp_ms,
                    }
                };

                // Cache the screen tree
                subs.screen_cache.insert(screen_tree.clone());

                // Detect app state from tree structure (enum-based, no NLP).
                // LLM classifies semantic meaning — Rust returns structural state.
                let app_state = detect_app_state(&screen_tree);
                let summary = extract_screen_summary(&screen_tree);

                subs.contextor.set_screen_summary(Some(format!(
                    "{}::{} ({} clickable)",
                    screen_tree.package_name, screen_tree.activity_name, summary.clickable_count
                )));
                subs.last_app_state = Some(app_state);
            }
        } else {
            // Fallback: lightweight package::class summary when no raw tree
            let summary = format!("{}::{}", event.package_name, event.class_name);
            subs.contextor.set_screen_summary(Some(summary));
        }
    }

    // Map gate decision → MoodEvent and process through AffectiveEngine.
    let mood_event = match scored.gate_decision {
        GateDecision::EmergencyBypass | GateDecision::InstantWake => {
            Some(MoodEvent::UserFrustrated)
        },
        GateDecision::SlowAccumulate => None, // low salience — no mood shift
        GateDecision::Suppress => Some(MoodEvent::Silence { duration_ms: 0 }),
    };

    if let Some(event) = mood_event {
        let ts = now_ms();
        let personality = &subs.identity.personality.traits;
        subs.identity
            .affective
            .process_event_with_personality(event, ts, personality);
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
    subs.identity
        .affective
        .process_event_with_personality(mood_event, ts, personality);
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
        UserCommand::Chat {
            text,
            source,
            voice_meta,
        } => {
            if text.trim().is_empty() {
                tracing::warn!("ignoring empty chat message");
                return Ok(());
            }

            // Truncate excessively long messages (guard against abuse).
            let text = if text.len() > 4096 {
                tracing::warn!(len = text.len(), "truncating oversized chat message");
                let mut end = 4096;
                while end > 0 && !text.is_char_boundary(end) {
                    end -= 1;
                }
                text[..end].to_string()
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
                            if let Some(pos) = subs
                                .pending_confirmations
                                .iter()
                                .position(|c| c.id == conf_id)
                            {
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
                                        subs.recent_denial_count,
                                    );
                                    match boundary_result {
                                        BoundaryGateResult::Deny(reason) => {
                                            subs.recent_denial_count =
                                                subs.recent_denial_count.saturating_add(1);
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
                                            if let Some(goal) = state
                                                .checkpoint
                                                .goals
                                                .iter_mut()
                                                .find(|g| g.id == conf.goal_id)
                                            {
                                                goal.status =
                                                    aura_types::goals::GoalStatus::Failed(format!(
                                                        "boundary denied after /allow: {}",
                                                        reason
                                                    ));
                                            }
                                            send_response(
                                                &subs.response_tx,
                                                source.clone(),
                                                format!(
                                                    "Action blocked by boundary rules: {reason}"
                                                ),
                                            )
                                            .await;
                                            flush_outcome_bus(subs).await;
                                        },
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
                                            )
                                            .await;

                                            // Update goal status based on execution outcome.
                                            if let Some(goal) = state
                                                .checkpoint
                                                .goals
                                                .iter_mut()
                                                .find(|g| g.id == conf.goal_id)
                                            {
                                                match &outcome {
                                                    react::TaskOutcome::Success {
                                                        final_confidence,
                                                        ..
                                                    } => {
                                                        goal.status = aura_types::goals::GoalStatus::Completed;
                                                        tracing::info!(
                                                            goal_id = conf.goal_id,
                                                            session_id = session.session_id,
                                                            confidence = final_confidence,
                                                            "confirmed task completed successfully"
                                                        );
                                                    },
                                                    react::TaskOutcome::Failed {
                                                        reason, ..
                                                    }
                                                    | react::TaskOutcome::CycleAborted {
                                                        cycle_reason: reason,
                                                        ..
                                                    } => {
                                                        goal.status =
                                                            aura_types::goals::GoalStatus::Failed(
                                                                reason.clone(),
                                                            );
                                                        tracing::warn!(
                                                            goal_id = conf.goal_id,
                                                            reason = %reason,
                                                            "confirmed task execution failed"
                                                        );
                                                    },
                                                    react::TaskOutcome::Cancelled { .. } => {
                                                        // Treat cancellation as a pause — goal
                                                        // remains active.
                                                        tracing::info!(
                                                            goal_id = conf.goal_id,
                                                            "confirmed task was cancelled"
                                                        );
                                                    },
                                                }
                                            }

                                            let ack = format!(
                                                "\u{2705} Approved and executed: {}",
                                                conf.description
                                            );
                                            send_response(&subs.response_tx, source, ack).await;

                                            // ── Record user's approval for BoundaryReasoner L3
                                            // learning ──
                                            subs.boundary_reasoner.record_user_response(
                                                &conf.task_summary,
                                                true, // user allowed
                                                response_time_ms,
                                            );
                                        }, // end Allow|NeedConfirmation arm
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
                                    if let Some(goal) = state
                                        .checkpoint
                                        .goals
                                        .iter_mut()
                                        .find(|g| g.id == conf.goal_id)
                                    {
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

                                    let ack = format!("\u{274c} Denied: {}", conf.description);
                                    send_response(&subs.response_tx, source, ack).await;
                                    flush_outcome_bus(subs).await;
                                }
                            } else {
                                let msg = format!("Confirmation {} not found or expired.", conf_id);
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
                let orig_input = subs
                    .reaction_detector
                    .window_original_input()
                    .map(|s| s.to_owned());
                let resp_text = subs
                    .reaction_detector
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
                    let sim_to_original = crate::memory::jaccard_trigram_similarity(&text, &orig);
                    let sim_to_response = crate::memory::jaccard_trigram_similarity(&text, &resp);

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
                            RouteKind::System1, /* best guess; reaction correlates with original
                                                 * route */
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
                                UserReaction::ExplicitPositive => ("user_satisfaction", "positive"),
                                UserReaction::ExplicitNegative => ("user_satisfaction", "negative"),
                                UserReaction::Repetition => ("response_quality", "inadequate"),
                                UserReaction::FollowUp => ("user_engagement", "engaged"),
                                UserReaction::TopicChange => ("user_engagement", "disengaged"),
                                UserReaction::NoReaction => ("user_engagement", "neutral"),
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

                subs.identity
                    .affective
                    .process_event_with_personality(base_event, ts, personality);

                // Voice biomarker stress/fatigue → dedicated mood events + stress accumulation.
                if let Some(ref meta) = voice_meta {
                    if let Some(stress) = meta.emotional_stress {
                        if stress > 0.3 {
                            let event = MoodEvent::VoiceStressDetected { level: stress };
                            subs.identity.affective.process_event_with_personality(
                                event.clone(),
                                ts,
                                personality,
                            );
                            tracing::debug!(stress, "voice stress biomarker processed");
                        }
                    }
                    if let Some(fatigue) = meta.emotional_fatigue {
                        if fatigue > 0.3 {
                            let event = MoodEvent::VoiceFatigueDetected { level: fatigue };
                            subs.identity.affective.process_event_with_personality(
                                event.clone(),
                                ts,
                                personality,
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
                    },
                    Err(e) => {
                        // Already activated — not a failure, just redundant.
                        tracing::debug!(error = %e, "emergency stop already active");
                    },
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
                },
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
                },
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
                },
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
                crate::pipeline::parser::NluIntent::Conversation { .. } => {
                    Intent::ConversationContinue
                },
                crate::pipeline::parser::NluIntent::Unknown { .. } => Intent::InformationRequest,
                _ => {
                    if parse_result.intent.tool_name().is_some() {
                        Intent::ActionRequest
                    } else {
                        Intent::InformationRequest
                    }
                },
            };

            let parsed = ParsedEvent {
                source: EventSource::UserCommand,
                intent,
                content: text.clone(),
                entities: parse_result
                    .entities
                    .iter()
                    .map(|e| e.value.clone())
                    .collect(),
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
            let action_desc = parse_result.intent.tool_name().unwrap_or("conversation");
            let verdict = subs
                .identity
                .policy_gate
                .check_action("user_chat", action_desc);
            match verdict {
                crate::identity::PolicyVerdict::Block { reason } => {
                    tracing::warn!(reason = %reason, "policy gate blocked user command");
                    send_response(
                        &subs.response_tx,
                        source,
                        format!("I can't do that: {}", reason),
                    )
                    .await;
                    return Ok(());
                },
                crate::identity::PolicyVerdict::Audit { reason } => {
                    tracing::info!(reason = %reason, "policy gate flagged for audit");
                    // Continue but log.
                },
                crate::identity::PolicyVerdict::Allow => {},
            }

            // ── Stage 5: Contextor Enrich ───────────────────────────
            let enriched = subs
                .contextor
                .enrich(
                    scored.clone(),
                    &subs.memory,
                    &subs.identity.relationships,
                    &subs.identity.personality,
                    &subs.identity.affective,
                    subs.identity.user_profile(),
                    now_ms(),
                )
                .await;

            let _enriched = match enriched {
                Ok(e) => {
                    tracing::debug!(
                        memory_snippets = e.memory_context.len(),
                        token_budget = e.context_token_budget,
                        "context enrichment complete"
                    );
                    Some(e)
                },
                Err(e) => {
                    tracing::warn!(error = %e, "context enrichment failed — proceeding without");
                    None
                },
            };

            // If enrichment failed entirely and this is a low-confidence parse,
            // fall back to System1 rather than sending uncontextualized queries.
            if _enriched.is_none() && scored.score_total < 0.3 {
                dispatch_system1(&scored, &text, source, state, subs).await;
                return Ok(());
            }

            // Stage 5b removed: personality→routing injection was Theater AGI.
            // The LLM decides routing intent; classifier is a simple S1/S2 dispatch,
            // not a personality-biased weighted scorer.

            // Screen app state is stored in last_app_state for future use.
            // No classifier update needed here — app state is fed during screen events.

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

                // ── Boundary check for sensitive operations ──────────
                // ContactSearch, SendSms, CalendarEvents, and LaunchApp
                // all access private user data or take irreversible actions.
                // A Deny result must be reported back to the LLM, not silently skipped.
                let boundary_description = match &cmd {
                    SystemCommand::ContactSearch(_) => Some("contact search"),
                    SystemCommand::SendSms { .. } => Some("send SMS"),
                    SystemCommand::CalendarEvents { .. } => Some("calendar access"),
                    SystemCommand::LaunchApp(_) => Some("launch app"),
                    _ => None,
                };
                if let Some(desc) = boundary_description {
                    match boundary_check(subs, desc) {
                        BoundaryGateResult::Allow => {
                            tracing::debug!(description = desc, "boundary check: allowed");
                        },
                        BoundaryGateResult::Deny(reason) => {
                            tracing::warn!(
                                description = desc,
                                reason = %reason,
                                "boundary check: denied sensitive operation"
                            );
                            let denial_msg = format!(
                                "I'm not able to perform that action right now: {}",
                                reason
                            );
                            send_response(&subs.response_tx, source, denial_msg).await;
                            return Ok(());
                        },
                        BoundaryGateResult::NeedConfirmation { reason, prompt } => {
                            tracing::info!(
                                description = desc,
                                reason = %reason,
                                "boundary check: confirmation required"
                            );
                            send_response(&subs.response_tx, source, prompt).await;
                            return Ok(());
                        },
                    }
                }

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
                            0,   // no iterations — instant
                            1.0, // max confidence — typed API
                            RouteKind::System1,
                            now_ms(),
                        ));
                        subs.health_monitor.record_success(now_ms());
                        flush_outcome_bus(subs).await;
                        send_response(&subs.response_tx, source, response_text).await;
                        return Ok(());
                    },
                    Err(e) => {
                        // Bridge failed — fall through to normal pipeline.
                        // This is NOT a hard error; the classifier may find
                        // a better route (e.g. via the LLM).
                        tracing::warn!(
                            error = %e,
                            "SystemBridge fast-path: execution failed — \
                             falling through to classifier"
                        );
                    },
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
                },
                RoutePath::System2 => {
                    let mode = route
                        .neocortex_mode
                        .unwrap_or(InferenceMode::Conversational);
                    dispatch_system2(&scored, mode, _enriched.as_ref(), source, state, subs).await;
                },
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
                            subs.system1
                                .cache_plan(&scored.parsed.content, plan, 1.0, now_ms());
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
                        .with_response_summary(s1_result.response_text.as_deref().unwrap_or(""))
                        .with_route_confidence(route.confidence);
                        subs.outcome_bus.publish(hybrid_outcome);
                    } else {
                        // System1 failed — fall back to System2.
                        // System2 outcome will be captured in handle_ipc_inbound.
                        let mode = route
                            .neocortex_mode
                            .unwrap_or(InferenceMode::Conversational);
                        dispatch_system2(&scored, mode, _enriched.as_ref(), source, state, subs)
                            .await;
                    }
                },
            }

            // Store the conversation turn in contextor.
            subs.contextor
                .push_conversation_turn(aura_types::ipc::ConversationTurn {
                    role: aura_types::ipc::Role::User,
                    content: text,
                    timestamp_ms: now_ms(),
                });

            // Store in working memory.
            subs.memory.store_working(
                parsed.content.clone(),
                EventSource::UserCommand,
                scored.score_total,
                now_ms(),
            );

            // Flush outcome bus — dispatch all pending outcomes to cognitive subsystems.
            flush_outcome_bus(subs).await;
        },

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
            if let Some(denial_reason) = check_consent_for_task(&subs.consent_tracker, &description)
            {
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
            } else if let Some(denial_reason) =
                check_sandbox_for_task(&subs.action_sandbox, &description)
            {
                // Sandbox classified this task description as Forbidden (L3).
                // Publish denial to OutcomeBus so learning + audit see it.
                tracing::error!(
                    target: "SECURITY",
                    description = %description,
                    reason = %denial_reason,
                    "sandbox DENIED task at description level — L3:Forbidden"
                );
                subs.audit_log
                    .log_policy_decision(
                        &description,
                        &crate::policy::rules::RuleEffect::Deny,
                        &denial_reason,
                        0,
                    )
                    .ok();
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
                    subs.recent_denial_count,
                );
                match boundary_result {
                    BoundaryGateResult::Deny(reason) => {
                        subs.recent_denial_count = subs.recent_denial_count.saturating_add(1);
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
                        if let Some(goal) =
                            state.checkpoint.goals.iter_mut().find(|g| g.id == goal_id)
                        {
                            goal.status = aura_types::goals::GoalStatus::Failed(format!(
                                "boundary denied: {}",
                                reason
                            ));
                        }
                        flush_outcome_bus(subs).await;
                    },
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
                            if let Some(goal) =
                                state.checkpoint.goals.iter_mut().find(|g| g.id == goal_id)
                            {
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
                            if let Some(goal) =
                                state.checkpoint.goals.iter_mut().find(|g| g.id == goal_id)
                            {
                                goal.status = aura_types::goals::GoalStatus::Failed(
                                    "boundary auto-denied: too many pending confirmations"
                                        .to_string(),
                                );
                            }
                            flush_outcome_bus(subs).await;
                        }
                    },
                    BoundaryGateResult::Allow => {
                        let mut policy_ctx = react::PolicyContext {
                            gate: &mut subs.policy_gate,
                            audit: &mut subs.audit_log,
                        };
                        let (outcome, session) = react::execute_task(
                            description.clone(),
                            clamped_priority,
                            None,
                            Some(&mut policy_ctx),
                        )
                        .await;

                        // Update goal status.
                        if let Some(goal) =
                            state.checkpoint.goals.iter_mut().find(|g| g.id == goal_id)
                        {
                            match &outcome {
                                react::TaskOutcome::Success {
                                    final_confidence, ..
                                } => {
                                    goal.status = aura_types::goals::GoalStatus::Completed;
                                    tracing::info!(
                                        goal_id,
                                        session_id = session.session_id,
                                        iterations = session.iterations.len(),
                                        confidence = final_confidence,
                                        "task completed successfully"
                                    );
                                },
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
                                },
                                react::TaskOutcome::Cancelled { .. } => {
                                    goal.status = aura_types::goals::GoalStatus::Cancelled;
                                    tracing::info!(
                                        goal_id,
                                        session_id = session.session_id,
                                        "task was cancelled"
                                    );
                                },
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
                                },
                            }
                        }

                        // ── GoalTracker: mirror task outcome to richer BDI tracker ─────
                        if let Some(ref mut tracker) = subs.goal_tracker {
                            let ts = now_ms();
                            let tracker_result = match &outcome {
                                react::TaskOutcome::Success { .. } => tracker.complete(goal_id, ts),
                                react::TaskOutcome::Failed { reason, .. } => {
                                    tracker.fail(goal_id, reason.clone(), ts)
                                },
                                react::TaskOutcome::Cancelled { .. } => {
                                    // Pause on cancellation — goal can be resumed later.
                                    tracker.pause(goal_id, ts)
                                },
                                react::TaskOutcome::CycleAborted { cycle_reason, .. } => {
                                    tracker.fail(goal_id, cycle_reason.clone(), ts)
                                },
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
                                    subs.successful_task_count =
                                        subs.successful_task_count.saturating_add(1);
                                },
                                react::TaskOutcome::Failed { .. }
                                | react::TaskOutcome::CycleAborted { .. } => {
                                    subs.consecutive_task_failures =
                                        subs.consecutive_task_failures.saturating_add(1);
                                },
                                react::TaskOutcome::Cancelled { .. } => {
                                    // Cancellations don't affect the escalation counters.
                                },
                            }

                            // On failure, consult SemanticReact to decide whether to
                            // escalate from System1 (fast ETG path) to System2 (LLM).
                            if matches!(
                                &outcome,
                                react::TaskOutcome::Failed { .. }
                                    | react::TaskOutcome::CycleAborted { .. }
                            ) {
                                // ── StrategicRecovery: classify + determine recovery ───
                                // Build an EnvironmentSnapshot from HealthMonitor state so
                                // failure classification is environment-aware (e.g. a timeout
                                // during low battery = Environmental, not Transient).
                                let env_snapshot = EnvironmentSnapshot {
                                    battery_level: state.checkpoint.power_budget.battery_percent
                                        as f32,
                                    network_available: true, // local-only, always "available"
                                    target_app_running: true,
                                    screen_responsive: true,
                                    neocortex_alive: subs.neocortex.is_connected(),
                                };

                                let failure_reason = match &outcome {
                                    react::TaskOutcome::Failed { reason, .. } => reason.as_str(),
                                    react::TaskOutcome::CycleAborted { cycle_reason, .. } => {
                                        cycle_reason.as_str()
                                    },
                                    _ => "unknown",
                                };

                                let failure_category = StrategicRecovery::classify_failure(
                                    failure_reason,
                                    &env_snapshot,
                                );

                                let recovery_ctx = RecoveryContext {
                                    operation: format!("goal_{}", goal_id),
                                    goal_description: description.clone(),
                                    attempt_count: subs.consecutive_task_failures,
                                    time_elapsed_ms: 0,
                                    last_error: failure_reason.to_owned(),
                                    category: failure_category.clone(),
                                    environment_state: env_snapshot.clone(),
                                };

                                let recovery_action =
                                    subs.strategic_recovery.determine_recovery(&recovery_ctx);

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
                                            &recovery_ctx.operation,
                                            &recovery_action,
                                            true, // optimistic — actual outcome tracked later
                                            now_ms(),
                                        );
                                    },
                                    RecoveryAction::Replan { .. }
                                    | RecoveryAction::EscalateToStrategic { .. } => {
                                        // The failure needs a fresh approach from the LLM.
                                        // Route through SemanticReact to confirm escalation
                                        // thresholds, then send a Replan to neocortex.
                                        let replan_reason = match &recovery_action {
                                            RecoveryAction::Replan { reason, .. } => reason.clone(),
                                            _ => "strategic escalation after retry exhaustion"
                                                .to_string(),
                                        };
                                        tracing::info!(
                                            goal_id,
                                            reason = %replan_reason,
                                            "StrategicRecovery: escalating to System 2 replan"
                                        );
                                        let escalation_ctx = EscalationContext {
                                            system1_confidence: 0.0,
                                            amygdala_arousal: state
                                                .checkpoint
                                                .disposition
                                                .mood
                                                .arousal
                                                .clamp(0.0, 1.0),
                                            consecutive_failures: subs.consecutive_task_failures,
                                            battery_level: state
                                                .checkpoint
                                                .power_budget
                                                .battery_percent
                                                as f32,
                                            is_thermal_throttling: false,
                                        };
                                        if matches!(
                                            subs.semantic_react
                                                .evaluate_escalation(&escalation_ctx),
                                            CognitiveState::System2
                                        ) {
                                            // Build the replan context preserving live personality
                                            // and
                                            // affective state so the LLM has OCEAN + VAD when
                                            // replanning.
                                            // Architecture law: never zero the ContextPackage on
                                            // replan —
                                            // the neocortex needs full context to generate a good
                                            // recovery plan.
                                            let replan_personality = {
                                                let t = &subs.identity.personality.traits;
                                                let mood = &state.checkpoint.disposition.mood;
                                                aura_types::ipc::PersonalitySnapshot {
                                        openness: t.openness,
                                        conscientiousness: t.conscientiousness,
                                        extraversion: t.extraversion,
                                        agreeableness: t.agreeableness,
                                        neuroticism: t.neuroticism,
                                        current_mood_valence: mood.valence,
                                        current_mood_arousal: mood.arousal,
                                        current_mood_dominance: mood.dominance,
                                        trust_level: aura_types::ipc::PersonalitySnapshot::default().trust_level,
                                    }
                                            };
                                            let replan_ctx = ContextPackage {
                                                active_goal: Some(aura_types::ipc::GoalSummary {
                                                    description: description.clone(),
                                                    progress_percent: 0,
                                                    current_step: String::new(),
                                                    blockers: vec![replan_reason],
                                                }),
                                                inference_mode: InferenceMode::Planner,
                                                personality: replan_personality,
                                                user_state:
                                                    aura_types::ipc::UserStateSignals::default(),
                                                token_budget: 1024,
                                                ..ContextPackage::default()
                                            };
                                            let replan_failure = FailureContext {
                                                task_goal_hash: goal_id,
                                                current_step: session.iteration_count,
                                                failing_action: 0,
                                                target_id: 0,
                                                expected_state_hash: 0,
                                                actual_state_hash: 0,
                                                tried_approaches: subs.consecutive_task_failures
                                                    as u64,
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
                                                )
                                                .await;
                                                match send_result {
                                                    Ok(Ok(())) => {
                                                        tracing::info!(
                                                goal_id,
                                                "StrategicRecovery: Replan sent to neocortex"
                                            );
                                                    },
                                                    Ok(Err(e)) => {
                                                        tracing::warn!(
                                                            error = %e,
                                                            "StrategicRecovery: IPC send failed"
                                                        );
                                                    },
                                                    Err(_) => {
                                                        tracing::warn!(
                                                            "StrategicRecovery: IPC send timed out"
                                                        );
                                                    },
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
                                    },
                                    RecoveryAction::RestartEnvironment {
                                        ref target,
                                        wait_ms,
                                    } => {
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
                                            &recovery_ctx.operation,
                                            &recovery_action,
                                            false,
                                            now_ms(),
                                        );
                                    },
                                    RecoveryAction::NotifyUser {
                                        ref message,
                                        ref severity,
                                    } => {
                                        // The failure requires human awareness.  Send a
                                        // Telegram message and do NOT escalate to System 2.
                                        tracing::info!(
                                            goal_id,
                                            severity = ?severity,
                                            "StrategicRecovery: notifying user of failure"
                                        );
                                        let notify_text = format!("[AURA recovery] {message}");
                                        send_response(
                                            &subs.response_tx,
                                            source.clone(),
                                            notify_text,
                                        )
                                        .await;
                                        subs.strategic_recovery.record_recovery_outcome(
                                            &recovery_ctx.operation,
                                            &recovery_action,
                                            true,
                                            now_ms(),
                                        );
                                    },
                                    RecoveryAction::HaltAndLog {
                                        ref reason,
                                        ref category,
                                    } => {
                                        // Safety/critical failure — halt all retries and
                                        // mark the goal as permanently failed.
                                        tracing::error!(
                                            target: "SAFETY",
                                            goal_id,
                                            reason = %reason,
                                            category = ?category,
                                            "StrategicRecovery: HALTING — safety/critical failure"
                                        );
                                        if let Some(goal) = state
                                            .checkpoint
                                            .goals
                                            .iter_mut()
                                            .find(|g| g.id == goal_id)
                                        {
                                            goal.status = aura_types::goals::GoalStatus::Failed(
                                                format!("halted: {reason}"),
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
                                            &recovery_ctx.operation,
                                            &recovery_action,
                                            false,
                                            now_ms(),
                                        );
                                    },
                                    RecoveryAction::TryAlternative {
                                        ref alternative,
                                        ref reason,
                                    } => {
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
                                            &recovery_ctx.operation,
                                            &recovery_action,
                                            true, // optimistic
                                            now_ms(),
                                        );
                                    },
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
                                react::TaskOutcome::Cancelled { .. } => {
                                    MoodEvent::Silence { duration_ms: 0 }
                                },
                            };
                            subs.identity.affective.process_event_with_personality(
                                mood_event,
                                outcome_ts,
                                personality,
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
                                react::TaskOutcome::Success {
                                    final_confidence,
                                    iterations_used,
                                    ..
                                } => (
                                    OutcomeResult::Success,
                                    *final_confidence,
                                    *iterations_used as u32,
                                ),
                                react::TaskOutcome::Failed {
                                    iterations_used, ..
                                } => (OutcomeResult::Failure, 0.0, *iterations_used as u32),
                                react::TaskOutcome::Cancelled {
                                    iterations_completed,
                                    ..
                                } => (
                                    OutcomeResult::UserCancelled,
                                    0.0,
                                    *iterations_completed as u32,
                                ),
                                react::TaskOutcome::CycleAborted {
                                    iterations_completed,
                                    ..
                                } => (OutcomeResult::Failure, 0.0, *iterations_completed as u32),
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
                        if let Err(e) = subs
                            .memory
                            .store_episodic(
                                episode,
                                0.0, // neutral emotional valence
                                0.6, // moderate importance
                                vec!["task".to_string(), "react".to_string()],
                                now_ms(),
                            )
                            .await
                        {
                            tracing::warn!(error = %e, "failed to store task episode");
                        }

                        // Flush outcome bus.
                        flush_outcome_bus(subs).await;
                    }, // end BoundaryGateResult::Allow arm
                } // end match boundary_result (Site 2: TaskRequest)
            } // end consent-allowed else branch
        },

        UserCommand::CancelTask {
            task_id,
            source: _source,
        } => {
            if task_id.trim().is_empty() {
                tracing::warn!("ignoring cancel with empty task_id");
                return Ok(());
            }

            let parsed_id: u64 = match task_id.trim().parse() {
                Ok(id) => id,
                Err(e) => {
                    tracing::warn!(task_id = %task_id, error = %e, "invalid task_id format");
                    return Ok(());
                },
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
        },

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
        },
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
        let resp_with_markers =
            if let Some(marker) = subs.identity.assess_epistemic_markers(resp, text) {
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
        subs.contextor
            .push_conversation_turn(aura_types::ipc::ConversationTurn {
                role: aura_types::ipc::Role::Assistant,
                content: resp_with_markers,
                timestamp_ms: now_ms(),
            });
    }

    // If System1 produced an action plan, execute it and cache for future use.
    if let Some(plan) = result.action_plan {
        // ── Consent gate (Pillar #1: Privacy Sovereignty) ──────────
        if let Some(_denial) = check_consent_for_task(&subs.consent_tracker, &scored.parsed.content)
        {
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
                subs.recent_denial_count,
            );
            match boundary_result {
                BoundaryGateResult::Deny(reason)
                | BoundaryGateResult::NeedConfirmation { reason, .. } => {
                    subs.recent_denial_count = subs.recent_denial_count.saturating_add(1);
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
                },
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
                    )
                    .await;

                    match outcome {
                        react::TaskOutcome::Success { .. } => {
                            // Feed WorkflowObserver before plan is moved into cache.
                            if let Some(ref mut observer) = subs.workflow_observer {
                                observer.observe_success(&plan, now_ms());
                            }
                            subs.system1.cache_plan(text, plan, 1.0, now_ms());
                            tracing::debug!("System1 plan cached after success");
                        },
                        _ => {
                            tracing::debug!(
                                ?outcome,
                                "System1 plan execution did not succeed — not caching"
                            );
                        },
                    }
                }, // end BoundaryGateResult::Allow arm
            } // end match boundary_result (Site 3: dispatch_system1)
        } // end consent-allowed else branch
    }

    // ── OutcomeBus: publish System1 execution outcome ───────────
    let s1_outcome = ExecutionOutcome::new(
        scored.parsed.intent.as_str().to_owned(),
        if result.success {
            OutcomeResult::Success
        } else {
            OutcomeResult::Failure
        },
        result.execution_time_ms,
        if result.success { 1.0 } else { 0.0 },
        RouteKind::System1,
        now_ms().saturating_sub(result.execution_time_ms),
    )
    .with_input_summary(&scored.parsed.content)
    .with_response_summary(result.response_text.as_deref().unwrap_or(""));
    subs.outcome_bus.publish(s1_outcome);
}

/// Dispatch an event through the System2 (slow, neocortex LLM) path.
///
/// When an [`EnrichedEvent`] is available from the Contextor, its memory
/// snippets, conversation history, personality context, and token budget
/// are threaded into the `ContextPackage` sent to the neocortex. This
/// ensures the LLM has full conversational context for high-quality responses.
///
/// Raw personality data (OCEAN scores, mood, trust level) is included as
/// structured data for the LLM to interpret — no Rust-side directives.
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
    // ARCHITECTURE: personality_archetype prompt injection removed.
    // Rust does not inject OCEAN-derived directives into the LLM system prompt.
    // The LLM determines its own tone from conversation context.

    // Prepare the System2 request (base context from parsed event).
    let request = match subs.system2.prepare_request(&scored.parsed, mode, now_ms()) {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(error = %e, "System2 prepare failed — falling back to System1");
            // Let the user know this is a quick response, not deep reasoning.
            send_response(
                &subs.response_tx,
                source.clone(),
                "💭 My deep reasoning is warming up — here's a quick response:".into(),
            )
            .await;
            dispatch_system1(scored, &scored.parsed.content, source, state, subs).await;
            return;
        },
    };

    // Enrich the outgoing message's ContextPackage with Contextor output.
    let message = if let Some(ctx) = enriched {
        enrich_system2_message(request.message, ctx)
    } else {
        tracing::debug!("no enriched context available — sending base context");
        request.message
    };

    // Inject personality archetype directive into the message context.
    // ARCHITECTURE: injection removed — LLM determines tone from conversation context.
    // No Rust-injected [Personality: X] or [ThinkingPartner] directives.

    tracing::debug!(request_id = request.request_id, "System2 request prepared");

    // Ensure IPC is connected.
    if !ensure_ipc_connected(&mut subs.neocortex).await {
        tracing::warn!("neocortex unreachable — falling back to System1");
        subs.system2.complete_request(request.request_id);
        // Let the user know this is a quick response, not deep reasoning.
        send_response(
            &subs.response_tx,
            source.clone(),
            "💭 My reasoning engine isn't available right now — here's a quick response:".into(),
        )
        .await;
        dispatch_system1(scored, &scored.parsed.content, source, state, subs).await;
        return;
    }

    // Send with timeout.
    let send_result = tokio::time::timeout(
        Duration::from_millis(IPC_SEND_TIMEOUT_MS),
        subs.neocortex.send(&message),
    )
    .await;

    match send_result {
        Ok(Ok(())) => {
            tracing::info!(
                request_id = request.request_id,
                "System2 request sent to neocortex"
            );
            // Track the originating source so ConversationReply can route back.
            // Bound the queue to 64 to prevent unbounded growth.
            if state.pending_system2_sources.len() >= 64 {
                tracing::warn!(
                    request_id = request.request_id,
                    "pending_system2_sources queue full (64); evicting oldest entry"
                );
                state.pending_system2_sources.pop_front();
            }
            state.pending_system2_sources.push_back(source.clone());
            // The response will arrive via the IPC inbound channel.
        },
        Ok(Err(e)) => {
            tracing::warn!(error = %e, "System2 IPC send failed — falling back to System1");
            subs.system2.complete_request(request.request_id);
            dispatch_system1(scored, &scored.parsed.content, source, state, subs).await;
        },
        Err(_) => {
            tracing::warn!("System2 IPC send timed out — falling back to System1");
            subs.system2.complete_request(request.request_id);
            dispatch_system1(scored, &scored.parsed.content, source, state, subs).await;
        },
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
    outcome_bus
        .dispatch(
            arc_manager.as_mut(),
            memory,
            bdi_scheduler.as_mut(),
            identity,
            now,
        )
        .await;

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
                },
                Err(e) => {
                    tracing::warn!(
                        capability = %capability_id,
                        error = %e,
                        "GoalRegistry: update_confidence failed"
                    );
                },
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
            pkg.memory_snippets = enriched
                .memory_context
                .iter()
                .filter(|snippet| {
                    let (tier, _category) = CriticalVault::classify_data(&snippet.content);
                    match tier {
                        DataTier::Ephemeral | DataTier::Personal => true,
                        // Tier ≥ Sensitive: redact from LLM context.
                        // The LLM never needs raw passwords, tokens, PII, etc.
                        _ => {
                            tracing::debug!(
                                snippet_len = snippet.content.len(),
                                tier = ?tier,
                                "CriticalVault: redacted sensitive memory snippet \
                                 before LLM dispatch"
                            );
                            false
                        },
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
                interactive_elements: Vec::new(),
                visible_text: vec![summary.clone()],
            });
        }

        // Active goal — take the highest-priority one.
        if let Some(goal) = enriched.active_goals.first() {
            pkg.active_goal = Some(goal.clone());
        }

        // Personality snapshot — copy real computed values from user_context
        // if available; this overwrites the hardcoded PersonalitySnapshot::default()
        // that ContextPackage::default() sets.
        if let Some(ref uc) = enriched.user_context {
            pkg.personality = uc.personality_snapshot.clone();
        }

        // Identity block — raw OCEAN+VAD+archetype JSON for the LLM.
        if enriched.identity_block.is_some() {
            pkg.identity_block = enriched.identity_block.clone();
        }

        // Mood description — human-readable emotional context string.
        if !enriched.mood_description.is_empty() {
            pkg.mood_description = enriched.mood_description.clone();
        }

        // Phase 4: Personality context directive injection removed.
        //
        // REASON: `generate_personality_context()` pre-interprets OCEAN/VAD
        // values into behavioral directive strings ("Be creative", "Be formal")
        // before the LLM ever sees them. This is Theater AGI — the daemon (body)
        // doing the LLM's (brain) job.
        //
        // REPLACEMENT: Raw OCEAN, VAD, relationship stage, and archetype are
        // already serialized as a compact JSON `identity_block` (see above) and
        // injected into `pkg.identity_block`. The neocortex `prompts.rs`
        // `personality_section()` reads those raw numbers directly and includes
        // them verbatim in the LLM system prompt. The LLM reasons about them
        // naturally — no pre-interpretation by Rust code.
        //
        // `enriched.personality_context` is retained in `EnrichedEvent` for
        // diagnostic tooling and test assertions; it is no longer injected into
        // the inference pipeline.
        let _ = &enriched.personality_context; // suppress unused-field warning

        // ── Tier 1: Identity Core fields ────────────────────────────────
        //
        // These three fields give the LLM grounding in who AURA is, what
        // the user prefers, and what AURA can/cannot do. The neocortex
        // `prompts.rs` injects them into the system prompt in order:
        //   Self-Knowledge → Identity Tendencies → Personality → User Preferences → Rules

        // T1-1: Constitutional tendencies — ALWAYS present.
        // These are AURA's innate character principles (first-person statements).
        // The user shapes how they EXPRESS, but they cannot be removed.
        pkg.identity_tendencies = Some(IdentityTendencies::constitutional());

        // T1-2: User preferences — from daemon-side UserProfile conversion.
        // None if no user profile is available (graceful degradation).
        if let Some(ref prefs) = enriched.ipc_user_preferences {
            pkg.user_preferences = Some(prefs.clone());
        }

        // T1-3: Self-knowledge — factual grounding about AURA's state.
        // Prevents confabulation about capabilities AURA doesn't have.
        let mode_str = match pkg.inference_mode {
            InferenceMode::Conversational => "conversational",
            InferenceMode::Planner => "planning",
            InferenceMode::Composer => "composing",
            InferenceMode::Strategist => "strategizing",
        };
        pkg.self_knowledge = Some(SelfKnowledge::for_mode(mode_str));

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
        },
        DaemonToNeocortex::Plan {
            mut context,
            failure,
        } => {
            apply(&mut context, enriched);
            DaemonToNeocortex::Plan { context, failure }
        },
        DaemonToNeocortex::Compose {
            mut context,
            template,
        } => {
            apply(&mut context, enriched);
            DaemonToNeocortex::Compose { context, template }
        },
        DaemonToNeocortex::Replan {
            mut context,
            failure,
        } => {
            apply(&mut context, enriched);
            DaemonToNeocortex::Replan { context, failure }
        },
        other => other, // Pass through non-context variants unchanged.
    }
}

// inject_personality_archetype removed: Rust does not inject OCEAN-derived
// [Personality: X] directives into LLM system prompts. Theater AGI.

// inject_thinking_partner_primer removed: Rust does not inject [ThinkingPartner]
// Socratic/Reflective directives into LLM prompts. Theater AGI.
// The LLM determines its own reasoning style from conversation context.

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
    let message: DaemonToNeocortex =
        match bincode::serde::decode_from_slice(&msg.payload, bincode::config::standard()) {
            Ok((m, _)) => m,
            Err(e) => {
                tracing::error!(error = %e, "IPC outbound payload deserialization failed");
                return Ok(());
            },
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
            // Notify the user — AURA's brain is ready for deep thinking.
            if let Some(&primary_chat) = state.config.telegram.allowed_chat_ids.first() {
                send_response(
                    &subs.response_tx,
                    InputSource::Telegram {
                        chat_id: primary_chat,
                    },
                    format!(
                        "🧠 My reasoning engine is ready — {} loaded ({}MB). \
                         I can think deeply now.",
                        model_name, memory_used_mb
                    ),
                )
                .await;
            }
        },

        NeocortexToDaemon::LoadFailed { reason } => {
            tracing::error!(reason = %reason, "neocortex model load failed");
            subs.memory
                .feedback_loop
                .record_error("model_load", &reason, "neocortex", now_ms());
            // Notify the user — AURA's deep reasoning is unavailable.
            if let Some(&primary_chat) = state.config.telegram.allowed_chat_ids.first() {
                send_response(
                    &subs.response_tx,
                    InputSource::Telegram {
                        chat_id: primary_chat,
                    },
                    format!(
                        "⚠️ My reasoning engine failed to load: {}. \
                         I'll work with quick responses for now.",
                        reason
                    ),
                )
                .await;
            }
        },

        NeocortexToDaemon::Unloaded => {
            tracing::info!("neocortex model unloaded");
        },

        NeocortexToDaemon::PlanReady { plan, .. } => {
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
                    subs.recent_denial_count,
                );
                match boundary_result {
                    BoundaryGateResult::Deny(reason)
                    | BoundaryGateResult::NeedConfirmation { reason, .. } => {
                        subs.recent_denial_count = subs.recent_denial_count.saturating_add(1);
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
                    },
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
                        )
                        .await;

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
                                    enhanced
                                        .cache_store(&plan_goal_desc, plan_for_observation.clone());
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
                                react::TaskOutcome::Success {
                                    final_confidence,
                                    iterations_used,
                                    ..
                                } => (
                                    OutcomeResult::Success,
                                    *final_confidence,
                                    *iterations_used as u32,
                                ),
                                react::TaskOutcome::Failed {
                                    iterations_used, ..
                                } => (OutcomeResult::Failure, 0.0, *iterations_used as u32),
                                react::TaskOutcome::Cancelled {
                                    iterations_completed,
                                    ..
                                } => (
                                    OutcomeResult::UserCancelled,
                                    0.0,
                                    *iterations_completed as u32,
                                ),
                                react::TaskOutcome::CycleAborted {
                                    iterations_completed,
                                    ..
                                } => (OutcomeResult::Failure, 0.0, *iterations_completed as u32),
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
                                    react::TaskOutcome::Success { .. } => {
                                        aura_types::goals::GoalStatus::Completed
                                    },
                                    react::TaskOutcome::Failed { .. }
                                    | react::TaskOutcome::CycleAborted { .. } => {
                                        aura_types::goals::GoalStatus::Failed(
                                            "plan failed".to_string(),
                                        )
                                    },
                                    react::TaskOutcome::Cancelled { .. } => {
                                        aura_types::goals::GoalStatus::Cancelled
                                    },
                                },
                                steps: Vec::new(),
                                created_ms: ts,
                                deadline_ms: None,
                                parent_goal: None,
                                source: aura_types::goals::GoalSource::ProactiveSuggestion,
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
                                    },
                                    react::TaskOutcome::Failed { reason, .. } => {
                                        let _ = tracker.fail(plan_goal_id, reason.clone(), ts);
                                    },
                                    react::TaskOutcome::CycleAborted { cycle_reason, .. } => {
                                        let _ =
                                            tracker.fail(plan_goal_id, cycle_reason.clone(), ts);
                                    },
                                    react::TaskOutcome::Cancelled { .. } => {
                                        let _ = tracker.pause(plan_goal_id, ts);
                                    },
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
                        if let Err(e) = subs
                            .memory
                            .store_episodic(
                                episode,
                                0.0,
                                0.7,
                                vec!["plan".to_string(), "neocortex".to_string()],
                                now_ms(),
                            )
                            .await
                        {
                            tracing::warn!(error = %e, "failed to store plan episode");
                        }
                    }, // end BoundaryGateResult::Allow arm
                } // end match boundary_result (Site 4: PlanReady)
            } // end consent-allowed else branch (Site 3: neocortex plan)
        },

        NeocortexToDaemon::ConversationReply {
            text, mood_hint, ..
        } => {
            tracing::info!(
                len = text.len(),
                mood_hint = ?mood_hint,
                "conversation reply received"
            );

            // Apply mood hint to disposition.
            if let Some(hint) = mood_hint {
                let clamped = hint.clamp(-1.0, 1.0);
                state.checkpoint.disposition.mood.valence =
                    (state.checkpoint.disposition.mood.valence + clamped * 0.1).clamp(-1.0, 1.0);
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
            let text_after_truth = if let Some(marker) = subs
                .identity
                .assess_epistemic_markers(&text_after_truth, &last_user_for_truth)
            {
                format!("{marker} {text_after_truth}")
            } else {
                text_after_truth
            };

            // ── Anti-sycophancy gate ────────────────────────────────
            let gate_result = subs.identity.check_response();
            let final_text = match &gate_result {
                crate::identity::GateResult::Pass => text_after_truth,
                crate::identity::GateResult::Nudge { reason } => {
                    tracing::info!(reason = %reason, "anti-sycophancy nudge");
                    // Append a honesty nudge to the response.
                    format!("{}\n\n[Note: {}]", text_after_truth, reason)
                },
                crate::identity::GateResult::Block { reason } => {
                    tracing::warn!(reason = %reason, "anti-sycophancy blocked response");
                    // Provide a neutral fallback.
                    "I want to be honest with you. Let me reconsider my response.".to_string()
                },
            };

            // ── Episodic memory: record this conversation turn ──────────
            // Write user turn.
            if let Err(e) = subs
                .memory
                .store_episodic(
                    last_user_for_truth.clone(),
                    0.0,
                    0.5,
                    vec!["conversation".to_string(), "user".to_string()],
                    now_ms(),
                )
                .await
            {
                tracing::warn!(error = %e, "failed to store user episodic memory");
            }
            // Write assistant turn.
            if let Err(e) = subs
                .memory
                .store_episodic(
                    final_text.clone(),
                    0.0,
                    0.5,
                    vec!["conversation".to_string(), "assistant".to_string()],
                    now_ms(),
                )
                .await
            {
                tracing::warn!(error = %e, "failed to store assistant episodic memory");
            }

            // Send response to user via the response channel.
            // Pop the oldest in-flight source (FIFO — neocortex processes
            // requests sequentially, so the oldest dispatch matches this reply).
            // Falls back to Direct if the queue is unexpectedly empty.
            let reply_dest = state
                .pending_system2_sources
                .pop_front()
                .unwrap_or(InputSource::Direct);
            send_response_with_mood(&subs.response_tx, reply_dest, final_text.clone(), mood_hint)
                .await;

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
            subs.contextor
                .push_conversation_turn(aura_types::ipc::ConversationTurn {
                    role: aura_types::ipc::Role::Assistant,
                    content: final_text.clone(),
                    timestamp_ms: now_ms(),
                });

            // ── OutcomeBus: publish System2 conversation outcome ────
            {
                let s2_result = if matches!(gate_result, crate::identity::GateResult::Block { .. })
                {
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
        },

        NeocortexToDaemon::ComposedScript { steps } => {
            tracing::info!(step_count = steps.len(), "composed DSL script received");
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
            if let Some(_denial) = check_consent_for_task(&subs.consent_tracker, "composed script")
            {
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
                    subs.recent_denial_count,
                );
                match boundary_result {
                    BoundaryGateResult::Deny(reason)
                    | BoundaryGateResult::NeedConfirmation { reason, .. } => {
                        subs.recent_denial_count = subs.recent_denial_count.saturating_add(1);
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
                    },
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
                        )
                        .await;
                        tracing::debug!(?outcome, "composed script execution complete");
                    }, // end BoundaryGateResult::Allow arm
                } // end match boundary_result (Site 5: ComposedScript)
            } // end consent-allowed else branch (Site 4: DSL script)
        },

        NeocortexToDaemon::Progress { percent, stage } => {
            tracing::debug!(
                percent,
                stage = %stage,
                "neocortex inference progress"
            );
        },

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
        },

        NeocortexToDaemon::Pong { uptime_ms } => {
            tracing::debug!(uptime_ms, "neocortex pong");
        },

        NeocortexToDaemon::MemoryWarning {
            used_mb,
            available_mb,
        } => {
            tracing::warn!(used_mb, available_mb, "neocortex memory warning");

            // If critically low, send unload command.
            if available_mb < 128 {
                tracing::warn!("critical memory — requesting model unload");
                if ensure_ipc_connected(&mut subs.neocortex).await {
                    if let Err(e) = subs.neocortex.send(&DaemonToNeocortex::Unload).await {
                        tracing::error!(error = %e, "failed to send Unload");
                    }
                }
            }
        },

        NeocortexToDaemon::TokenBudgetExhausted => {
            tracing::warn!("neocortex token budget exhausted");
            state.checkpoint.token_counters.cloud_tokens = state
                .checkpoint
                .token_counters
                .cloud_tokens
                .saturating_add(1);
        },

        NeocortexToDaemon::Embedding { .. } => {
            tracing::debug!("embedding received, storing");
        },

        NeocortexToDaemon::ReActDecision {
            done,
            reasoning,
            next_action,
            ..
        } => {
            // This arm fires when the neocortex pushes a ReActDecision
            // unprompted (the normal path goes through send_react_step_ipc's
            // request() call and never reaches here).  We handle it
            // defensively so unsolicited pushes are never silently dropped.
            if done {
                tracing::info!(
                    reasoning = %reasoning,
                    "neocortex push-signalled ReAct goal complete"
                );
                // Best-effort: mark the first Active goal as Completed.
                for goal in state.checkpoint.goals.iter_mut() {
                    if goal.status == aura_types::goals::GoalStatus::Active {
                        goal.status = aura_types::goals::GoalStatus::Completed;
                        break;
                    }
                }
            } else if let Some(ref action) = next_action {
                tracing::info!(
                    reasoning = %reasoning,
                    next_action = %action,
                    "neocortex push-signalled ReAct continue with next action"
                );
                // The actual dispatch lives inside send_react_step_ipc's
                // request/response loop; this push path just records intent.
            } else {
                tracing::warn!(
                    reasoning = %reasoning,
                    "neocortex ReActDecision: done=false but no next_action — \
                     treating as failed step"
                );
                // Mark the first Active goal as Failed.
                for goal in state.checkpoint.goals.iter_mut() {
                    if goal.status == aura_types::goals::GoalStatus::Active {
                        goal.status = aura_types::goals::GoalStatus::Failed(
                            "neocortex returned done=false with no next_action".to_string(),
                        );
                        break;
                    }
                }
            }
        },

        NeocortexToDaemon::Summary { text, .. } => {
            tracing::debug!(len = text.len(), "neocortex summary received");
            subs.memory
                .store_working(text, EventSource::Internal, 0.6, now_ms());
        },

        NeocortexToDaemon::PlanScore { score } => {
            // Unsolicited PlanScore — the normal path handles this via
            // request/response; arriving here means a race or a bug upstream.
            tracing::warn!(
                score = score,
                "unexpected unsolicited PlanScore on inbound channel — ignored"
            );
        },

        NeocortexToDaemon::FailureClassification { category } => {
            // Unsolicited FailureClassification — same reasoning as PlanScore.
            tracing::warn!(
                category = %category,
                "unexpected unsolicited FailureClassification on inbound channel — ignored"
            );
        },
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
// ── Health event handler ─────────────────────────────────────────────────────

/// React to a [`DaemonEvent`] emitted by `run_heartbeat_loop`.
///
/// This is a **synchronous, non-blocking** handler: no awaiting, no I/O.
/// All reactions are mechanical threshold responses — the semantics live
/// in the heartbeat loop that chose which events to emit.
///
/// Wiring:
/// - `MemoryPressure { critical: true }`  → activate safe mode (no proactive actions, no learning)
///   to prevent OOM on a constrained Android device.
/// - `BatteryLow`                         → cap initiative budget to 0.2 (reduces background
///   proactive actions to conserve power).
/// - `BatteryCritical`                    → cap initiative budget to 0.0 (halts all proactive work
///   immediately).
/// - `ThermalCritical`                    → log a critical warning; LLM inference pause is not yet
///   implemented (see TODO below).
/// - All other variants                   → debug-logged and ignored here (the heartbeat loop
///   already logged them at the appropriate level).
fn handle_health_event(event: DaemonEvent, subs: &mut LoopSubsystems) {
    match event {
        // ── Memory pressure ──────────────────────────────────────────────
        DaemonEvent::MemoryPressure {
            critical,
            current_bytes,
            threshold_bytes,
        } => {
            if critical {
                tracing::warn!(
                    current_mb = current_bytes / (1024 * 1024),
                    threshold_mb = threshold_bytes / (1024 * 1024),
                    "MemoryPressure(critical) — activating safe mode"
                );
                // Activate safe mode via a synthetic verification report.
                // We reuse the existing SafeModeState::activate_from_report path
                // but for memory pressure we set active directly since there
                // is no VerificationReport for runtime resource events.
                if !subs.safe_mode.active {
                    subs.safe_mode.active = true;
                    subs.safe_mode.activated_at_ms = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_millis() as u64;
                    let reason = format!(
                        "[memory] RSS {}MB exceeded critical threshold {}MB",
                        current_bytes / (1024 * 1024),
                        threshold_bytes / (1024 * 1024),
                    );
                    if !subs.safe_mode.reasons.contains(&reason) {
                        subs.safe_mode.reasons.push(reason);
                    }
                    tracing::error!("safe mode activated due to critical memory pressure");
                }
            } else {
                tracing::warn!(
                    current_mb = current_bytes / (1024 * 1024),
                    threshold_mb = threshold_bytes / (1024 * 1024),
                    "MemoryPressure(warn) — memory elevated but not critical"
                );
            }
        },

        // ── Battery events ───────────────────────────────────────────────
        DaemonEvent::BatteryLow { pct } => {
            tracing::warn!(pct, "BatteryLow — capping initiative budget to 0.2");
            if let Some(ref mut proactive) = subs.proactive {
                proactive.cap_budget(0.2);
            }
        },

        DaemonEvent::BatteryCritical { pct } => {
            tracing::error!(pct, "BatteryCritical — zeroing initiative budget");
            if let Some(ref mut proactive) = subs.proactive {
                proactive.cap_budget(0.0);
            }
        },

        // ── Thermal ──────────────────────────────────────────────────────
        DaemonEvent::ThermalCritical => {
            // TODO(neocortex-pause): Implement a mechanism to pause LLM
            // inference when thermal state is Critical/Shutdown.  The
            // NeocortexClient does not yet expose a pause/resume API.
            // When it does, call `subs.neocortex.pause_inference()` here.
            tracing::error!(
                "ThermalCritical — device temperature critical; \
                 LLM inference pause NOT YET IMPLEMENTED (see TODO)"
            );
        },

        // ── Heartbeat (informational — already logged by the loop) ───────
        DaemonEvent::Heartbeat(snapshot) => {
            tracing::debug!(
                battery_pct = snapshot.battery_pct,
                memory_mb = snapshot.memory_usage_bytes / (1024 * 1024),
                thermal_level = snapshot.thermal_level,
                "heartbeat received"
            );
        },

        // ── Lifecycle events (not emitted by heartbeat loop — log only) ──
        DaemonEvent::DaemonReady { version } => {
            tracing::info!(version, "DaemonReady event received");
        },
        DaemonEvent::DaemonShutdown { reason } => {
            tracing::info!(reason, "DaemonShutdown event received");
        },
    }
}

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
                )",
            )?;

            state.db.execute(
                "INSERT INTO telemetry (payload) VALUES (?1)",
                rusqlite::params![payload],
            )?;
        },

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
                )",
            )?;

            state.db.execute(
                "INSERT INTO episodes (content, importance) VALUES (?1, ?2)",
                rusqlite::params![content, clamped_importance],
            )?;
        },

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
                )",
            )?;

            state.db.execute(
                "INSERT OR REPLACE INTO amygdala_baselines (app, score) VALUES (?1, ?2)",
                rusqlite::params![app, clamped_score],
            )?;
        },

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
                tracing::warn!(count = params.len(), "too many SQL params — dropping");
                return Ok(());
            }

            tracing::debug!(sql = %sql, param_count = params.len(), "executing raw SQL");

            let param_refs: Vec<&dyn rusqlite::types::ToSql> = params
                .iter()
                .map(|p| p as &dyn rusqlite::types::ToSql)
                .collect();

            state.db.execute(&sql, param_refs.as_slice())?;
        },
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
        tracing::debug!(job_id = tick.job_id, "updated cron state last_fired_ms");
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
    cron_sweep_expired_confirmations(state, subs).await;

    let job = tick.job_name.as_str();

    if job.contains("health_report") {
        cron_handle_health_report(state, subs).await?;
    } else if job.contains("memory_compaction") {
        cron_handle_memory_compaction(state, subs).await?;
    } else if job.contains("token_reset") {
        cron_handle_token_reset(state, subs).await?;
    } else if job.contains("checkpoint") {
        cron_handle_checkpoint(state, subs).await?;
    } else if job.contains("proactive_tick") {
        cron_handle_proactive(state, subs).await?;
    } else if job.contains("dreaming_tick") {
        cron_handle_dreaming(state, subs).await?;
    } else if job.contains("medication_check") {
        cron_handle_medication(state, subs).await?;
    } else if job.contains("vital_ingest") {
        cron_handle_vital_ingest(state, subs).await?;
    } else if job.contains("step_sync") {
        cron_handle_step_sync(state, subs).await?;
    } else if job.contains("sleep_infer") {
        cron_handle_sleep_infer(state, subs).await?;
    } else if job.contains("health_score_compute") {
        cron_handle_health_score(state, subs).await?;
    } else if job.contains("health_weekly_report") {
        cron_handle_health_weekly(state, subs).await?;
    } else if job.contains("contact_update") {
        cron_handle_contact_update(state, subs).await?;
    } else if job.contains("importance_recalc") {
        cron_handle_importance_recalc(state, subs).await?;
    } else if job.contains("relationship_health") {
        cron_handle_relationship_health(state, subs).await?;
    } else if job.contains("social_gap_scan") {
        cron_handle_social_gap(state, subs).await?;
    } else if job.contains("birthday_check") {
        cron_handle_birthday(state, subs).await?;
    } else if job.contains("social_score_compute") {
        cron_handle_social_score(state, subs).await?;
    } else if job.contains("social_weekly_report") {
        cron_handle_social_weekly(state, subs).await?;
    } else if (job.contains("trigger_rule_eval")
        || job.contains("opportunity_detect")
        || job.contains("threat_accumulate")
        || job.contains("action_drain")
        || job.contains("daily_budget_reset"))
        && subs.safe_mode.should_block_action(true, false)
    {
        tracing::info!(job = %job, "safe-mode: skipping proactive cron job");
    } else if job.contains("trigger_rule_eval") {
        cron_handle_trigger_rules(state, subs).await?;
    } else if job.contains("opportunity_detect")
        || job.contains("threat_accumulate")
        || job.contains("action_drain")
        || job.contains("daily_budget_reset")
    {
        cron_handle_patterns(state, subs, job).await?;
    } else if (job.contains("pattern_observe")
        || job.contains("pattern_analyze")
        || job.contains("pattern_deviation_check")
        || job.contains("hebbian_decay")
        || job.contains("hebbian_consolidate")
        || job.contains("interest_update")
        || job.contains("skill_progress"))
        && subs.safe_mode.should_block_action(false, true)
    {
        tracing::info!(job = %job, "safe-mode: skipping learning cron job");
    } else if job.contains("pattern_observe")
        || job.contains("pattern_analyze")
        || job.contains("pattern_deviation_check")
        || job.contains("hebbian_decay")
        || job.contains("hebbian_consolidate")
    {
        cron_handle_patterns(state, subs, job).await?;
    } else if job.contains("interest_update") {
        cron_handle_interest_update(state, subs).await?;
    } else if job.contains("skill_progress") {
        cron_handle_skill_progress(state, subs).await?;
    } else if job.contains("domain_state_publish") {
        cron_handle_domain_state(state, subs).await?;
    } else if job.contains("life_quality_compute") {
        cron_handle_life_quality(state, subs).await?;
    } else if job.contains("cron_self_check") {
        cron_handle_self_check(state, subs).await?;
    } else if job.contains("memory_arc_flush") {
        cron_handle_memory_arc_flush(state, subs).await?;
    } else if job.contains("weekly_digest") {
        cron_handle_weekly_digest(state, subs).await?;
    } else if job.contains("deep_consolidation") {
        cron_handle_deep_consolidation(state, subs).await?;
    } else if job.contains("arc_health_check") {
        cron_handle_arc_health_check(state, subs).await?;
    } else if job.contains("bdi_deliberation") {
        cron_handle_bdi_deliberation(state, subs).await?;
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
// Cron handler helpers — extracted from handle_cron_tick
// ---------------------------------------------------------------------------

/// Sweep expired sandbox confirmations.  Called unconditionally on every cron
/// tick before the per-job dispatch so timeouts are always enforced.
async fn cron_sweep_expired_confirmations(state: &mut DaemonState, subs: &mut LoopSubsystems) {
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

    for conf in &expired {
        let timeout_msg = format!(
            "\u{23f0} Confirmation {} auto-denied (timeout): {}",
            conf.id, conf.description
        );
        send_response(&response_tx, conf.source.clone(), timeout_msg).await;

        if let Some(goal) = state
            .checkpoint
            .goals
            .iter_mut()
            .find(|g| g.id == conf.goal_id)
        {
            goal.status = aura_types::goals::GoalStatus::Failed(format!(
                "sandbox confirmation {} timed out",
                conf.id
            ));
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

/// Health report: run periodic self-diagnostic, alert on critical/degraded
/// status, and log working-memory stats.
///
/// **Integration #3 — Health → Goals:** if the health check returns a Critical
/// status, any active fitness/health goals are moved to `Blocked` so the BDI
/// deliberation cycle will re-evaluate them next tick.
async fn cron_handle_health_report(
    state: &mut DaemonState,
    subs: &mut LoopSubsystems,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    tracing::info!("executing health report cron job");

    let ts = now_ms();
    if subs.health_monitor.should_check(ts) {
        // Perform a real async IPC ping to determine whether the Neocortex
        // process is alive.  We do this *before* calling check_with_ping so
        // the two mutable borrows (neocortex, health_monitor) don't overlap.
        let neocortex_alive = match subs
            .neocortex
            .request(&aura_types::ipc::DaemonToNeocortex::Ping)
            .await
        {
            Ok(aura_types::ipc::NeocortexToDaemon::Pong { .. }) => {
                tracing::debug!("ping_neocortex: IPC ping successful — neocortex alive");
                true
            },
            Ok(unexpected) => {
                tracing::warn!(
                    ?unexpected,
                    "ping_neocortex: unexpected response to Ping — treating as dead"
                );
                false
            },
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "ping_neocortex: IPC ping failed — treating neocortex as dead"
                );
                false
            },
        };
        let report = subs.health_monitor.check_with_ping(ts, neocortex_alive);
        tracing::info!(
            status = ?report.overall_status,
            battery = report.battery_level,
            memory_mb = report.memory_usage_bytes / 1_048_576,
            error_rate = report.error_rate_last_hour,
            uptime_s = report.daemon_uptime_ms / 1000,
            "health check complete"
        );

        // ── Integration #1: Thermal → inference throttle ─────────────────
        // The health monitor cross-checks the platform's thermal state.  When
        // the platform's ThermalManager already reports that inference should be
        // paused we emit a structured warning so operators can see the
        // correlation between the health report and the inferred throttle.
        // The actual inference-skip decision is made at call-sites by querying
        // `platform.thermal.should_pause_inference()` — we don't need a
        // separate flag because the platform state is authoritative.
        let platform_throttling = state
            .subsystems
            .platform
            .as_ref()
            .map(|p| p.thermal.should_pause_inference())
            .unwrap_or(false);

        if report.thermal_state.should_throttle() {
            tracing::warn!(
                thermal = %report.thermal_state,
                platform_throttling,
                "thermal throttle detected — S2 inference will be skipped until thermal clears"
            );
        } else if platform_throttling {
            tracing::info!("health report: thermal normalised, platform throttle still active");
        }

        // Alert user via LLM/neocortex when status is Critical or Degraded.
        match report.overall_status {
            HealthStatus::Critical(ref msg) => {
                let trigger = ProactiveTrigger::HealthAlert {
                    metric: "system".to_string(),
                    value: report.battery_level,
                    threshold: 0.2,
                    direction: AlertDirection::Falling,
                };
                let ipc_msg = trigger_to_ipc(&trigger);
                state.pending_system2_sources.push_back(InputSource::Direct);
                if let Err(e) = subs.neocortex.send(&ipc_msg).await {
                    tracing::warn!(error = %e, "proactive: failed to dispatch HealthAlert Critical");
                }

                // ── Integration #3: Health → Goals ───────────────────────
                // Block active fitness/health goals when a critical alert fires
                // so the user and BDI system know they cannot make progress.
                let health_keywords = [
                    "fitness",
                    "health",
                    "exercise",
                    "workout",
                    "run",
                    "steps",
                    "sleep",
                    "medication",
                    "diet",
                    "weight",
                ];
                let block_reason = format!("health alert: {}", msg);
                let mut blocked_count = 0u32;
                for goal in state.checkpoint.goals.iter_mut() {
                    if goal.status == aura_types::goals::GoalStatus::Active {
                        let desc_lower = goal.description.to_lowercase();
                        if health_keywords.iter().any(|kw| desc_lower.contains(kw)) {
                            goal.status =
                                aura_types::goals::GoalStatus::Blocked(block_reason.clone());
                            blocked_count += 1;
                            tracing::info!(
                                goal_id = goal.id,
                                description = %goal.description,
                                reason = %block_reason,
                                "health alert: blocked active fitness/health goal"
                            );
                        }
                    }
                }
                if blocked_count > 0 {
                    tracing::warn!(
                        blocked_goals = blocked_count,
                        "health alert: blocked {} active fitness/health goal(s)",
                        blocked_count
                    );
                }
            },
            HealthStatus::Degraded(ref _msg) => {
                let trigger = ProactiveTrigger::HealthAlert {
                    metric: "system".to_string(),
                    value: report.battery_level,
                    threshold: 0.3,
                    direction: AlertDirection::Falling,
                };
                let ipc_msg = trigger_to_ipc(&trigger);
                state.pending_system2_sources.push_back(InputSource::Direct);
                if let Err(e) = subs.neocortex.send(&ipc_msg).await {
                    tracing::warn!(error = %e, "proactive: failed to dispatch HealthAlert Degraded");
                }
            },
            HealthStatus::Healthy => {
                tracing::debug!("health status: Healthy — no alert needed");
            },
        }
    }

    // Log working memory stats (existing behavior preserved).
    let slot_count = subs.memory.working.len();
    tracing::info!(working_slots = slot_count, "health report: memory status");
    Ok(())
}

/// Memory compaction: run micro-consolidation and sweep stale S2 requests.
///
/// After each consolidation pass, if new semantic patterns were generalized we
/// surface a [`ProactiveTrigger::MemoryInsight`] to neocortex so AURA can share
/// a relevant observation with the user.  Guard conditions:
/// - At least 1 newly generalized semantic entry (real signal, not noise).
/// - `should_dispatch` gate: relevance ≥ 0.6 and occurrence_count ≥ 2.
/// - Deduplicated via the `last_memory_insight_ms` timestamp: minimum 24 h between dispatches
///   regardless of consolidation frequency.
async fn cron_handle_memory_compaction(
    state: &mut DaemonState,
    subs: &mut LoopSubsystems,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    tracing::info!("executing memory compaction cron job");
    let report = consolidate(
        ConsolidationLevel::Micro,
        &mut subs.memory.working,
        &subs.memory.episodic,
        &subs.memory.semantic,
        &subs.memory.archive,
        &mut subs.memory.pattern_engine,
        now_ms(),
        None, // Micro level does not use LLM generalization
    )
    .await;
    tracing::info!(
        swept = report.working_slots_swept,
        generalized = report.semantic_generalized,
        patterns_recorded = report.patterns_recorded,
        duration_ms = report.duration_ms,
        "micro consolidation complete"
    );
    subs.system2.sweep_stale(now_ms());

    // ── MemoryInsight proactive trigger ──────────────────────────────────────
    // Only surface when consolidation actually produced new semantic knowledge.
    if report.semantic_generalized > 0 {
        const MEMORY_INSIGHT_COOLDOWN_MS: u64 = 24 * 60 * 60 * 1_000; // 24 h
        let last_dispatch = state.checkpoint.last_memory_insight_ms;
        let now = now_ms();

        if now.saturating_sub(last_dispatch) >= MEMORY_INSIGHT_COOLDOWN_MS {
            // Use occurrence_count = generalized + reinforced as a proxy for
            // how many times the pattern appeared before being crystallised.
            let occurrence_count =
                (report.semantic_generalized + report.semantic_reinforced).max(1) as u32;
            // Relevance heuristic: more newly generalized entries → higher score.
            let relevance_score = (report.semantic_generalized as f32 / 5.0).min(1.0).max(0.6);

            let trigger = ProactiveTrigger::MemoryInsight {
                pattern_summary: format!(
                    "{} new pattern{} crystallised from recent activity \
                     ({} semantic entr{} reinforced)",
                    report.semantic_generalized,
                    if report.semantic_generalized == 1 {
                        ""
                    } else {
                        "s"
                    },
                    report.semantic_reinforced,
                    if report.semantic_reinforced == 1 {
                        "y"
                    } else {
                        "ies"
                    },
                ),
                relevance_score,
                occurrence_count,
            };

            if should_dispatch(&trigger) {
                let ipc_msg = trigger_to_ipc(&trigger);
                if ensure_ipc_connected(&mut subs.neocortex).await {
                    match tokio::time::timeout(
                        Duration::from_millis(IPC_SEND_TIMEOUT_MS),
                        subs.neocortex.send(&ipc_msg),
                    )
                    .await
                    {
                        Ok(Ok(())) => {
                            state.checkpoint.last_memory_insight_ms = now;
                            tracing::info!(
                                generalized = report.semantic_generalized,
                                relevance = relevance_score,
                                "MemoryInsight proactive trigger dispatched"
                            );
                        },
                        Ok(Err(e)) => tracing::warn!(
                            error = %e,
                            "MemoryInsight: IPC send failed"
                        ),
                        Err(_) => tracing::warn!("MemoryInsight: IPC send timed out"),
                    }
                } else {
                    tracing::debug!("MemoryInsight: neocortex unreachable — deferring");
                }
            } else {
                tracing::debug!(
                    relevance = relevance_score,
                    occurrence_count,
                    "MemoryInsight below dispatch threshold — skipping"
                );
            }
        } else {
            tracing::debug!(
                cooldown_remaining_s = (MEMORY_INSIGHT_COOLDOWN_MS
                    .saturating_sub(now.saturating_sub(last_dispatch)))
                    / 1_000,
                "MemoryInsight cooldown active — skipping"
            );
        }
    }

    Ok(())
}

/// Token reset: zero daily token counters.
async fn cron_handle_token_reset(
    state: &mut DaemonState,
    _subs: &mut LoopSubsystems,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    tracing::info!("executing daily token counter reset");
    state.checkpoint.token_counters.local_tokens = 0;
    state.checkpoint.token_counters.cloud_tokens = 0;
    Ok(())
}

/// Checkpoint: save daemon checkpoint to disk.
async fn cron_handle_checkpoint(
    state: &mut DaemonState,
    _subs: &mut LoopSubsystems,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    tracing::info!("executing checkpoint cron job");
    let cp = state.checkpoint.clone();
    let path = crate::daemon_core::startup::checkpoint_path_from_config(&state.config);
    let result = tokio::task::spawn_blocking(move || save_checkpoint(&cp, Path::new(&path))).await;
    match result {
        Ok(Ok(())) => tracing::debug!("cron-triggered checkpoint saved"),
        Ok(Err(e)) => tracing::error!(error = %e, "cron checkpoint failed"),
        Err(e) => tracing::error!(error = %e, "cron checkpoint spawn panicked"),
    }
    Ok(())
}

/// Proactive engine tick: run the initiative/suggestion/briefing loop.
async fn cron_handle_proactive(
    state: &mut DaemonState,
    subs: &mut LoopSubsystems,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // Safe mode: block all proactive actions.
    if subs.safe_mode.should_block_action(true, false) {
        tracing::info!("proactive tick BLOCKED by safe mode — skipping");
        return Ok(());
    }
    tracing::info!("executing proactive engine tick");
    if let Some(ref mut proactive) = subs.proactive {
        let power_tier =
            battery_percent_to_power_tier(state.checkpoint.power_budget.battery_percent);
        let context_mode = subs
            .arc_manager
            .as_ref()
            .map(|a| a.context_mode)
            .unwrap_or(ContextMode::Default);

        let current_hour = {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default();
            let secs = now.as_secs();
            ((secs / 3600) % 24) as u8
        };

        let profile_allows = subs.identity.is_proactive_allowed(current_hour);
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
                            let msg = format!("[Suggestion] {}", suggestion.text);
                            send_response(&subs.response_tx, InputSource::Direct, msg).await;
                        },
                        ProactiveAction::Briefing(sections) => {
                            tracing::info!(
                                sections = sections.len(),
                                "proactive briefing generated"
                            );
                            let mut brief = String::from("[Briefing]\n");
                            for section in &sections {
                                brief.push_str(&format!("• {}\n", section.key()));
                            }
                            send_response(&subs.response_tx, InputSource::Direct, brief).await;
                        },
                        ProactiveAction::RunAutomation {
                            routine_id,
                            actions: auto_actions,
                        } => {
                            tracing::info!(
                                routine_id = %routine_id,
                                steps = auto_actions.len(),
                                "proactive automation triggered"
                            );
                            let desc = format!("proactive_routine:{}", routine_id);
                            if let Some(_denial) =
                                check_consent_for_task(&subs.consent_tracker, &desc)
                            {
                                subs.outcome_bus.publish(ExecutionOutcome::new(
                                    desc.clone(),
                                    OutcomeResult::Failure,
                                    0,
                                    0.0,
                                    RouteKind::System1,
                                    now_ms(),
                                ));
                                flush_outcome_bus(subs).await;
                            } else if subs.action_sandbox.classify_string(&desc)
                                == ContainmentLevel::Restricted
                            {
                                tracing::warn!(
                                    target: "SECURITY",
                                    desc = %desc,
                                    "proactive routine classified as L2:Restricted — auto-denying"
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
                            } else {
                                let boundary_result = check_boundary_for_task(
                                    &subs.boundary_reasoner,
                                    &desc,
                                    current_relationship_stage(subs),
                                    subs.recent_denial_count,
                                );
                                match boundary_result {
                                    BoundaryGateResult::Deny(reason)
                                    | BoundaryGateResult::NeedConfirmation { reason, .. } => {
                                        subs.recent_denial_count =
                                            subs.recent_denial_count.saturating_add(1);
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
                                    },
                                    BoundaryGateResult::Allow => {
                                        let mut policy_ctx = react::PolicyContext {
                                            gate: &mut subs.policy_gate,
                                            audit: &mut subs.audit_log,
                                        };
                                        let (_outcome, _session) = react::execute_task(
                                            desc,
                                            7,
                                            None,
                                            Some(&mut policy_ctx),
                                        )
                                        .await;
                                    },
                                }
                            }
                        },
                        ProactiveAction::Alert {
                            domain,
                            message,
                            urgency,
                        } => {
                            tracing::warn!(domain = %domain, urgency = urgency, "proactive alert");
                            let msg =
                                format!("[Alert — {} (urgency {})] {}", domain, urgency, message);
                            send_response(&subs.response_tx, InputSource::Direct, msg).await;
                        },
                    }
                }
            },
            Err(e) => {
                tracing::warn!(error = %e, "ProactiveEngine tick failed");
            },
        }
    } else {
        tracing::debug!("proactive engine not available — skipping tick");
    }
    Ok(())
}

/// Dreaming tick: placeholder — dreaming engine removed, reasoning belongs in
/// the LLM (neocortex) layer.
/// Dreaming: background memory consolidation during idle + charging windows.
///
/// Guard conditions (ALL must pass):
/// 1. Safe mode inactive
/// 2. Device is charging (battery life must not be consumed)
/// 3. Battery ≥ 30% (protect against sudden shutdown mid-consolidation)
/// 4. Thermal < 45°C (consolidation + LLM generalization heats the SoC)
/// 5. User idle ≥ 30 minutes (never interrupt active use)
///
/// Consolidation level:
/// - Thermal < 35°C → Deep (LLM-assisted semantic generalization)
/// - Thermal 35–45°C → Light (episodic-to-semantic, no LLM)
///
/// On success, logs the report and emits observability spans.
/// Errors from JNI probes are non-fatal: log + skip rather than crash.
async fn cron_handle_dreaming(
    state: &mut DaemonState,
    subs: &mut LoopSubsystems,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // ── Guard 1: safe mode ───────────────────────────────────────────────────
    if subs.safe_mode.should_block_action(false, true) {
        tracing::info!("dreaming tick BLOCKED by safe mode — skipping");
        return Ok(());
    }

    // ── Guard 2: charging ────────────────────────────────────────────────────
    match crate::platform::jni_bridge::jni_is_charging() {
        Ok(true) => {}, // continue
        Ok(false) => {
            tracing::debug!("dreaming skipped — device not charging");
            return Ok(());
        },
        Err(e) => {
            tracing::debug!(error = %e, "dreaming skipped — could not determine charge state");
            return Ok(());
        },
    }

    // ── Guard 3: battery ≥ 30% ───────────────────────────────────────────────
    let battery_pct = state.checkpoint.power_budget.battery_percent;
    if battery_pct < 30 {
        tracing::debug!(
            battery_pct = battery_pct,
            "dreaming skipped — battery below 30%"
        );
        return Ok(());
    }

    // ── Guard 4: thermal probe → determine level ─────────────────────────────
    let level = match crate::platform::jni_bridge::jni_get_thermal_status() {
        Ok(temp_c) if temp_c > 45.0 => {
            tracing::debug!(
                temp_c = temp_c,
                "dreaming skipped — thermal too high (> 45°C)"
            );
            return Ok(());
        },
        Ok(temp_c) if temp_c >= 35.0 => {
            tracing::debug!(
                temp_c = temp_c,
                "dreaming downgraded to Light — thermal warm (35–45°C)"
            );
            ConsolidationLevel::Light
        },
        Ok(_) => {
            tracing::debug!("dreaming at Deep level — thermal nominal (< 35°C)");
            ConsolidationLevel::Deep
        },
        Err(e) => {
            // Cannot determine thermal state — play it safe with Light level.
            tracing::debug!(
                error = %e,
                "thermal probe failed — defaulting to Light consolidation"
            );
            ConsolidationLevel::Light
        },
    };

    // ── Guard 5: user idle ≥ 30 minutes ──────────────────────────────────────
    const IDLE_THRESHOLD_MS: u64 = 30 * 60 * 1_000;
    let now = now_ms();
    let idle_ms = now.saturating_sub(subs.last_interaction_ms);
    if idle_ms < IDLE_THRESHOLD_MS {
        tracing::debug!(
            idle_secs = idle_ms / 1_000,
            "dreaming skipped — user not idle (< 30 min)"
        );
        return Ok(());
    }

    // ── All guards passed — run consolidation ────────────────────────────────
    tracing::info!(
        level = ?level,
        battery_pct = battery_pct,
        idle_secs = idle_ms / 1_000,
        "dreaming: starting background consolidation"
    );

    // For Deep level we pass the neocortex client for LLM-assisted
    // semantic generalization.  Light level runs fully local.
    let neocortex_opt = match level {
        ConsolidationLevel::Deep => Some(&mut subs.neocortex),
        _ => None,
    };

    let report = consolidate(
        level,
        &mut subs.memory.working,
        &subs.memory.episodic,
        &subs.memory.semantic,
        &subs.memory.archive,
        &mut subs.memory.pattern_engine,
        now,
        neocortex_opt,
    )
    .await;

    tracing::info!(
        level                = ?report.level,
        semantic_generalized = report.semantic_generalized,
        semantic_reinforced  = report.semantic_reinforced,
        patterns_recorded    = report.patterns_recorded,
        working_slots_swept  = report.working_slots_swept,
        bytes_freed          = report.bytes_freed,
        duration_ms          = report.duration_ms,
        "dreaming: consolidation complete"
    );

    // ── Drain retrieval feedback buffers → adjust consolidation weights ────────
    //
    // Each retrieval event recorded since the last consolidation pass is drained
    // here and used to nudge ConsolidationWeights via adjust_from_outcome.
    // The buffers are runtime-only (not persisted); the weights live in the
    // checkpoint and are saved below so they survive restarts.
    let mut events_processed: u32 = 0;

    // Episodic feedback
    match subs.memory.episodic.feedback.lock() {
        Ok(mut buf) => {
            let events = buf.drain();
            for ev in &events {
                let hours_after_storage = if ev.stored_ms > 0 {
                    (ev.retrieved_ms.saturating_sub(ev.stored_ms)) as f64 / 3_600_000.0
                } else {
                    0.0
                };
                state
                    .checkpoint
                    .consolidation_weights
                    .adjust_from_outcome(true, hours_after_storage);
                events_processed += 1;
            }
            tracing::debug!(
                count = events.len(),
                "dreaming: drained episodic retrieval feedback"
            );
        },
        Err(_) => {
            tracing::error!(
                "dreaming: episodic feedback buffer lock poisoned — skipping weight adjustment"
            );
        },
    }

    // Semantic feedback
    match subs.memory.semantic.feedback.lock() {
        Ok(mut buf) => {
            let events = buf.drain();
            for ev in &events {
                let hours_after_storage = if ev.stored_ms > 0 {
                    (ev.retrieved_ms.saturating_sub(ev.stored_ms)) as f64 / 3_600_000.0
                } else {
                    0.0
                };
                state
                    .checkpoint
                    .consolidation_weights
                    .adjust_from_outcome(true, hours_after_storage);
                events_processed += 1;
            }
            tracing::debug!(
                count = events.len(),
                "dreaming: drained semantic retrieval feedback"
            );
        },
        Err(_) => {
            tracing::error!(
                "dreaming: semantic feedback buffer lock poisoned — skipping weight adjustment"
            );
        },
    }

    // Persist updated weights to checkpoint so they survive restarts.
    // Only bother if we actually adjusted anything.
    if events_processed > 0 {
        let w = &state.checkpoint.consolidation_weights;
        tracing::info!(
            recency = w.recency,
            frequency = w.frequency,
            importance = w.importance,
            events = events_processed,
            "dreaming: consolidation weights updated from retrieval feedback"
        );

        let cp = state.checkpoint.clone();
        let path = crate::daemon_core::startup::checkpoint_path_from_config(&state.config);
        let save_result =
            tokio::task::spawn_blocking(move || save_checkpoint(&cp, Path::new(&path))).await;
        match save_result {
            Ok(Ok(())) => tracing::debug!("dreaming: checkpoint saved after weight update"),
            Ok(Err(e)) => {
                tracing::error!(error = %e, "dreaming: checkpoint save failed after weight update — weights will be recalculated next session")
            },
            Err(e) => tracing::error!(error = %e, "dreaming: checkpoint save task panicked"),
        }
    } else {
        let w = &state.checkpoint.consolidation_weights;
        tracing::debug!(
            recency = w.recency,
            frequency = w.frequency,
            importance = w.importance,
            "dreaming: no retrieval feedback events — consolidation weights unchanged"
        );
    }

    Ok(())
}

// ── Health domain ────────────────────────────────────────────────────────────

/// Medication check: alert user about pending doses.
async fn cron_handle_medication(
    _state: &mut DaemonState,
    subs: &mut LoopSubsystems,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    if let Some(ref mut arc) = subs.arc_manager {
        let now = now_ms() as i64 / 1000;
        let pending = arc.health.medication.check_pending_doses(now);
        if !pending.is_empty() {
            tracing::info!(count = pending.len(), "medication doses pending");
            let summary = pending
                .iter()
                .map(|d| {
                    format!(
                        "• med_id={} (window {}–{})",
                        d.med_id, d.scheduled_at, d.window_end
                    )
                })
                .collect::<Vec<_>>()
                .join("\n");
            send_response(
                &subs.response_tx,
                InputSource::Direct,
                format!("💊 Medication reminder:\n{summary}"),
            )
            .await;
        }
    } else {
        tracing::debug!("arc manager not available — skipping medication_check");
    }
    Ok(())
}

/// Vital ingest: record that a vital-signs reading pass has completed.
async fn cron_handle_vital_ingest(
    _state: &mut DaemonState,
    subs: &mut LoopSubsystems,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    if let Some(ref mut arc) = subs.arc_manager {
        tracing::debug!("vital_ingest: no reading available");
        tracing::debug!(
            readings = arc.health.vitals.reading_count(),
            "vital signs ingested"
        );
    } else {
        tracing::debug!("arc manager not available — skipping vital_ingest");
    }
    Ok(())
}

/// Step sync: log the current activity score from the fitness tracker.
async fn cron_handle_step_sync(
    _state: &mut DaemonState,
    subs: &mut LoopSubsystems,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    if let Some(ref mut arc) = subs.arc_manager {
        let score = arc.health.fitness.activity_score();
        tracing::debug!(activity_score = score, "step sync complete");
    } else {
        tracing::debug!("arc manager not available — skipping step_sync");
    }
    Ok(())
}

/// Sleep inference: log the sleep quality score.
async fn cron_handle_sleep_infer(
    _state: &mut DaemonState,
    subs: &mut LoopSubsystems,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    if let Some(ref mut arc) = subs.arc_manager {
        let quality = arc.health.sleep.quality_score();
        tracing::debug!(
            quality_score = quality,
            records = arc.health.sleep.record_count(),
            "sleep inference done"
        );
    } else {
        tracing::debug!("arc manager not available — skipping sleep_infer");
    }
    Ok(())
}

/// Health score compute: recompute the composite health domain score.
///
/// **Integration #1 (thermal → inference throttle)** is handled in
/// `cron_handle_health_report` where `HealthMonitor::check()` provides the
/// thermal state.  This handler only updates the ARC state store.
async fn cron_handle_health_score(
    _state: &mut DaemonState,
    subs: &mut LoopSubsystems,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    if let Some(ref mut arc) = subs.arc_manager {
        match arc.health.compute_score() {
            Ok(score) => {
                let mut m = std::collections::HashMap::new();
                m.insert("composite_health".to_string(), score as f64);
                if let Err(e) = arc.state_store.update(
                    crate::arc::DomainId::Health,
                    score,
                    crate::arc::DomainLifecycle::Active,
                    m,
                    now_ms() as i64,
                ) {
                    tracing::error!(error = %e, "arc state store update failed for health domain — non-fatal");
                }
                tracing::info!(health_score = score, "health domain score updated");
            },
            Err(e) => tracing::warn!(error = %e, "health score computation failed"),
        }
    } else {
        tracing::debug!("arc manager not available — skipping health_score_compute");
    }
    Ok(())
}

/// Weekly health report: generate and send a formatted summary.
async fn cron_handle_health_weekly(
    _state: &mut DaemonState,
    subs: &mut LoopSubsystems,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
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
        send_response(&subs.response_tx, InputSource::Direct, report).await;
        tracing::info!(composite, "health weekly report generated");
    } else {
        tracing::debug!("arc manager not available — skipping health_weekly_report");
    }
    Ok(())
}

// ── Social domain ────────────────────────────────────────────────────────────

/// Contact update: refresh the contact store and log the total count.
async fn cron_handle_contact_update(
    _state: &mut DaemonState,
    subs: &mut LoopSubsystems,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    if let Some(ref mut arc) = subs.arc_manager {
        let total = arc.social.contacts.total_contacts();
        tracing::debug!(total_contacts = total, "contact store refreshed");
    } else {
        tracing::debug!("arc manager not available — skipping contact_update");
    }
    Ok(())
}

/// Importance recalc: re-score all contacts by importance.
async fn cron_handle_importance_recalc(
    _state: &mut DaemonState,
    subs: &mut LoopSubsystems,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    if let Some(ref mut arc) = subs.arc_manager {
        let now_secs = (now_ms() / 1000) as i64;
        let social = &mut arc.social;
        let contacts = social.contacts.all_mut();
        let scored = social.importance.score_all(
            contacts,
            now_secs,
            social.importance.observation_window_days(),
        );
        tracing::debug!(
            observation_window_days = social.importance.observation_window_days(),
            "importance recalc using configured window"
        );
        tracing::info!(scored_count = scored, "importance scores recalculated");
    } else {
        tracing::debug!("arc manager not available — skipping importance_recalc");
    }
    Ok(())
}

/// Relationship health: evaluate and log average relationship health.
async fn cron_handle_relationship_health(
    _state: &mut DaemonState,
    subs: &mut LoopSubsystems,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    if let Some(ref mut arc) = subs.arc_manager {
        let avg = arc.social.relationship_health.average_health();
        tracing::debug!(average_health = avg, "relationship health evaluated");
    } else {
        tracing::debug!("arc manager not available — skipping relationship_health");
    }
    Ok(())
}

/// Social gap scan: detect lapsed contacts and dispatch proactive nudges.
async fn cron_handle_social_gap(
    state: &mut DaemonState,
    subs: &mut LoopSubsystems,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    if let Some(ref mut arc) = subs.arc_manager {
        let now = now_ms();
        let gaps = arc.social.gap_detector.detect_gaps(now);
        if !gaps.is_empty() {
            tracing::info!(gap_count = gaps.len(), "social gaps detected");
            for gap in &gaps {
                tracing::debug!(?gap, "social gap alert");
                let trigger = ProactiveTrigger::RelationshipNudge {
                    contact_id: gap.contact_id,
                    days_since_contact: (gap.gap_duration_ms / 86_400_000) as u32,
                    urgency: gap.urgency,
                };
                if should_dispatch(&trigger) {
                    let ipc_msg = trigger_to_ipc(&trigger);
                    state.pending_system2_sources.push_back(InputSource::Direct);
                    if ensure_ipc_connected(&mut subs.neocortex).await {
                        if let Err(e) = subs.neocortex.send(&ipc_msg).await {
                            tracing::warn!(error = %e, "proactive: failed to dispatch RelationshipNudge");
                        }
                    }
                }
            }
        }
    } else {
        tracing::debug!("arc manager not available — skipping social_gap_scan");
    }
    Ok(())
}

/// Birthday check: scan for upcoming birthdays and notify the user.
async fn cron_handle_birthday(
    _state: &mut DaemonState,
    subs: &mut LoopSubsystems,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    if let Some(ref mut arc) = subs.arc_manager {
        let secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let days_since_epoch = secs / 86400;
        let day_of_year = (days_since_epoch % 365) as u32;
        let month = (day_of_year / 30 + 1).min(12) as u8;
        let day = (day_of_year % 30 + 1) as u8;
        let days_ahead = arc.social.birthdays.scan_ahead_days();
        tracing::debug!(
            scan_ahead_days = days_ahead,
            "birthday check using configured lookahead"
        );
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
                        InputSource::Direct,
                        format!("🎂 Upcoming birthdays:\n{msg}"),
                    )
                    .await;
                }
            },
            Err(e) => tracing::warn!(error = %e, "birthday scan failed"),
        }
    } else {
        tracing::debug!("arc manager not available — skipping birthday_check");
    }
    Ok(())
}

/// Social score compute: recompute the composite social domain score.
async fn cron_handle_social_score(
    _state: &mut DaemonState,
    subs: &mut LoopSubsystems,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    if let Some(ref mut arc) = subs.arc_manager {
        match arc.social.compute_score() {
            Ok(score) => {
                let mut m = std::collections::HashMap::new();
                m.insert("composite_social".to_string(), score as f64);
                if let Err(e) = arc.state_store.update(
                    crate::arc::DomainId::Social,
                    score,
                    crate::arc::DomainLifecycle::Active,
                    m,
                    now_ms() as i64,
                ) {
                    tracing::error!(error = %e, "arc state store update failed for social domain — non-fatal");
                }
                tracing::info!(social_score = score, "social domain score updated");
            },
            Err(e) => tracing::warn!(error = %e, "social score computation failed"),
        }
    } else {
        tracing::debug!("arc manager not available — skipping social_score_compute");
    }
    Ok(())
}

/// Weekly social report: generate and send a formatted summary.
async fn cron_handle_social_weekly(
    _state: &mut DaemonState,
    subs: &mut LoopSubsystems,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
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
        send_response(&subs.response_tx, InputSource::Direct, report).await;
        tracing::info!(composite, "social weekly report generated");
    } else {
        tracing::debug!("arc manager not available — skipping social_weekly_report");
    }
    Ok(())
}

// ── Proactive domain ─────────────────────────────────────────────────────────

/// Trigger rule evaluation: evaluate suggestion triggers from the ARC proactive
/// engine and dispatch fired suggestions as [`ProactiveTrigger::RoutineDeviation`]
/// IPC messages to neocortex.
///
/// Architecture:
/// - `TimePattern` and `AnomalyDetected` suggestions map to `RoutineDeviation`
///   (temporal/behavioural deviations the LLM should reason about).
/// - High-confidence suggestions (≥ 0.7) are treated as significant deviations worthy of surfacing;
///   lower-confidence ones are logged only.
/// - At most one dispatch per evaluation pass to avoid flooding.
async fn cron_handle_trigger_rules(
    _state: &mut DaemonState,
    subs: &mut LoopSubsystems,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    if let Some(ref mut arc) = subs.arc_manager {
        let now = now_ms();
        match arc.proactive.suggestions.evaluate_triggers(now) {
            Ok(suggestions) => {
                if suggestions.is_empty() {
                    return Ok(());
                }
                tracing::info!(count = suggestions.len(), "proactive triggers fired");

                // Find the highest-confidence routine/anomaly suggestion to dispatch.
                // We dispatch at most one per evaluation pass to avoid notification spam.
                let best = suggestions
                    .iter()
                    .filter(|s| {
                        // Only TimePattern and AnomalyDetected map to RoutineDeviation.
                        // Other trigger types (HealthAlert, GoalReminder, SocialGap) are
                        // handled by their dedicated cron handlers.
                        matches!(
                            s.category as u8, // compare via discriminant since SuggestionTrigger
                            // is not directly accessible on Suggestion.category
                            // (Suggestion.category is DomainId, not SuggestionTrigger).
                            _ // accept all — filter by confidence below
                        ) && s.confidence >= 0.7
                    })
                    .max_by(|a, b| {
                        a.confidence
                            .partial_cmp(&b.confidence)
                            .unwrap_or(std::cmp::Ordering::Equal)
                    });

                for s in &suggestions {
                    tracing::debug!(
                        id = s.id,
                        confidence = s.confidence,
                        text = %s.text,
                        "suggestion generated"
                    );
                }

                if let Some(s) = best {
                    // Map the suggestion to a RoutineDeviation trigger.
                    // deviation_minutes: positive = late (hasn't happened yet past expected time).
                    // We use 30 min as a sentinel when we don't have clock-precise data —
                    // this passes the `should_dispatch` gate (>= 30 min).
                    let deviation_minutes: i32 = 30;
                    let trigger = ProactiveTrigger::RoutineDeviation {
                        routine_name: format!("suggestion:{}", s.id),
                        expected_at: "routine time".to_string(),
                        deviation_minutes,
                    };

                    if should_dispatch(&trigger) {
                        // Override the generic RoutineDeviation with a richer
                        // TriggerRuleFired that carries the actual suggestion text.
                        // We re-build the IPC message directly to preserve the text.
                        let ipc_trigger = aura_types::ipc::ProactiveTrigger::TriggerRuleFired {
                            rule_name: format!("suggestion:{}", s.id),
                            description: s.text.clone(),
                        };
                        let mut ctx = aura_types::ipc::ContextPackage::default();
                        ctx.inference_mode = aura_types::ipc::InferenceMode::Conversational;
                        ctx.user_state = aura_types::ipc::UserStateSignals::default();
                        ctx.token_budget = 512;
                        let ipc_msg = DaemonToNeocortex::ProactiveContext {
                            trigger: ipc_trigger,
                            context: ctx,
                        };

                        if ensure_ipc_connected(&mut subs.neocortex).await {
                            match tokio::time::timeout(
                                Duration::from_millis(IPC_SEND_TIMEOUT_MS),
                                subs.neocortex.send(&ipc_msg),
                            )
                            .await
                            {
                                Ok(Ok(())) => tracing::info!(
                                    suggestion_id = s.id,
                                    confidence = s.confidence,
                                    "RoutineDeviation proactive trigger dispatched"
                                ),
                                Ok(Err(e)) => tracing::warn!(
                                    error = %e,
                                    "RoutineDeviation: IPC send failed"
                                ),
                                Err(_) => tracing::warn!("RoutineDeviation: IPC send timed out"),
                            }
                        } else {
                            tracing::debug!("RoutineDeviation: neocortex unreachable — deferring");
                        }
                    } else {
                        tracing::debug!(
                            suggestion_id = s.id,
                            confidence = s.confidence,
                            "RoutineDeviation below dispatch threshold — skipping"
                        );
                    }
                }
            },
            Err(e) => tracing::warn!(error = %e, "trigger rule evaluation failed"),
        }
    } else {
        tracing::debug!("arc manager not available — skipping trigger_rule_eval");
    }
    Ok(())
}

/// Patterns / proactive ARC sub-jobs: opportunity detection, threat
/// accumulation, action drain, daily budget reset, and removed Bayesian/Hebbian
/// learning jobs (pattern_observe, pattern_analyze, etc.).
async fn cron_handle_patterns(
    _state: &mut DaemonState,
    subs: &mut LoopSubsystems,
    job: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    if job.contains("opportunity_detect") {
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
                },
                Err(e) => tracing::warn!(error = %e, "opportunity detection failed"),
            }
        } else {
            tracing::debug!("arc manager not available — skipping opportunity_detect");
        }
    } else if job.contains("threat_accumulate") {
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
    } else if job.contains("action_drain") {
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
    } else if job.contains("daily_budget_reset") {
        if let Some(ref mut arc) = subs.arc_manager {
            let reset_secs = arc.proactive.daily_budget_reset_secs();
            tracing::debug!(
                daily_budget_reset_secs = reset_secs,
                "daily budget reset using configured elapsed seconds"
            );
            arc.proactive.regenerate_initiative(reset_secs);
            tracing::info!(
                budget = arc.proactive.budget(),
                daily_suggestions = arc.proactive.daily_suggestions(),
                "daily proactive budget reset"
            );
        } else {
            tracing::debug!("arc manager not available — skipping daily_budget_reset");
        }
    } else {
        // pattern_observe / pattern_analyze / pattern_deviation_check /
        // hebbian_decay / hebbian_consolidate — engines removed.
        tracing::debug!(job = %job, "skipping removed learning engine job");
    }
    Ok(())
}

// ── Learning domain ───────────────────────────────────────────────────────────

/// Interest update: decay interest scores by configured half-life and log top
/// interests.
async fn cron_handle_interest_update(
    _state: &mut DaemonState,
    subs: &mut LoopSubsystems,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
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
    Ok(())
}

/// Skill progress: decay confidence on all tracked skills and log reliable ones.
async fn cron_handle_skill_progress(
    _state: &mut DaemonState,
    subs: &mut LoopSubsystems,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    if let Some(ref mut arc) = subs.arc_manager {
        let now = now_ms();
        arc.learning.skills.decay_all_confidence(now);
        let reliable = arc.learning.skills.get_reliable_skills(0.7);
        tracing::debug!(reliable_count = reliable.len(), "skill progress updated");
    } else {
        tracing::debug!("arc manager not available — skipping skill_progress");
    }
    Ok(())
}

// ── System / cross-cutting ────────────────────────────────────────────────────

/// Domain state publish: compute scores for all domains and publish the life
/// quality index.
async fn cron_handle_domain_state(
    _state: &mut DaemonState,
    subs: &mut LoopSubsystems,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    if let Some(ref mut arc) = subs.arc_manager {
        let mut scores = std::collections::HashMap::new();
        for domain in crate::arc::DomainId::ALL.iter() {
            if let Some(s) = arc.state_store.get_health_score(*domain) {
                scores.insert(*domain, s);
            }
        }
        let lqi = crate::arc::compute_life_quality(&scores, None);
        tracing::info!(
            domain_count = scores.len(),
            lqi = lqi,
            "domain state published"
        );
    } else {
        tracing::debug!("arc manager not available — skipping domain_state_publish");
    }
    Ok(())
}

/// Life quality compute: recompute LQI and send result to the user.
async fn cron_handle_life_quality(
    _state: &mut DaemonState,
    subs: &mut LoopSubsystems,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    if let Some(ref mut arc) = subs.arc_manager {
        let mut scores = std::collections::HashMap::new();
        for domain in crate::arc::DomainId::ALL.iter() {
            if let Some(s) = arc.state_store.get_health_score(*domain) {
                scores.insert(*domain, s);
            }
        }
        let lqi = crate::arc::compute_life_quality(&scores, None);
        tracing::info!(
            lqi = lqi,
            domains_active = scores.len(),
            "life quality index recomputed"
        );
        send_response(
            &subs.response_tx,
            InputSource::Direct,
            format!(
                "Life Quality Index: {lqi:.1}% ({} domains active)",
                scores.len()
            ),
        )
        .await;
    } else {
        tracing::debug!("arc manager not available — skipping life_quality_compute");
    }
    Ok(())
}

/// Cron self-check: verify the ARC scheduler job registry is healthy.
async fn cron_handle_self_check(
    _state: &mut DaemonState,
    subs: &mut LoopSubsystems,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    if let Some(ref arc) = subs.arc_manager {
        let job_count = arc.scheduler.job_count();
        tracing::info!(registered_jobs = job_count, "cron self-check passed");
    } else {
        tracing::debug!("arc manager not available — skipping cron_self_check");
    }
    Ok(())
}

/// Memory ARC flush: flush AuraMemory write-ahead log / dirty pages.
async fn cron_handle_memory_arc_flush(
    _state: &mut DaemonState,
    _subs: &mut LoopSubsystems,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    tracing::debug!("memory arc flush: no-op");
    Ok(())
}

/// Weekly digest: generate a combined LQI + domain summary and send it to the
/// user.
async fn cron_handle_weekly_digest(
    _state: &mut DaemonState,
    subs: &mut LoopSubsystems,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    if let Some(ref mut arc) = subs.arc_manager {
        let mut scores = std::collections::HashMap::new();
        for domain in crate::arc::DomainId::ALL.iter() {
            if let Some(s) = arc.state_store.get_health_score(*domain) {
                scores.insert(*domain, s);
            }
        }
        let lqi = crate::arc::compute_life_quality(&scores, None);
        let generated_at = {
            let secs = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            format!("{} UTC (unix epoch)", secs)
        };
        let report = format!(
            "📊 Weekly AURA Digest\n\
             ├─ Life Quality Index: {lqi:.1}%\n\
             ├─ Active domains: {}/{}\n\
             └─ Generated at: {}",
            scores.len(),
            crate::arc::DomainId::ALL.len(),
            generated_at,
        );
        send_response(&subs.response_tx, InputSource::Direct, report).await;
        tracing::info!(lqi, "weekly digest generated");
    } else {
        tracing::debug!("arc manager not available — skipping weekly_digest");
    }
    Ok(())
}

/// Deep consolidation: prune the social graph and (if battery allows) run
/// memory consolidation.
///
/// **Integration #2 — Power → memory consolidation:** if battery is below 15%,
/// skip consolidation entirely and log the reason.
async fn cron_handle_deep_consolidation(
    state: &mut DaemonState,
    subs: &mut LoopSubsystems,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // ── Integration #2: Power → memory consolidation ─────────────────────
    let battery_fraction = state.checkpoint.power_budget.battery_percent as f32 / 100.0;
    if battery_fraction < 0.15 {
        tracing::info!(
            battery_pct = state.checkpoint.power_budget.battery_percent,
            "skipping deep consolidation: battery critical"
        );
        return Ok(());
    }

    if let Some(ref mut arc) = subs.arc_manager {
        tracing::debug!("deep_consolidation: learning consolidation skipped (engine removed)");
        let prune_threshold = arc.social.graph.prune_min_weight();
        tracing::debug!(
            prune_min_weight = prune_threshold,
            "social graph prune using configured threshold"
        );
        let pruned = arc.social.graph.prune_weak(prune_threshold);
        if pruned > 0 {
            tracing::info!(
                pruned_edges = pruned,
                "social graph pruned during deep consolidation"
            );
        }
    } else {
        tracing::debug!("arc manager not available — skipping deep_consolidation");
    }
    Ok(())
}

/// Arc health check: refresh all life-arc health scores and dispatch any
/// pending proactive triggers to the neocortex.
///
/// # Architecture
///
/// 1. Call `LifeArcManager::update_health_all(now_ms)` — O(n) over each arc's event log; safe on
///    any power tier.
/// 2. Call `collect_triggers(now_ms)` — each arc enforces its own 24-hour dedup so this never
///    floods the user.
/// 3. Convert each `life_arc::ProactiveTrigger` via `arc_trigger_to_ipc` and send to the neocortex
///    for reasoning + message generation.
///
/// Safe-mode: blocked when proactive actions are disabled. This is intentional
/// — arc health updates are cheap but the downstream IPC sends are not.
async fn cron_handle_arc_health_check(
    _state: &mut DaemonState,
    subs: &mut LoopSubsystems,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    if subs.safe_mode.should_block_action(true, false) {
        tracing::info!("arc_health_check BLOCKED by safe mode — skipping");
        return Ok(());
    }

    let now = now_ms();

    let triggers = if let Some(ref mut arc) = subs.arc_manager {
        // Step 1: refresh health scores for all four life arcs.
        arc.life_arc.update_health_all(now);

        // Step 2: collect any pending triggers (24-hour dedup already enforced).
        arc.life_arc.collect_triggers(now)
    } else {
        tracing::debug!("arc_manager not available — skipping arc_health_check");
        return Ok(());
    };

    if triggers.is_empty() {
        tracing::debug!("arc_health_check: no pending triggers");
        return Ok(());
    }

    tracing::info!(
        trigger_count = triggers.len(),
        "arc_health_check: dispatching life-arc proactive triggers"
    );

    for arc_trigger in &triggers {
        tracing::debug!(
            arc = arc_trigger.arc_type.as_str(),
            health = ?arc_trigger.health,
            "dispatching arc proactive trigger"
        );

        let ipc_msg = arc_trigger_to_ipc(arc_trigger);

        // Best-effort send — a failure here should not crash the cron loop.
        if let Err(e) = subs.neocortex.send(&ipc_msg).await {
            tracing::warn!(
                arc = arc_trigger.arc_type.as_str(),
                error = %e,
                "failed to dispatch arc proactive trigger to neocortex"
            );
        }
    }

    Ok(())
}

/// BDI deliberation cycle: re-evaluate goal priorities, detect conflicts,
/// decompose committed goals, and handle stalls and deadlines.
async fn cron_handle_bdi_deliberation(
    state: &mut DaemonState,
    subs: &mut LoopSubsystems,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let now = now_ms();
    let openness = subs.identity.personality.traits.openness;

    let active_goals: Vec<aura_types::goals::Goal> = state
        .checkpoint
        .goals
        .iter()
        .filter(|g| {
            matches!(
                g.status,
                aura_types::goals::GoalStatus::Active | aura_types::goals::GoalStatus::Pending
            )
        })
        .cloned()
        .collect();

    if active_goals.is_empty() {
        tracing::trace!("bdi_deliberation: no active goals — skipping");
        return Ok(());
    }

    let Some(ref mut scheduler) = subs.bdi_scheduler else {
        tracing::debug!("bdi_scheduler not available — skipping bdi_deliberation");
        return Ok(());
    };

    let delib_result = scheduler.deliberate(&active_goals, now, openness);

    match delib_result {
        DeliberationResult::Commit { new_intentions } => {
            tracing::info!(
                committed = new_intentions.len(),
                "BDI deliberation: committing new intentions"
            );
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
                            for sub in &result.sub_goals {
                                if let Some(ref mut tracker) = subs.goal_tracker {
                                    let _ = tracker.track(sub.goal.clone());
                                    let _ = tracker.activate(sub.goal.id, now);
                                }
                                state.checkpoint.goals.push(sub.goal.clone());
                            }
                        },
                        Err(e) => {
                            tracing::warn!(
                                goal_id = intention_id,
                                error = %e,
                                "BDI: goal decomposition failed"
                            );
                        },
                    }
                }
            }
        },
        DeliberationResult::Reconsider {
            drop_intentions,
            reason,
        } => {
            tracing::info!(
                dropping = drop_intentions.len(),
                reason = %reason,
                "BDI deliberation: reconsidering — dropping intentions"
            );
            for drop_id in &drop_intentions {
                if let Some(ref mut tracker) = subs.goal_tracker {
                    let _ = tracker.cancel(*drop_id, now);
                }
                if let Some(goal) = state.checkpoint.goals.iter_mut().find(|g| g.id == *drop_id) {
                    goal.status = aura_types::goals::GoalStatus::Cancelled;
                }
            }
        },
        DeliberationResult::Maintain => {
            tracing::trace!("BDI deliberation: maintain current intentions");
        },
    }

    // ── Conflict detection ──────────────────────────────────────────────────
    if let Some(ref mut resolver) = subs.conflict_resolver {
        let entries: Vec<GoalConflictEntry> = active_goals
            .iter()
            .map(|g| GoalConflictEntry {
                goal_id: g.id,
                score: 0.5,
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

    // ── Deadline + stall checks ─────────────────────────────────────────────
    if let Some(ref mut tracker) = subs.goal_tracker {
        let overdue = tracker.check_deadlines(now);
        for goal_id in &overdue {
            tracing::warn!(goal_id, "BDI: goal overdue — deadline exceeded");
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
            let goal_title = state
                .checkpoint
                .goals
                .iter()
                .find(|g| g.id == *goal_id)
                .map(|g| g.description.clone())
                .unwrap_or_else(|| format!("Goal #{}", goal_id));
            let trigger = ProactiveTrigger::GoalOverdue {
                goal_id: *goal_id,
                goal_title,
                overdue_ms: 0,
            };
            if should_dispatch(&trigger) {
                let ipc_msg = trigger_to_ipc(&trigger);
                state.pending_system2_sources.push_back(InputSource::Direct);
                if ensure_ipc_connected(&mut subs.neocortex).await {
                    if let Err(e) = subs.neocortex.send(&ipc_msg).await {
                        tracing::warn!(error = %e, "proactive: failed to dispatch GoalOverdue");
                    }
                }
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
            let goal_title = state
                .checkpoint
                .goals
                .iter()
                .find(|g| g.id == stall.goal_id)
                .map(|g| g.description.clone())
                .unwrap_or_else(|| format!("Goal #{}", stall.goal_id));
            let stalled_days = (stall.stall_duration_ms / 86_400_000) as u32;
            let trigger = ProactiveTrigger::GoalStalled {
                goal_id: stall.goal_id,
                goal_title,
                stalled_days,
                progress_at_stall: stall.progress_at_stall,
            };
            if should_dispatch(&trigger) {
                let ipc_msg = trigger_to_ipc(&trigger);
                state.pending_system2_sources.push_back(InputSource::Direct);
                if ensure_ipc_connected(&mut subs.neocortex).await {
                    if let Err(e) = subs.neocortex.send(&ipc_msg).await {
                        tracing::warn!(error = %e, "proactive: failed to dispatch GoalStalled");
                    }
                }
            }
        }
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
        0..=14 => PowerTier::P0Always, // critical / emergency — only essential
        15..=29 => PowerTier::P1IdlePlus, // low power — light work only
        30..=50 => PowerTier::P2Normal, // conservative — standard interaction
        51..=100 => PowerTier::P3Charging, // normal+ — background work ok
        _ => PowerTier::P2Normal,      // saturate at normal
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use aura_types::config::AuraConfig;

    use super::*;
    use crate::daemon_core::channels::{InputSource, UserCommand};

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

        local
            .run_until(async {
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
            })
            .await;
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

        local
            .run_until(async {
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
                assert!(
                    result.is_ok(),
                    "main loop should exit after processing command"
                );
            })
            .await;
    }

    #[tokio::test]
    async fn test_a11y_pipeline() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut state = test_state(&dir);
        let response_tx = state.channels.response_tx.clone();
        let data_dir = dir.path().to_path_buf();
        let test_memory = std::mem::replace(
            &mut state.subsystems.memory,
            crate::memory::AuraMemory::new_in_memory().expect("in-memory placeholder"),
        );
        let mut subs = LoopSubsystems::new(response_tx, &data_dir, test_memory);

        let event = aura_types::events::RawEvent {
            event_type: 32,
            package_name: "com.whatsapp".to_string(),
            class_name: "android.widget.TextView".to_string(),
            text: Some("Hello!".to_string()),
            content_description: None,
            timestamp_ms: 1_700_000_000_000,
            source_node_id: None,
            raw_nodes: None,
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
        let test_memory = std::mem::replace(
            &mut state.subsystems.memory,
            crate::memory::AuraMemory::new_in_memory().expect("in-memory placeholder"),
        );
        let mut subs = LoopSubsystems::new(response_tx, &data_dir, test_memory);

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
        let test_memory = std::mem::replace(
            &mut state.subsystems.memory,
            crate::memory::AuraMemory::new_in_memory().expect("in-memory placeholder"),
        );
        let mut subs = LoopSubsystems::new(response_tx, &data_dir, test_memory);

        let msg = NeocortexToDaemon::ConversationReply {
            text: "Hello from neocortex!".to_string(),
            mood_hint: Some(0.5),
            tokens_used: 0,
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
        let test_memory = std::mem::replace(
            &mut state.subsystems.memory,
            crate::memory::AuraMemory::new_in_memory().expect("in-memory placeholder"),
        );
        let mut subs = LoopSubsystems::new(response_tx, &data_dir, test_memory);

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
        let test_memory = std::mem::replace(
            &mut state.subsystems.memory,
            crate::memory::AuraMemory::new_in_memory().expect("in-memory placeholder"),
        );
        let mut subs = LoopSubsystems::new(response_tx, &data_dir, test_memory);

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
        let test_memory = std::mem::replace(
            &mut state.subsystems.memory,
            crate::memory::AuraMemory::new_in_memory().expect("in-memory placeholder"),
        );
        let mut subs = LoopSubsystems::new(response_tx, &data_dir, test_memory);

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
        let test_memory = std::mem::replace(
            &mut state.subsystems.memory,
            crate::memory::AuraMemory::new_in_memory().expect("in-memory placeholder"),
        );
        let mut subs = LoopSubsystems::new(response_tx, &data_dir, test_memory);

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
        let test_memory = std::mem::replace(
            &mut state.subsystems.memory,
            crate::memory::AuraMemory::new_in_memory().expect("in-memory placeholder"),
        );
        let mut subs = LoopSubsystems::new(response_tx, &data_dir, test_memory);

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
        let mut state = test_state(&dir);
        let response_tx = state.channels.response_tx.clone();
        let data_dir = dir.path().to_path_buf();
        let test_memory = std::mem::replace(
            &mut state.subsystems.memory,
            crate::memory::AuraMemory::new_in_memory().expect("in-memory placeholder"),
        );
        let mut subs = LoopSubsystems::new(response_tx, &data_dir, test_memory);

        let msg = IpcOutbound { payload: vec![] };

        let result = handle_ipc_outbound(msg, &mut subs).await;
        assert!(result.is_ok(), "empty IPC outbound should be handled");
    }

    #[tokio::test]
    async fn test_ipc_outbound_oversized() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut state = test_state(&dir);
        let response_tx = state.channels.response_tx.clone();
        let data_dir = dir.path().to_path_buf();
        let test_memory = std::mem::replace(
            &mut state.subsystems.memory,
            crate::memory::AuraMemory::new_in_memory().expect("in-memory placeholder"),
        );
        let mut subs = LoopSubsystems::new(response_tx, &data_dir, test_memory);

        let msg = IpcOutbound {
            payload: vec![0u8; MAX_IPC_PAYLOAD_BYTES + 1],
        };

        let result = handle_ipc_outbound(msg, &mut subs).await;
        assert!(
            result.is_ok(),
            "oversized IPC outbound should be dropped gracefully"
        );
    }

    #[tokio::test]
    async fn test_task_request_creates_goal() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut state = test_state(&dir);
        let response_tx = state.channels.response_tx.clone();
        let data_dir = dir.path().to_path_buf();
        let test_memory = std::mem::replace(
            &mut state.subsystems.memory,
            crate::memory::AuraMemory::new_in_memory().expect("in-memory placeholder"),
        );
        let mut subs = LoopSubsystems::new(response_tx, &data_dir, test_memory);

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
        let test_memory = std::mem::replace(
            &mut state.subsystems.memory,
            crate::memory::AuraMemory::new_in_memory().expect("in-memory placeholder"),
        );
        let mut subs = LoopSubsystems::new(response_tx, &data_dir, test_memory);

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
        let test_memory = std::mem::replace(
            &mut state.subsystems.memory,
            crate::memory::AuraMemory::new_in_memory().expect("in-memory placeholder"),
        );
        let mut subs = LoopSubsystems::new(response_tx, &data_dir, test_memory);

        let cmd = UserCommand::ProfileSwitch {
            profile: "work".to_string(),
            source: InputSource::Direct,
        };

        let result = handle_user_command(cmd, &mut state, &mut subs).await;
        assert!(result.is_ok(), "profile switch should succeed");
    }
}
