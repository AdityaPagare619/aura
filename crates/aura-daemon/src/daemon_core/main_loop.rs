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
        ArcManager, ContextMode, CronScheduler,
    },
    bridge::{
        router::ResponseRouter,
        spawn_bridge,
        system_api::{SystemBridge, SystemCommand, SystemResult},
        telegram_bridge::TelegramBridge,
    },
    daemon_core::{
        channels::{
            CronTick, CronTickTx, DaemonResponse, DaemonResponseTx, DbWriteRequest, InputSource,
            IpcOutbound, UserCommand,
        },
        checkpoint::save_checkpoint,
        proactive_dispatcher::{
            arc_trigger_to_ipc, should_dispatch, trigger_to_ipc, AlertDirection, ProactiveTrigger,
        },
        react::{self, ReactEngine},
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
    ipc::{NeocortexClient, NeocortexProcess},
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
    screen::{
        actions::AndroidScreenProvider,
        detect_app_state,
        extract_screen_summary,
        AppState,
        ScreenCache,
    },
    telegram::{
        queue::MessageQueue,
        reqwest_backend::ReqwestHttpBackend,
        TelegramConfig,
        TelegramEngine,
    },
};
#[cfg(feature = "voice")]
use crate::{bridge::voice_bridge::VoiceBridge, voice::VoiceEngine};