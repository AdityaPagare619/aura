pub mod affective;
pub mod anti_sycophancy;
pub mod behavior_modifiers;
pub mod epistemic;
pub mod ethics;
pub mod personality;
pub mod proactive_consent;
pub mod prompt_personality;
pub mod relationship;
pub mod user_profile;

pub use affective::{AffectiveEngine, MoodEvent};
pub use anti_sycophancy::{GateResult, SycophancyGuard, SycophancyVerdict};
pub use behavior_modifiers::{AutonomyLevel, GoalWeights, ResponseStyleParams, VerbosityLevel};
pub use epistemic::{EpistemicAwareness, EpistemicLevel, KnowledgeDomain};
pub use ethics::{
    ManipulationCheckResult, ManipulationVerdict, PolicyGate, PolicyVerdict, TruthFramework,
    TruthValidation,
};
pub use personality::{
    ConsistencyReport, Personality, PersonalityArchetype, PersonalityEngine, PersonalityEvent,
    PersonalityInfluence, ResponseStyle, ToneParameters,
};
pub use proactive_consent::{ProactiveConsent, ProactiveSettings};
pub use prompt_personality::PersonalityPromptInjector;
pub use relationship::{InteractionType, RelationshipTracker, RiskLevel, UserRelationship};
pub use user_profile::UserProfile;

// ---------------------------------------------------------------------------
// Unified Identity Engine — facade over all identity subsystems
// ---------------------------------------------------------------------------

/// Unified facade that owns all identity subsystems and provides a single
/// entry point for the pipeline to query personality, mood, trust, ethics,
/// and anti-sycophancy state.
pub struct IdentityEngine {
    pub personality: Personality,
    pub affective: AffectiveEngine,
    pub relationships: RelationshipTracker,
    pub sycophancy_guard: SycophancyGuard,
    pub policy_gate: PolicyGate,
    /// TRUTH framework for validating responses against the five principles.
    pub truth_framework: TruthFramework,
    /// User profile containing preferences and consent settings.
    pub user_profile: Option<UserProfile>,
    /// Epistemic awareness: tracks what AURA knows vs. doesn't know (§5.2).
    pub epistemic: EpistemicAwareness,
}

impl IdentityEngine {
    /// Create a new `IdentityEngine` with default subsystems.
    pub fn new() -> Self {
        Self {
            personality: Personality::new(),
            affective: AffectiveEngine::new(),
            relationships: RelationshipTracker::new(),
            sycophancy_guard: SycophancyGuard::new(),
            policy_gate: PolicyGate::new(),
            truth_framework: TruthFramework::new(),
            user_profile: None,
            epistemic: EpistemicAwareness::new(),
        }
    }

    /// Generate a full personality context string for prompt injection.
    ///
    /// Combines personality traits, current mood, and relationship stage
    /// into a set of directives for the LLM.
    #[tracing::instrument(skip(self))]
    pub fn personality_context(&self, user_id: &str) -> String {
        let traits = &self.personality.traits;
        let mood = self.affective.current_state();
        let relationship = self.relationships.get_relationship(user_id);

        let stage = relationship
            .map(|r| r.stage.clone())
            .unwrap_or(aura_types::identity::RelationshipStage::Stranger);

        let trust = relationship.map(|r| r.trust).unwrap_or(0.0);

        PersonalityPromptInjector::generate_personality_context(traits, mood, stage, trust)
    }

    /// Run the anti-sycophancy gate on the current response window.
    ///
    /// Returns a `GateResult` that the pipeline should act on:
    /// - `Pass` → send response as-is
    /// - `Nudge` → append honesty directive, send response
    /// - `Block` → regenerate with honesty block directive
    #[tracing::instrument(skip(self))]
    pub fn check_response(&mut self) -> GateResult {
        self.sycophancy_guard.gate()
    }

    /// Check whether the given risk level requires user permission based
    /// on the user's trust level.
    #[tracing::instrument(skip(self))]
    pub fn requires_permission(&self, user_id: &str, risk: &RiskLevel) -> bool {
        self.relationships.requires_permission(user_id, *risk)
    }

    /// Process a mood event through the affective engine.
    #[tracing::instrument(skip(self))]
    pub fn process_mood(&mut self, event: MoodEvent, now_ms: u64) {
        self.affective.process_event(event, now_ms);
    }

    /// Record a user interaction in the relationship tracker.
    #[tracing::instrument(skip(self))]
    pub fn record_interaction(
        &mut self,
        user_id: &str,
        interaction: InteractionType,
        timestamp: u64,
    ) {
        self.relationships
            .record_interaction(user_id, interaction, timestamp);
    }

    /// Validate a response against the TRUTH framework principles.
    ///
    /// Delegates to [`TruthFramework::validate_response`] and returns
    /// per-principle scores plus an overall pass/fail verdict.
    ///
    /// Should be called in the response path **before** the anti-sycophancy
    /// gate to catch deceptive, biased, or evasive content.
    #[tracing::instrument(skip(self, text), fields(text_len = text.len()))]
    pub fn validate_response(&self, text: &str) -> TruthValidation {
        self.truth_framework.validate_response(text)
    }

    /// Check user input text for manipulation patterns.
    ///
    /// Delegates to [`ethics::check_manipulation`] and returns a score,
    /// detected patterns, and a verdict (Clean/Suspicious/Manipulative).
    ///
    /// Should be called in the input path **before** the policy gate to
    /// detect emotional manipulation, authority abuse, and urgency pressure.
    #[tracing::instrument(skip(self, text), fields(text_len = text.len()))]
    pub fn check_manipulation(&self, text: &str) -> ManipulationCheckResult {
        ethics::check_manipulation(text)
    }

    /// Check if proactive behavior is allowed for the given hour.
    /// Returns false if no user profile is loaded (safety default).
    pub fn is_proactive_allowed(&self, hour: u8) -> bool {
        match &self.user_profile {
            Some(profile) => profile.is_proactive_allowed(hour),
            None => false,
        }
    }

    /// Load user profile from database.
    pub fn load_user_profile(
        &mut self,
        db: &rusqlite::Connection,
    ) -> Result<(), aura_types::errors::OnboardingError> {
        match UserProfile::load_from_db(db)? {
            Some(profile) => {
                self.user_profile = Some(profile);
                Ok(())
            }
            None => Ok(()),
        }
    }

    /// Get a reference to the user profile if loaded.
    pub fn user_profile(&self) -> Option<&UserProfile> {
        self.user_profile.as_ref()
    }

    /// Get a mutable reference to the user profile if loaded.
    pub fn user_profile_mut(&mut self) -> Option<&mut UserProfile> {
        self.user_profile.as_mut()
    }
}

impl Default for IdentityEngine {
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

    #[test]
    fn test_identity_engine_default() {
        let engine = IdentityEngine::default();
        // Personality context should at minimum contain TRUTH framework
        let ctx = engine.personality_context("test_user");
        assert!(
            ctx.contains("TRUTH"),
            "context should contain TRUTH framework"
        );
    }

    #[test]
    fn test_check_response_on_fresh_engine() {
        let mut engine = IdentityEngine::new();
        // No responses recorded — should pass
        let result = engine.check_response();
        assert_eq!(result, GateResult::Pass);
    }

    #[test]
    fn test_requires_permission_unknown_user() {
        let engine = IdentityEngine::new();
        // Unknown user → trust 0.0 → should require permission for everything
        assert!(engine.requires_permission("nobody", &RiskLevel::Low));
        assert!(engine.requires_permission("nobody", &RiskLevel::Critical));
    }
}
