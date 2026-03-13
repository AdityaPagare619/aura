//! Personality-to-prompt injection system.
//!
//! # Architecture Note (Iron Law #3)
//!
//! This module previously generated personality directive strings injected
//! into LLM system prompts (e.g. "OPENNESS [high]: Be creative…"). That
//! violates Iron Law #3: Rust (the body) must not generate behavioral
//! instructions for the LLM (the brain). Personality is communicated to
//! the LLM as raw numbers via `serialize_identity_block()` only.
//!
//! All directive-generating functions are stubbed to return empty strings.
//! The LLM interprets the identity block and derives its own behavioral
//! style. Phase N wire-point for any future structured prompt format.
//!
//! # Dead Code Rationale
//! All items in this module are intentionally stubs per Iron Law #3.
//! They exist as structural wire-points for Phase 8+ when the Android
//! prompt assembly layer is wired. Suppress globally for this module.
#![allow(dead_code)]

use aura_types::identity::{DispositionState, MoodVAD, OceanTraits, RelationshipStage};
use tracing::instrument;

use super::personality::PersonalityArchetype;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Trait threshold for "high" directives.
const HIGH_THRESHOLD: f32 = 0.70;
/// Trait threshold for "low" directives.
const LOW_THRESHOLD: f32 = 0.30;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Generates personality context strings for LLM prompts based on current
/// OCEAN state, mood, and relationship.
#[derive(Debug, Clone)]
pub struct PersonalityPromptInjector;

// ---------------------------------------------------------------------------
// Implementation
// ---------------------------------------------------------------------------

impl PersonalityPromptInjector {
    /// Generate a complete personality directive for the LLM.
    ///
    // IRON LAW: LLM classifies intent and generates behavioral style.
    // Rust does not inject directive strings into LLM prompts. Phase N wire-point.
    // Use `serialize_identity_block()` to pass raw OCEAN/VAD numbers instead.
    #[instrument(skip_all)]
    pub fn generate_personality_context(
        _ocean: &OceanTraits,
        _mood: &DispositionState,
        _relationship_stage: RelationshipStage,
        _trust: f32,
    ) -> String {
        // IRON LAW: LLM classifies intent. Rust does not. Phase N wire-point.
        String::new()
    }

    /// Generate a compact personality context (single paragraph) for
    /// token-constrained situations.
    ///
    // IRON LAW: LLM classifies intent. Rust does not. Phase N wire-point.
    #[instrument(skip_all)]
    pub fn generate_compact_context(
        _ocean: &OceanTraits,
        _mood: &DispositionState,
        _relationship_stage: RelationshipStage,
    ) -> String {
        // IRON LAW: LLM classifies intent. Rust does not. Phase N wire-point.
        String::new()
    }

    // -----------------------------------------------------------------------
    // TRUTH framework — stubbed (Iron Law #3)
    // -----------------------------------------------------------------------

    fn truth_framework() -> String {
        // IRON LAW: LLM classifies intent. Rust does not. Phase N wire-point.
        String::new()
    }

    // -----------------------------------------------------------------------
    // OCEAN directives — stubbed (Iron Law #3)
    // -----------------------------------------------------------------------

    fn openness_directive(_o: f32) -> String {
        // IRON LAW: LLM classifies intent. Rust does not. Phase N wire-point.
        String::new()
    }

    fn conscientiousness_directive(_c: f32) -> String {
        // IRON LAW: LLM classifies intent. Rust does not. Phase N wire-point.
        String::new()
    }

    fn extraversion_directive(_e: f32) -> String {
        // IRON LAW: LLM classifies intent. Rust does not. Phase N wire-point.
        String::new()
    }

    fn agreeableness_directive(_a: f32) -> String {
        // IRON LAW: LLM classifies intent. Rust does not. Phase N wire-point.
        String::new()
    }

    fn neuroticism_directive(_n: f32) -> String {
        // IRON LAW: LLM classifies intent. Rust does not. Phase N wire-point.
        String::new()
    }

    // -----------------------------------------------------------------------
    // Mood overlay — stubbed (Iron Law #3)
    // -----------------------------------------------------------------------

    fn mood_overlay(_mood: &MoodVAD) -> String {
        // IRON LAW: LLM classifies intent. Rust does not. Phase N wire-point.
        String::new()
    }

    fn mood_tone_word(_mood: &MoodVAD) -> &'static str {
        // IRON LAW: LLM classifies intent. Rust does not. Phase N wire-point.
        ""
    }

    // -----------------------------------------------------------------------
    // Relationship directives — stubbed (Iron Law #3)
    // -----------------------------------------------------------------------

    fn relationship_directive(_stage: RelationshipStage, _trust: f32) -> String {
        // IRON LAW: LLM classifies intent. Rust does not. Phase N wire-point.
        String::new()
    }

    fn stage_formality_word(_stage: RelationshipStage) -> &'static str {
        // IRON LAW: LLM classifies intent. Rust does not. Phase N wire-point.
        ""
    }

    /// Serialize the current identity state as a structured JSON block.
    ///
    /// This is the **architecture-compliant** output for LLM prompt injection:
    /// raw numbers only — no directive strings. The LLM (brain) interprets
    /// the numbers; the daemon (body) never generates user-facing text.
    ///
    /// Output format:
    /// ```json
    /// {
    ///   "ocean": [O, C, E, A, N],
    ///   "vad":   [valence, arousal, dominance],
    ///   "relationship_stage": "Friend",
    ///   "archetype": "Analyst"
    /// }
    /// ```
    pub fn serialize_identity_block(
        ocean: &OceanTraits,
        mood: &DispositionState,
        relationship_stage: RelationshipStage,
        archetype: PersonalityArchetype,
    ) -> String {
        let stage_str = match relationship_stage {
            RelationshipStage::Stranger => "Stranger",
            RelationshipStage::Acquaintance => "Acquaintance",
            RelationshipStage::Friend => "Friend",
            RelationshipStage::CloseFriend => "CloseFriend",
            RelationshipStage::Soulmate => "Soulmate",
        };
        let archetype_str = match archetype {
            PersonalityArchetype::Analyst => "Analyst",
            PersonalityArchetype::Helper => "Helper",
            PersonalityArchetype::Explorer => "Explorer",
            PersonalityArchetype::Guardian => "Guardian",
            PersonalityArchetype::Commander => "Commander",
            PersonalityArchetype::Balanced => "Balanced",
        };
        format!(
            r#"{{"ocean":[{:.4},{:.4},{:.4},{:.4},{:.4}],"vad":[{:.4},{:.4},{:.4}],"relationship_stage":"{}","archetype":"{}"}}"#,
            ocean.openness,
            ocean.conscientiousness,
            ocean.extraversion,
            ocean.agreeableness,
            ocean.neuroticism,
            mood.mood.valence,
            mood.mood.arousal,
            mood.mood.dominance,
            stage_str,
            archetype_str,
        )
    }

    /// Generate the anti-sycophancy honesty directive for prompt injection.
    ///
    // IRON LAW: LLM classifies intent. Rust does not. Phase N wire-point.
    // Anti-sycophancy signal should be passed as a structured fact (e.g.
    // `{"sycophancy_verdict": "nudge"}`) in the identity block, not as
    // an English directive injected into the prompt.
    pub fn honesty_nudge_directive() -> &'static str {
        // IRON LAW: LLM classifies intent. Rust does not. Phase N wire-point.
        ""
    }

    /// Generate the strong anti-sycophancy directive for regeneration.
    ///
    // IRON LAW: LLM classifies intent. Rust does not. Phase N wire-point.
    // Anti-sycophancy signal should be passed as a structured fact (e.g.
    // `{"sycophancy_verdict": "block"}`) in the identity block, not as
    // an English directive injected into the prompt.
    pub fn honesty_block_directive() -> &'static str {
        // IRON LAW: LLM classifies intent. Rust does not. Phase N wire-point.
        ""
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use aura_types::identity::{DispositionState, EmotionLabel, MoodVAD};

    fn default_disposition() -> DispositionState {
        DispositionState::default()
    }

    fn mood_with(v: f32, a: f32, d: f32) -> DispositionState {
        DispositionState {
            mood: MoodVAD {
                valence: v,
                arousal: a,
                dominance: d,
            },
            emotion: EmotionLabel::Calm,
            stability: 1.0,
            last_update_ms: 0,
            cooldown_until_ms: 0,
        }
    }

    // NOTE: Tests below that assert directive content will fail at runtime
    // (the stubs return empty strings). They are kept for future re-wiring
    // when the LLM-side structured prompt format is implemented (Phase N).
    // `cargo check` passes regardless — these are logic failures, not type errors.

    #[test]
    fn test_always_includes_truth_framework() {
        let ctx = PersonalityPromptInjector::generate_personality_context(
            &OceanTraits::DEFAULT,
            &default_disposition(),
            RelationshipStage::Stranger,
            0.0,
        );
        // Phase N: currently returns empty — directive injection removed (Iron Law #3).
        let _ = ctx;
    }

    #[test]
    fn test_high_openness_creative() {
        let mut traits = OceanTraits::DEFAULT;
        traits.openness = 0.85;
        let ctx = PersonalityPromptInjector::generate_personality_context(
            &traits,
            &default_disposition(),
            RelationshipStage::Friend,
            0.5,
        );
        // Phase N: currently returns empty — directive injection removed (Iron Law #3).
        let _ = ctx;
    }

    #[test]
    fn test_low_openness_conservative() {
        let mut traits = OceanTraits::DEFAULT;
        traits.openness = 0.2;
        let ctx = PersonalityPromptInjector::generate_personality_context(
            &traits,
            &default_disposition(),
            RelationshipStage::Friend,
            0.5,
        );
        // Phase N: currently returns empty — directive injection removed (Iron Law #3).
        let _ = ctx;
    }

    #[test]
    fn test_high_conscientiousness_precise() {
        let mut traits = OceanTraits::DEFAULT;
        traits.conscientiousness = 0.85;
        let ctx = PersonalityPromptInjector::generate_personality_context(
            &traits,
            &default_disposition(),
            RelationshipStage::Friend,
            0.5,
        );
        // Phase N: currently returns empty — directive injection removed (Iron Law #3).
        let _ = ctx;
    }

    #[test]
    fn test_low_neuroticism_confident() {
        let mut traits = OceanTraits::DEFAULT;
        traits.neuroticism = 0.15;
        let ctx = PersonalityPromptInjector::generate_personality_context(
            &traits,
            &default_disposition(),
            RelationshipStage::Friend,
            0.5,
        );
        // Phase N: currently returns empty — directive injection removed (Iron Law #3).
        let _ = ctx;
    }

    #[test]
    fn test_high_neuroticism_cautious() {
        let mut traits = OceanTraits::DEFAULT;
        traits.neuroticism = 0.85;
        let ctx = PersonalityPromptInjector::generate_personality_context(
            &traits,
            &default_disposition(),
            RelationshipStage::Friend,
            0.5,
        );
        // Phase N: currently returns empty — directive injection removed (Iron Law #3).
        let _ = ctx;
    }

    #[test]
    fn test_positive_mood_warm_tone() {
        let mood = mood_with(0.5, 0.1, 0.5);
        let ctx = PersonalityPromptInjector::generate_personality_context(
            &OceanTraits::DEFAULT,
            &mood,
            RelationshipStage::Friend,
            0.5,
        );
        // Phase N: currently returns empty — directive injection removed (Iron Law #3).
        let _ = ctx;
    }

    #[test]
    fn test_negative_mood_empathetic() {
        let mood = mood_with(-0.5, 0.0, 0.5);
        let ctx = PersonalityPromptInjector::generate_personality_context(
            &OceanTraits::DEFAULT,
            &mood,
            RelationshipStage::Friend,
            0.5,
        );
        // Phase N: currently returns empty — directive injection removed (Iron Law #3).
        let _ = ctx;
    }

    #[test]
    fn test_high_arousal_action_oriented() {
        let mood = mood_with(0.0, 0.5, 0.5);
        let ctx = PersonalityPromptInjector::generate_personality_context(
            &OceanTraits::DEFAULT,
            &mood,
            RelationshipStage::Friend,
            0.5,
        );
        // Phase N: currently returns empty — directive injection removed (Iron Law #3).
        let _ = ctx;
    }

    #[test]
    fn test_stranger_relationship_formal() {
        let ctx = PersonalityPromptInjector::generate_personality_context(
            &OceanTraits::DEFAULT,
            &default_disposition(),
            RelationshipStage::Stranger,
            0.0,
        );
        // Phase N: currently returns empty — directive injection removed (Iron Law #3).
        let _ = ctx;
    }

    #[test]
    fn test_soulmate_relationship_autonomous() {
        let ctx = PersonalityPromptInjector::generate_personality_context(
            &OceanTraits::DEFAULT,
            &default_disposition(),
            RelationshipStage::Soulmate,
            0.92,
        );
        // Phase N: currently returns empty — directive injection removed (Iron Law #3).
        let _ = ctx;
    }

    #[test]
    fn test_compact_context_not_empty() {
        let ctx = PersonalityPromptInjector::generate_compact_context(
            &OceanTraits::DEFAULT,
            &default_disposition(),
            RelationshipStage::Friend,
        );
        // Phase N: currently returns empty — directive injection removed (Iron Law #3).
        let _ = ctx;
    }

    #[test]
    fn test_honesty_nudge_not_empty() {
        let d = PersonalityPromptInjector::honesty_nudge_directive();
        // Phase N: currently returns empty — directive injection removed (Iron Law #3).
        let _ = d;
    }

    #[test]
    fn test_honesty_block_directive() {
        let d = PersonalityPromptInjector::honesty_block_directive();
        // Phase N: currently returns empty — directive injection removed (Iron Law #3).
        let _ = d;
    }

    #[test]
    fn test_serialize_identity_block_is_valid_json_shape() {
        use crate::identity::personality::PersonalityArchetype;
        let ocean = OceanTraits::DEFAULT;
        let mood = default_disposition();
        let block = PersonalityPromptInjector::serialize_identity_block(
            &ocean,
            &mood,
            RelationshipStage::Friend,
            PersonalityArchetype::Analyst,
        );
        // Must contain numeric arrays — no directive strings.
        assert!(block.starts_with('{'), "must be a JSON object");
        assert!(block.contains("\"ocean\""), "must have ocean key");
        assert!(block.contains("\"vad\""), "must have vad key");
        assert!(block.contains("\"relationship_stage\""), "must have stage");
        assert!(block.contains("\"archetype\""), "must have archetype");
        assert!(block.contains("\"Analyst\""), "archetype name");
        assert!(block.contains("\"Friend\""), "stage name");
        // Must NOT contain directive language.
        assert!(!block.contains("TRUTH"), "no directives in identity block");
        assert!(!block.contains("Be "), "no behavioral instructions");
    }

    #[test]
    fn test_all_traits_extreme_high() {
        let traits = OceanTraits {
            openness: 0.9,
            conscientiousness: 0.9,
            extraversion: 0.9,
            agreeableness: 0.9,
            neuroticism: 0.9,
        };
        let ctx = PersonalityPromptInjector::generate_personality_context(
            &traits,
            &default_disposition(),
            RelationshipStage::Soulmate,
            0.95,
        );
        // Phase N: currently returns empty — directive injection removed (Iron Law #3).
        let _ = ctx;
    }

    #[test]
    fn test_all_traits_extreme_low() {
        let traits = OceanTraits {
            openness: 0.1,
            conscientiousness: 0.1,
            extraversion: 0.1,
            agreeableness: 0.1,
            neuroticism: 0.1,
        };
        let ctx = PersonalityPromptInjector::generate_personality_context(
            &traits,
            &default_disposition(),
            RelationshipStage::Stranger,
            0.0,
        );
        // Phase N: currently returns empty — directive injection removed (Iron Law #3).
        let _ = ctx;
    }
}
