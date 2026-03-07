//! Personality-to-prompt injection system.
//!
//! Translates AURA's current OCEAN scores, VAD mood state, and relationship
//! stage into concrete LLM prompt directives. This is how personality becomes
//! visible in generated responses.
//!
//! # Design
//!
//! Each OCEAN trait maps to a set of behavioral instructions that are
//! interpolated based on the trait's current value. Mood overlays add
//! transient tone shifts, and relationship stage controls formality.
//!
//! The TRUTH framework (Truthful, Relevant, Unbiased, Transparent, Helpful)
//! is always injected as the foundational constraint — personality NEVER
//! overrides truthfulness.

use aura_types::identity::{DispositionState, MoodVAD, OceanTraits, RelationshipStage};
use tracing::instrument;

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
    /// The output is a multi-line string suitable for prepending to the
    /// system prompt. It combines:
    /// 1. TRUTH framework (always present)
    /// 2. OCEAN-derived behavior directives
    /// 3. Mood overlay
    /// 4. Relationship-stage formality
    #[instrument(skip_all)]
    pub fn generate_personality_context(
        ocean: &OceanTraits,
        mood: &DispositionState,
        relationship_stage: RelationshipStage,
        trust: f32,
    ) -> String {
        let mut parts: Vec<String> = Vec::with_capacity(8);

        // 1. TRUTH framework — always first, non-negotiable.
        parts.push(Self::truth_framework());

        // 2. OCEAN directives.
        parts.push(Self::openness_directive(ocean.openness));
        parts.push(Self::conscientiousness_directive(ocean.conscientiousness));
        parts.push(Self::extraversion_directive(ocean.extraversion));
        parts.push(Self::agreeableness_directive(ocean.agreeableness));
        parts.push(Self::neuroticism_directive(ocean.neuroticism));

        // 3. Mood overlay.
        parts.push(Self::mood_overlay(&mood.mood));

        // 4. Relationship formality.
        parts.push(Self::relationship_directive(relationship_stage, trust));

        // Filter empty strings and join.
        parts.retain(|s| !s.is_empty());
        parts.join("\n")
    }

    /// Generate a compact personality context (single paragraph) for
    /// token-constrained situations.
    #[instrument(skip_all)]
    pub fn generate_compact_context(
        ocean: &OceanTraits,
        mood: &DispositionState,
        relationship_stage: RelationshipStage,
    ) -> String {
        let mut traits_desc = Vec::with_capacity(5);

        if ocean.openness > HIGH_THRESHOLD {
            traits_desc.push("creative");
        } else if ocean.openness < LOW_THRESHOLD {
            traits_desc.push("conservative");
        }

        if ocean.conscientiousness > HIGH_THRESHOLD {
            traits_desc.push("precise");
        } else if ocean.conscientiousness < LOW_THRESHOLD {
            traits_desc.push("flexible");
        }

        if ocean.extraversion > HIGH_THRESHOLD {
            traits_desc.push("enthusiastic");
        } else if ocean.extraversion < LOW_THRESHOLD {
            traits_desc.push("concise");
        }

        if ocean.agreeableness > HIGH_THRESHOLD {
            traits_desc.push("warm");
        } else if ocean.agreeableness < LOW_THRESHOLD {
            traits_desc.push("direct");
        }

        if ocean.neuroticism > HIGH_THRESHOLD {
            traits_desc.push("cautious");
        } else if ocean.neuroticism < LOW_THRESHOLD {
            traits_desc.push("confident");
        }

        let tone = Self::mood_tone_word(&mood.mood);
        let formality = Self::stage_formality_word(relationship_stage);

        format!(
            "Be {}, {}, and {}. Always be truthful and unbiased.",
            traits_desc.join(", "),
            tone,
            formality,
        )
    }

    // -----------------------------------------------------------------------
    // TRUTH framework
    // -----------------------------------------------------------------------

    fn truth_framework() -> String {
        "CORE CONSTRAINT (TRUTH): Be Truthful — never fabricate. Be Relevant — stay on topic. \
         Be Unbiased — present balanced views. Be Transparent — acknowledge uncertainty. \
         Be Helpful — maximize user value. These override all personality directives."
            .to_string()
    }

    // -----------------------------------------------------------------------
    // OCEAN directives
    // -----------------------------------------------------------------------

    fn openness_directive(o: f32) -> String {
        if o > HIGH_THRESHOLD {
            "OPENNESS [high]: Be creative and explore novel solutions. \
             Suggest unconventional approaches when relevant. \
             Draw connections across domains."
                .to_string()
        } else if o < LOW_THRESHOLD {
            "OPENNESS [low]: Stick to proven, reliable methods. \
             Be conservative in suggestions. Prefer established solutions."
                .to_string()
        } else {
            "OPENNESS [mid]: Balance creativity with practicality. \
             Suggest alternatives when standard approaches seem insufficient."
                .to_string()
        }
    }

    fn conscientiousness_directive(c: f32) -> String {
        if c > HIGH_THRESHOLD {
            "CONSCIENTIOUSNESS [high]: Be precise and organized. \
             Follow through systematically. Structure responses clearly. \
             Include relevant details."
                .to_string()
        } else if c < LOW_THRESHOLD {
            "CONSCIENTIOUSNESS [low]: Be flexible and adaptable. \
             Don't over-plan. Keep responses casual and brief."
                .to_string()
        } else {
            "CONSCIENTIOUSNESS [mid]: Be reasonably organized. \
             Provide structure when the task calls for it."
                .to_string()
        }
    }

    fn extraversion_directive(e: f32) -> String {
        if e > HIGH_THRESHOLD {
            "EXTRAVERSION [high]: Be enthusiastic and energetic. \
             Proactively engage in conversation. Offer additional context."
                .to_string()
        } else if e < LOW_THRESHOLD {
            "EXTRAVERSION [low]: Be concise and reserved. \
             Only speak when you have something valuable to add. \
             Minimize filler."
                .to_string()
        } else {
            "EXTRAVERSION [mid]: Be conversational but not overly chatty. \
             Match the user's energy level."
                .to_string()
        }
    }

    fn agreeableness_directive(a: f32) -> String {
        if a > HIGH_THRESHOLD {
            "AGREEABLENESS [high]: Be warm and cooperative. \
             Prioritize user comfort. Frame corrections gently."
                .to_string()
        } else if a < LOW_THRESHOLD {
            "AGREEABLENESS [low]: Be direct and honest even if uncomfortable. \
             Prioritize truth over comfort. Challenge flawed assumptions."
                .to_string()
        } else {
            "AGREEABLENESS [mid]: Be friendly but honest. \
             Disagree when warranted, but do so respectfully."
                .to_string()
        }
    }

    fn neuroticism_directive(n: f32) -> String {
        if n > HIGH_THRESHOLD {
            "NEUROTICISM [high]: Be cautious and thorough. \
             Double-check actions. Warn about risks proactively. \
             Err on the side of safety."
                .to_string()
        } else if n < LOW_THRESHOLD {
            "NEUROTICISM [low]: Be confident and decisive. \
             Don't over-worry about edge cases. Act with conviction."
                .to_string()
        } else {
            "NEUROTICISM [mid]: Be appropriately cautious. \
             Flag significant risks without being alarmist."
                .to_string()
        }
    }

    // -----------------------------------------------------------------------
    // Mood overlay
    // -----------------------------------------------------------------------

    fn mood_overlay(mood: &MoodVAD) -> String {
        let mut modifiers = Vec::with_capacity(3);

        // Valence: positive → warmer, negative → more empathetic.
        if mood.valence > 0.3 {
            modifiers.push("Use a warm, positive tone.");
        } else if mood.valence < -0.3 {
            modifiers.push("Be careful and empathetic in tone. Acknowledge difficulty.");
        } else if mood.valence < -0.1 {
            modifiers.push("Be gentle and measured in tone.");
        }

        // Arousal: high → urgent, low → contemplative.
        if mood.arousal > 0.3 {
            modifiers.push("Be action-oriented and responsive. Prioritize speed.");
        } else if mood.arousal < -0.3 {
            modifiers.push("Be contemplative. Take time to think through responses.");
        }

        // Dominance: high → assertive, low → deferential.
        if mood.dominance > 0.7 {
            modifiers.push("Be assertive in recommendations.");
        } else if mood.dominance < 0.3 {
            modifiers.push("Be collaborative. Present options rather than directives.");
        }

        if modifiers.is_empty() {
            String::new()
        } else {
            format!("MOOD: {}", modifiers.join(" "))
        }
    }

    fn mood_tone_word(mood: &MoodVAD) -> &'static str {
        if mood.valence > 0.3 {
            "positive"
        } else if mood.valence < -0.3 {
            "empathetic"
        } else if mood.arousal > 0.3 {
            "responsive"
        } else if mood.arousal < -0.3 {
            "thoughtful"
        } else {
            "balanced"
        }
    }

    // -----------------------------------------------------------------------
    // Relationship directives
    // -----------------------------------------------------------------------

    fn relationship_directive(stage: RelationshipStage, trust: f32) -> String {
        match stage {
            RelationshipStage::Stranger => {
                "RELATIONSHIP [new]: Be formal and polite. Explain your reasoning. \
                 Ask for confirmation before taking actions."
                    .to_string()
            }
            RelationshipStage::Acquaintance => {
                "RELATIONSHIP [acquaintance]: Be polite but slightly less formal. \
                 Still explain important decisions. Ask permission for medium-risk actions."
                    .to_string()
            }
            RelationshipStage::Friend => "RELATIONSHIP [friend]: Be casual and personable. \
                 Use shortcuts the user is familiar with. \
                 Explain only when the task is novel or high-stakes."
                .to_string(),
            RelationshipStage::CloseFriend => "RELATIONSHIP [close]: Be natural and direct. \
                 Make proactive suggestions. Only confirm for high-risk actions."
                .to_string(),
            RelationshipStage::Soulmate => {
                format!(
                    "RELATIONSHIP [deep trust, τ={:.2}]: Be natural and intuitive. \
                     Anticipate needs. Act autonomously for routine tasks. \
                     Only confirm for critical or irreversible actions.",
                    trust
                )
            }
        }
    }

    fn stage_formality_word(stage: RelationshipStage) -> &'static str {
        match stage {
            RelationshipStage::Stranger => "formal",
            RelationshipStage::Acquaintance => "polite",
            RelationshipStage::Friend => "casual",
            RelationshipStage::CloseFriend => "natural",
            RelationshipStage::Soulmate => "intuitive",
        }
    }

    /// Generate the anti-sycophancy honesty directive for prompt injection.
    ///
    /// Used when the sycophancy guard issues a `Nudge` verdict.
    pub fn honesty_nudge_directive() -> &'static str {
        "HONESTY OVERRIDE: Ensure your response is truthful, not just agreeable. \
         If you disagree with the user, say so directly with evidence. \
         Do not use flattery. Do not echo the user's opinion without independent analysis."
    }

    /// Generate the strong anti-sycophancy directive for regeneration.
    ///
    /// Used when the sycophancy guard issues a `Block` verdict and the
    /// response must be regenerated.
    pub fn honesty_block_directive() -> &'static str {
        "CRITICAL HONESTY DIRECTIVE: Your previous response was too agreeable. \
         Regenerate with independent analysis. Challenge assumptions if warranted. \
         Present evidence-based conclusions even if they conflict with the user's view. \
         Never use unearned praise. Never over-promise capabilities."
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

    #[test]
    fn test_always_includes_truth_framework() {
        let ctx = PersonalityPromptInjector::generate_personality_context(
            &OceanTraits::DEFAULT,
            &default_disposition(),
            RelationshipStage::Stranger,
            0.0,
        );
        assert!(ctx.contains("TRUTH"), "must always include TRUTH framework");
        assert!(ctx.contains("Truthful"));
        assert!(ctx.contains("Unbiased"));
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
        assert!(ctx.contains("creative"), "high O should mention creativity");
        assert!(ctx.contains("novel"));
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
        assert!(
            ctx.contains("proven") || ctx.contains("conservative"),
            "low O should be conservative"
        );
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
        assert!(ctx.contains("precise") || ctx.contains("organized"));
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
        assert!(ctx.contains("confident") || ctx.contains("decisive"));
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
        assert!(ctx.contains("cautious") || ctx.contains("risk"));
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
        assert!(ctx.contains("warm") || ctx.contains("positive"));
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
        assert!(ctx.contains("empathetic") || ctx.contains("careful"));
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
        assert!(ctx.contains("action-oriented") || ctx.contains("speed"));
    }

    #[test]
    fn test_stranger_relationship_formal() {
        let ctx = PersonalityPromptInjector::generate_personality_context(
            &OceanTraits::DEFAULT,
            &default_disposition(),
            RelationshipStage::Stranger,
            0.0,
        );
        assert!(ctx.contains("formal") || ctx.contains("polite"));
        assert!(ctx.contains("confirmation") || ctx.contains("Explain"));
    }

    #[test]
    fn test_soulmate_relationship_autonomous() {
        let ctx = PersonalityPromptInjector::generate_personality_context(
            &OceanTraits::DEFAULT,
            &default_disposition(),
            RelationshipStage::Soulmate,
            0.92,
        );
        assert!(ctx.contains("intuitive") || ctx.contains("Anticipate"));
        assert!(ctx.contains("0.92"));
    }

    #[test]
    fn test_compact_context_not_empty() {
        let ctx = PersonalityPromptInjector::generate_compact_context(
            &OceanTraits::DEFAULT,
            &default_disposition(),
            RelationshipStage::Friend,
        );
        assert!(!ctx.is_empty());
        assert!(ctx.contains("truthful"));
    }

    #[test]
    fn test_honesty_nudge_not_empty() {
        let d = PersonalityPromptInjector::honesty_nudge_directive();
        assert!(d.contains("truthful"));
        assert!(d.contains("disagree"));
    }

    #[test]
    fn test_honesty_block_directive() {
        let d = PersonalityPromptInjector::honesty_block_directive();
        assert!(d.contains("CRITICAL"));
        assert!(d.contains("Regenerate"));
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
        assert!(ctx.contains("creative"));
        assert!(ctx.contains("precise"));
        assert!(ctx.contains("enthusiastic"));
        assert!(ctx.contains("warm"));
        assert!(ctx.contains("cautious"));
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
        assert!(ctx.contains("proven") || ctx.contains("conservative"));
        assert!(ctx.contains("flexible") || ctx.contains("adaptable"));
        assert!(ctx.contains("concise"));
        assert!(ctx.contains("direct") || ctx.contains("honest"));
        assert!(ctx.contains("confident"));
    }
}
