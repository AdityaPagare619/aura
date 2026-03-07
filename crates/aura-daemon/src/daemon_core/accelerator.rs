//! Accelerated Onboarding (Day-1 Capability)
//!
//! # Product Discovery Validation
//! A blank-slate AI is a useless AI. To achieve Day-1 capability,
//! AURA doesn't wait for the user to issue commands. It actively observes
//! the user's natural device usage over the first few hours/days and
//! reverse-engineers their ETG (Element-Transition Graph) and Personalization profile.
//!
//! # Precise System Modeling
//! - **Points**: `OnboardingProfile`
//! - **Events**: `observe_user_action()`
//! - **Lines**: Progression from `BlankSlate` -> `BasicContext` -> `ReadyToAssist`

use tracing::{info, debug};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ObservationStage {
    /// Fresh installation. No passive context yet.
    BlankSlate,
    /// Has observed basic app usage (e.g., knows favorite messaging app).
    BasicContext,
    /// Has enough passive ETG coverage to execute basic Day-1 templates securely.
    ReadyToAssist,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppPreference {
    pub package_name: String,
    pub primary_use_case: String, // e.g., "messaging", "food_delivery"
    pub usage_frequency: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OnboardingProfile {
    pub stage: ObservationStage,
    pub observed_preferences: Vec<AppPreference>,
    pub hours_observed: f32,
}

impl Default for OnboardingProfile {
    fn default() -> Self {
        Self {
            stage: ObservationStage::BlankSlate,
            observed_preferences: Vec::new(),
            hours_observed: 0.0,
        }
    }
}

/// The passive observer that accelerates Day-1 utility.
pub struct OnboardingAccelerator {
    profile: OnboardingProfile,
}

impl Default for OnboardingAccelerator {
    fn default() -> Self {
        Self::new()
    }
}

impl OnboardingAccelerator {
    pub fn new() -> Self {
        Self {
            profile: OnboardingProfile::default(),
        }
    }

    /// Continuously called by the OS event bus during the first 48 hours.
    pub fn observe_user_action(&mut self, app_package: &str, interaction_type: &str) {
        // Precise System Modeling: State evolution based on events.
        debug!("Onboarding observation: App '{}' used for '{}'", app_package, interaction_type);

        if let Some(pref) = self.profile.observed_preferences.iter_mut().find(|p| p.package_name == app_package) {
            pref.usage_frequency += 1;
        } else {
            self.profile.observed_preferences.push(AppPreference {
                package_name: app_package.to_string(),
                primary_use_case: interaction_type.to_string(),
                usage_frequency: 1,
            });
        }

        self.evaluate_stage_progression();
    }

    fn evaluate_stage_progression(&mut self) {
        let total_observations: u32 = self.profile.observed_preferences.iter().map(|p| p.usage_frequency).sum();
        let unique_apps = self.profile.observed_preferences.len();

        let new_stage = match self.profile.stage {
            ObservationStage::BlankSlate if total_observations > 20 && unique_apps >= 2 => {
                info!("Onboarding stage progressed: BlankSlate -> BasicContext");
                ObservationStage::BasicContext
            }
            ObservationStage::BasicContext if total_observations > 100 && unique_apps >= 4 => {
                info!("Onboarding stage progressed: BasicContext -> ReadyToAssist");
                ObservationStage::ReadyToAssist
            }
            current => current,
        };

        if new_stage != self.profile.stage {
            self.profile.stage = new_stage;
            // Optionally: Trigger the "Thinking Partner" to notify the user we are ready.
        }
    }
    
    pub fn get_profile(&self) -> &OnboardingProfile {
        &self.profile
    }
}
