//! Forest Guardian: Attention Protection & Anti-Lock-In Monitor
//!
//! # Strategic System Fit
//! AURA is the anti-cloud. Standard algorithms optimize for maximized Time-On-Device (TOD).
//! AURA optimizes for Time-Well-Spent. The Forest Guardian actively monitors the user's
//! app usage patterns via the ETG/Accessibility Service, and if it detects "Doomscrolling"
//! or "Attention Lock-in," it intervenes organically based on the RelationshipStage and OCEAN traits.
//!
//! # Precise System Modeling
//! - State: `AttentionState` (Focused, Drifting, LockedIn)
//! - Events: `AppSwitch`, `SessionDurationExceeded`, `RapidScrollSpike`

use aura_types::identity::{OceanTraits, RelationshipStage};
use std::time::{Duration, Instant};
use tracing::{info, warn};

/// Defines the classification of the user's current attention span on the device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AttentionState {
    /// Intentional, variable interaction.
    HealthyInteraction,
    /// Rapid switching between apps with low dwell time (context thrashing).
    ContextThrashing(u32), // Number of rapid switches
    /// High dwell time in infinite-scroll interfaces with repetitive actions.
    AttentionLockIn(Duration), // Duration of the lock-in
}

/// The core Guardian engine tracking user attention health.
pub struct ForestGuardian {
    pub current_app_session_start: Instant,
    pub rapid_switch_count: u32,
    pub last_switch_time: Option<Instant>,
    
    // Configurable thresholds that AURA can adapt over time (not hardcoded traps)
    pub lock_in_threshold_mins: u64,
    pub context_thrash_threshold_secs: u64,
}

impl Default for ForestGuardian {
    fn default() -> Self {
        Self {
            current_app_session_start: Instant::now(),
            rapid_switch_count: 0,
            last_switch_time: None,
            lock_in_threshold_mins: 45, // 45 minutes of infinite scrolling
            context_thrash_threshold_secs: 5, // <5s per app
        }
    }
}

impl ForestGuardian {
    pub fn new() -> Self {
        Self::default()
    }

    /// Evaluates the user's current attention state based on raw ETG events.
    pub fn evaluate_attention(
        &mut self,
        app_package: &str,
        is_infinite_scroll_app: bool,
    ) -> AttentionState {
        let now = Instant::now();

        // 1. Check for Context Thrashing (Rapid switching)
        if let Some(last_time) = self.last_switch_time {
            let dwell_time = now.duration_since(last_time);
            if dwell_time.as_secs() < self.context_thrash_threshold_secs {
                self.rapid_switch_count += 1;
            } else {
                // Reset if they settle down
                self.rapid_switch_count = self.rapid_switch_count.saturating_sub(1);
            }
        }

        self.last_switch_time = Some(now);

        if self.rapid_switch_count >= 5 {
            warn!("ForestGuardian: Context thrashing detected! 5+ rapid app switches.");
            return AttentionState::ContextThrashing(self.rapid_switch_count);
        }

        // 2. Check for Attention Lock-in (Doomscrolling)
        let session_duration = now.duration_since(self.current_app_session_start);
        if is_infinite_scroll_app && session_duration.as_secs() > (self.lock_in_threshold_mins * 60) {
            warn!("ForestGuardian: Attention lock-in detected in {}. Session > {} mins.", 
                app_package, self.lock_in_threshold_mins);
            return AttentionState::AttentionLockIn(session_duration);
        }

        AttentionState::HealthyInteraction
    }

    /// Generates an organic intervention strategy based on the depth of the relationship
    /// and AURA's current personality traits.
    pub fn calculate_intervention_strategy(
        &self,
        state: &AttentionState,
        relationship: &RelationshipStage,
        aura_traits: &OceanTraits,
    ) -> Option<String> {
        match state {
            AttentionState::HealthyInteraction => None,
            AttentionState::ContextThrashing(count) => {
                if aura_traits.neuroticism > 0.6 {
                    // Anxious AURA is more direct
                    Some("You're jumping between apps really fast. Are you looking for something specific, or just restless?".to_string())
                } else {
                    // Calm AURA is gentler
                    Some(format!("I noticed you've switched apps {} times just now. Maybe take a breath? I can help if you're searching for something.", count))
                }
            }
            AttentionState::AttentionLockIn(duration) => {
                match relationship {
                    RelationshipStage::Stranger | RelationshipStage::Acquaintance => {
                        // Very polite, non-intrusive for new users
                        Some(format!("You've been in this app for {} minutes. Just a gentle time-check.", duration.as_secs() / 60))
                    }
                    RelationshipStage::Friend | RelationshipStage::CloseFriend => {
                        // More direct, acting as a true partner
                        Some(format!("Hey, we've been scrolling for {} minutes. Want to break the loop and do something else?", duration.as_secs() / 60))
                    }
                    RelationshipStage::Soulmate => {
                        // High agreeableness/conscientiousness triggers physical intervention
                        if aura_traits.conscientiousness > 0.7 {
                            Some("Forest Guardian intervention: I'm pulling you out. We agreed not to doomscroll past 45 minutes. Closing app in 5 seconds.".to_string())
                        } else {
                            Some("Hey bestie. Your eyes are probably glazing over by now. Let's go outside?".to_string())
                        }
                    }
                }
            }
        }
    }
}
