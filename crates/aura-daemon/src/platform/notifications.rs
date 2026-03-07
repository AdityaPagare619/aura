//! Android notification management for AURA's foreground service and user-facing alerts.
//!
//! AURA uses Android notification channels to categorize and prioritize its
//! communications with the user:
//!
//! - **Foreground Service**: persistent notification required by Android to keep
//!   the daemon alive.  Low priority, non-dismissable.
//! - **Proactive Suggestions**: context-aware suggestions surfaced by the bio-
//!   cognitive loop.  Default priority.
//! - **Goal Completion/Failure**: results of agentic task execution.
//! - **Health & Social Reminders**: well-being and relationship check-ins.
//! - **System Status**: power warnings, thermal throttle alerts, error reports.
//!
//! On Android, each channel maps to an `android.app.NotificationChannel` created
//! at startup via JNI.  On non-Android hosts, notifications are logged via
//! `tracing` for development visibility.
//!
//! # Spec Reference
//!
//! See `AURA-V4-POWER-AGENCY-REBALANCE.md` §3 — ForegroundService Lifecycle.

use std::time::Instant;

use aura_types::errors::PlatformError;
use serde::{Deserialize, Serialize};

// ─── Constants ──────────────────────────────────────────────────────────────

/// Maximum number of notifications in the history ring.
const MAX_NOTIFICATION_HISTORY: usize = 64;

/// Maximum length of notification title (characters).
const MAX_TITLE_LEN: usize = 80;

/// Maximum length of notification body (characters).
const MAX_BODY_LEN: usize = 500;

/// Android notification ID for the persistent foreground service notification.
const FOREGROUND_NOTIFICATION_ID: i32 = 1;

// ─── Notification Channel ──────────────────────────────────────────────────

/// Categorized notification channels matching Android `NotificationChannel`s.
///
/// Each channel has a fixed importance level and is created at daemon startup.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum NotificationChannel {
    /// Persistent foreground service notification (IMPORTANCE_LOW).
    /// Required by Android to keep the daemon alive.
    ForegroundService,
    /// Proactive context-aware suggestions (IMPORTANCE_DEFAULT).
    ProactiveSuggestion,
    /// Goal execution completion or failure (IMPORTANCE_DEFAULT).
    GoalResult,
    /// Health, wellness, and social reminders (IMPORTANCE_DEFAULT).
    HealthReminder,
    /// System status: power warnings, thermal alerts, errors (IMPORTANCE_HIGH).
    SystemStatus,
}

impl NotificationChannel {
    /// Android channel ID string for JNI registration.
    pub fn channel_id(&self) -> &'static str {
        match self {
            Self::ForegroundService => "aura_foreground",
            Self::ProactiveSuggestion => "aura_proactive",
            Self::GoalResult => "aura_goals",
            Self::HealthReminder => "aura_health",
            Self::SystemStatus => "aura_system",
        }
    }

    /// Human-readable channel name shown in Android settings.
    pub fn display_name(&self) -> &'static str {
        match self {
            Self::ForegroundService => "AURA Background Service",
            Self::ProactiveSuggestion => "Proactive Suggestions",
            Self::GoalResult => "Goal Results",
            Self::HealthReminder => "Health & Social Reminders",
            Self::SystemStatus => "System Status",
        }
    }

    /// Android importance level (maps to NotificationManager.IMPORTANCE_*).
    pub fn importance(&self) -> AndroidImportance {
        match self {
            Self::ForegroundService => AndroidImportance::Low,
            Self::ProactiveSuggestion => AndroidImportance::Default,
            Self::GoalResult => AndroidImportance::Default,
            Self::HealthReminder => AndroidImportance::Default,
            Self::SystemStatus => AndroidImportance::High,
        }
    }

    /// All channels (for registration at startup).
    pub fn all() -> &'static [NotificationChannel] {
        &[
            Self::ForegroundService,
            Self::ProactiveSuggestion,
            Self::GoalResult,
            Self::HealthReminder,
            Self::SystemStatus,
        ]
    }
}

impl std::fmt::Display for NotificationChannel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.display_name())
    }
}

// ─── Android Importance ─────────────────────────────────────────────────────

/// Maps to `android.app.NotificationManager.IMPORTANCE_*` constants.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AndroidImportance {
    /// No sound or visual interruption (drawer only).
    Low,
    /// Sound, appears in status bar.
    Default,
    /// Sound + heads-up display.
    High,
}

impl AndroidImportance {
    /// Numeric value matching the Android SDK constant.
    pub fn to_android_int(self) -> i32 {
        match self {
            Self::Low => 2,     // IMPORTANCE_LOW
            Self::Default => 3, // IMPORTANCE_DEFAULT
            Self::High => 4,    // IMPORTANCE_HIGH
        }
    }
}

// ─── Notification Priority ──────────────────────────────────────────────────

/// Priority level for individual notifications within a channel.
///
/// This controls ordering and visual emphasis within AURA's internal
/// notification queue, independent of the Android channel importance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum NotificationPriority {
    /// Lowest priority — informational, no urgency.
    Low,
    /// Standard priority — normal user-facing alerts.
    Normal,
    /// Elevated — time-sensitive or important.
    High,
    /// Urgent — requires immediate attention.
    Urgent,
}

impl std::fmt::Display for NotificationPriority {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Low => write!(f, "Low"),
            Self::Normal => write!(f, "Normal"),
            Self::High => write!(f, "High"),
            Self::Urgent => write!(f, "Urgent"),
        }
    }
}

// ─── Notification Payload ───────────────────────────────────────────────────

/// A notification to be posted to Android or logged on host.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotificationPayload {
    /// Unique notification ID (Android uses this for update/replace).
    pub id: i32,
    /// Channel this notification belongs to.
    pub channel: NotificationChannel,
    /// Internal priority for queue ordering.
    pub priority: NotificationPriority,
    /// Title line (truncated to [`MAX_TITLE_LEN`]).
    pub title: String,
    /// Body text (truncated to [`MAX_BODY_LEN`]).
    pub body: String,
    /// Whether the notification is ongoing (non-dismissable).
    pub ongoing: bool,
    /// Optional action label (e.g., "Open", "Dismiss", "View Goal").
    pub action_label: Option<String>,
}

impl NotificationPayload {
    /// Create a new notification payload, truncating title/body to limits.
    pub fn new(
        id: i32,
        channel: NotificationChannel,
        priority: NotificationPriority,
        title: impl Into<String>,
        body: impl Into<String>,
    ) -> Self {
        let mut title = title.into();
        let mut body = body.into();

        // Bounded truncation — never allocate unbounded strings.
        if title.len() > MAX_TITLE_LEN {
            title.truncate(MAX_TITLE_LEN);
        }
        if body.len() > MAX_BODY_LEN {
            body.truncate(MAX_BODY_LEN);
        }

        Self {
            id,
            channel,
            priority,
            title,
            body,
            ongoing: false,
            action_label: None,
        }
    }

    /// Mark this notification as ongoing (non-dismissable).
    pub fn with_ongoing(mut self, ongoing: bool) -> Self {
        self.ongoing = ongoing;
        self
    }

    /// Add an action button label.
    pub fn with_action(mut self, label: impl Into<String>) -> Self {
        self.action_label = Some(label.into());
        self
    }
}

// ─── Notification Record ────────────────────────────────────────────────────

/// Record of a posted notification for history tracking.
#[derive(Debug, Clone)]
#[allow(dead_code)] // Fields used for diagnostics and future query APIs.
struct NotificationRecord {
    id: i32,
    channel: NotificationChannel,
    priority: NotificationPriority,
    title: String,
    posted_at: Instant,
}

// ─── NotificationManager ────────────────────────────────────────────────────

/// Manages Android notification posting, channel registration, and history.
///
/// On Android, this uses JNI to interact with `NotificationManager`.
/// On host builds, notifications are logged via `tracing`.
pub struct NotificationManager {
    /// Whether notification channels have been registered.
    channels_registered: bool,
    /// Whether the foreground service notification is currently active.
    foreground_active: bool,
    /// Current foreground notification content (for update detection).
    foreground_title: String,
    foreground_body: String,
    /// Notification history ring (bounded to [`MAX_NOTIFICATION_HISTORY`]).
    history: Vec<NotificationRecord>,
    /// Counter for auto-incrementing notification IDs.
    /// Starts at 100 to avoid collision with the foreground service ID (1).
    next_id: i32,
    /// Total notifications posted since creation.
    total_posted: u64,
}

impl NotificationManager {
    /// Create a new `NotificationManager`.
    ///
    /// Call [`register_channels`](Self::register_channels) during startup
    /// before posting any notifications.
    pub fn new() -> Self {
        Self {
            channels_registered: false,
            foreground_active: false,
            foreground_title: String::new(),
            foreground_body: String::new(),
            history: Vec::new(),
            next_id: 100,
            total_posted: 0,
        }
    }

    /// Register all notification channels with the Android OS.
    ///
    /// Idempotent — safe to call multiple times. On Android, this creates
    /// the `NotificationChannel` objects via JNI. On host, it's a no-op
    /// logged for visibility.
    pub fn register_channels(&mut self) -> Result<(), PlatformError> {
        if self.channels_registered {
            return Ok(());
        }

        for channel in NotificationChannel::all() {
            register_platform_channel(channel)?;
        }

        self.channels_registered = true;
        tracing::info!(
            channel_count = NotificationChannel::all().len(),
            "notification channels registered"
        );
        Ok(())
    }

    /// Post or update the persistent foreground service notification.
    ///
    /// This notification is required by Android to keep the daemon alive
    /// as a foreground service. It uses a fixed notification ID and is
    /// marked as ongoing (non-dismissable).
    pub fn post_foreground(&mut self, title: &str, body: &str) -> Result<(), PlatformError> {
        // Skip redundant updates.
        if self.foreground_active && self.foreground_title == title && self.foreground_body == body
        {
            return Ok(());
        }

        let payload = NotificationPayload::new(
            FOREGROUND_NOTIFICATION_ID,
            NotificationChannel::ForegroundService,
            NotificationPriority::Low,
            title,
            body,
        )
        .with_ongoing(true);

        self.post_notification(&payload)?;

        self.foreground_active = true;
        self.foreground_title = title.to_string();
        self.foreground_body = body.to_string();

        Ok(())
    }

    /// Post a proactive suggestion notification.
    pub fn post_suggestion(&mut self, title: &str, body: &str) -> Result<i32, PlatformError> {
        let id = self.allocate_id();
        let payload = NotificationPayload::new(
            id,
            NotificationChannel::ProactiveSuggestion,
            NotificationPriority::Normal,
            title,
            body,
        )
        .with_action("View");

        self.post_notification(&payload)?;
        Ok(id)
    }

    /// Post a goal completion notification.
    pub fn post_goal_completion(
        &mut self,
        goal_name: &str,
        summary: &str,
    ) -> Result<i32, PlatformError> {
        let id = self.allocate_id();
        let title = format!("Goal Complete: {goal_name}");
        let payload = NotificationPayload::new(
            id,
            NotificationChannel::GoalResult,
            NotificationPriority::Normal,
            title,
            summary,
        )
        .with_action("View Details");

        self.post_notification(&payload)?;
        Ok(id)
    }

    /// Post a goal failure notification.
    pub fn post_goal_failure(
        &mut self,
        goal_name: &str,
        reason: &str,
    ) -> Result<i32, PlatformError> {
        let id = self.allocate_id();
        let title = format!("Goal Failed: {goal_name}");
        let payload = NotificationPayload::new(
            id,
            NotificationChannel::GoalResult,
            NotificationPriority::High,
            title,
            reason,
        )
        .with_action("Retry");

        self.post_notification(&payload)?;
        Ok(id)
    }

    /// Post a health or social reminder notification.
    pub fn post_health_reminder(&mut self, title: &str, body: &str) -> Result<i32, PlatformError> {
        let id = self.allocate_id();
        let payload = NotificationPayload::new(
            id,
            NotificationChannel::HealthReminder,
            NotificationPriority::Normal,
            title,
            body,
        );

        self.post_notification(&payload)?;
        Ok(id)
    }

    /// Post a system status notification (power warning, thermal alert, etc.).
    pub fn post_system_status(
        &mut self,
        title: &str,
        body: &str,
        priority: NotificationPriority,
    ) -> Result<i32, PlatformError> {
        let id = self.allocate_id();
        let payload =
            NotificationPayload::new(id, NotificationChannel::SystemStatus, priority, title, body);

        self.post_notification(&payload)?;
        Ok(id)
    }

    /// Cancel a previously posted notification by ID.
    pub fn cancel_notification(&self, id: i32) -> Result<(), PlatformError> {
        cancel_platform_notification(id)?;
        tracing::debug!(id, "notification cancelled");
        Ok(())
    }

    /// Remove the foreground service notification.
    ///
    /// Call this during graceful shutdown. On Android, this also calls
    /// `stopForeground()` via JNI.
    pub fn remove_foreground(&mut self) -> Result<(), PlatformError> {
        if !self.foreground_active {
            return Ok(());
        }

        self.cancel_notification(FOREGROUND_NOTIFICATION_ID)?;
        self.foreground_active = false;
        self.foreground_title.clear();
        self.foreground_body.clear();

        tracing::info!("foreground service notification removed");
        Ok(())
    }

    // ─── Internal ───────────────────────────────────────────────────────

    /// Core posting logic — dispatches to platform and records history.
    fn post_notification(&mut self, payload: &NotificationPayload) -> Result<(), PlatformError> {
        post_platform_notification(payload)?;

        // Record in history (bounded ring buffer).
        if self.history.len() >= MAX_NOTIFICATION_HISTORY {
            self.history.remove(0);
        }
        self.history.push(NotificationRecord {
            id: payload.id,
            channel: payload.channel,
            priority: payload.priority,
            title: payload.title.clone(),
            posted_at: Instant::now(),
        });

        self.total_posted += 1;

        tracing::debug!(
            id = payload.id,
            channel = %payload.channel,
            priority = %payload.priority,
            title = %payload.title,
            "notification posted"
        );

        Ok(())
    }

    /// Allocate the next notification ID (bounded, wraps at i32::MAX).
    fn allocate_id(&mut self) -> i32 {
        let id = self.next_id;
        // Wrap around, skipping the reserved foreground ID range.
        self.next_id = if self.next_id >= i32::MAX - 1 {
            100
        } else {
            self.next_id + 1
        };
        id
    }

    // ─── Read-only Queries ──────────────────────────────────────────────

    /// Whether channels have been registered.
    pub fn channels_registered(&self) -> bool {
        self.channels_registered
    }

    /// Whether the foreground service notification is active.
    pub fn is_foreground_active(&self) -> bool {
        self.foreground_active
    }

    /// Total number of notifications posted since creation.
    pub fn total_posted(&self) -> u64 {
        self.total_posted
    }

    /// Number of entries in the history ring.
    pub fn history_len(&self) -> usize {
        self.history.len()
    }

    /// Recent notification history count by channel.
    pub fn history_count_for_channel(&self, channel: NotificationChannel) -> usize {
        self.history.iter().filter(|r| r.channel == channel).count()
    }
}

impl Default for NotificationManager {
    fn default() -> Self {
        Self::new()
    }
}

// ─── Platform Dispatch (cfg-gated) ──────────────────────────────────────────

/// Register a single notification channel with the Android OS.
#[cfg(target_os = "android")]
fn register_platform_channel(channel: &NotificationChannel) -> Result<(), PlatformError> {
    super::jni_bridge::jni_register_notification_channel(
        channel.channel_id(),
        channel.display_name(),
        channel.importance().to_android_int(),
    )
}

#[cfg(not(target_os = "android"))]
fn register_platform_channel(channel: &NotificationChannel) -> Result<(), PlatformError> {
    tracing::trace!(
        channel_id = channel.channel_id(),
        name = channel.display_name(),
        importance = ?channel.importance(),
        "host stub: registered notification channel"
    );
    Ok(())
}

/// Post a notification to the Android notification tray.
#[cfg(target_os = "android")]
fn post_platform_notification(payload: &NotificationPayload) -> Result<(), PlatformError> {
    super::jni_bridge::jni_post_notification(
        payload.id,
        payload.channel.channel_id(),
        &payload.title,
        &payload.body,
        payload.ongoing,
    )
}

#[cfg(not(target_os = "android"))]
fn post_platform_notification(payload: &NotificationPayload) -> Result<(), PlatformError> {
    tracing::trace!(
        id = payload.id,
        channel = payload.channel.channel_id(),
        title = %payload.title,
        ongoing = payload.ongoing,
        "host stub: posted notification"
    );
    Ok(())
}

/// Cancel a notification by ID.
#[cfg(target_os = "android")]
fn cancel_platform_notification(id: i32) -> Result<(), PlatformError> {
    super::jni_bridge::jni_cancel_notification(id)
}

#[cfg(not(target_os = "android"))]
fn cancel_platform_notification(id: i32) -> Result<(), PlatformError> {
    tracing::trace!(id, "host stub: cancelled notification");
    Ok(())
}

// ─── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_notification_manager_default() {
        let nm = NotificationManager::new();
        assert!(!nm.channels_registered());
        assert!(!nm.is_foreground_active());
        assert_eq!(nm.total_posted(), 0);
        assert_eq!(nm.history_len(), 0);
    }

    #[test]
    fn test_register_channels_idempotent() {
        let mut nm = NotificationManager::new();
        nm.register_channels().expect("first registration");
        assert!(nm.channels_registered());

        // Second call should be a no-op, not an error.
        nm.register_channels().expect("idempotent re-registration");
        assert!(nm.channels_registered());
    }

    #[test]
    fn test_foreground_notification_lifecycle() {
        let mut nm = NotificationManager::new();
        nm.register_channels().expect("register");

        // Post foreground.
        nm.post_foreground("AURA Running", "Monitoring your device")
            .expect("post foreground");
        assert!(nm.is_foreground_active());
        assert_eq!(nm.total_posted(), 1);

        // Redundant update should be skipped (no new post).
        nm.post_foreground("AURA Running", "Monitoring your device")
            .expect("redundant update");
        assert_eq!(nm.total_posted(), 1);

        // Different content should trigger an update.
        nm.post_foreground("AURA Running", "Conserve mode active")
            .expect("updated foreground");
        assert_eq!(nm.total_posted(), 2);

        // Remove foreground.
        nm.remove_foreground().expect("remove foreground");
        assert!(!nm.is_foreground_active());

        // Removing again should be idempotent.
        nm.remove_foreground().expect("idempotent remove");
    }

    #[test]
    fn test_post_various_notification_types() {
        let mut nm = NotificationManager::new();
        nm.register_channels().expect("register");

        let id1 = nm
            .post_suggestion("Try this", "Based on your schedule")
            .expect("suggestion");
        assert!(id1 >= 100);

        let id2 = nm
            .post_goal_completion("Send email", "Email sent to Bob")
            .expect("goal completion");
        assert_ne!(id1, id2);

        let id3 = nm
            .post_goal_failure("Book flight", "No availability found")
            .expect("goal failure");
        assert_ne!(id2, id3);

        let id4 = nm
            .post_health_reminder("Hydration", "Time to drink water")
            .expect("health reminder");
        assert_ne!(id3, id4);

        let id5 = nm
            .post_system_status("Low Battery", "10% remaining", NotificationPriority::High)
            .expect("system status");
        assert_ne!(id4, id5);

        assert_eq!(nm.total_posted(), 5);
        assert_eq!(nm.history_len(), 5);
    }

    #[test]
    fn test_notification_id_auto_increment() {
        let mut nm = NotificationManager::new();
        nm.register_channels().expect("register");

        let id1 = nm.post_suggestion("A", "Body A").expect("first");
        let id2 = nm.post_suggestion("B", "Body B").expect("second");
        let id3 = nm.post_suggestion("C", "Body C").expect("third");

        assert_eq!(id2, id1 + 1);
        assert_eq!(id3, id2 + 1);
    }

    #[test]
    fn test_cancel_notification() {
        let mut nm = NotificationManager::new();
        nm.register_channels().expect("register");

        let id = nm.post_suggestion("Test", "To be cancelled").expect("post");

        nm.cancel_notification(id).expect("cancel should succeed");
    }

    #[test]
    fn test_history_ring_bounded() {
        let mut nm = NotificationManager::new();
        nm.register_channels().expect("register");

        // Post more than MAX_NOTIFICATION_HISTORY notifications.
        for i in 0..(MAX_NOTIFICATION_HISTORY + 10) {
            nm.post_suggestion(&format!("Suggestion {i}"), &format!("Body {i}"))
                .expect("post");
        }

        assert_eq!(nm.history_len(), MAX_NOTIFICATION_HISTORY);
        assert_eq!(nm.total_posted(), (MAX_NOTIFICATION_HISTORY + 10) as u64);
    }

    #[test]
    fn test_notification_payload_truncation() {
        let long_title = "A".repeat(200);
        let long_body = "B".repeat(1000);

        let payload = NotificationPayload::new(
            1,
            NotificationChannel::SystemStatus,
            NotificationPriority::Urgent,
            long_title,
            long_body,
        );

        assert!(payload.title.len() <= MAX_TITLE_LEN);
        assert!(payload.body.len() <= MAX_BODY_LEN);
    }

    #[test]
    fn test_notification_channel_properties() {
        // Verify all channels have distinct IDs.
        let channels = NotificationChannel::all();
        assert_eq!(channels.len(), 5);

        let ids: Vec<&str> = channels.iter().map(|c| c.channel_id()).collect();
        let mut unique_ids = ids.clone();
        unique_ids.sort();
        unique_ids.dedup();
        assert_eq!(ids.len(), unique_ids.len(), "channel IDs must be unique");

        // Verify importance levels.
        assert_eq!(
            NotificationChannel::ForegroundService.importance(),
            AndroidImportance::Low
        );
        assert_eq!(
            NotificationChannel::SystemStatus.importance(),
            AndroidImportance::High
        );
    }

    #[test]
    fn test_android_importance_values() {
        assert_eq!(AndroidImportance::Low.to_android_int(), 2);
        assert_eq!(AndroidImportance::Default.to_android_int(), 3);
        assert_eq!(AndroidImportance::High.to_android_int(), 4);
    }

    #[test]
    fn test_notification_priority_ordering() {
        assert!(NotificationPriority::Low < NotificationPriority::Normal);
        assert!(NotificationPriority::Normal < NotificationPriority::High);
        assert!(NotificationPriority::High < NotificationPriority::Urgent);
    }

    #[test]
    fn test_payload_builder_pattern() {
        let payload = NotificationPayload::new(
            42,
            NotificationChannel::GoalResult,
            NotificationPriority::Normal,
            "Test",
            "Body",
        )
        .with_ongoing(true)
        .with_action("Open");

        assert!(payload.ongoing);
        assert_eq!(payload.action_label.as_deref(), Some("Open"));
        assert_eq!(payload.id, 42);
    }
}
