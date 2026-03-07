//! Personality → TTS parameter mapping.
//!
//! Maps AURA's OCEAN personality scores and current mood to voice parameters
//! (speed, pitch, volume, voice selection). This makes AURA's voice feel
//! consistent with its personality and emotionally responsive.

use super::tts::TtsParams;

// ---------------------------------------------------------------------------
// OCEAN personality scores
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

/// Current emotional mood state (from Amygdala).
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

/// Map personality + mood + context → TTS parameters.
pub fn personality_to_tts_params(
    ocean: &OceanScores,
    mood: &MoodState,
    context: &SpeechContext,
) -> TtsParams {
    // -- Base parameters from personality --------------------------------

    // Extraversion → speed: extroverts speak faster
    let base_speed = 0.9 + ocean.extraversion * 0.3; // 0.9 – 1.2

    // Neuroticism → pitch: higher N → slightly higher pitch
    let base_pitch = 0.95 + ocean.neuroticism * 0.15; // 0.95 – 1.10

    // Agreeableness → softer volume (less aggressive)
    let base_volume = 0.6 + ocean.agreeableness * 0.3; // 0.6 – 0.9

    // -- Mood overlay ---------------------------------------------------

    // Positive valence → slightly faster, brighter pitch
    let mood_speed_mod = (mood.valence - 0.5) * 0.1; // -0.05 to +0.05
    let mood_pitch_mod = (mood.valence - 0.5) * 0.08; // -0.04 to +0.04

    // High arousal → faster, louder
    let arousal_speed_mod = (mood.arousal - 0.3) * 0.15; // varies
    let arousal_volume_mod = (mood.arousal - 0.3) * 0.1;

    let mut speed = base_speed + mood_speed_mod + arousal_speed_mod;
    let mut pitch = base_pitch + mood_pitch_mod;
    let mut volume = base_volume + arousal_volume_mod;

    // -- Context adjustments --------------------------------------------
    match context {
        SpeechContext::Casual => {} // no adjustments
        SpeechContext::Notification => {
            speed *= 1.05; // slightly faster for notifications
            volume = volume.max(0.7); // ensure audible
        }
        SpeechContext::Alert => {
            speed *= 0.95; // slightly slower for clarity
            pitch *= 1.05; // slightly higher for urgency
            volume = 0.95; // loud
        }
        SpeechContext::LongForm => {
            speed *= 0.95; // slower for comprehension
            volume *= 0.9; // slightly softer for comfort
        }
        SpeechContext::Whisper => {
            speed *= 0.85;
            volume = 0.3; // whisper
            pitch *= 0.95;
        }
        SpeechContext::PhoneCall => {
            speed *= 0.95; // clear articulation
            volume = 0.85;
        }
    }

    // -- Voice selection ------------------------------------------------
    let voice_id = select_voice(ocean, mood);

    TtsParams {
        speed: speed.clamp(0.5, 2.0),
        pitch: pitch.clamp(0.5, 2.0),
        volume: volume.clamp(0.0, 1.0),
        voice_id,
    }
}

/// Select the best Piper voice model based on personality.
fn select_voice(ocean: &OceanScores, _mood: &MoodState) -> String {
    // Higher extraversion → more expressive voice
    // Higher conscientiousness → more formal voice
    if ocean.extraversion > 0.7 {
        "en_US-lessac-medium".to_string() // expressive
    } else if ocean.conscientiousness > 0.7 {
        "en_US-ryan-medium".to_string() // measured, clear
    } else {
        "en_US-amy-medium".to_string() // neutral, default
    }
}

/// Shorthand: map with default mood and context.
pub fn personality_to_tts_params_simple(ocean: &OceanScores) -> TtsParams {
    personality_to_tts_params(ocean, &MoodState::default(), &SpeechContext::default())
}

/// Create TTS params for an alert/notification with urgency.
pub fn alert_tts_params() -> TtsParams {
    personality_to_tts_params(
        &OceanScores::default(),
        &MoodState {
            valence: 0.3,
            arousal: 0.8,
            dominance: 0.7,
        },
        &SpeechContext::Alert,
    )
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_personality_produces_moderate_params() {
        let ocean = OceanScores::default();
        let params = personality_to_tts_params_simple(&ocean);

        // Speed should be moderate (around 1.0)
        assert!(
            params.speed > 0.8 && params.speed < 1.3,
            "speed = {}",
            params.speed
        );
        // Pitch should be near 1.0
        assert!(
            params.pitch > 0.9 && params.pitch < 1.15,
            "pitch = {}",
            params.pitch
        );
        // Volume should be moderate
        assert!(
            params.volume > 0.5 && params.volume < 1.0,
            "volume = {}",
            params.volume
        );
    }

    #[test]
    fn extrovert_speaks_faster() {
        let extrovert = OceanScores {
            extraversion: 1.0,
            ..OceanScores::default()
        };
        let introvert = OceanScores {
            extraversion: 0.0,
            ..OceanScores::default()
        };

        let ext_params = personality_to_tts_params_simple(&extrovert);
        let int_params = personality_to_tts_params_simple(&introvert);

        assert!(
            ext_params.speed > int_params.speed,
            "extrovert speed {} should exceed introvert speed {}",
            ext_params.speed,
            int_params.speed
        );
    }

    #[test]
    fn positive_mood_brightens_voice() {
        let ocean = OceanScores::default();
        let happy = MoodState {
            valence: 1.0,
            arousal: 0.6,
            dominance: 0.5,
        };
        let sad = MoodState {
            valence: 0.0,
            arousal: 0.1,
            dominance: 0.3,
        };

        let happy_params = personality_to_tts_params(&ocean, &happy, &SpeechContext::Casual);
        let sad_params = personality_to_tts_params(&ocean, &sad, &SpeechContext::Casual);

        assert!(happy_params.speed > sad_params.speed);
        assert!(happy_params.pitch > sad_params.pitch);
    }

    #[test]
    fn whisper_context_lowers_volume() {
        let ocean = OceanScores::default();
        let mood = MoodState::default();
        let params = personality_to_tts_params(&ocean, &mood, &SpeechContext::Whisper);
        assert!(params.volume < 0.4, "whisper volume = {}", params.volume);
    }

    #[test]
    fn alert_context_raises_volume() {
        let params = alert_tts_params();
        assert!(params.volume > 0.8, "alert volume = {}", params.volume);
    }

    #[test]
    fn params_always_clamped() {
        let extreme = OceanScores {
            openness: 10.0,
            conscientiousness: -1.0,
            extraversion: 5.0,
            agreeableness: 0.0,
            neuroticism: 3.0,
        };
        let mood = MoodState {
            valence: 5.0,
            arousal: 5.0,
            dominance: 5.0,
        };
        let params = personality_to_tts_params(&extreme, &mood, &SpeechContext::Alert);

        assert!(params.speed >= 0.5 && params.speed <= 2.0);
        assert!(params.pitch >= 0.5 && params.pitch <= 2.0);
        assert!(params.volume >= 0.0 && params.volume <= 1.0);
    }

    #[test]
    fn voice_selection_by_personality() {
        let extrovert = OceanScores {
            extraversion: 0.9,
            ..OceanScores::default()
        };
        let params = personality_to_tts_params_simple(&extrovert);
        assert_eq!(params.voice_id, "en_US-lessac-medium");

        let conscientious = OceanScores {
            extraversion: 0.3,
            conscientiousness: 0.9,
            ..OceanScores::default()
        };
        let params = personality_to_tts_params_simple(&conscientious);
        assert_eq!(params.voice_id, "en_US-ryan-medium");
    }
}
