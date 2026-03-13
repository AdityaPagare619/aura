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

// TODO(types): Phase 3 wire-point — InteractionTensor cohesion computation and
// RelationshipStage classification (evaluate_stage, hysteresis logic, ordinal mapping)
// belong in the identity engine crate, not in aura-types. Consumers receive a
// pre-computed RelationshipStage variant over IPC; no Rust-side classification
// should occur here.

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
