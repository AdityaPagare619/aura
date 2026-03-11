//! Channel definitions for the daemon's internal message passing.
//!
//! All inter-component communication flows through typed `tokio::sync::mpsc` channels.
//! This module defines the message types and a factory that creates all channels
//! with correct capacities.

use aura_types::events::{NotificationEvent, RawEvent};
use aura_types::ipc::NeocortexToDaemon;
use tokio::sync::mpsc;

// ---------------------------------------------------------------------------
// Channel message types
// ---------------------------------------------------------------------------

/// Identifies where a command originated.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum InputSource {
    /// Direct JNI / local chat.
    Direct,
    /// Voice pipeline (wake-word → STT → text).
    Voice,
    /// Telegram bot.
    Telegram {
        /// The Telegram chat ID that sent the message.
        chat_id: i64,
    },
}

impl InputSource {
    /// Returns a discriminant-level key that identifies the *kind* of source
    /// without considering per-instance details (e.g., Telegram chat ID).
    ///
    /// Used by [`ResponseRouter`](crate::bridge::router::ResponseRouter) to
    /// route daemon responses to the correct bridge.
    pub fn variant_key(&self) -> &'static str {
        match self {
            Self::Direct => "direct",
            Self::Voice => "voice",
            Self::Telegram { .. } => "telegram",
        }
    }
}

impl std::fmt::Display for InputSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Direct => write!(f, "direct"),
            Self::Voice => write!(f, "voice"),
            Self::Telegram { chat_id } => write!(f, "telegram:{chat_id}"),
        }
    }
}

/// Optional voice biomarker metadata attached to voice-originated commands.
#[derive(Debug, Clone, Default)]
pub struct VoiceMetadata {
    /// Duration of the utterance in milliseconds.
    pub duration_ms: u32,
    /// Emotional valence derived from voice biomarkers (−1.0 … 1.0).
    pub emotional_valence: Option<f32>,
    /// Emotional arousal derived from voice biomarkers (0.0 … 1.0).
    pub emotional_arousal: Option<f32>,
    /// Vocal stress level derived from voice biomarkers (0.0 … 1.0).
    pub emotional_stress: Option<f32>,
    /// Vocal fatigue level derived from voice biomarkers (0.0 … 1.0).
    pub emotional_fatigue: Option<f32>,
}

/// A user command received via JNI, voice, or Telegram.
#[derive(Debug, Clone)]
pub enum UserCommand {
    /// User typed or spoke a chat message.
    Chat {
        text: String,
        /// Where the message came from.
        source: InputSource,
        /// Voice biomarkers (present only for [`InputSource::Voice`]).
        voice_meta: Option<VoiceMetadata>,
    },
    /// User requested a task.
    TaskRequest {
        description: String,
        priority: u32,
        /// Where the request came from.
        source: InputSource,
    },
    /// User cancelled a task.
    CancelTask {
        task_id: String,
        /// Where the cancel came from.
        source: InputSource,
    },
    /// User switched execution profile.
    ProfileSwitch {
        profile: String,
        /// Where the switch came from.
        source: InputSource,
    },
}

impl UserCommand {
    /// Returns the [`InputSource`] for any variant.
    pub fn source(&self) -> &InputSource {
        match self {
            Self::Chat { source, .. }
            | Self::TaskRequest { source, .. }
            | Self::CancelTask { source, .. }
            | Self::ProfileSwitch { source, .. } => source,
        }
    }
}

/// A response the daemon wants to deliver to a specific input channel.
#[derive(Debug, Clone)]
pub struct DaemonResponse {
    /// Where this response should be routed.
    pub destination: InputSource,
    /// The response text.
    pub text: String,
}

pub type DaemonResponseTx = mpsc::Sender<DaemonResponse>;
pub type DaemonResponseRx = mpsc::Receiver<DaemonResponse>;

/// A request to write data to the database (batched via channel).
#[derive(Debug)]
pub enum DbWriteRequest {
    /// Store a telemetry event.
    Telemetry { payload: Vec<u8> },
    /// Store an episodic memory.
    Episode { content: String, importance: f32 },
    /// Update Amygdala baselines.
    AmygdalaBaseline { app: String, score: f32 },
    /// Arbitrary SQL (for arc jobs, etc.).
    RawSql { sql: String, params: Vec<String> },
}

/// A tick from the cron scheduler indicating a job should fire.
#[derive(Debug, Clone)]
pub struct CronTick {
    /// Identifier of the cron job that fired.
    pub job_id: u32,
    /// Human-readable name (e.g., "weekly_health_report").
    pub job_name: String,
    /// Scheduled fire time (monotonic ms).
    pub scheduled_at_ms: u64,
}

/// An IPC message to send to the Neocortex (outbound).
#[derive(Debug)]
pub struct IpcOutbound {
    pub payload: Vec<u8>,
}

// ---------------------------------------------------------------------------
// Type aliases for clarity
// ---------------------------------------------------------------------------

pub type A11yEventTx = mpsc::Sender<RawEvent>;
pub type A11yEventRx = mpsc::Receiver<RawEvent>;

pub type NotificationEventTx = mpsc::Sender<NotificationEvent>;
pub type NotificationEventRx = mpsc::Receiver<NotificationEvent>;

pub type UserCommandTx = mpsc::Sender<UserCommand>;
pub type UserCommandRx = mpsc::Receiver<UserCommand>;

pub type IpcOutboundTx = mpsc::Sender<IpcOutbound>;
pub type IpcOutboundRx = mpsc::Receiver<IpcOutbound>;

pub type IpcInboundTx = mpsc::Sender<NeocortexToDaemon>;
pub type IpcInboundRx = mpsc::Receiver<NeocortexToDaemon>;

pub type DbWriteTx = mpsc::Sender<DbWriteRequest>;
pub type DbWriteRx = mpsc::Receiver<DbWriteRequest>;

pub type CronTickTx = mpsc::Sender<CronTick>;
pub type CronTickRx = mpsc::Receiver<CronTick>;

/// Capacity for the daemon→bridge response channel.
const RESPONSE_CAPACITY: usize = 64;

// ---------------------------------------------------------------------------
// Channel capacities (from architecture spec)
// ---------------------------------------------------------------------------

/// AccessibilityService events — high frequency, bounded to prevent backpressure.
const A11Y_CAPACITY: usize = 64;
/// Notification events — slightly larger buffer for storm resilience.
const NOTIFICATION_CAPACITY: usize = 128;
/// User commands are rare — small buffer.
const USER_COMMAND_CAPACITY: usize = 16;
/// IPC outbound to Neocortex — only 1 outstanding request, but allow small queue.
const IPC_OUTBOUND_CAPACITY: usize = 4;
/// IPC inbound from Neocortex — small queue.
const IPC_INBOUND_CAPACITY: usize = 4;
/// Database write batching — larger to absorb telemetry bursts.
const DB_WRITE_CAPACITY: usize = 256;
/// Cron ticks — 31 jobs, but they don't all fire at once.
const CRON_TICK_CAPACITY: usize = 32;

// ---------------------------------------------------------------------------
// DaemonChannels — the aggregate
// ---------------------------------------------------------------------------

/// Holds all channel endpoints used by the daemon's main loop and subsystems.
///
/// Transmitter halves are cloned and distributed to producers (JNI bridge,
/// IPC listener, cron scheduler). Receiver halves are owned exclusively
/// by the main loop's `tokio::select!`.
pub struct DaemonChannels {
    // Accessibility events
    pub a11y_tx: A11yEventTx,
    pub a11y_rx: A11yEventRx,

    // Notifications
    pub notification_tx: NotificationEventTx,
    pub notification_rx: NotificationEventRx,

    // User commands
    pub user_command_tx: UserCommandTx,
    pub user_command_rx: UserCommandRx,

    // IPC outbound (daemon -> neocortex)
    pub ipc_outbound_tx: IpcOutboundTx,
    pub ipc_outbound_rx: IpcOutboundRx,

    // IPC inbound (neocortex -> daemon)
    pub ipc_inbound_tx: IpcInboundTx,
    pub ipc_inbound_rx: IpcInboundRx,

    // Database write queue
    pub db_write_tx: DbWriteTx,
    pub db_write_rx: DbWriteRx,

    // Cron tick events
    pub cron_tick_tx: CronTickTx,
    pub cron_tick_rx: CronTickRx,

    // Daemon → bridge response channel
    pub response_tx: DaemonResponseTx,
    pub response_rx: DaemonResponseRx,
}

/// Receiver halves only — owned exclusively by the main loop's `select!`.
///
/// Created by [`DaemonChannels::split_for_run`] so that the main loop does
/// **not** keep tx halves alive (which would prevent channel closure detection).
pub struct ChannelReceivers {
    pub a11y_rx: A11yEventRx,
    pub notification_rx: NotificationEventRx,
    pub user_command_rx: UserCommandRx,
    pub ipc_outbound_rx: IpcOutboundRx,
    pub ipc_inbound_rx: IpcInboundRx,
    pub db_write_rx: DbWriteRx,
    pub cron_tick_rx: CronTickRx,
    pub response_rx: DaemonResponseRx,
}

/// Transmitter halves only — distributed to producers.
///
/// Created by [`DaemonChannels::split_for_run`].
pub struct ChannelSenders {
    pub a11y_tx: A11yEventTx,
    pub notification_tx: NotificationEventTx,
    pub user_command_tx: UserCommandTx,
    pub ipc_outbound_tx: IpcOutboundTx,
    pub ipc_inbound_tx: IpcInboundTx,
    pub db_write_tx: DbWriteTx,
    pub cron_tick_tx: CronTickTx,
    pub response_tx: DaemonResponseTx,
}

impl DaemonChannels {
    /// Create all channel pairs with architecture-specified capacities.
    pub fn new() -> Self {
        let (a11y_tx, a11y_rx) = mpsc::channel(A11Y_CAPACITY);
        let (notification_tx, notification_rx) = mpsc::channel(NOTIFICATION_CAPACITY);
        let (user_command_tx, user_command_rx) = mpsc::channel(USER_COMMAND_CAPACITY);
        let (ipc_outbound_tx, ipc_outbound_rx) = mpsc::channel(IPC_OUTBOUND_CAPACITY);
        let (ipc_inbound_tx, ipc_inbound_rx) = mpsc::channel(IPC_INBOUND_CAPACITY);
        let (db_write_tx, db_write_rx) = mpsc::channel(DB_WRITE_CAPACITY);
        let (cron_tick_tx, cron_tick_rx) = mpsc::channel(CRON_TICK_CAPACITY);
        let (response_tx, response_rx) = mpsc::channel(RESPONSE_CAPACITY);

        Self {
            a11y_tx,
            a11y_rx,
            notification_tx,
            notification_rx,
            user_command_tx,
            user_command_rx,
            ipc_outbound_tx,
            ipc_outbound_rx,
            ipc_inbound_tx,
            ipc_inbound_rx,
            db_write_tx,
            db_write_rx,
            cron_tick_tx,
            cron_tick_rx,
            response_tx,
            response_rx,
        }
    }

    /// Split into separate sender/receiver structs.
    ///
    /// The main loop takes ownership of `ChannelReceivers` and drops the
    /// `ChannelSenders` that were inside `DaemonState`, so channels close
    /// properly when external producers drop their cloned tx handles.
    pub fn split(self) -> (ChannelSenders, ChannelReceivers) {
        let senders = ChannelSenders {
            a11y_tx: self.a11y_tx,
            notification_tx: self.notification_tx,
            user_command_tx: self.user_command_tx,
            ipc_outbound_tx: self.ipc_outbound_tx,
            ipc_inbound_tx: self.ipc_inbound_tx,
            db_write_tx: self.db_write_tx,
            cron_tick_tx: self.cron_tick_tx,
            response_tx: self.response_tx,
        };
        let receivers = ChannelReceivers {
            a11y_rx: self.a11y_rx,
            notification_rx: self.notification_rx,
            user_command_rx: self.user_command_rx,
            ipc_outbound_rx: self.ipc_outbound_rx,
            ipc_inbound_rx: self.ipc_inbound_rx,
            db_write_rx: self.db_write_rx,
            cron_tick_rx: self.cron_tick_rx,
            response_rx: self.response_rx,
        };
        (senders, receivers)
    }
}

impl Default for DaemonChannels {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_channels_created_with_correct_capacity() {
        let channels = DaemonChannels::new();

        // Verify we can send up to capacity without blocking.
        // A11y channel capacity = 64; send a few to prove it works.
        for i in 0..10 {
            let event = RawEvent {
                event_type: i,
                package_name: "com.test".to_string(),
                class_name: "TestClass".to_string(),
                text: None,
                content_description: None,
                timestamp_ms: 1000 + u64::from(i),
                source_node_id: None,
            };
            channels
                .a11y_tx
                .send(event)
                .await
                .expect("a11y channel should accept events");
        }

        // Verify we can receive them back.
        let mut rx = channels.a11y_rx;
        for _ in 0..10 {
            let evt = rx.recv().await.expect("should receive event");
            assert_eq!(evt.package_name, "com.test");
        }
    }

    #[tokio::test]
    async fn test_user_command_channel() {
        let channels = DaemonChannels::new();
        channels
            .user_command_tx
            .send(UserCommand::Chat {
                text: "hello".to_string(),
                source: InputSource::Direct,
                voice_meta: None,
            })
            .await
            .expect("send should succeed");

        let mut rx = channels.user_command_rx;
        let cmd = rx.recv().await.expect("should receive command");
        assert!(matches!(cmd, UserCommand::Chat { text, .. } if text == "hello"));
    }

    #[tokio::test]
    async fn test_channel_close_signals_none() {
        let channels = DaemonChannels::new();
        let mut rx = channels.a11y_rx;
        drop(channels.a11y_tx);
        assert!(rx.recv().await.is_none(), "closed channel should yield None");
    }
}
