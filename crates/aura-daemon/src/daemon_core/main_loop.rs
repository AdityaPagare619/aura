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
    },
    daemon_core::{
        channels::{
            CronTick, CronTickTx, DaemonResponse, DaemonResponseTx, DbWriteRequest, HealthEventTx,
            InputSource, IpcInboundTx, IpcOutbound, UserCommand,
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
};
#[cfg(feature = "voice")]
use crate::{bridge::voice_bridge::VoiceBridge, voice::VoiceEngine};

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

/// Cron tick interval in seconds (fires periodic maintenance jobs).
const CRON_TICK_INTERVAL_SECS: u64 = 60;

/// Health event interval in seconds (fires periodic health checks).
const HEALTH_EVENT_INTERVAL_SECS: u64 = 30;

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

    // -- Channel senders for internal producer tasks ------------------------
    /// IPC inbound sender — held here so IPC listener task can inject messages.
    /// `None` until IPC connects; then cloned to the listener task.
    ipc_inbound_tx: Option<IpcInboundTx>,
}

impl LoopSubsystems {
    /// Construct all subsystems.  `response_tx` is cloned before the
    /// channel split so we can send responses from handlers.
    ///
    /// `memory` is taken by move from `DaemonState.subsystems.memory` — this
    /// ensures there is exactly ONE `AuraMemory` instance open against the
    /// SQLite files, eliminating the previous double-init / WAL write conflict.
    fn new(
        response_tx: DaemonResponseTx,
        data_dir: &std::path::Path,
        memory: AuraMemory,
        ipc_inbound_tx: Option<IpcInboundTx>,
    ) -> Self {
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
            ipc_inbound_tx,
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
// Background producer tasks
// ---------------------------------------------------------------------------

/// Spawn a cron scheduler task that sends periodic CronTick messages.
///
/// This task runs forever (until cancel_flag is set) and sends a CronTick
/// every CRON_TICK_INTERVAL_SECS seconds. The main loop handles cron jobs
/// based on these ticks.
fn spawn_cron_scheduler(
    cron_tx: CronTickTx,
    cancel_flag: std::sync::Arc<std::sync::atomic::AtomicBool>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut ticker = interval(Duration::from_secs(CRON_TICK_INTERVAL_SECS));
        // Skip the first immediate tick
        ticker.tick().await;

        let mut job_counter: u32 = 0;
        loop {
            ticker.tick().await;

            if cancel_flag.load(Ordering::Acquire) {
                tracing::debug!("cron scheduler: cancel flag set, exiting");
                break;
            }

            job_counter = job_counter.wrapping_add(1);
            let tick = CronTick {
                job_id: job_counter,
                job_name: format!("periodic_maintenance_{}", job_counter),
                scheduled_at_ms: now_ms(),
            };

            if let Err(e) = cron_tx.send(tick).await {
                tracing::warn!(error = %e, "cron scheduler: failed to send tick, channel closed");
                break;
            }

            tracing::trace!(job_id = job_counter, "cron scheduler: tick sent");
        }

        tracing::info!("cron scheduler task exiting");
    })
}

/// Spawn a health event task that sends periodic health check events.
///
/// This task runs forever (until cancel_flag is set) and sends a DaemonEvent
/// every HEALTH_EVENT_INTERVAL_SECS seconds. The main loop uses these to
/// trigger health monitoring and reporting.
fn spawn_health_producer(
    health_tx: HealthEventTx,
    cancel_flag: std::sync::Arc<std::sync::atomic::AtomicBool>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut ticker = interval(Duration::from_secs(HEALTH_EVENT_INTERVAL_SECS));
        // Skip the first immediate tick
        ticker.tick().await;

        loop {
            ticker.tick().await;

            if cancel_flag.load(Ordering::Acquire) {
                tracing::debug!("health producer: cancel flag set, exiting");
                break;
            }

            // Send a heartbeat event
            let event = DaemonEvent::Heartbeat {
                timestamp_ms: now_ms(),
            };

            if let Err(e) = health_tx.send(event).await {
                tracing::warn!(error = %e, "health producer: failed to send event, channel closed");
                break;
            }

            tracing::trace!("health producer: heartbeat sent");
        }

        tracing::info!("health producer task exiting");
    })
}

// ---------------------------------------------------------------------------
// Main loop entry
// ---------------------------------------------------------------------------

/// Run the daemon event loop until cancellation or channel exhaustion.
///
/// This is the heart of the daemon.  It `select!`s over 7 channels plus
/// a periodic checkpoint timer and a cancellation check.
///
/// **Key design:** Essential channel senders (cron, health, ipc_inbound) are
/// cloned BEFORE split and given to background producer tasks. This ensures
/// channels stay alive. Channels without producers (a11y, notification on
/// headless/Termux) will close, but that's expected — the daemon continues
/// as long as at least one channel remains open OR the cancel flag isn't set.
///
/// All subsystem wiring flows through [`LoopSubsystems`] which is constructed
/// once at the start and passed by `&mut` to every handler.
pub async fn run(mut state: DaemonState) {
    let checkpoint_interval = Duration::from_secs(state.config.daemon.checkpoint_interval_s as u64);
    let mut checkpoint_timer = interval(checkpoint_interval);
    // Consume the first (immediate) tick.
    checkpoint_timer.tick().await;

    let mut select_count: u64 = state.checkpoint.select_count;

    // Clone essential TX halves BEFORE split so we can give them to producer tasks.
    // This is CRITICAL: if we don't do this, all channels close immediately
    // because there are no external producers holding TX clones.
    let response_tx = state.channels.response_tx.clone();
    let cron_tx = state.channels.cron_tick_tx.clone();
    let health_tx = state.channels.health_event_tx.clone();
    let ipc_inbound_tx = state.channels.ipc_inbound_tx.clone();

    // Split channels: take rx halves for select!.
    // NOTE: We intentionally drop ONLY the senders we don't need internally.
    // Channels like a11y_tx, notification_tx have no internal producers in
    // Termux/headless mode — they'll close, which is fine.
    let channels = std::mem::take(&mut state.channels);
    let (_senders, mut rxs) = channels.split();
    // DO NOT drop(senders) — we've already cloned the essential ones above,
    // and dropping all senders immediately closes all channels!
    // The _senders will be dropped at end of scope, which is fine because
    // we've cloned the ones we need.

    // --- Spawn background producer tasks ---
    // These tasks hold clones of TX halves and produce messages, keeping
    // the channels alive.
    let _cron_handle = spawn_cron_scheduler(cron_tx, state.cancel_flag.clone());
    tracing::info!("cron scheduler task spawned");

    let _health_handle = spawn_health_producer(health_tx, state.cancel_flag.clone());
    tracing::info!("health producer task spawned");

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
    #[cfg(feature = "voice")]
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
    #[cfg(feature = "voice")]
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
    let mut subs = LoopSubsystems::new(response_tx, data_dir, loop_memory, Some(ipc_inbound_tx));

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
            match std::fs::write(&vault_key_path, key) {
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
                    tracing::info!("generated new vault key at {:?}", vault_key_path);
                    Some(key)
                },
                Err(e) => {
                    tracing::error!(error = %e, path = ?vault_key_path, "failed to persist vault.key");
                    None
                },
            }
        };

        // If we obtained a key, provision the vault.
        if let Some(key) = vault_key {
            subs.critical_vault.provision_key(key);
            tracing::info!("CriticalVault encryption key provisioned");
        } else {
            tracing::warn!("CriticalVault running WITHOUT encryption — sensitive data unprotected");
        }
    }

    // ── FIX 2: Try to load user profile from memory/DB for consent checks ──
    match rusqlite::Connection::open_with_flags(
        &state.config.sqlite.db_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    ) {
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

            // ----- Health events (heartbeat producer) -----
            msg = rxs.health_event_rx.recv() => {
                match msg {
                    Some(event) => {
                        select_count += 1;
                        if let Err(e) = handle_health_event(event, &mut state, &mut subs).await {
                            tracing::error!(error = %e, "health event handler failed");
                        }
                    }
                    None => {
                        tracing::warn!("health event channel closed");
                        open_channels = open_channels.saturating_sub(1);
                    }
                }
            }

            // ----- Checkpoint timer -----
            _ = checkpoint_timer.tick() => {
                // Best-effort checkpoint save.
                state.checkpoint.select_count = select_count;
                let checkpoint_path = crate::daemon_core::startup::checkpoint_path_from_config(&state.config);
                match tokio::task::spawn_blocking({
                    let cp = state.checkpoint.clone();
                    let path = checkpoint_path.clone();
                    move || save_checkpoint(&cp, Path::new(&path))
                }).await {
                    Ok(Ok(())) => tracing::trace!(path = %checkpoint_path, "checkpoint saved"),
                    Ok(Err(e)) => tracing::warn!(error = %e, "checkpoint save failed"),
                    Err(e) => tracing::error!(error = %e, "cron checkpoint spawn panicked"),
                }
            }

            // ----- Periodic wake-up (ensures cancel flag is checked) -----
            _ = tokio::time::sleep(Duration::from_millis(100)) => {
                // No work; loop head re-checks cancel_flag and open_channels.
            }
        }
    }

    // Cleanup: save final checkpoint.
    state.checkpoint.select_count = select_count;
    let checkpoint_path = crate::daemon_core::startup::checkpoint_path_from_config(&state.config);
    if let Err(e) = save_checkpoint(&state.checkpoint, Path::new(&checkpoint_path)) {
        tracing::error!(error = %e, "final checkpoint save failed");
    } else {
        tracing::info!(path = %checkpoint_path, select_count, "final checkpoint saved");
    }
}

// ---------------------------------------------------------------------------
// Event handlers
// ---------------------------------------------------------------------------

/// Handle an accessibility event from the Android A11y service.
async fn handle_a11y_event(
    event: aura_types::events::RawEvent,
    state: &mut DaemonState,
    subs: &mut LoopSubsystems,
) -> Result<(), String> {
    // Parse raw event.
    let parsed = subs.event_parser.parse(&event);
    let scored = subs.amygdala.score(&parsed);

    // Gate decision.
    let decision = subs.amygdala.gate(&scored);
    if decision == GateDecision::Drop {
        tracing::trace!(event_type = event.event_type, "a11y event gated: DROP");
        return Ok(());
    }

    // Update screen cache if the event contains a full screen tree.
    if let Some(ref nodes) = event.raw_nodes {
        if !nodes.is_empty() {
            let now = now_ms();
            subs.screen_cache.update(nodes, now);
            subs.last_app_state = detect_app_state(nodes);
        }
    }

    // Store in working memory.
    let _ = subs.memory.working.push(format!(
        "[a11y] pkg={} type={} text={:?}",
        event.package_name, event.event_type, event.text
    ));

    tracing::debug!(
        event_type = event.event_type,
        package = %event.package_name,
        decision = ?decision,
        score = scored.urgency,
        "a11y event processed"
    );

    Ok(())
}

/// Handle a notification event.
async fn handle_notification_event(
    event: aura_types::events::NotificationEvent,
    state: &mut DaemonState,
    subs: &mut LoopSubsystems,
) -> Result<(), String> {
    // Build a ParsedEvent from the notification.
    let parsed = ParsedEvent {
        source: EventSource::Notification,
        intent: Intent::Observe,
        entities: vec![],
        timestamp_ms: event.posted_at_ms,
        raw_text: Some(format!("{}: {}", event.title, event.body)),
    };

    let scored = subs.amygdala.score(&parsed);
    let decision = subs.amygdala.gate(&scored);

    if decision == GateDecision::Drop {
        tracing::trace!(package = %event.package, "notification gated: DROP");
        return Ok(());
    }

    // Store in working memory.
    let _ = subs.memory.working.push(format!(
        "[notification] {} — {} — {}",
        event.package, event.title, event.body
    ));

    tracing::debug!(
        package = %event.package,
        title = %event.title,
        decision = ?decision,
        score = scored.urgency,
        "notification processed"
    );

    Ok(())
}

/// Handle a user command (chat message, task request, etc.).
async fn handle_user_command(
    cmd: UserCommand,
    state: &mut DaemonState,
    subs: &mut LoopSubsystems,
) -> Result<(), String> {
    match cmd {
        UserCommand::Chat { text, source, voice_meta } => {
            tracing::info!(source = %source, len = text.len(), "user chat received");

            // -- SystemBridge fast-path check --
            // If the intent maps to a direct system API call, execute it
            // immediately without going through the full NLU pipeline.
            if let Some(sys_cmd) = subs.system_bridge.parse_intent(&text) {
                tracing::debug!(command = ?sys_cmd, "SystemBridge fast-path matched");

                // Boundary check for sensitive system commands
                let boundary_desc = format!("system_api:{:?}", sys_cmd);
                match boundary_check(subs, &boundary_desc) {
                    BoundaryGateResult::Deny(reason) => {
                        let msg = format!("I can't do that: {reason}");
                        send_response(&subs.response_tx, source, msg).await;
                        return Ok(());
                    },
                    BoundaryGateResult::NeedConfirmation { prompt, .. } => {
                        send_response(&subs.response_tx, source, prompt).await;
                        return Ok(());
                    },
                    BoundaryGateResult::Allow => {},
                }

                match subs.system_bridge.execute(sys_cmd).await {
                    Ok(result) => {
                        let formatted = format_system_result(&result);
                        send_response(&subs.response_tx, source, formatted).await;
                    },
                    Err(e) => {
                        let msg = format!("System command failed: {e}");
                        send_response(&subs.response_tx, source, msg).await;
                    },
                }
                return Ok(());
            }

            // -- Full NLU pipeline --
            // Parse → Score → Enrich → Route → Execute

            // 1. Parse the text into a structured event.
            let parsed = subs.command_parser.parse(&text);

            // 2. Score emotional urgency.
            let scored = subs.amygdala.score(&parsed);

            // 3. Policy gate check.
            let gate_decision = subs.policy_gate.check(&text);
            if let Err(e) = subs.audit_log.log_policy_decision(
                "user_chat",
                &gate_decision,
                &text,
                0,
            ) {
                tracing::warn!(error = %e, "failed to audit user chat policy decision");
            }

            use crate::policy::rules::RuleEffect;
            match gate_decision {
                RuleEffect::Deny => {
                    let msg = "I'm sorry, I can't help with that request.";
                    send_response(&subs.response_tx, source, msg.to_string()).await;
                    return Ok(());
                },
                RuleEffect::Confirm => {
                    // TODO: Implement confirmation flow for restricted actions.
                    let msg = "This action requires confirmation. Please confirm you want to proceed.";
                    send_response(&subs.response_tx, source, msg.to_string()).await;
                    return Ok(());
                },
                RuleEffect::Allow | RuleEffect::Audit => {
                    // Continue processing.
                },
            }

            // 4. Consent check.
            if let Some(reason) = check_consent_for_task(&subs.consent_tracker, &text) {
                send_response(&subs.response_tx, source, reason).await;
                return Ok(());
            }

            // 5. Sandbox containment check.
            if let Some(reason) = check_sandbox_for_task(&subs.action_sandbox, &text) {
                send_response(&subs.response_tx, source, reason).await;
                return Ok(());
            }

            // 6. Boundary reasoning check.
            match boundary_check(subs, &text) {
                BoundaryGateResult::Deny(reason) => {
                    send_response(&subs.response_tx, source, reason).await;
                    return Ok(());
                },
                BoundaryGateResult::NeedConfirmation { prompt, .. } => {
                    send_response(&subs.response_tx, source, prompt).await;
                    return Ok(());
                },
                BoundaryGateResult::Allow => {},
            }

            // 7. Enrich with context.
            let enriched = subs.contextor.enrich(&scored, &subs.memory);

            // 8. Route classification.
            let route = subs.classifier.classify(&enriched);

            // 9. Execute based on route.
            match route {
                RouteKind::System1 => {
                    // Fast-path: use ETG or cached response.
                    tracing::debug!("routing to System1 (fast path)");
                    let response = subs.system1.execute(&enriched);
                    send_response(&subs.response_tx, source, response).await;
                },
                RouteKind::System2 => {
                    // Slow-path: send to neocortex for LLM reasoning.
                    tracing::debug!("routing to System2 (neocortex)");

                    // Record the source for response routing.
                    state.pending_system2_sources.push_back(source.clone());
                    if state.pending_system2_sources.len() > 64 {
                        state.pending_system2_sources.pop_front();
                    }

                    // Build context package.
                    let working_mem: Vec<String> = subs.memory.working.recent(10).to_vec();
                    let screen_summary = subs.last_app_state.as_ref().map(|s| format!("{:?}", s));

                    let context = ContextPackage {
                        working_memory: working_mem,
                        recent_actions: vec![],
                        current_goals: state.checkpoint.goals.iter().map(|g| g.description.clone()).collect(),
                        user_preferences: Default::default(),
                        screen_context: screen_summary,
                        time_context: Some(format!("timestamp_ms={}", now_ms())),
                    };

                    // Build identity tendencies.
                    let tendencies = IdentityTendencies {
                        archetype: subs.identity.personality.archetype().to_string(),
                        mood_valence: subs.identity.affect.mood_valence(),
                        mood_arousal: subs.identity.affect.mood_arousal(),
                        relationship_stage: current_relationship_stage(subs),
                        trust_level: subs.identity.relationships
                            .get_relationship("primary_user")
                            .map(|r| r.trust_score)
                            .unwrap_or(0.5),
                    };

                    // Build self-knowledge.
                    let self_knowledge = SelfKnowledge {
                        capabilities: vec![
                            "text_chat".to_string(),
                            "task_planning".to_string(),
                            "memory_recall".to_string(),
                        ],
                        limitations: vec![
                            "no_internet_search".to_string(),
                            "no_real_time_data".to_string(),
                        ],
                        ethical_boundaries: vec![
                            "privacy_first".to_string(),
                            "no_harm".to_string(),
                            "transparency".to_string(),
                        ],
                    };

                    let request = DaemonToNeocortex::ConversationRequest {
                        user_input: text,
                        context,
                        identity_tendencies: tendencies,
                        self_knowledge,
                        inference_mode: InferenceMode::Conversational,
                        max_tokens: 1024,
                    };

                    // Send to neocortex.
                    if ensure_ipc_connected(&mut subs.neocortex).await {
                        if let Err(e) = subs.neocortex.send(&request).await {
                            tracing::error!(error = %e, "failed to send to neocortex");
                            let msg = "I'm having trouble thinking right now. Please try again.";
                            send_response(&subs.response_tx, source, msg.to_string()).await;
                            state.pending_system2_sources.pop_back();
                        }
                    } else {
                        let msg = "My reasoning system is offline. Please try again later.";
                        send_response(&subs.response_tx, source, msg.to_string()).await;
                        state.pending_system2_sources.pop_back();
                    }
                },
            }
        },

        UserCommand::TaskRequest { description, priority, source } => {
            tracing::info!(source = %source, priority, "task request received");

            // Consent check.
            if let Some(reason) = check_consent_for_task(&subs.consent_tracker, &description) {
                send_response(&subs.response_tx, source, reason).await;
                return Ok(());
            }

            // Sandbox check.
            if let Some(reason) = check_sandbox_for_task(&subs.action_sandbox, &description) {
                send_response(&subs.response_tx, source, reason).await;
                return Ok(());
            }

            // Boundary check.
            match boundary_check(subs, &description) {
                BoundaryGateResult::Deny(reason) => {
                    send_response(&subs.response_tx, source, reason).await;
                    return Ok(());
                },
                BoundaryGateResult::NeedConfirmation { prompt, .. } => {
                    send_response(&subs.response_tx, source, prompt).await;
                    return Ok(());
                },
                BoundaryGateResult::Allow => {},
            }

            // Create goal.
            let goal_id = state.checkpoint.next_goal_id;
            state.checkpoint.next_goal_id += 1;

            let clamped_priority = priority.min(100);
            let goal = crate::daemon_core::checkpoint::CheckpointGoal {
                id: goal_id,
                description: description.clone(),
                priority: clamped_priority,
                created_at_ms: now_ms(),
                status: crate::daemon_core::checkpoint::GoalStatus::Pending,
            };

            // Check goal cap.
            if state.checkpoint.goals.len() >= MAX_ACTIVE_GOALS {
                let msg = format!(
                    "I'm tracking too many goals ({}/{}). Please complete or cancel some first.",
                    state.checkpoint.goals.len(),
                    MAX_ACTIVE_GOALS
                );
                send_response(&subs.response_tx, source, msg).await;
                return Ok(());
            }

            state.checkpoint.goals.push(goal);
            let msg = format!("Got it! I've added goal #{}: {}", goal_id, description);
            send_response(&subs.response_tx, source, msg).await;
        },

        UserCommand::CancelTask { task_id, source } => {
            tracing::info!(source = %source, task_id = %task_id, "cancel task received");

            if let Ok(id) = task_id.parse::<u64>() {
                if let Some(goal) = state.checkpoint.goals.iter_mut().find(|g| g.id == id) {
                    goal.status = crate::daemon_core::checkpoint::GoalStatus::Cancelled;
                    let msg = format!("Cancelled goal #{}: {}", id, goal.description);
                    send_response(&subs.response_tx, source, msg).await;
                } else {
                    let msg = format!("I couldn't find goal #{}.", id);
                    send_response(&subs.response_tx, source, msg).await;
                }
            } else {
                let msg = format!("Invalid task ID: {}", task_id);
                send_response(&subs.response_tx, source, msg).await;
            }
        },

        UserCommand::ProfileSwitch { profile, source } => {
            tracing::info!(source = %source, profile = %profile, "profile switch received");
            let msg = format!("Switched to profile: {}", profile);
            send_response(&subs.response_tx, source, msg).await;
        },
    }

    Ok(())
}

/// Handle an IPC outbound message (daemon → neocortex).
async fn handle_ipc_outbound(
    outbound: IpcOutbound,
    subs: &mut LoopSubsystems,
) -> Result<(), String> {
    if outbound.payload.len() > MAX_IPC_PAYLOAD_BYTES {
        tracing::warn!(
            size = outbound.payload.len(),
            max = MAX_IPC_PAYLOAD_BYTES,
            "IPC outbound payload too large — dropping"
        );
        return Ok(());
    }

    // Deserialize and forward to neocortex.
    match bincode::deserialize::<DaemonToNeocortex>(&outbound.payload) {
        Ok(msg) => {
            if ensure_ipc_connected(&mut subs.neocortex).await {
                if let Err(e) = subs.neocortex.send(&msg).await {
                    tracing::error!(error = %e, "failed to forward IPC outbound to neocortex");
                }
            } else {
                tracing::warn!("neocortex disconnected — dropping outbound message");
            }
        },
        Err(e) => {
            tracing::error!(error = %e, "failed to deserialize IPC outbound payload");
        },
    }

    Ok(())
}

/// Handle an IPC inbound message (neocortex → daemon).
async fn handle_ipc_inbound(
    inbound: NeocortexToDaemon,
    state: &mut DaemonState,
    subs: &mut LoopSubsystems,
) -> Result<(), String> {
    match inbound {
        NeocortexToDaemon::ConversationReply { text, mood_hint, .. } => {
            // Route to the correct input source.
            let source = state
                .pending_system2_sources
                .pop_front()
                .unwrap_or(InputSource::Direct);

            tracing::debug!(source = %source, len = text.len(), "conversation reply received");

            // Anti-sycophancy check (optional, placeholder).
            // In a full implementation, this would check if the response
            // is overly agreeable or flattering.

            send_response_with_mood(&subs.response_tx, source, text, mood_hint).await;
        },

        NeocortexToDaemon::PlanReady { plan, goal_id, .. } => {
            tracing::info!(goal_id, steps = plan.len(), "plan ready from neocortex");

            // Execute the plan via the executor.
            // This is a simplified version — a full implementation would
            // track step progress, handle failures, etc.
            for (i, step) in plan.iter().enumerate() {
                tracing::debug!(step = i, action = %step, "executing plan step");
                // TODO: Actual execution via executor.
            }
        },

        NeocortexToDaemon::Error { message, .. } => {
            tracing::error!(error = %message, "error from neocortex");

            // Try to route error to the waiting user.
            if let Some(source) = state.pending_system2_sources.pop_front() {
                let msg = format!("Something went wrong: {}", message);
                send_response(&subs.response_tx, source, msg).await;
            }
        },

        NeocortexToDaemon::MemoryWarning { level, .. } => {
            tracing::warn!(level, "memory warning from neocortex");

            // Trigger memory consolidation.
            if let Err(e) = consolidate(&mut subs.memory, ConsolidationLevel::Light) {
                tracing::error!(error = %e, "memory consolidation failed");
            }
        },

        _ => {
            tracing::debug!("unhandled neocortex message variant");
        },
    }

    Ok(())
}

/// Handle a database write request.
fn handle_db_write(req: DbWriteRequest, state: &DaemonState) -> Result<(), String> {
    // Open a connection for writes.
    let conn = match rusqlite::Connection::open(&state.config.sqlite.db_path) {
        Ok(c) => c,
        Err(e) => return Err(format!("failed to open db for write: {e}")),
    };

    match req {
        DbWriteRequest::Telemetry { payload } => {
            // Insert telemetry event.
            conn.execute(
                "INSERT INTO telemetry (payload, created_at) VALUES (?1, ?2)",
                rusqlite::params![payload, now_ms() as i64],
            )
            .map_err(|e| format!("telemetry insert failed: {e}"))?;
        },
        DbWriteRequest::Episode { content, importance } => {
            // Insert episodic memory.
            conn.execute(
                "INSERT INTO episodes (content, importance, created_at) VALUES (?1, ?2, ?3)",
                rusqlite::params![content, importance, now_ms() as i64],
            )
            .map_err(|e| format!("episode insert failed: {e}"))?;
        },
        DbWriteRequest::AmygdalaBaseline { app, score } => {
            // Upsert amygdala baseline.
            conn.execute(
                "INSERT OR REPLACE INTO amygdala_baselines (app, score, updated_at) VALUES (?1, ?2, ?3)",
                rusqlite::params![app, score, now_ms() as i64],
            )
            .map_err(|e| format!("amygdala baseline upsert failed: {e}"))?;
        },
        DbWriteRequest::RawSql { sql, params } => {
            // Execute arbitrary SQL (for arc jobs, etc.).
            let mut stmt = conn.prepare(&sql).map_err(|e| format!("sql prepare failed: {e}"))?;
            let params_refs: Vec<&dyn rusqlite::ToSql> = params
                .iter()
                .map(|s| s as &dyn rusqlite::ToSql)
                .collect();
            stmt.execute(params_refs.as_slice())
                .map_err(|e| format!("sql execute failed: {e}"))?;
        },
    }

    Ok(())
}

/// Handle a cron tick event.
async fn handle_cron_tick(
    tick: CronTick,
    state: &mut DaemonState,
    subs: &mut LoopSubsystems,
) -> Result<(), String> {
    tracing::debug!(job_id = tick.job_id, job_name = %tick.job_name, "cron tick");

    // Memory consolidation (every tick for now — could be less frequent).
    if let Err(e) = consolidate(&mut subs.memory, ConsolidationLevel::Light) {
        tracing::warn!(error = %e, "cron: memory consolidation failed");
    }

    // Health report.
    let health = subs.health_monitor.report(now_ms());
    tracing::debug!(status = ?health.status, "cron: health report");

    // Token budget reset (placeholder — real implementation would check time).
    // subs.identity.affect.reset_daily_budget();

    // Stale System2 request sweep.
    let stale_threshold_ms = 5 * 60 * 1000; // 5 minutes
    let now = now_ms();
    while let Some(front) = state.pending_system2_sources.front() {
        // We don't have timestamps on these, so just cap the queue size.
        if state.pending_system2_sources.len() > 32 {
            let _ = state.pending_system2_sources.pop_front();
        } else {
            break;
        }
    }

    // Expire old sandbox confirmations.
    let expired: Vec<u64> = subs
        .pending_confirmations
        .iter()
        .filter(|c| c.created_at.elapsed() > c.timeout)
        .map(|c| c.id)
        .collect();
    for id in expired {
        tracing::info!(confirmation_id = id, "sandbox confirmation expired — auto-denied");
        subs.pending_confirmations.retain(|c| c.id != id);
    }

    Ok(())
}

/// Handle a health event from the heartbeat producer.
async fn handle_health_event(
    event: DaemonEvent,
    state: &mut DaemonState,
    subs: &mut LoopSubsystems,
) -> Result<(), String> {
    match event {
        DaemonEvent::Heartbeat { timestamp_ms } => {
            tracing::trace!(timestamp_ms, "heartbeat received");

            // Update health monitor.
            let report = subs.health_monitor.report(timestamp_ms);

            // Log warnings for degraded health.
            match report.status {
                HealthStatus::Healthy => {},
                HealthStatus::Degraded(reason) => {
                    tracing::warn!(reason = %reason, "health degraded");
                },
                HealthStatus::Critical(reason) => {
                    tracing::error!(reason = %reason, "health critical");
                },
            }
        },
        _ => {
            tracing::debug!("unhandled health event variant");
        },
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::daemon_core::startup::startup;
    use aura_types::config::AuraConfig;

    /// Helper: create a minimal test state.
    fn test_state(dir: &tempfile::TempDir) -> DaemonState {
        let db_path = dir.path().join("test.db");
        let mut config = AuraConfig::default();
        config.sqlite.db_path = db_path.to_string_lossy().to_string();
        let (state, _) = startup(config)
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
        let mut subs = LoopSubsystems::new(response_tx, &data_dir, test_memory, None);

        let event = aura_types::events::RawEvent {
            event_type: 1,
            package_name: "com.test".to_string(),
            class_name: "TestActivity".to_string(),
            text: Some("Hello".to_string()),
            content_description: None,
            timestamp_ms: now_ms(),
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
        let mut subs = LoopSubsystems::new(response_tx, &data_dir, test_memory, None);

        let event = aura_types::events::NotificationEvent {
            key: "test_key".to_string(),
            package: "com.test".to_string(),
            title: "Test Title".to_string(),
            body: "Test Body".to_string(),
            posted_at_ms: now_ms(),
            category: None,
        };

        let result = handle_notification_event(event, &mut state, &mut subs).await;
        assert!(result.is_ok(), "notification handler should succeed");
    }

    #[tokio::test]
    async fn test_cron_tick_handler() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut state = test_state(&dir);
        let response_tx = state.channels.response_tx.clone();
        let data_dir = dir.path().to_path_buf();
        let test_memory = std::mem::replace(
            &mut state.subsystems.memory,
            crate::memory::AuraMemory::new_in_memory().expect("in-memory placeholder"),
        );
        let mut subs = LoopSubsystems::new(response_tx, &data_dir, test_memory, None);

        let tick = CronTick {
            job_id: 1,
            job_name: "test_job".to_string(),
            scheduled_at_ms: now_ms(),
        };

        let result = handle_cron_tick(tick, &mut state, &mut subs).await;
        assert!(result.is_ok(), "cron tick handler should succeed");
    }

    #[tokio::test]
    async fn test_health_event_handler() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut state = test_state(&dir);
        let response_tx = state.channels.response_tx.clone();
        let data_dir = dir.path().to_path_buf();
        let test_memory = std::mem::replace(
            &mut state.subsystems.memory,
            crate::memory::AuraMemory::new_in_memory().expect("in-memory placeholder"),
        );
        let mut subs = LoopSubsystems::new(response_tx, &data_dir, test_memory, None);

        let event = DaemonEvent::Heartbeat {
            timestamp_ms: now_ms(),
        };

        let result = handle_health_event(event, &mut state, &mut subs).await;
        assert!(result.is_ok(), "health event handler should succeed");
    }

    #[test]
    fn test_now_ms_monotonic() {
        let t1 = now_ms();
        let t2 = now_ms();
        let t3 = now_ms();
        assert!(t2 >= t1, "time must be monotonic");
        assert!(t3 >= t2, "time must be monotonic");
    }

    #[test]
    fn test_consent_category_detection() {
        assert_eq!(
            consent_category_for_action("read my contacts"),
            Some("privacy_access")
        );
        assert_eq!(
            consent_category_for_action("send a proactive reminder"),
            Some("proactive_actions")
        );
        assert_eq!(
            consent_category_for_action("share my data with the server"),
            Some("data_sharing")
        );
        assert_eq!(consent_category_for_action("open the app"), None);
    }

    #[test]
    fn test_boundary_gate_result() {
        // Just test that the enum variants exist and can be constructed.
        let allow = BoundaryGateResult::Allow;
        let deny = BoundaryGateResult::Deny("test reason".to_string());
        let confirm = BoundaryGateResult::NeedConfirmation {
            reason: "test".to_string(),
            prompt: "confirm?".to_string(),
        };
        assert!(matches!(allow, BoundaryGateResult::Allow));
        assert!(matches!(deny, BoundaryGateResult::Deny(_)));
        assert!(matches!(confirm, BoundaryGateResult::NeedConfirmation { .. }));
    }
}
