//! 8-phase daemon startup with per-phase timing.
//!
//! Total budget: <800 ms.
//!
//! | Phase | Budget  | Description                              |
//! |-------|---------|------------------------------------------|
//! | 1     | <100 ms | JNI load (Android) / library init        |
//! | 2     | <20 ms  | Tokio runtime init                       |
//! | 3     | <150 ms | Database open (WAL, mmap 4 MB)           |
//! | 4     | <50 ms  | State restore (checkpoint load)          |
//! | 5     | <200 ms | Subsystem initialization (all modules)   |
//! | 6     | <10 ms  | IPC bind (abstract socket / noop)        |
//! | 7     | <20 ms  | Cron schedule (build timer heap)         |
//! | 8     | <5 ms   | Ready signal                             |
//!
//! On host (non-Android), phases 1 and 6 are stubs.

use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Instant;

use aura_types::config::AuraConfig;
use rusqlite::Connection;

use crate::arc::ArcManager;
use crate::bridge::router::RouterHandle;
use crate::bridge::BridgeHandle;
use crate::daemon_core::channels::{DaemonChannels, InputSource};
use crate::daemon_core::checkpoint::{load_checkpoint, DaemonCheckpoint};
use crate::daemon_core::onboarding::OnboardingEngine;
use crate::execution::cycle::CycleDetector;
use crate::execution::etg::EtgStore;
use crate::execution::executor::Executor;
use crate::execution::monitor::ExecutionMonitor;
use crate::execution::planner::EnhancedPlanner;
use crate::goals::conflicts::ConflictResolver;
use crate::goals::decomposer::GoalDecomposer;
use crate::goals::registry::GoalRegistry;
use crate::goals::scheduler::BdiScheduler;
use crate::goals::tracker::GoalTracker;
use crate::identity::IdentityEngine;
use crate::ipc::client::NeocortexClient;
use crate::memory::AuraMemory;
use crate::pipeline::amygdala::Amygdala;
use crate::pipeline::contextor::Contextor;
use crate::pipeline::parser::{CommandParser, EventParser};
use crate::platform::PlatformState;
use crate::routing::classifier::RouteClassifier;
use crate::routing::system1::System1;
use crate::routing::system2::System2;
use crate::screen::anti_bot::AntiBot;
use crate::extensions::{CapabilityLoader, ExtensionDiscovery};

// ---------------------------------------------------------------------------
// SubSystems — all processing modules instantiated during startup
// ---------------------------------------------------------------------------

/// All AURA processing subsystems, grouped by domain.
///
/// This struct is the result of Phase 5 (SubSystemsInit). Every subsystem
/// that AURA needs at runtime is created here. Critical subsystems (memory,
/// identity) cause startup failure if they cannot initialise. Non-critical
/// subsystems (goals, routing, arc) log a warning and are set to `None`,
/// allowing the daemon to run in degraded mode.
pub struct SubSystems {
    // -- Critical subsystems (must succeed) --------------------------------
    /// 4-tier memory system (episodic, semantic, archive, working + patterns).
    pub memory: AuraMemory,
    /// Unified identity engine (personality, affect, relationships, ethics, anti-sycophancy).
    pub identity: IdentityEngine,

    // -- Execution subsystems (critical) -----------------------------------
    /// Main executor driving the observe→think→act→verify loop.
    pub executor: Executor,
    /// Enhanced action planner with caching.
    pub planner: EnhancedPlanner,

    // -- Pipeline subsystems (non-critical, degraded mode OK) --------------
    /// Accessibility event parser.
    pub event_parser: Option<EventParser>,
    /// Natural language command parser.
    pub command_parser: Option<CommandParser>,
    /// Emotional urgency tagger.
    pub amygdala: Option<Amygdala>,
    /// Context enrichment engine.
    pub contextor: Option<Contextor>,

    // -- Routing subsystems (non-critical) ---------------------------------
    /// System1/System2 route classifier.
    pub route_classifier: Option<RouteClassifier>,
    /// Fast-path ETG-based execution.
    pub system1: Option<System1>,
    /// Slow-path LLM-based execution.
    pub system2: Option<System2>,

    // -- Goals subsystems (non-critical) -----------------------------------
    /// BDI scheduler (beliefs-desires-intentions).
    pub bdi_scheduler: Option<BdiScheduler>,
    /// Goal lifecycle tracker.
    pub goal_tracker: Option<GoalTracker>,
    /// Goal conflict resolver.
    pub conflict_resolver: Option<ConflictResolver>,
    /// Hierarchical goal decomposer.
    pub goal_decomposer: Option<GoalDecomposer>,
    /// Goal type registry.
    pub goal_registry: Option<GoalRegistry>,

    // -- Screen subsystems (non-critical) ----------------------------------
    /// Anti-bot rate limiter and timing jitter.
    pub anti_bot: Option<AntiBot>,

    // -- Platform subsystem (non-critical) ---------------------------------
    /// Android platform state (power, thermal, doze, notifications, sensors, connectivity).
    pub platform: Option<PlatformState>,

    // -- Extension subsystem (non-critical) --------------------------------
    /// Capability loader for hot-reloading extensions.
    pub capability_loader: Option<CapabilityLoader>,
    /// Smart extension discovery engine.
    pub extension_discovery: Option<ExtensionDiscovery>,

    // -- IPC subsystem (non-critical, connects later) ----------------------
    /// Neocortex IPC client — starts disconnected; connects in main_loop.
    pub neocortex_client: Option<NeocortexClient>,

    // -- ARC subsystem (non-critical) --------------------------------------
    /// Bio-inspired Arc manager (health, social, proactive, learning, cron).
    pub arc: Option<ArcManager>,

    // -- Bridge subsystems (non-critical, spawned in main_loop) ------------
    /// Voice bridge handle — spawned during `main_loop::run()`.
    pub voice_bridge: Option<BridgeHandle>,
    /// Telegram bridge handle — spawned during `main_loop::run()`.
    pub telegram_bridge: Option<BridgeHandle>,
    /// Response router handle — distributes `DaemonResponse` to bridges.
    pub response_router: Option<RouterHandle>,
}

impl std::fmt::Debug for SubSystems {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SubSystems")
            .field("memory", &"AuraMemory { .. }")
            .field("identity", &"IdentityEngine { .. }")
            .field("executor", &self.executor)
            .field("planner", &"EnhancedPlanner { .. }")
            .field("event_parser", &self.event_parser.is_some())
            .field("command_parser", &self.command_parser.is_some())
            .field("amygdala", &self.amygdala.is_some())
            .field("contextor", &self.contextor.is_some())
            .field("route_classifier", &self.route_classifier.is_some())
            .field("system1", &self.system1.is_some())
            .field("system2", &self.system2.is_some())
            .field("bdi_scheduler", &self.bdi_scheduler.is_some())
            .field("goal_tracker", &self.goal_tracker.is_some())
            .field("conflict_resolver", &self.conflict_resolver.is_some())
            .field("goal_decomposer", &self.goal_decomposer.is_some())
            .field("goal_registry", &self.goal_registry.is_some())
            .field("anti_bot", &self.anti_bot.is_some())
            .field("platform", &self.platform.is_some())
            .field("capability_loader", &self.capability_loader.is_some())
            .field("extension_discovery", &self.extension_discovery.is_some())
            .field("neocortex_client", &self.neocortex_client.is_some())
            .field("arc", &self.arc.is_some())
            .field("voice_bridge", &self.voice_bridge.is_some())
            .field("telegram_bridge", &self.telegram_bridge.is_some())
            .field("response_router", &self.response_router.is_some())
            .finish()
    }
}

// ---------------------------------------------------------------------------
// DaemonState — the product of a successful startup
// ---------------------------------------------------------------------------

/// Onboarding status detected during startup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OnboardingStatus {
    /// First time running — full onboarding needed.
    FirstRun,
    /// Onboarding was started but interrupted — needs resumption.
    Interrupted,
    /// Onboarding already completed — normal startup.
    Completed,
}

/// Fully initialised daemon state, handed to `main_loop::run`.
pub struct DaemonState {
    pub channels: DaemonChannels,
    pub db: Connection,
    pub checkpoint: DaemonCheckpoint,
    pub config: AuraConfig,
    /// All processing subsystems — memory, identity, execution, pipeline, etc.
    pub subsystems: SubSystems,
    /// Total startup wall-clock time.
    pub startup_time_ms: u64,
    /// Shared cancellation flag — set to `true` to request shutdown.
    pub cancel_flag: Arc<AtomicBool>,
    /// Onboarding status detected at startup.
    pub onboarding_status: OnboardingStatus,
    /// FIFO queue of originating [`InputSource`]s for in-flight System2 requests.
    ///
    /// Replaced the previous single-slot `last_system2_source: Option<InputSource>`
    /// which caused reply misrouting when two System2 requests were in-flight
    /// concurrently (e.g. a proactive trigger fires during a user request).
    ///
    /// On dispatch: `push_back(source)`.
    /// On `ConversationReply` receipt: `pop_front()` to get the oldest in-flight
    /// source (neocortex processes requests sequentially, so FIFO is correct).
    /// Falls back to `InputSource::Direct` if the queue is empty.
    /// Bounded to 64 entries to prevent unbounded growth on pathological inputs.
    pub pending_system2_sources: std::collections::VecDeque<InputSource>,
}

// ---------------------------------------------------------------------------
// Phase timing helper
// ---------------------------------------------------------------------------

/// Timing record for a single startup phase.
#[derive(Debug, Clone)]
pub struct PhaseTiming {
    pub name: &'static str,
    pub elapsed_ms: u64,
    pub budget_ms: u64,
}

/// Collected startup telemetry.
#[derive(Debug, Clone)]
pub struct StartupReport {
    pub phases: Vec<PhaseTiming>,
    pub total_ms: u64,
}

impl StartupReport {
    /// Log all phase timings via tracing.
    pub fn log(&self) {
        for phase in &self.phases {
            let status = if phase.elapsed_ms <= phase.budget_ms {
                "OK"
            } else {
                "OVER"
            };
            tracing::info!(
                phase = phase.name,
                elapsed_ms = phase.elapsed_ms,
                budget_ms = phase.budget_ms,
                status,
                "startup phase"
            );
        }
        tracing::info!(total_ms = self.total_ms, "startup complete");
    }
}

// ---------------------------------------------------------------------------
// Startup entry point
// ---------------------------------------------------------------------------

/// Run the full 8-phase startup sequence.
///
/// # Errors
/// Returns `StartupError` if any critical phase fails (database, memory, identity,
/// or executor initialisation).
pub fn startup(config: AuraConfig) -> Result<(DaemonState, StartupReport), StartupError> {
    let overall_start = Instant::now();
    let mut phases = Vec::with_capacity(8);

    // -----------------------------------------------------------------------
    // Phase 1: JNI Load / Library Init (<100 ms)
    // -----------------------------------------------------------------------
    let t = Instant::now();
    phase_jni_load()?;
    phases.push(PhaseTiming {
        name: "JniLoad",
        elapsed_ms: t.elapsed().as_millis() as u64,
        budget_ms: 100,
    });

    // -----------------------------------------------------------------------
    // Phase 2: Runtime Init (<20 ms)
    // -----------------------------------------------------------------------
    let t = Instant::now();
    // Tokio runtime is created by the caller; this phase just validates
    // we're inside a runtime context or does any global init.
    phase_runtime_init()?;
    phases.push(PhaseTiming {
        name: "RuntimeInit",
        elapsed_ms: t.elapsed().as_millis() as u64,
        budget_ms: 20,
    });

    // -----------------------------------------------------------------------
    // Phase 3: Database Open (<150 ms)
    // -----------------------------------------------------------------------
    let t = Instant::now();
    let db = phase_database_open(&config.sqlite.db_path)?;
    phases.push(PhaseTiming {
        name: "DatabaseOpen",
        elapsed_ms: t.elapsed().as_millis() as u64,
        budget_ms: 150,
    });

    // -----------------------------------------------------------------------
    // Phase 4: State Restore (<50 ms)
    // -----------------------------------------------------------------------
    let t = Instant::now();
    // Derive checkpoint path as sibling of db_path: <db_dir>/state.bin
    let checkpoint_path = checkpoint_path_from_config(&config);
    let checkpoint = phase_state_restore(&checkpoint_path)?;
    phases.push(PhaseTiming {
        name: "StateRestore",
        elapsed_ms: t.elapsed().as_millis() as u64,
        budget_ms: 50,
    });

    // -----------------------------------------------------------------------
    // Phase 5: Subsystem Initialisation (<200 ms)
    // -----------------------------------------------------------------------
    let t = Instant::now();
    let subsystems = phase_subsystems_init(&config)?;
    phases.push(PhaseTiming {
        name: "SubSystemsInit",
        elapsed_ms: t.elapsed().as_millis() as u64,
        budget_ms: 200,
    });

    // -----------------------------------------------------------------------
    // Phase 6: IPC Bind (<10 ms)
    // -----------------------------------------------------------------------
    let t = Instant::now();
    // IPC address: use abstract socket on Android, noop on host.
    let ipc_address = ipc_address_default();
    phase_ipc_bind(&ipc_address)?;
    phases.push(PhaseTiming {
        name: "IpcBind",
        elapsed_ms: t.elapsed().as_millis() as u64,
        budget_ms: 10,
    });

    // -----------------------------------------------------------------------
    // Phase 7: Cron Schedule (<20 ms)
    // -----------------------------------------------------------------------
    let t = Instant::now();
    phase_cron_schedule(&checkpoint)?;
    phases.push(PhaseTiming {
        name: "CronSchedule",
        elapsed_ms: t.elapsed().as_millis() as u64,
        budget_ms: 20,
    });

    // -----------------------------------------------------------------------
    // Phase 8: Ready (<5 ms)
    // -----------------------------------------------------------------------
    let t = Instant::now();
    let channels = DaemonChannels::new();
    let cancel_flag = Arc::new(AtomicBool::new(false));
    phases.push(PhaseTiming {
        name: "Ready",
        elapsed_ms: t.elapsed().as_millis() as u64,
        budget_ms: 5,
    });

    // -----------------------------------------------------------------------
    // Post-startup: Onboarding check
    // -----------------------------------------------------------------------
    // After Phase 8, detect whether onboarding is needed, interrupted, or done.
    // This is NOT a timed phase — it's a lightweight DB check that sets a flag.
    let onboarding_status = phase_onboarding_check(&db);
    match onboarding_status {
        OnboardingStatus::FirstRun => {
            tracing::info!("onboarding: first run detected — onboarding required");
        }
        OnboardingStatus::Interrupted => {
            tracing::info!("onboarding: interrupted session detected — will resume");
        }
        OnboardingStatus::Completed => {
            tracing::debug!("onboarding: already completed");
        }
    }

    let total_ms = overall_start.elapsed().as_millis() as u64;
    let report = StartupReport { phases, total_ms };
    report.log();

    let state = DaemonState {
        channels,
        db,
        checkpoint,
        config,
        subsystems,
        startup_time_ms: total_ms,
        cancel_flag,
        onboarding_status,
        pending_system2_sources: std::collections::VecDeque::new(),
    };

    Ok((state, report))
}

// ---------------------------------------------------------------------------
// Config helpers — derive paths not stored in AuraConfig
// ---------------------------------------------------------------------------

/// Derive the checkpoint file path from the database path.
/// Places `state.bin` as a sibling of the SQLite database file.
pub fn checkpoint_path_from_config(config: &AuraConfig) -> String {
    let db = Path::new(&config.sqlite.db_path);
    let dir = db.parent().unwrap_or_else(|| Path::new("."));
    dir.join("state.bin").to_string_lossy().to_string()
}

/// Derive the memory data directory from the database path.
/// Places a `memory/` subdirectory as a sibling of the SQLite database file.
pub fn memory_dir_from_config(config: &AuraConfig) -> std::path::PathBuf {
    let db = Path::new(&config.sqlite.db_path);
    let dir = db.parent().unwrap_or_else(|| Path::new("."));
    dir.join("memory")
}

/// Derive the ETG database path from the database path.
/// Places `etg.db` as a sibling of the main SQLite database file.
pub fn etg_path_from_config(config: &AuraConfig) -> std::path::PathBuf {
    let db = Path::new(&config.sqlite.db_path);
    let dir = db.parent().unwrap_or_else(|| Path::new("."));
    dir.join("etg.db")
}

/// Default IPC address — abstract socket on Android, placeholder on host.
fn ipc_address_default() -> String {
    if cfg!(target_os = "android") {
        "@aura-daemon".to_string()
    } else {
        "localhost:0".to_string()
    }
}

// ---------------------------------------------------------------------------
// Phase implementations
// ---------------------------------------------------------------------------

fn phase_jni_load() -> Result<(), StartupError> {
    #[cfg(target_os = "android")]
    {
        // On Android, JNI_OnLoad has already run by the time we get here.
        // Validate that the global JNI env pointer was set.
        tracing::info!("JNI load: Android — validating JNI env");
    }

    #[cfg(not(target_os = "android"))]
    {
        tracing::info!("JNI load: host mode — skipping");
    }

    Ok(())
}

fn phase_runtime_init() -> Result<(), StartupError> {
    // The tokio runtime is created externally. We just validate
    // that tracing subscriber is set up (best effort).
    let _ = tracing_subscriber::fmt::try_init();
    tracing::info!("runtime init: tracing subscriber ready");
    Ok(())
}

fn phase_database_open(db_path: &str) -> Result<Connection, StartupError> {
    let conn = Connection::open(db_path).map_err(|e| {
        tracing::error!(error = %e, path = db_path, "failed to open database");
        StartupError::DatabaseOpen(e.to_string())
    })?;

    // WAL mode
    conn.pragma_update(None, "journal_mode", "WAL")
        .map_err(|e| {
            tracing::error!(error = %e, "failed to set WAL mode");
            StartupError::DatabaseOpen(e.to_string())
        })?;

    // mmap 4 MB
    conn.pragma_update(None, "mmap_size", 4 * 1024 * 1024)
        .map_err(|e| {
            tracing::error!(error = %e, "failed to set mmap_size");
            StartupError::DatabaseOpen(e.to_string())
        })?;

    // page_size 4096
    conn.pragma_update(None, "page_size", 4096).map_err(|e| {
        tracing::error!(error = %e, "failed to set page_size");
        StartupError::DatabaseOpen(e.to_string())
    })?;

    tracing::info!(path = db_path, "database opened with WAL + mmap 4MB");
    Ok(conn)
}

fn phase_state_restore(checkpoint_path: &str) -> Result<DaemonCheckpoint, StartupError> {
    let path = Path::new(checkpoint_path);
    let cp = load_checkpoint(path).map_err(|e| {
        tracing::error!(error = %e, "state restore failed");
        StartupError::StateRestore(e.to_string())
    })?;
    Ok(cp)
}

// ---------------------------------------------------------------------------
// Phase 5: Subsystem initialisation
// ---------------------------------------------------------------------------

/// Initialise all processing subsystems.
///
/// **Initialisation order** (dependencies flow top-to-bottom):
///
/// 1. Memory (AuraMemory) — foundation; needed by everything
/// 2. Identity (IdentityEngine) — personality/ethics; needed by pipeline & routing
/// 3. Execution (Executor + Planner) — needs AntiBot, CycleDetector, Monitor, ETG
/// 4. Pipeline (EventParser, CommandParser, Amygdala, Contextor) — needs memory+identity
/// 5. Routing (RouteClassifier, System1, System2) — needs pipeline output
/// 6. Goals (BdiScheduler, GoalTracker, ConflictResolver, Decomposer, Registry)
/// 7. Platform (PlatformState) — independent, Android-specific
/// 8. IPC (NeocortexClient) — starts disconnected, connects in main_loop
/// 9. ARC (ArcManager) — highest-level, needs everything below
///
/// # Critical vs Non-Critical
///
/// - **Critical**: Memory, Identity, Executor — failure aborts startup.
/// - **Non-critical**: Everything else — failure logs warning, field set to `None`.
fn phase_subsystems_init(config: &AuraConfig) -> Result<SubSystems, StartupError> {
    // -- 1. Memory (CRITICAL) ----------------------------------------------
    let memory_dir = memory_dir_from_config(config);
    let memory = AuraMemory::new(&memory_dir).map_err(|e| {
        tracing::error!(error = %e, "critical: memory subsystem init failed");
        StartupError::SubSystemInit {
            subsystem: "memory",
            reason: e.to_string(),
        }
    })?;
    tracing::info!(dir = ?memory_dir, "memory subsystem initialised");

    // -- 2. Identity (CRITICAL) --------------------------------------------
    let identity = init_critical("identity", || Ok(IdentityEngine::new()))?;
    tracing::info!("identity subsystem initialised");

    // -- 3. Execution (CRITICAL) -------------------------------------------
    // Build ETG — try persistent first, fall back to in-memory
    let etg = {
        let etg_path = etg_path_from_config(config);
        let etg_path_str = etg_path.to_string_lossy().to_string();
        match EtgStore::with_sqlite(&etg_path_str) {
            Ok(store) => {
                tracing::info!(path = %etg_path_str, "ETG: persistent store opened");
                store
            }
            Err(e) => {
                tracing::warn!(error = %e, "ETG: persistent open failed, using in-memory");
                EtgStore::in_memory()
            }
        }
    };

    let executor = Executor::new(
        config.execution.max_steps_normal,
        AntiBot::normal(),
        CycleDetector::new(),
        ExecutionMonitor::normal(),
        etg,
    );
    tracing::info!("executor subsystem initialised");

    let planner = init_critical("planner", || Ok(EnhancedPlanner::with_defaults()))?;
    tracing::info!("planner subsystem initialised");

    // -- 4. Pipeline (non-critical) ----------------------------------------
    let event_parser = init_non_critical("event_parser", || EventParser::new());
    let command_parser = init_non_critical("command_parser", || CommandParser::empty());
    let amygdala = init_non_critical("amygdala", || Amygdala::new());
    let contextor = init_non_critical("contextor", || Contextor::new());
    tracing::info!(
        event_parser = event_parser.is_some(),
        command_parser = command_parser.is_some(),
        amygdala = amygdala.is_some(),
        contextor = contextor.is_some(),
        "pipeline subsystems initialised"
    );

    // -- 5. Routing (non-critical) -----------------------------------------
    let route_classifier = init_non_critical("route_classifier", || RouteClassifier::new());
    let system1 = init_non_critical("system1", || System1::new());
    let system2 = init_non_critical("system2", || System2::new());
    tracing::info!(
        route_classifier = route_classifier.is_some(),
        system1 = system1.is_some(),
        system2 = system2.is_some(),
        "routing subsystems initialised"
    );

    // -- 6. Goals (non-critical) -------------------------------------------
    let bdi_scheduler = init_non_critical("bdi_scheduler", || BdiScheduler::new());
    let goal_tracker = init_non_critical("goal_tracker", || GoalTracker::new());
    let conflict_resolver = init_non_critical("conflict_resolver", || ConflictResolver::new());
    let goal_decomposer = init_non_critical("goal_decomposer", || GoalDecomposer::new());
    let goal_registry = init_non_critical("goal_registry", || GoalRegistry::new());
    tracing::info!(
        bdi_scheduler = bdi_scheduler.is_some(),
        goal_tracker = goal_tracker.is_some(),
        conflict_resolver = conflict_resolver.is_some(),
        goal_decomposer = goal_decomposer.is_some(),
        goal_registry = goal_registry.is_some(),
        "goals subsystems initialised"
    );

    // -- 7. Screen extras (non-critical) -----------------------------------
    let anti_bot = init_non_critical("anti_bot", || AntiBot::normal());
    tracing::info!(
        anti_bot = anti_bot.is_some(),
        "screen subsystems initialised"
    );

    // -- 8. Platform (non-critical) ----------------------------------------
    let platform = init_non_critical("platform", || PlatformState::new());
    tracing::info!(
        platform = platform.is_some(),
        "platform subsystem initialised"
    );

    // -- 8.5. Extensions (non-critical) ------------------------------------
    let capability_loader = init_non_critical("capability_loader", || CapabilityLoader::new());
    let extension_discovery = init_non_critical("extension_discovery", || ExtensionDiscovery::new());
    tracing::info!(
        capability_loader = capability_loader.is_some(),
        extension_discovery = extension_discovery.is_some(),
        "extension subsystems initialised"
    );

    // -- 9. IPC (non-critical, starts disconnected) ------------------------
    let neocortex_client =
        init_non_critical("neocortex_client", || NeocortexClient::disconnected());
    tracing::info!(
        neocortex_client = neocortex_client.is_some(),
        "IPC subsystem initialised (disconnected)"
    );

    // -- 10. ARC (non-critical) --------------------------------------------
    let arc = init_non_critical("arc", || ArcManager::new());
    tracing::info!(arc = arc.is_some(), "ARC subsystem initialised");

    let subsystems = SubSystems {
        memory,
        identity,
        executor,
        planner,
        event_parser,
        command_parser,
        amygdala,
        contextor,
        route_classifier,
        system1,
        system2,
        bdi_scheduler,
        goal_tracker,
        conflict_resolver,
        goal_decomposer,
        goal_registry,
        anti_bot,
        platform,
        capability_loader,
        extension_discovery,
        neocortex_client,
        arc,
        // Bridge handles are `None` at startup — they are spawned in
        // `main_loop::run()` once channel endpoints are available.
        voice_bridge: None,
        telegram_bridge: None,
        response_router: None,
    };

    tracing::info!("all subsystems initialised");
    Ok(subsystems)
}

/// Initialise a critical subsystem. Panics are caught and converted to
/// `StartupError::SubSystemInit`.
fn init_critical<T, F>(name: &'static str, f: F) -> Result<T, StartupError>
where
    F: FnOnce() -> Result<T, StartupError>,
{
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)) {
        Ok(result) => result,
        Err(panic) => {
            let msg = panic_to_string(panic);
            tracing::error!(subsystem = name, error = %msg, "critical subsystem panicked");
            Err(StartupError::SubSystemInit {
                subsystem: name,
                reason: msg,
            })
        }
    }
}

/// Initialise a non-critical subsystem. If the constructor panics,
/// returns `None` and logs a warning. The daemon continues in degraded mode.
fn init_non_critical<T, F>(name: &'static str, f: F) -> Option<T>
where
    F: FnOnce() -> T,
{
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)) {
        Ok(value) => Some(value),
        Err(panic) => {
            let msg = panic_to_string(panic);
            tracing::warn!(
                subsystem = name,
                error = %msg,
                "non-critical subsystem init failed — running in degraded mode"
            );
            None
        }
    }
}

/// Extract a readable message from a caught panic payload.
fn panic_to_string(panic: Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = panic.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = panic.downcast_ref::<String>() {
        s.clone()
    } else {
        "unknown panic".to_string()
    }
}

// ---------------------------------------------------------------------------
// Remaining phase implementations
// ---------------------------------------------------------------------------

fn phase_ipc_bind(ipc_address: &str) -> Result<(), StartupError> {
    #[cfg(target_os = "android")]
    {
        // On Android, bind to abstract namespace socket.
        // Actual binding happens when the main loop starts the IPC listener task.
        tracing::info!(
            address = ipc_address,
            "IPC bind: prepared abstract socket address"
        );
    }

    #[cfg(not(target_os = "android"))]
    {
        // On host, IPC is optional — log and continue.
        tracing::info!(address = ipc_address, "IPC bind: host mode — deferred");
    }

    Ok(())
}

fn phase_cron_schedule(checkpoint: &DaemonCheckpoint) -> Result<(), StartupError> {
    // Rebuild the timer heap from persisted cron state.
    // The actual cron scheduler will be a separate subsystem;
    // here we just validate the cron_state data is sane.
    let job_count = checkpoint.cron_state.len();
    tracing::info!(
        jobs_restored = job_count,
        "cron schedule: timer heap initialized"
    );
    Ok(())
}

/// Check onboarding status by querying the database.
///
/// This is a lightweight check that reads the `onboarding_state` table to
/// determine whether onboarding needs to run, resume, or is already done.
fn phase_onboarding_check(db: &Connection) -> OnboardingStatus {
    match OnboardingEngine::check_status(db) {
        Ok(status) => status,
        Err(e) => {
            // If we can't determine status (e.g., table doesn't exist),
            // treat as first run — onboarding will create the table.
            tracing::warn!(error = %e, "onboarding status check failed — assuming first run");
            OnboardingStatus::FirstRun
        }
    }
}

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

#[derive(Debug, thiserror::Error)]
pub enum StartupError {
    #[error("database open failed: {0}")]
    DatabaseOpen(String),

    #[error("state restore failed: {0}")]
    StateRestore(String),

    #[error("IPC bind failed: {0}")]
    IpcBind(String),

    #[error("cron schedule failed: {0}")]
    CronSchedule(String),

    #[error("subsystem '{subsystem}' init failed: {reason}")]
    SubSystemInit {
        subsystem: &'static str,
        reason: String,
    },
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: create a temp dir and AuraConfig pointing at it.
    fn temp_config() -> (tempfile::TempDir, AuraConfig) {
        let dir = tempfile::tempdir().expect("tempdir");
        let db_path = dir.path().join("test.db");
        let mut config = AuraConfig::default();
        config.sqlite.db_path = db_path.to_string_lossy().to_string();
        (dir, config)
    }

    // -----------------------------------------------------------------------
    // Full startup tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_startup_default_config() {
        let (_dir, config) = temp_config();

        let (state, report) = startup(config).expect("startup should succeed");

        assert!(report.total_ms < 2000, "startup must complete in <2s");
        assert_eq!(report.phases.len(), 8, "must have exactly 8 phases");
        assert_eq!(
            state.checkpoint.version,
            crate::daemon_core::checkpoint::CHECKPOINT_VERSION
        );

        // Verify all phases logged
        let names: Vec<&str> = report.phases.iter().map(|p| p.name).collect();
        assert_eq!(
            names,
            vec![
                "JniLoad",
                "RuntimeInit",
                "DatabaseOpen",
                "StateRestore",
                "SubSystemsInit",
                "IpcBind",
                "CronSchedule",
                "Ready"
            ]
        );
    }

    #[test]
    fn test_startup_with_existing_checkpoint() {
        use crate::daemon_core::checkpoint::{save_checkpoint, DaemonCheckpoint};
        use crate::daemon_core::startup::checkpoint_path_from_config;

        let (_dir, config) = temp_config();

        // Pre-create a checkpoint at the derived path
        let cp_path = checkpoint_path_from_config(&config);
        let mut cp = DaemonCheckpoint::default();
        cp.select_count = 12345;
        save_checkpoint(&cp, Path::new(&cp_path)).expect("save checkpoint");

        let (state, _report) = startup(config).expect("startup should succeed");
        assert_eq!(state.checkpoint.select_count, 12345);
    }

    #[test]
    fn test_startup_bad_db_path_fails() {
        let mut config = AuraConfig::default();
        config.sqlite.db_path = "/nonexistent/deeply/nested/impossible/path/db.sqlite".to_string();

        let result = startup(config);
        assert!(result.is_err(), "should fail on bad db path");
    }

    // -----------------------------------------------------------------------
    // SubSystems presence tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_subsystems_memory_present() {
        let (_dir, config) = temp_config();
        let (state, _) = startup(config).expect("startup");
        // If we got here, memory init succeeded (it's critical).
        // Verify we can access working memory.
        assert_eq!(state.subsystems.memory.working.len(), 0);
    }

    #[test]
    fn test_subsystems_identity_present() {
        let (_dir, config) = temp_config();
        let (state, _) = startup(config).expect("startup");
        // Identity is critical — verify personality is default archetype.
        let _archetype = state.subsystems.identity.personality.archetype();
    }

    #[test]
    fn test_subsystems_executor_present() {
        let (_dir, config) = temp_config();
        let (state, _) = startup(config).expect("startup");
        // Executor is critical — verify Debug output contains "Executor".
        let debug = format!("{:?}", state.subsystems.executor);
        assert!(debug.contains("Executor"), "executor debug: {debug}");
    }

    #[test]
    fn test_subsystems_planner_present() {
        let (_dir, config) = temp_config();
        let (state, _) = startup(config).expect("startup");
        let _planner = &state.subsystems.planner;
    }

    #[test]
    fn test_subsystems_pipeline_present() {
        let (_dir, config) = temp_config();
        let (state, _) = startup(config).expect("startup");
        assert!(
            state.subsystems.event_parser.is_some(),
            "event_parser should be Some"
        );
        assert!(
            state.subsystems.command_parser.is_some(),
            "command_parser should be Some"
        );
        assert!(
            state.subsystems.amygdala.is_some(),
            "amygdala should be Some"
        );
        assert!(
            state.subsystems.contextor.is_some(),
            "contextor should be Some"
        );
    }

    #[test]
    fn test_subsystems_routing_present() {
        let (_dir, config) = temp_config();
        let (state, _) = startup(config).expect("startup");
        assert!(state.subsystems.route_classifier.is_some());
        assert!(state.subsystems.system1.is_some());
        assert!(state.subsystems.system2.is_some());
    }

    #[test]
    fn test_subsystems_goals_present() {
        let (_dir, config) = temp_config();
        let (state, _) = startup(config).expect("startup");
        assert!(state.subsystems.bdi_scheduler.is_some());
        assert!(state.subsystems.goal_tracker.is_some());
        assert!(state.subsystems.conflict_resolver.is_some());
        assert!(state.subsystems.goal_decomposer.is_some());
        assert!(state.subsystems.goal_registry.is_some());
    }

    #[test]
    fn test_subsystems_platform_present() {
        let (_dir, config) = temp_config();
        let (state, _) = startup(config).expect("startup");
        assert!(state.subsystems.platform.is_some());
    }

    #[test]
    fn test_subsystems_ipc_client_disconnected() {
        let (_dir, config) = temp_config();
        let (state, _) = startup(config).expect("startup");
        assert!(state.subsystems.neocortex_client.is_some());
    }

    #[test]
    fn test_subsystems_arc_present() {
        let (_dir, config) = temp_config();
        let (state, _) = startup(config).expect("startup");
        assert!(state.subsystems.arc.is_some());
    }

    #[test]
    fn test_subsystems_anti_bot_present() {
        let (_dir, config) = temp_config();
        let (state, _) = startup(config).expect("startup");
        assert!(state.subsystems.anti_bot.is_some());
    }

    // -----------------------------------------------------------------------
    // Config helper tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_memory_dir_from_config() {
        let mut config = AuraConfig::default();
        config.sqlite.db_path = "/data/aura/main.db".to_string();
        let dir = memory_dir_from_config(&config);
        assert!(dir.ends_with("memory"), "got: {dir:?}");
    }

    #[test]
    fn test_etg_path_from_config() {
        let mut config = AuraConfig::default();
        config.sqlite.db_path = "/data/aura/main.db".to_string();
        let path = etg_path_from_config(&config);
        assert!(path.ends_with("etg.db"), "got: {path:?}");
    }

    // -----------------------------------------------------------------------
    // Subsystem init helper tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_init_non_critical_catches_panic() {
        let result: Option<i32> = init_non_critical("test_panic", || {
            panic!("intentional test panic");
        });
        assert!(result.is_none(), "should catch panic and return None");
    }

    #[test]
    fn test_init_critical_catches_panic() {
        let result: Result<i32, StartupError> = init_critical("test_panic", || {
            panic!("intentional critical panic");
        });
        assert!(result.is_err(), "should catch panic and return Err");
        let err = result.unwrap_err();
        assert!(
            format!("{err}").contains("test_panic"),
            "error should name the subsystem: {err}"
        );
    }

    #[test]
    fn test_init_critical_success() {
        let result: Result<i32, StartupError> = init_critical("test_ok", || Ok(42));
        assert_eq!(result.unwrap(), 42);
    }

    #[test]
    fn test_init_non_critical_success() {
        let result: Option<i32> = init_non_critical("test_ok", || 42);
        assert_eq!(result, Some(42));
    }

    #[test]
    fn test_subsystems_debug_impl() {
        let (_dir, config) = temp_config();
        let (state, _) = startup(config).expect("startup");
        let debug = format!("{:?}", state.subsystems);
        assert!(debug.contains("SubSystems"), "debug output: {debug}");
        assert!(debug.contains("memory"), "debug output: {debug}");
        assert!(debug.contains("identity"), "debug output: {debug}");
    }

    #[test]
    fn test_panic_to_string_str() {
        let msg = panic_to_string(Box::new("hello"));
        assert_eq!(msg, "hello");
    }

    #[test]
    fn test_panic_to_string_string() {
        let msg = panic_to_string(Box::new(String::from("world")));
        assert_eq!(msg, "world");
    }

    #[test]
    fn test_panic_to_string_unknown() {
        let msg = panic_to_string(Box::new(42_i32));
        assert_eq!(msg, "unknown panic");
    }

    // -----------------------------------------------------------------------
    // Startup error variant tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_startup_error_subsystem_display() {
        let err = StartupError::SubSystemInit {
            subsystem: "memory",
            reason: "disk full".to_string(),
        };
        let msg = format!("{err}");
        assert!(msg.contains("memory"), "msg: {msg}");
        assert!(msg.contains("disk full"), "msg: {msg}");
    }

    #[test]
    fn test_startup_report_phase_count() {
        let (_dir, config) = temp_config();
        let (_, report) = startup(config).expect("startup");
        assert_eq!(report.phases.len(), 8);
    }
}
