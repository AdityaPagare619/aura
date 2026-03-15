//! Forest Guardian: Attention Protection & Anti-Lock-In Monitor
//!
//! # Strategic System Fit
//! AURA is the anti-cloud. Standard algorithms optimize for maximized Time-On-Device (TOD).
//! AURA optimizes for Time-Well-Spent. The Forest Guardian actively monitors the user's
//! app usage patterns via the ETG/Accessibility Service, and if it detects "Doomscrolling"
//! or "Attention Lock-in," it surfaces raw factual context for the LLM to reason about.
//!
//! # Architecture boundary
//! This module produces FACTS only. It NEVER generates user-facing language.
//! All intervention wording is produced by the LLM, which receives an
//! `AttentionContext` struct and decides tone/phrasing based on the full
//! relationship + personality context it already holds.
//!
//! # Precise System Modeling
//! - State: `AttentionState` (HealthyInteraction, ContextThrashing, AttentionLockIn)
//! - Events: `AppSwitch`, `SessionDurationExceeded`, `RapidScrollSpike`

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
    ContextThrashing(u32), // Number of rapid switches
    /// High dwell time in infinite-scroll interfaces with repetitive actions.
    AttentionLockIn(Duration), // Duration of the lock-in
}

/// The core Guardian engine tracking user attention health.
#[derive(Debug, Serialize, Deserialize)]
pub struct ForestGuardian {
    #[serde(skip, default = "Instant::now")]
    pub current_app_session_start: Instant,
    pub rapid_switch_count: u32,
    #[serde(skip)]
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
                rapid_switch_count: Some(*count),
                lock_in_duration_secs: None,
                relationship_stage: relationship.clone(),
                // Raw trait values passed as-is — the LLM reads them in its
                // system prompt and adjusts tone. Rust does NOT branch on them.
                neuroticism: aura_traits.neuroticism,
                conscientiousness: aura_traits.conscientiousness,
            }),
            AttentionState::AttentionLockIn(duration) => Some(AttentionContext {
                intervention_kind: InterventionKind::AttentionLockIn,
                rapid_switch_count: None,
                lock_in_duration_secs: Some(duration.as_secs()),
                relationship_stage: relationship.clone(),
                neuroticism: aura_traits.neuroticism,
                conscientiousness: aura_traits.conscientiousness,
            }),
        }
    }
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
    /// What kind of attention problem was detected.
    pub intervention_kind: InterventionKind,
    /// Rapid app-switch count (set for `ContextThrashing`, else `None`).
    pub rapid_switch_count: Option<u32>,
    /// How long the user has been locked in, in seconds (set for `AttentionLockIn`).
    pub lock_in_duration_secs: Option<u64>,
    /// Current relationship stage — informs how the LLM should pitch its tone.
    pub relationship_stage: RelationshipStage,
    /// Raw neuroticism score [0.0, 1.0] — passed to LLM, NOT branched on in Rust.
    pub neuroticism: f32,
    /// Raw conscientiousness score [0.0, 1.0] — passed to LLM, NOT branched on in Rust.
    pub conscientiousness: f32,
}

/// The kind of attention problem detected by the Forest Guardian.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum InterventionKind {
    /// User is switching apps too rapidly (context thrashing).
    ContextThrashing,
    /// User is locked into an infinite-scroll interface (doomscrolling).
    AttentionLockIn,
}
