//! Personality → TTS parameter mapping.
//!
//! Voice parameters come from LLM mood_hint, not from personality computation.
//!
//! The LLM emits a `mood_hint: Option<f32>` (valence, -1.0 to +1.0) in
//! `ConversationReply`. This signal is the authoritative source of emotional
//! state for TTS — AURA's voice should reflect what the LLM is actually
//! expressing, not a mechanical mapping from OCEAN personality scores.

use super::tts::TtsParams;

// ---------------------------------------------------------------------------
// OCEAN personality scores (retained for personality configuration storage)
// ---------------------------------------------------------------------------

/// Big Five (OCEAN) personality scores, each in [0.0, 1.0].
#[derive(Debug, Clone)]
pub struct OceanScores {
    /// Openness to experience (creative, curious).
    pub openness: f32,
    /// Conscientiousness (organized, disciplined).
    pub conscientiousness: f32,
    /// Extraversion (outgoing, energetic).
    pub extraversion: f32,
    /// Agreeableness (friendly, compassionate).
    pub agreeableness: f32,
    /// Neuroticism (emotional instability, anxiety).
    pub neuroticism: f32,
}

impl Default for OceanScores {
    fn default() -> Self {
        // AURA's default personality: balanced, slightly agreeable and open
        Self {
            openness: 0.7,
            conscientiousness: 0.6,
            extraversion: 0.5,
            agreeableness: 0.65,
            neuroticism: 0.3,
        }
    }
}

impl OceanScores {
    /// Clamp all scores to [0.0, 1.0].
    pub fn clamped(mut self) -> Self {
        self.openness = self.openness.clamp(0.0, 1.0);
        self.conscientiousness = self.conscientiousness.clamp(0.0, 1.0);
        self.extraversion = self.extraversion.clamp(0.0, 1.0);
        self.agreeableness = self.agreeableness.clamp(0.0, 1.0);
        self.neuroticism = self.neuroticism.clamp(0.0, 1.0);
        self
    }
}

// ---------------------------------------------------------------------------
// Mood state
// ---------------------------------------------------------------------------

/// Current emotional mood state (from LLM mood_hint).
#[derive(Debug, Clone)]
pub struct MoodState {
    /// Valence: negative (0.0) to positive (1.0).
    pub valence: f32,
    /// Arousal: calm (0.0) to excited (1.0).
    pub arousal: f32,
    /// Dominance: submissive (0.0) to dominant (1.0).
    pub dominance: f32,
}

impl Default for MoodState {
    fn default() -> Self {
        // Neutral mood
        Self {
            valence: 0.5,
            arousal: 0.3,
            dominance: 0.5,
        }
    }
}

impl MoodState {
    pub fn clamped(mut self) -> Self {
        self.valence = self.valence.clamp(0.0, 1.0);
        self.arousal = self.arousal.clamp(0.0, 1.0);
        self.dominance = self.dominance.clamp(0.0, 1.0);
        self
    }

    /// Build a MoodState from the LLM's raw mood_hint float.
    ///
    /// `hint` is a valence signal in [-1.0, 1.0] where:
    ///   - negative → sad/concerned tone
    ///   - zero      → neutral
    ///   - positive  → happy/excited tone
    ///
    /// Arousal is derived as `|hint|` (stronger emotion = higher arousal).
    pub fn from_llm_hint(hint: f32) -> Self {
        let clamped = hint.clamp(-1.0, 1.0);
        Self {
            valence: (clamped + 1.0) / 2.0,     // map [-1,1] → [0,1]
            arousal: clamped.abs().clamp(0.0, 1.0),
            dominance: 0.5,
        }
    }
}

// ---------------------------------------------------------------------------
// Context for TTS parameter computation
// ---------------------------------------------------------------------------

/// Situational context that affects voice.
#[derive(Debug, Clone)]
pub enum SpeechContext {
    /// Normal conversation.
    Casual,
    /// Reading a notification.
    Notification,
    /// Urgent alert.
    Alert,
    /// Reading a long passage.
    LongForm,
    /// Whispering (e.g., late at night, user preference).
    Whisper,
    /// Phone call (need clarity).
    PhoneCall,
}

impl Default for SpeechContext {
    fn default() -> Self {
        Self::Casual
    }
}

// ---------------------------------------------------------------------------
// Mapping function
// ---------------------------------------------------------------------------

/// Map LLM mood_hint + context → TTS parameters.
///
/// Voice parameters come from LLM mood_hint, not from personality computation.
/// The LLM is the authoritative source of emotional state; OCEAN scores do NOT
/// drive voice parameters.
///
/// `mood_hint` is the raw `Option<f32>` from `ConversationReply`, a valence
/// signal in [-1.0, 1.0]. `None` means the LLM did not emit a hint → neutral.
pub fn mood_to_tts_params(mood_hint: Option<f32>, context: &SpeechContext) -> TtsParams {
    // -- Map LLM hint to named mood category ----------------------------
    // Voice parameters come from LLM mood_hint, not from personality computation.
    let (mut speed, mut pitch, mut volume) = match mood_hint {
        Some(hint) => {
            let v = hint.clamp(-1.0, 1.0);
            if v > 0.3 {
                // excited / positive
                (1.1_f32, 1.05_f32, 0.85_f32)
            } else if v < -0.3 {
                // concerned / negative
                (0.9_f32, 0.95_f32, 0.75_f32)
            } else {
                // calm / neutral
                (0.95_f32, 1.0_f32, 0.8_f32)
            }
        }
        // No hint from LLM → neutral defaults
        None => (1.0_f32, 1.0_f32, 0.8_f32),
    };

    // -- Context adjustments --------------------------------------------
    match context {
        SpeechContext::Casual => {} // no adjustments
        SpeechContext::Notification => {
            speed *= 1.05;
            volume = volume.max(0.7);
        }
        SpeechContext::Alert => {
            speed *= 0.95;
            pitch *= 1.05;
            volume = 0.95;
        }
        SpeechContext::LongForm => {
            speed *= 0.95;
            volume *= 0.9;
        }
        SpeechContext::Whisper => {
            speed *= 0.85;
            volume = 0.3;
            pitch *= 0.95;
        }
        SpeechContext::PhoneCall => {
            speed *= 0.95;
            volume = 0.85;
        }
    }

    TtsParams {
        speed: speed.clamp(0.5, 2.0),
        pitch: pitch.clamp(0.5, 2.0),
        volume: volume.clamp(0.0, 1.0),
        voice_id: DEFAULT_VOICE_ID.to_string(),
    }
}

/// The default Piper voice model used for all speech output.
pub const DEFAULT_VOICE_ID: &str = "en_US-amy-medium";

/// Convenience: build TTS params for an alert/notification (no LLM hint needed).
pub fn alert_tts_params() -> TtsParams {
    mood_to_tts_params(Some(-0.5), &SpeechContext::Alert)
}

// ---------------------------------------------------------------------------
// Legacy shim — kept so callers that still reference personality_to_tts_params
// compile without errors. Delegates to mood_to_tts_params, ignoring OCEAN.
// ---------------------------------------------------------------------------

/// Deprecated: use `mood_to_tts_params` instead.
///
/// The `ocean` and `mood` arguments are accepted but OCEAN scores are NOT used
/// to compute voice parameters. Only the mood's valence is forwarded.
/// Voice parameters come from LLM mood_hint, not from personality computation.
#[allow(dead_code)]
pub fn personality_to_tts_params(
    _ocean: &OceanScores,
    mood: &MoodState,
    context: &SpeechContext,
) -> TtsParams {
    // Convert MoodState valence back to the [-1, 1] hint range.
    let hint = mood.valence * 2.0 - 1.0;
    mood_to_tts_params(Some(hint), context)
}

/// Deprecated: use `mood_to_tts_params` instead.
#[allow(dead_code)]
pub fn personality_to_tts_params_simple(_ocean: &OceanScores) -> TtsParams {
    mood_to_tts_params(None, &SpeechContext::default())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn neutral_hint_produces_moderate_params() {
        let params = mood_to_tts_params(None, &SpeechContext::Casual);
        assert!(params.speed > 0.8 && params.speed < 1.3, "speed = {}", params.speed);
        assert!(params.pitch > 0.9 && params.pitch < 1.15, "pitch = {}", params.pitch);
        assert!(params.volume > 0.5 && params.volume < 1.0, "volume = {}", params.volume);
    }

    #[test]
    fn positive_hint_is_faster_and_brighter() {
        let excited = mood_to_tts_params(Some(0.8), &SpeechContext::Casual);
        let neutral = mood_to_tts_params(None, &SpeechContext::Casual);
        assert!(excited.speed > neutral.speed, "excited speed {} should exceed neutral {}", excited.speed, neutral.speed);
        assert!(excited.pitch >= neutral.pitch, "excited pitch {} should be >= neutral {}", excited.pitch, neutral.pitch);
    }

    #[test]
    fn negative_hint_is_slower_and_softer() {
        let concerned = mood_to_tts_params(Some(-0.8), &SpeechContext::Casual);
        let neutral = mood_to_tts_params(None, &SpeechContext::Casual);
        assert!(concerned.speed < neutral.speed, "concerned speed {} should be < neutral {}", concerned.speed, neutral.speed);
        assert!(concerned.pitch < neutral.pitch, "concerned pitch {} should be < neutral {}", concerned.pitch, neutral.pitch);
    }

    #[test]
    fn whisper_context_lowers_volume() {
        let params = mood_to_tts_params(None, &SpeechContext::Whisper);
        assert!(params.volume < 0.4, "whisper volume = {}", params.volume);
    }

    #[test]
    fn alert_context_raises_volume() {
        let params = alert_tts_params();
        assert!(params.volume > 0.8, "alert volume = {}", params.volume);
    }

    #[test]
    fn params_always_clamped() {
        let params = mood_to_tts_params(Some(100.0), &SpeechContext::Alert);
        assert!(params.speed >= 0.5 && params.speed <= 2.0);
        assert!(params.pitch >= 0.5 && params.pitch <= 2.0);
        assert!(params.volume >= 0.0 && params.volume <= 1.0);
    }

    #[test]
    fn from_llm_hint_maps_correctly() {
        let positive = MoodState::from_llm_hint(1.0);
        assert!((positive.valence - 1.0).abs() < f32::EPSILON);
        assert!((positive.arousal - 1.0).abs() < f32::EPSILON);

        let negative = MoodState::from_llm_hint(-1.0);
        assert!((negative.valence - 0.0).abs() < f32::EPSILON);
        assert!((negative.arousal - 1.0).abs() < f32::EPSILON);

        let neutral = MoodState::from_llm_hint(0.0);
        assert!((neutral.valence - 0.5).abs() < f32::EPSILON);
        assert!((neutral.arousal - 0.0).abs() < f32::EPSILON);
    }

    #[test]
    fn all_speech_contexts_produce_valid_params() {
        let contexts = [
            SpeechContext::Casual,
            SpeechContext::Notification,
            SpeechContext::Alert,
            SpeechContext::LongForm,
            SpeechContext::Whisper,
            SpeechContext::PhoneCall,
        ];
        for ctx in &contexts {
            let p = mood_to_tts_params(Some(0.5), ctx);
            assert!(p.speed >= 0.5 && p.speed <= 2.0);
            assert!(p.pitch >= 0.5 && p.pitch <= 2.0);
            assert!(p.volume >= 0.0 && p.volume <= 1.0);
        }
    }
}
