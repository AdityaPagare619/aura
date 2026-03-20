//! Forest Guardian: Attention Protection & Anti-Lock-In Monitor
//!
//! # Strategic System Fit
//! AURA is the anti-cloud. Standard algorithms optimize for maximized Time-On-Device (TOD).
//! AURA optimizes for Time-Well-Spent. The Forest Guardian actively monitors the user's
//! app usage patterns via the ETG/Accessibility Service, and if it detects harmful digital
//! consumption patterns, it surfaces raw factual context for the LLM to intervene appropriately.
//!
//! # Architecture boundary
//! This module produces FACTS only. It NEVER generates user-facing language.
//! All intervention wording is produced by the LLM, which receives an
//! `AttentionContext` struct and decides tone/phrasing based on the full
//! relationship + personality context it already holds.
//!
//! # Precise System Modeling
//! - State: `AttentionState` (HealthyInteraction, ContextThrashing, AttentionLockIn,
//!   NotificationSpiral, CompulsiveAppReturn, PreSleepScreen)
//! - Events: `AppSwitch`, `SessionDurationExceeded`, `RapidScrollSpike`, `Notification`, `ScreenOn`
//!
//! # Pattern Detection (5 patterns)
//! 1. AttentionLockIn: >15min in infinite-scroll app
//! 2. ContextThrashing: >5 rapid app switches
//! 3. NotificationSpiral: >10 notifications in 5 minutes
//! 4. CompulsiveAppReturn: same app opened >5 times in 10 minutes
//! 5. PreSleepScreen: screen on after 22:00

use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

use aura_types::identity::{OceanTraits, RelationshipStage};
use serde::{Deserialize, Serialize};
use tracing::warn;

/// Defines the classification of the user's current attention span on the device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AttentionState {
    /// Intentional, variable interaction.
    HealthyInteraction,
    /// Rapid switching between apps with low dwell time (context thrashing).
    ContextThrashing(u32),
    /// High dwell time in infinite-scroll interfaces with repetitive actions.
    AttentionLockIn(Duration),
    /// Excessive notifications in a short time window.
    NotificationSpiral { count: u32, window_secs: u64 },
    /// Repeatedly returning to the same app obsessively.
    CompulsiveAppReturn {
        app_package: String,
        return_count: u32,
        window_mins: u64,
    },
    /// Screen active during pre-sleep hours.
    PreSleepScreen { sleep_hour: u8, current_hour: u8 },
}

/// Intervention level for escalating responses.
/// AURA HELPS but never COERCES — each level is a suggestion, never forced.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, Default)]
pub enum InterventionLevel {
    /// No intervention needed.
    #[default]
    None = 0,
    /// L1 (0.15): Gentle reminder — first threshold breach.
    L1GentleReminder = 1,
    /// L2 (0.20): Stronger suggestion — second breach within 2 hours.
    L2SoftBoundary = 2,
    /// L3 (0.30): Active intervention — third breach or extended session.
    L3ClearConcern = 3,
    /// L4 (0.40): Strong intervention — 4+ breaches. AURA suggests, user decides.
    L4MindfulnessPrompt = 4,
}

impl InterventionLevel {
    pub fn initiative_cost(&self) -> f32 {
        match self {
            InterventionLevel::None => 0.0,
            InterventionLevel::L1GentleReminder => 0.15,
            InterventionLevel::L2SoftBoundary => 0.20,
            InterventionLevel::L3ClearConcern => 0.30,
            InterventionLevel::L4MindfulnessPrompt => 0.40,
        }
    }
}

/// The core Guardian engine tracking user attention health.
#[derive(Debug, Serialize, Deserialize)]
pub struct ForestGuardian {
    #[serde(skip, default = "Instant::now")]
    pub current_app_session_start: Instant,
    pub rapid_switch_count: u32,
    #[serde(skip)]
    pub last_switch_time: Option<Instant>,

    #[serde(skip)]
    notification_timestamps: VecDeque<Instant>,
    #[serde(skip)]
    app_return_timestamps: HashMap<String, VecDeque<Instant>>,

    #[serde(skip)]
    pub current_app_package: Option<String>,

    pub breach_count: u32,
    #[serde(skip, default = "Instant::now")]
    pub last_breach_time: Instant,
    pub current_level: InterventionLevel,

    pub lock_in_threshold_mins: u64,
    pub context_thrash_threshold_secs: u64,
    pub notification_spiral_threshold: u32,
    pub notification_window_secs: u64,
    pub compulsive_return_threshold: u32,
    pub compulsive_return_window_mins: u64,
    pub pre_sleep_hour: u8,
}

impl Default for ForestGuardian {
    fn default() -> Self {
        Self {
            current_app_session_start: Instant::now(),
            rapid_switch_count: 0,
            last_switch_time: None,
            notification_timestamps: VecDeque::new(),
            app_return_timestamps: HashMap::new(),
            current_app_package: None,
            breach_count: 0,
            last_breach_time: Instant::now(),
            current_level: InterventionLevel::None,
            lock_in_threshold_mins: 15,
            context_thrash_threshold_secs: 5,
            notification_spiral_threshold: 10,
            notification_window_secs: 300,
            compulsive_return_threshold: 5,
            compulsive_return_window_mins: 10,
            pre_sleep_hour: 22,
        }
    }
}

impl ForestGuardian {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record_notification(&mut self) {
        let now = Instant::now();
        self.notification_timestamps.push_back(now);
        self.cleanup_old_notifications(now);
    }

    pub fn record_app_open(&mut self, app_package: &str) {
        let now = Instant::now();
        let timestamps = self
            .app_return_timestamps
            .entry(app_package.to_string())
            .or_default();
        timestamps.push_back(now);
        self.cleanup_old_app_returns(app_package, now);
    }

    fn cleanup_old_notifications(&mut self, now: Instant) {
        let cutoff = now - Duration::from_secs(self.notification_window_secs);
        while self
            .notification_timestamps
            .front()
            .map(|t| *t < cutoff)
            .unwrap_or(false)
        {
            self.notification_timestamps.pop_front();
        }
    }

    fn cleanup_old_app_returns(&mut self, app_package: &str, now: Instant) {
        if let Some(timestamps) = self.app_return_timestamps.get_mut(app_package) {
            let cutoff = now - Duration::from_secs(self.compulsive_return_window_mins * 60);
            while timestamps.front().map(|t| *t < cutoff).unwrap_or(false) {
                timestamps.pop_front();
            }
            if timestamps.is_empty() {
                self.app_return_timestamps.remove(app_package);
            }
        }
    }

    fn detect_notification_spiral(&self) -> Option<(u32, u64)> {
        let count = self.notification_timestamps.len() as u32;
        if count > self.notification_spiral_threshold {
            Some((count, self.notification_window_secs))
        } else {
            None
        }
    }

    fn detect_compulsive_app_return(&self) -> Option<(String, u32, u64)> {
        for (app_package, timestamps) in &self.app_return_timestamps {
            let count = timestamps.len() as u32;
            if count > self.compulsive_return_threshold {
                return Some((
                    app_package.clone(),
                    count,
                    self.compulsive_return_window_mins,
                ));
            }
        }
        None
    }

    fn detect_pre_sleep_screen(&self, current_hour: u8) -> Option<(u8, u8)> {
        if current_hour >= self.pre_sleep_hour {
            Some((self.pre_sleep_hour, current_hour))
        } else {
            None
        }
    }

    pub fn escalate_level(&mut self) {
        let now = Instant::now();
        let two_hours = Duration::from_secs(7200);

        if now.duration_since(self.last_breach_time) > two_hours {
            self.breach_count = 1;
        } else {
            self.breach_count += 1;
        }
        self.last_breach_time = now;

        self.current_level = match self.breach_count {
            1 => InterventionLevel::L1GentleReminder,
            2 => InterventionLevel::L2SoftBoundary,
            3 => InterventionLevel::L3ClearConcern,
            _ => InterventionLevel::L4MindfulnessPrompt,
        };

        warn!(
            "ForestGuardian: Escalated to {:?} (breach #{})",
            self.current_level, self.breach_count
        );
    }

    pub fn reset_if_healthy(&mut self, healthy_duration_secs: u64) {
        let now = Instant::now();
        if now.duration_since(self.last_breach_time) > Duration::from_secs(healthy_duration_secs)
            && self.current_level != InterventionLevel::None
        {
            warn!("ForestGuardian: User returned to healthy state. Resetting level.");
            self.current_level = InterventionLevel::None;
            self.breach_count = 0;
        }
    }

    /// Evaluates the user's current attention state based on raw ETG events.
    ///
    /// # Arguments
    /// * `app_package` - The package name of the currently active app
    /// * `is_infinite_scroll_app` - Whether the app is an infinite-scroll interface
    /// * `current_hour` - The current hour (0-23), used for pre-sleep detection
    pub fn evaluate_attention(
        &mut self,
        app_package: &str,
        is_infinite_scroll_app: bool,
        current_hour: u8,
    ) -> AttentionState {
        let now = Instant::now();

        if let Some(ref current) = self.current_app_package {
            if current != app_package {
                if let Some(last_time) = self.last_switch_time {
                    let dwell_time = now.duration_since(last_time);
                    if dwell_time.as_secs() < self.context_thrash_threshold_secs {
                        self.rapid_switch_count += 1;
                    } else {
                        self.rapid_switch_count = self.rapid_switch_count.saturating_sub(1);
                    }
                }
                self.record_app_open(app_package);
            }
        }
        self.current_app_package = Some(app_package.to_string());
        self.last_switch_time = Some(now);

        if self.rapid_switch_count >= 5 {
            warn!("ForestGuardian: Context thrashing detected! 5+ rapid app switches.");
            return AttentionState::ContextThrashing(self.rapid_switch_count);
        }

        if let Some((count, window_secs)) = self.detect_notification_spiral() {
            warn!(
                "ForestGuardian: Notification spiral detected! {} notifications in {} seconds.",
                count, window_secs
            );
            return AttentionState::NotificationSpiral { count, window_secs };
        }

        if let Some((ref app, count, window_mins)) = self.detect_compulsive_app_return() {
            warn!(
                "ForestGuardian: Compulsive app return detected! {} opened {} times in {} minutes.",
                app, count, window_mins
            );
            return AttentionState::CompulsiveAppReturn {
                app_package: app.clone(),
                return_count: count,
                window_mins,
            };
        }

        if let Some((sleep_hour, hour)) = self.detect_pre_sleep_screen(current_hour) {
            warn!(
                "ForestGuardian: Pre-sleep screen detected! Screen on at {}:00 (sleep time: {}:00).",
                hour, sleep_hour
            );
            return AttentionState::PreSleepScreen {
                sleep_hour,
                current_hour: hour,
            };
        }

        let session_duration = now.duration_since(self.current_app_session_start);
        if is_infinite_scroll_app && session_duration.as_secs() > (self.lock_in_threshold_mins * 60)
        {
            warn!(
                "ForestGuardian: Attention lock-in detected in {}. Session > {} mins.",
                app_package, self.lock_in_threshold_mins
            );
            return AttentionState::AttentionLockIn(session_duration);
        }

        AttentionState::HealthyInteraction
    }

    /// Builds a structured attention context for LLM-driven intervention.
    ///
    /// # Architecture contract
    /// This method returns FACTS — measurable signals and relationship metadata.
    /// It NEVER produces user-facing language. Tone, phrasing, and personality
    /// expression are the LLM's responsibility. The LLM receives this struct
    /// alongside the full OCEAN + relationship context it already holds and
    /// decides how to communicate.
    ///
    /// Returning `None` means the state is healthy and no intervention is needed.
    pub fn build_intervention_context(
        &self,
        state: &AttentionState,
        relationship: &RelationshipStage,
        aura_traits: &OceanTraits,
    ) -> Option<AttentionContext> {
        match state {
            AttentionState::HealthyInteraction => None,
            AttentionState::ContextThrashing(count) => Some(AttentionContext {
                intervention_kind: InterventionKind::ContextThrashing,
                intervention_level: self.current_level,
                detected_patterns: vec![DetectedPattern::ContextThrashing],
                rapid_switch_count: Some(*count),
                lock_in_duration_secs: None,
                notification_count: None,
                app_return_count: None,
                app_package: None,
                sleep_hour: None,
                current_hour: None,
                relationship_stage: *relationship,
                neuroticism: aura_traits.neuroticism,
                conscientiousness: aura_traits.conscientiousness,
            }),
            AttentionState::AttentionLockIn(duration) => Some(AttentionContext {
                intervention_kind: InterventionKind::AttentionLockIn,
                intervention_level: self.current_level,
                detected_patterns: vec![DetectedPattern::AttentionLockIn],
                rapid_switch_count: None,
                lock_in_duration_secs: Some(duration.as_secs()),
                notification_count: None,
                app_return_count: None,
                app_package: None,
                sleep_hour: None,
                current_hour: None,
                relationship_stage: *relationship,
                neuroticism: aura_traits.neuroticism,
                conscientiousness: aura_traits.conscientiousness,
            }),
            AttentionState::NotificationSpiral {
                count,
                window_secs: _,
            } => Some(AttentionContext {
                intervention_kind: InterventionKind::NotificationSpiral,
                intervention_level: self.current_level,
                detected_patterns: vec![DetectedPattern::NotificationSpiral],
                rapid_switch_count: None,
                lock_in_duration_secs: None,
                notification_count: Some(*count),
                app_return_count: None,
                app_package: None,
                sleep_hour: None,
                current_hour: None,
                relationship_stage: *relationship,
                neuroticism: aura_traits.neuroticism,
                conscientiousness: aura_traits.conscientiousness,
            }),
            AttentionState::CompulsiveAppReturn {
                app_package,
                return_count,
                window_mins: _,
            } => Some(AttentionContext {
                intervention_kind: InterventionKind::CompulsiveAppReturn,
                intervention_level: self.current_level,
                detected_patterns: vec![DetectedPattern::CompulsiveAppReturn],
                rapid_switch_count: None,
                lock_in_duration_secs: None,
                notification_count: None,
                app_return_count: Some(*return_count),
                app_package: Some(app_package.clone()),
                sleep_hour: None,
                current_hour: None,
                relationship_stage: *relationship,
                neuroticism: aura_traits.neuroticism,
                conscientiousness: aura_traits.conscientiousness,
            }),
            AttentionState::PreSleepScreen {
                sleep_hour,
                current_hour,
            } => Some(AttentionContext {
                intervention_kind: InterventionKind::PreSleepScreen,
                intervention_level: self.current_level,
                detected_patterns: vec![DetectedPattern::PreSleepScreen],
                rapid_switch_count: None,
                lock_in_duration_secs: None,
                notification_count: None,
                app_return_count: None,
                app_package: None,
                sleep_hour: Some(*sleep_hour),
                current_hour: Some(*current_hour),
                relationship_stage: *relationship,
                neuroticism: aura_traits.neuroticism,
                conscientiousness: aura_traits.conscientiousness,
            }),
        }
    }

    pub fn get_max_intervention_level(&self, states: &[AttentionState]) -> InterventionLevel {
        let mut max_level = self.current_level;
        for state in states {
            let level = match state {
                AttentionState::HealthyInteraction => InterventionLevel::None,
                AttentionState::ContextThrashing(_) => InterventionLevel::L1GentleReminder,
                AttentionState::AttentionLockIn(_) => InterventionLevel::L1GentleReminder,
                AttentionState::NotificationSpiral { .. } => InterventionLevel::L2SoftBoundary,
                AttentionState::CompulsiveAppReturn { .. } => InterventionLevel::L2SoftBoundary,
                AttentionState::PreSleepScreen { .. } => InterventionLevel::L1GentleReminder,
            };
            if level > max_level {
                max_level = level;
            }
        }
        max_level
    }
}

/// Pattern detected by Forest Guardian.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum DetectedPattern {
    AttentionLockIn,
    ContextThrashing,
    NotificationSpiral,
    CompulsiveAppReturn,
    PreSleepScreen,
}

/// The kind of attention problem detected by the Forest Guardian.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum InterventionKind {
    ContextThrashing,
    AttentionLockIn,
    NotificationSpiral,
    CompulsiveAppReturn,
    PreSleepScreen,
}

// ---------------------------------------------------------------------------
// AttentionContext — structured output consumed by the LLM layer
// ---------------------------------------------------------------------------

/// Factual context produced by the Forest Guardian, ready for LLM consumption.
///
/// This is the ONLY output surface for intervention signals. The LLM receives
/// this struct (serialised into its context window) and writes the actual words.
/// No Rust code downstream of this type should produce user-visible strings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttentionContext {
    pub intervention_kind: InterventionKind,
    pub intervention_level: InterventionLevel,
    pub detected_patterns: Vec<DetectedPattern>,
    pub rapid_switch_count: Option<u32>,
    pub lock_in_duration_secs: Option<u64>,
    pub notification_count: Option<u32>,
    pub app_return_count: Option<u32>,
    pub app_package: Option<String>,
    pub sleep_hour: Option<u8>,
    pub current_hour: Option<u8>,
    pub relationship_stage: RelationshipStage,
    pub neuroticism: f32,
    pub conscientiousness: f32,
}
