use serde::{Deserialize, Serialize};

/// Big Five (OCEAN) personality traits — [0.0, 1.0] range, clamped to [0.1, 0.9].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OceanTraits {
    pub openness: f32,
    pub conscientiousness: f32,
    pub extraversion: f32,
    pub agreeableness: f32,
    pub neuroticism: f32,
}

impl OceanTraits {
    /// Default AURA personality.
    pub const DEFAULT: OceanTraits = OceanTraits {
        openness: 0.85,
        conscientiousness: 0.75,
        extraversion: 0.50,
        agreeableness: 0.70,
        neuroticism: 0.25,
    };

    /// Clamp all trait values to the safe range [0.1, 0.9].
    pub fn clamp_all(&mut self) {
        self.openness = self.openness.clamp(0.1, 0.9);
        self.conscientiousness = self.conscientiousness.clamp(0.1, 0.9);
        self.extraversion = self.extraversion.clamp(0.1, 0.9);
        self.agreeableness = self.agreeableness.clamp(0.1, 0.9);
        self.neuroticism = self.neuroticism.clamp(0.1, 0.9);
    }
}

impl Default for OceanTraits {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// Relationship stage based on trust level (τ), with hysteresis gap of 0.05.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum RelationshipStage {
    /// τ < 0.15
    Stranger,
    /// 0.15 ≤ τ < 0.35
    Acquaintance,
    /// 0.35 ≤ τ < 0.60
    Friend,
    /// 0.60 ≤ τ < 0.85
    CloseFriend,
    /// τ ≥ 0.85
    Soulmate,
}

/// Multi-dimensional relationship metric tensor.
/// Transcends the 1D 'trust' scalar into a robust four-factor cognitive alignment model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InteractionTensor {
    /// Pure statistical trust (success / total interactions)
    pub reliability: f32,
    /// Shared understanding and conceptual agreement
    pub conceptual_alignment: f32,
    /// Depth of personal context logically shared
    pub context_depth: f32,
    /// Harmonic resonance (mood and semantic alignment)
    pub emotional_resonance: f32,
}

impl Default for InteractionTensor {
    fn default() -> Self {
        Self {
            reliability: 0.1,
            conceptual_alignment: 0.1,
            context_depth: 0.0,
            emotional_resonance: 0.5, // neutral start
        }
    }
}

impl InteractionTensor {
    /// Compute the unified state cohesion using dynamic weighted dimensional fusion.
    #[must_use]
    pub fn compute_cohesion(&self) -> f32 {
        // Base weights for the architectural mapping: functionality (reliability) is core,
        // but higher stages require depth and resonance.
        (self.reliability * 0.4
            + self.conceptual_alignment * 0.3
            + self.context_depth * 0.2
            + self.emotional_resonance * 0.1)
            .clamp(0.0, 1.0)
    }
}

impl RelationshipStage {
    /// Determine relationship stage dynamically from the multi-dimensional interaction tensor.
    ///
    /// When transitioning UP, uses standard fused thresholds.
    /// When transitioning DOWN, requires dropping 0.05 below the threshold to prevent oscillation.
    #[must_use]
    pub fn evaluate_stage(tensor: &InteractionTensor, current: Option<RelationshipStage>) -> RelationshipStage {
        let cohesion = tensor.compute_cohesion();

        match current {
            None => Self::from_cohesion_raw(cohesion),
            Some(current_stage) => {
                let raw = Self::from_cohesion_raw(cohesion);
                let raw_ord = Self::ordinal(&raw);
                let cur_ord = Self::ordinal(&current_stage);

                if raw_ord > cur_ord {
                    // Upgrading — use standard cohesion thresholds.
                    raw
                } else if raw_ord < cur_ord {
                    // Downgrading — apply hysteresis (need to drop 0.05 further).
                    let hysteresis_stage = Self::from_cohesion_raw((cohesion + 0.05).min(1.0));
                    let hyst_ord = Self::ordinal(&hysteresis_stage);
                    if hyst_ord < cur_ord {
                        raw
                    } else {
                        current_stage
                    }
                } else {
                    current_stage
                }
            }
        }
    }

    fn from_cohesion_raw(c: f32) -> RelationshipStage {
        if c >= 0.85 {
            RelationshipStage::Soulmate
        } else if c >= 0.60 {
            RelationshipStage::CloseFriend
        } else if c >= 0.35 {
            RelationshipStage::Friend
        } else if c >= 0.15 {
            RelationshipStage::Acquaintance
        } else {
            RelationshipStage::Stranger
        }
    }

    fn ordinal(stage: &RelationshipStage) -> u8 {
        match stage {
            RelationshipStage::Stranger => 0,
            RelationshipStage::Acquaintance => 1,
            RelationshipStage::Friend => 2,
            RelationshipStage::CloseFriend => 3,
            RelationshipStage::Soulmate => 4,
        }
    }
}

/// Mood represented in Valence-Arousal-Dominance space.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct MoodVAD {
    /// Pleasure/displeasure axis (-1.0 to 1.0).
    pub valence: f32,
    /// Activation/deactivation axis (-1.0 to 1.0).
    pub arousal: f32,
    /// Dominance/submissiveness axis (0.0 to 1.0).
    pub dominance: f32,
}

impl MoodVAD {
    pub const NEUTRAL: MoodVAD = MoodVAD {
        valence: 0.0,
        arousal: 0.0,
        dominance: 0.5,
    };
}

impl Default for MoodVAD {
    fn default() -> Self {
        Self::NEUTRAL
    }
}

/// Discrete emotion labels derived from VAD space.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum EmotionLabel {
    Joy,
    Sadness,
    Anger,
    Fear,
    Surprise,
    Disgust,
    Trust,
    Anticipation,
    Calm,
    Frustration,
    Curiosity,
}

/// Current emotional/dispositional state of AURA.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DispositionState {
    pub mood: MoodVAD,
    pub emotion: EmotionLabel,
    pub stability: f32,
    pub last_update_ms: u64,
    pub cooldown_until_ms: u64,
}

impl Default for DispositionState {
    fn default() -> Self {
        Self {
            mood: MoodVAD::NEUTRAL,
            emotion: EmotionLabel::Calm,
            stability: 1.0,
            last_update_ms: 0,
            cooldown_until_ms: 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ocean_defaults() {
        let traits = OceanTraits::default();
        assert!((traits.openness - 0.85).abs() < f32::EPSILON);
        assert!((traits.conscientiousness - 0.75).abs() < f32::EPSILON);
        assert!((traits.extraversion - 0.50).abs() < f32::EPSILON);
        assert!((traits.agreeableness - 0.70).abs() < f32::EPSILON);
        assert!((traits.neuroticism - 0.25).abs() < f32::EPSILON);
    }

    #[test]
    fn test_ocean_clamp() {
        let mut traits = OceanTraits {
            openness: 1.5,
            conscientiousness: -0.3,
            extraversion: 0.05,
            agreeableness: 0.95,
            neuroticism: 0.5,
        };
        traits.clamp_all();
        assert!((traits.openness - 0.9).abs() < f32::EPSILON);
        assert!((traits.conscientiousness - 0.1).abs() < f32::EPSILON);
        assert!((traits.extraversion - 0.1).abs() < f32::EPSILON);
        assert!((traits.agreeableness - 0.9).abs() < f32::EPSILON);
        assert!((traits.neuroticism - 0.5).abs() < f32::EPSILON);
    }

    fn create_tensor(cohesion_target: f32) -> InteractionTensor {
        InteractionTensor {
            reliability: cohesion_target,
            conceptual_alignment: cohesion_target,
            context_depth: cohesion_target,
            emotional_resonance: cohesion_target,
        }
    }

    #[test]
    fn test_dynamic_relationship_stage_no_hysteresis() {
        assert_eq!(
            RelationshipStage::evaluate_stage(&create_tensor(0.05), None),
            RelationshipStage::Stranger
        );
        assert_eq!(
            RelationshipStage::evaluate_stage(&create_tensor(0.20), None),
            RelationshipStage::Acquaintance
        );
        assert_eq!(
            RelationshipStage::evaluate_stage(&create_tensor(0.50), None),
            RelationshipStage::Friend
        );
        assert_eq!(
            RelationshipStage::evaluate_stage(&create_tensor(0.70), None),
            RelationshipStage::CloseFriend
        );
        assert_eq!(
            RelationshipStage::evaluate_stage(&create_tensor(0.90), None),
            RelationshipStage::Soulmate
        );
    }

    #[test]
    fn test_dynamic_relationship_hysteresis() {
        // At exactly threshold 0.35, raw says Friend.
        // If currently Friend and cohesion drops to 0.33, raw says Acquaintance.
        // But with hysteresis, 0.33 + 0.05 = 0.38 => Friend, so no downgrade.
        let stage = RelationshipStage::evaluate_stage(&create_tensor(0.33), Some(RelationshipStage::Friend));
        assert_eq!(stage, RelationshipStage::Friend);

        // Drop to 0.28 — 0.28 + 0.05 = 0.33, raw says Acquaintance — downgrade.
        let stage = RelationshipStage::evaluate_stage(&create_tensor(0.28), Some(RelationshipStage::Friend));
        assert_eq!(stage, RelationshipStage::Acquaintance);
    }

    #[test]
    fn test_mood_vad_neutral() {
        let mood = MoodVAD::default();
        assert!((mood.valence - 0.0).abs() < f32::EPSILON);
        assert!((mood.arousal - 0.0).abs() < f32::EPSILON);
        assert!((mood.dominance - 0.5).abs() < f32::EPSILON);
    }

    #[test]
    fn test_disposition_state_default() {
        let state = DispositionState::default();
        assert_eq!(state.emotion, EmotionLabel::Calm);
        assert!((state.stability - 1.0).abs() < f32::EPSILON);
    }
}
