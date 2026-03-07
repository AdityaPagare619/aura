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
use crate::routing::classifier::RouteClassifier;
use crate::routing::system1::System1;
use crate::routing::system2::System2;

use crate::arc::proactive::{ProactiveAction, ProactiveEngine};
use crate::arc::{ArcManager, ContextMode};
use crate::goals::scheduler::{BdiScheduler, ScoredGoal, ScoreComponents};
use crate::goals::tracker::GoalTracker;
use crate::identity::affective::MoodEvent;
use aura_types::identity::RelationshipStage;
use aura_types::power::PowerTier;
use crate::identity::personality::PersonalityInfluence;
use crate::telegram::TelegramConfig;
use crate::voice::VoiceEngine;

/// Maximum IPC payload size we'll accept for outbound writes (256 KB).
const MAX_IPC_PAYLOAD_BYTES: usize = 256 * 1024;

/// Cap on goals to prevent unbounded growth.
const MAX_ACTIVE_GOALS: usize = 64;

/// Timeout for IPC send operations (5 seconds).
const IPC_SEND_TIMEOUT_MS: u64 = 5_000;

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
    /// Emergency stop system — kills execution on anomalies or user request.
    emergency: EmergencyStop,

    // -- ARC subsystems (non-critical, Option<T> for degraded mode) --------
    /// BDI scheduler for goal deliberation (beliefs–desires–intentions).
    bdi_scheduler: Option<BdiScheduler>,
    /// Goal lifecycle tracker — mirrors checkpoint goals with richer state.
    goal_tracker: Option<GoalTracker>,
    /// Proactive engine — initiative budget, suggestions, routines, briefings.
    proactive: Option<ProactiveEngine>,
    /// ARC manager — owns cron, health, social, proactive, learning sub-engines.
    arc_manager: Option<ArcManager>,
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
            policy_gate: PolicyGate::allow_all(),
            audit_log: AuditLog::new(4096),
            emergency: EmergencyStop::new(),
            bdi_scheduler,
            goal_tracker,
            proactive,
            arc_manager,
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Current wall-clock time in milliseconds since UNIX epoch.
fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
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
        let summary = format!(
            "{}::{}",
            event.package_name, event.class_name
        );
        subs.contextor.set_screen_summary(Some(summary));
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
                    let s1_result = subs.system1.execute(&scored.parsed, now_ms());
                    if s1_result.success {
                        if let Some(ref resp) = s1_result.response_text {
                            send_response(&subs.response_tx, source, resp.clone()).await;
                        }
                        if let Some(plan) = s1_result.action_plan {
                            subs.system1.cache_plan(
                                &scored.parsed.content,
                                plan,
                                1.0,
                                now_ms(),
                            );
                        }
                    } else {
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

                // Update GoalTracker lifecycle.
                if let Some(ref mut tracker) = subs.goal_tracker {
                    let tracker_result = match &outcome {
                        react::TaskOutcome::Success { .. } => {
                            tracker.complete(goal_id, outcome_ts)
                        }
                        react::TaskOutcome::Failed { reason, .. } => {
                            tracker.fail(goal_id, reason.to_string(), outcome_ts)
                        }
                        react::TaskOutcome::CycleAborted { cycle_reason, .. } => {
                            tracker.fail(goal_id, cycle_reason.to_string(), outcome_ts)
                        }
                        react::TaskOutcome::Cancelled { .. } => {
                            tracker.fail(goal_id, "cancelled".to_string(), outcome_ts)
                        }
                    };
                    if let Err(e) = tracker_result {
                        tracing::warn!(
                            error = %e,
                            goal_id,
                            "GoalTracker lifecycle update failed"
                        );
                    }
                }
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
        send_response(&subs.response_tx, source, resp.clone()).await;

        // Record AURA's response as a conversation turn.
        subs.contextor.push_conversation_turn(
            aura_types::ipc::ConversationTurn {
                role: aura_types::ipc::Role::Assistant,
                content: resp.clone(),
                timestamp_ms: now_ms(),
            },
        );
    }

    // If System1 produced an action plan, execute it and cache for future use.
    if let Some(plan) = result.action_plan {
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
                subs.system1.cache_plan(text, plan, 1.0, now_ms());
                tracing::debug!("System1 plan cached after success");
            }
            _ => {
                tracing::debug!(?outcome, "System1 plan execution did not succeed — not caching");
            }
        }
    }
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
        // Memory snippets — replace the empty default with enriched retrieval.
        if !enriched.memory_context.is_empty() {
            pkg.memory_snippets = enriched.memory_context.clone();
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

            // Execute the plan through the react engine.
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

            // ── TRUTH framework validation ─────────────────────────────
            let truth_result = subs.identity.validate_response(&text);
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
            // Route to Direct by default (the bridge layer maps this to the
            // correct output based on the original request source).
            send_response(&subs.response_tx, InputSource::Direct, final_text.clone()).await;

            // Record as conversation turn.
            subs.contextor.push_conversation_turn(
                aura_types::ipc::ConversationTurn {
                    role: aura_types::ipc::Role::Assistant,
                    content: final_text,
                    timestamp_ms: now_ms(),
                },
            );
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
    if tick.job_name.contains("health_report") {
        tracing::info!("executing health report cron job");
        // Log working memory stats.
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
            let proactive_allowed = subs.identity.is_proactive_allowed(current_hour);
            tracing::debug!(
                proactive_allowed = proactive_allowed,
                hour = current_hour,
                "proactive consent check"
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
                                // Execute via react engine.
                                let mut policy_ctx = react::PolicyContext {
                                    gate: &mut subs.policy_gate,
                                    audit: &mut subs.audit_log,
                                };
                                let desc = format!("proactive_routine:{}", routine_id);
                                let (_outcome, _session) = react::execute_task(
                                    desc, 7, None, Some(&mut policy_ctx),
                                ).await;
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
            let thermal_nominal = true; // Would need thermal monitoring to be accurate
            
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
    } else {
        tracing::debug!(job_name = %tick.job_name, "unrecognised cron job — no-op");
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
