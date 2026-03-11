use aura_types::identity::{DispositionState, EmotionLabel, MoodVAD, OceanTraits};
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Decay half-life in milliseconds (300 s).
const DECAY_HALF_LIFE_MS: f32 = 300_000.0;

/// EMA time constant in milliseconds (15 min = 900 s).
const EMA_TAU_MS: f32 = 900_000.0;

/// Major mood change threshold that triggers cooldown.
const MAJOR_CHANGE_THRESHOLD: f32 = 0.3;

/// Cooldown duration in milliseconds (30 min).
const COOLDOWN_DURATION_MS: u64 = 30 * 60 * 1_000;

/// Stability gate — if stability < this, reduce effect by 50%.
const STABILITY_GATE: f32 = 0.6;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Mood-affecting events with preset (valence, arousal) deltas.
#[derive(Debug, Clone)]
pub enum MoodEvent {
    TaskSucceeded,
    TaskFailed,
    UserHappy,
    UserFrustrated,
    Compliment,
    Criticism,
    /// `duration_ms` is how long the silence has lasted.
    Silence {
        duration_ms: u64,
    },
    /// Voice biomarker detected elevated stress (0.0 … 1.0).
    VoiceStressDetected {
        level: f32,
    },
    /// Voice biomarker detected fatigue (0.0 … 1.0).
    VoiceFatigueDetected {
        level: f32,
    },
}

impl MoodEvent {
    /// Returns `(delta_valence, delta_arousal)` for this event.
    fn deltas(&self) -> Option<(f32, f32)> {
        match self {
            MoodEvent::TaskSucceeded => Some((0.10, 0.05)),
            MoodEvent::TaskFailed => Some((-0.15, 0.10)),
            MoodEvent::UserHappy => Some((0.20, 0.10)),
            MoodEvent::UserFrustrated => Some((-0.10, 0.15)),
            MoodEvent::Compliment => Some((0.15, 0.05)),
            MoodEvent::Criticism => Some((-0.10, 0.10)),
            MoodEvent::Silence { duration_ms } => {
                if *duration_ms >= 300_000 {
                    Some((-0.05, -0.10))
                } else {
                    None // Silence not long enough to affect mood.
                }
            }
            // Stress lowers valence and raises arousal proportionally.
            MoodEvent::VoiceStressDetected { level } => {
                let l = level.clamp(&0.0, &1.0);
                Some((-0.10 * l, 0.15 * l))
            }
            // Fatigue lowers both valence and arousal proportionally.
            MoodEvent::VoiceFatigueDetected { level } => {
                let l = level.clamp(&0.0, &1.0);
                Some((-0.05 * l, -0.10 * l))
            }
        }
    }
}

/// Engine that manages AURA's emotional state using the VAD model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AffectiveEngine {
    state: DispositionState,
}

// ---------------------------------------------------------------------------
// Implementation
// ---------------------------------------------------------------------------

impl AffectiveEngine {
    pub fn new() -> Self {
        Self {
            state: DispositionState::default(),
        }
    }

    /// Process a mood event, updating the internal disposition state.
    ///
    /// Steps:
    /// 1. Check cooldown — if active, reject.
    /// 2. Decay existing mood toward neutral.
    /// 3. Compute deltas; apply stability gate if stability < 0.6.
    /// 4. Apply EMA smoothing.
    /// 5. Classify emotion.
    /// 6. If major change, set cooldown.
    pub fn process_event(&mut self, event: MoodEvent, now_ms: u64) {
        // 1. Cooldown check.
        if now_ms < self.state.cooldown_until_ms {
            tracing::warn!(
                cooldown_remaining_ms = self.state.cooldown_until_ms - now_ms,
                "mood update rejected — cooldown active"
            );
            return;
        }

        let elapsed_ms = now_ms.saturating_sub(self.state.last_update_ms);

        // 2. Decay existing mood toward neutral.
        if elapsed_ms > 0 && self.state.last_update_ms > 0 {
            let decay = 0.5_f32.powf(elapsed_ms as f32 / DECAY_HALF_LIFE_MS);
            self.state.mood.valence *= decay;
            self.state.mood.arousal *= decay;
            // Dominance does not decay.
        }

        // 3. Compute deltas from event.
        let (dv, da) = match event.deltas() {
            Some(d) => d,
            None => {
                // No mood effect; just update timestamp.
                self.state.last_update_ms = now_ms;
                return;
            }
        };

        // Apply stability gate.
        let (dv, da) = if self.state.stability < STABILITY_GATE {
            (dv * 0.5, da * 0.5)
        } else {
            (dv, da)
        };

        // 4. EMA smoothing.
        //    On the very first event (`last_update_ms == 0`) the elapsed
        //    time to `now_ms` is meaningless — there is no prior mood to
        //    smooth against.  Apply the full delta immediately (alpha = 1.0)
        //    so the first mood event takes full effect.
        let alpha = if self.state.last_update_ms == 0 {
            1.0
        } else if elapsed_ms > 0 {
            1.0 - (-(elapsed_ms as f32) / EMA_TAU_MS).exp()
        } else {
            1.0
        };

        let target_v = self.state.mood.valence + dv;
        let target_a = self.state.mood.arousal + da;

        let old_valence = self.state.mood.valence;
        let old_arousal = self.state.mood.arousal;

        self.state.mood.valence = (old_valence + alpha * (target_v - old_valence)).clamp(-1.0, 1.0);
        self.state.mood.arousal = (old_arousal + alpha * (target_a - old_arousal)).clamp(-1.0, 1.0);
        self.state.mood.dominance = self.state.mood.dominance.clamp(0.0, 1.0);

        // 5. Classify emotion.
        self.state.emotion = Self::classify_emotion(&self.state.mood);

        // 6. Major change detection → cooldown.
        let delta_magnitude = ((self.state.mood.valence - old_valence).powi(2)
            + (self.state.mood.arousal - old_arousal).powi(2))
        .sqrt();

        if delta_magnitude > MAJOR_CHANGE_THRESHOLD {
            self.state.cooldown_until_ms = now_ms + COOLDOWN_DURATION_MS;
            tracing::info!(
                delta = delta_magnitude,
                "major mood change — cooldown set for 30 min"
            );
        }

        // Update stability based on magnitude of change (simple model).
        // Larger changes reduce stability, small changes restore it.
        if delta_magnitude > 0.1 {
            self.state.stability = (self.state.stability - 0.05).clamp(0.0, 1.0);
        } else {
            self.state.stability = (self.state.stability + 0.01).clamp(0.0, 1.0);
        }

        self.state.last_update_ms = now_ms;
    }

    /// Returns a reference to the current disposition state.
    pub fn current_state(&self) -> &DispositionState {
        &self.state
    }

    // -- prompt-facing methods (Team 5 wiring) ------------------------------

    /// Generate a human-readable mood context string for prompt injection.
    ///
    /// Returns a short description of the current emotional state suitable
    /// for embedding in an LLM system prompt.
    #[tracing::instrument(skip(self))]
    pub fn mood_context_string(&self) -> String {
        let v = self.state.mood.valence;
        let a = self.state.mood.arousal;
        let d = self.state.mood.dominance;

        let valence_str = if v > 0.3 {
            "positive"
        } else if v < -0.3 {
            "low"
        } else if v < -0.1 {
            "slightly low"
        } else {
            "neutral"
        };

        let arousal_str = if a > 0.3 {
            "high energy"
        } else if a < -0.3 {
            "contemplative"
        } else {
            "moderate energy"
        };

        let dominance_str = if d > 0.7 {
            "assertive"
        } else if d < 0.3 {
            "collaborative"
        } else {
            "balanced"
        };

        format!(
            "Mood: {} valence, {}, {} stance. Emotion: {:?}. Stability: {:.0}%.",
            valence_str,
            arousal_str,
            dominance_str,
            self.state.emotion,
            self.state.stability * 100.0,
        )
    }

    /// Compute an urgency modifier from arousal.
    ///
    /// Returns a value in \[0.8, 1.2\] that can multiply response latency
    /// targets or token budgets.
    ///
    /// - High arousal → urgency modifier > 1.0 (more urgent, prioritize speed)
    /// - Low arousal  → urgency modifier < 1.0 (take more time, be thorough)
    pub fn urgency_modifier(&self) -> f32 {
        // Map arousal [-1, 1] to [0.8, 1.2]
        let modifier = 1.0 + self.state.mood.arousal * 0.2;
        modifier.clamp(0.8, 1.2)
    }

    /// Compute a tone modifier from valence.
    ///
    /// Returns a value in \[-1.0, 1.0\]:
    /// - Positive → warmer, more encouraging tone
    /// - Negative → more cautious, empathetic tone
    /// - Zero → neutral
    pub fn tone_modifier(&self) -> f32 {
        self.state.mood.valence.clamp(-1.0, 1.0)
    }

    /// Classify a `MoodVAD` into a discrete `EmotionLabel`.
    ///
    /// Rules (evaluated in order):
    /// - Joy:       V > 0.3,  A > 0,   D > 0.3  (D > 0.3 on 0–1 scale)
    /// - Anger:     V < -0.1, A > 0.3, D > 0.3
    /// - Fear:      V < -0.1, A > 0.3, D < 0.2  (low dominance)
    /// - Sadness:   V < -0.2, A < 0,   D < 0.5  (below neutral dominance)
    /// - Curiosity: V > 0,    A > 0.2, D > 0.5
    /// - Calm:      |V| < 0.2, |A| < 0.2
    /// - Default:   Calm
    pub fn classify_emotion(mood: &MoodVAD) -> EmotionLabel {
        let v = mood.valence;
        let a = mood.arousal;
        let d = mood.dominance;

        if v > 0.3 && a > 0.0 && d > 0.3 {
            EmotionLabel::Joy
        } else if v < -0.1 && a > 0.3 && d > 0.3 {
            EmotionLabel::Anger
        } else if v < -0.1 && a > 0.3 && d < 0.2 {
            EmotionLabel::Fear
        } else if v < -0.2 && a < 0.0 && d < 0.5 {
            EmotionLabel::Sadness
        } else if v > 0.0 && a > 0.2 && d > 0.5 {
            EmotionLabel::Curiosity
        } else if v.abs() < 0.2 && a.abs() < 0.2 {
            EmotionLabel::Calm
        } else {
            EmotionLabel::Calm
        }
    }
}

impl Default for AffectiveEngine {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Mood Modifier — unified snapshot for pipeline consumers
// ---------------------------------------------------------------------------

/// Unified mood influence snapshot for pipeline consumers.
///
/// Combines urgency, tone, patience, empathy, and creativity boosts
/// derived from the current VAD state into a single struct that other
/// subsystems can query without knowing about VAD internals.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MoodModifier {
    /// Urgency factor in \[0.8, 1.2\].  > 1.0 = more urgent.
    pub urgency: f32,
    /// Tone factor in \[-1.0, 1.0\].  Positive = warmer.
    pub tone: f32,
    /// Patience factor in \[0.0, 1.0\].  Inverse of arousal.
    pub patience: f32,
    /// Empathy boost in \[0.0, 1.0\].  Higher when valence is negative.
    pub empathy_boost: f32,
    /// Creativity boost in \[0.0, 1.0\].  Higher when arousal is moderate
    /// and valence is positive.
    pub creativity_boost: f32,
}

/// Response style modifiers derived from affective state.
///
/// These drive adaptive response behavior in the daemon:
/// - `shorten_factor`: Multiply response target length (0.5-1.0)
/// - `emoji_boost`: Emoji frequency modifier (0.0-0.5)
/// - `empathy_level`: Empathy injection level (0.0-1.0)
/// - `warmth_level`: Warmth/positive tone level (0.0-1.0)
/// - `directness_level`: Direct vs. verbose (0.0-1.0)
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct ResponseStyleModifier {
    /// Response length multiplier. < 1.0 = shorten response.
    pub shorten_factor: f32,
    /// Emoji usage boost. > 0.0 = add more emojis.
    pub emoji_boost: f32,
    /// Empathy injection level. Higher for distressed users.
    pub empathy_level: f32,
    /// Warmth/positive tone level.
    pub warmth_level: f32,
    /// Directness vs. verbose. Higher = more direct.
    pub directness_level: f32,
}

impl Default for ResponseStyleModifier {
    fn default() -> Self {
        Self {
            shorten_factor: 1.0,
            emoji_boost: 0.0,
            empathy_level: 0.0,
            warmth_level: 0.5,
            directness_level: 0.2,
        }
    }
}

impl AffectiveEngine {
    /// Build a [`MoodModifier`] snapshot from the current VAD state.
    ///
    /// Formulas:
    /// - `urgency`:        see [`urgency_modifier`]
    /// - `tone`:           see [`tone_modifier`]
    /// - `patience`:       `(1.0 - arousal) / 2.0`, clamped \[0, 1\]
    /// - `empathy_boost`:  `(-valence).max(0) * 0.8`, clamped \[0, 1\]
    /// - `creativity_boost`: `gaussian(arousal, μ=0.3, σ=0.4) * valence_pos`
    pub fn get_mood_modifier(&self) -> MoodModifier {
        let v = self.state.mood.valence;
        let a = self.state.mood.arousal;

        let patience = ((1.0 - a) / 2.0).clamp(0.0, 1.0);
        let empathy_boost = ((-v).max(0.0) * 0.8).clamp(0.0, 1.0);

        // Creativity peaks at moderate arousal (~0.3) and positive valence.
        let arousal_gauss = (-(a - 0.3_f32).powi(2) / (2.0 * 0.4_f32.powi(2))).exp();
        let valence_pos = v.max(0.0);
        let creativity_boost = (arousal_gauss * valence_pos).clamp(0.0, 1.0);

        MoodModifier {
            urgency: self.urgency_modifier(),
            tone: self.tone_modifier(),
            patience,
            empathy_boost,
            creativity_boost,
        }
    }

    /// Compute response style modifiers based on current affective state.
    ///
    /// This drives adaptive response behavior:
    /// - High stress/arousal → shorter, more direct responses
    /// - Positive valence + moderate arousal → warmer, emoji-enhanced
    /// - Negative valence (fear/sadness) → more empathetic responses
    pub fn response_style_modifier(&self) -> ResponseStyleModifier {
        let v = self.state.mood.valence;
        let a = self.state.mood.arousal;
        let d = self.state.mood.dominance;
        let emotion = &self.state.emotion;

        let should_shorten = a > 0.6 || v < -0.3;
        let shorten_factor = if should_shorten {
            (1.0 - (a.max(0.0) * 0.3)).clamp(0.5, 1.0)
        } else {
            1.0
        };

        let should_add_emoji = v > 0.3 && a > 0.1 && a < 0.6;
        let emoji_boost = if should_add_emoji { v * 0.5 } else { 0.0 };

        let empathy_level =
            if v < -0.2 || matches!(emotion, EmotionLabel::Fear | EmotionLabel::Sadness) {
                ((-v).max(0.0) * 0.8).clamp(0.0, 1.0)
            } else {
                0.0
            };

        let warmth_level = (v.max(0.0) * 0.6 + (1.0 - d) * 0.2).clamp(0.0, 1.0);

        let directness_level = if a > 0.4 {
            (a * 0.5).clamp(0.0, 1.0)
        } else {
            0.2
        };

        ResponseStyleModifier {
            shorten_factor,
            emoji_boost,
            empathy_level,
            warmth_level,
            directness_level,
        }
    }

    /// Process a mood event with personality-influenced volatility.
    ///
    /// The OCEAN traits modulate how strongly the event affects mood:
    /// - High **Neuroticism** → larger mood swings (volatility × 1.0–1.5)
    /// - High **Agreeableness** → dampens criticism, amplifies compliments
    /// - Low **Extraversion** → slower arousal changes
    ///
    /// The `volatility_multiplier` is `0.5 + N * 1.0`, clamped \[0.5, 1.5\].
    pub fn process_event_with_personality(
        &mut self,
        event: MoodEvent,
        now_ms: u64,
        traits: &OceanTraits,
    ) {
        // 1. Cooldown check.
        if now_ms < self.state.cooldown_until_ms {
            tracing::warn!(
                cooldown_remaining_ms = self.state.cooldown_until_ms - now_ms,
                "mood update rejected — cooldown active (personality path)"
            );
            return;
        }

        let elapsed_ms = now_ms.saturating_sub(self.state.last_update_ms);

        // 2. Personality-influenced decay.
        if elapsed_ms > 0 && self.state.last_update_ms > 0 {
            let decay_mul = Self::personality_decay_rate(traits, self.state.mood.valence);
            let adjusted_half_life = DECAY_HALF_LIFE_MS / decay_mul;
            let decay = 0.5_f32.powf(elapsed_ms as f32 / adjusted_half_life);
            self.state.mood.valence *= decay;
            self.state.mood.arousal *= decay;
        }

        // 3. Compute base deltas.
        let (base_dv, base_da) = match event.deltas() {
            Some(d) => d,
            None => {
                self.state.last_update_ms = now_ms;
                return;
            }
        };

        // 4. Apply personality modifiers.
        let volatility = (0.5 + traits.neuroticism * 1.0).clamp(0.5, 1.5);

        // Agreeableness: dampens negative valence deltas, amplifies positive.
        let a_modifier = if base_dv < 0.0 {
            // Criticism: high A → less affected (multiply by 1.0 - (A-0.5)*0.4)
            (1.0 - (traits.agreeableness - 0.5) * 0.4).clamp(0.5, 1.5)
        } else {
            // Compliment: high A → more receptive (multiply by 1.0 + (A-0.5)*0.4)
            (1.0 + (traits.agreeableness - 0.5) * 0.4).clamp(0.5, 1.5)
        };

        // Extraversion: modulates arousal change rate.
        let e_arousal_mul = (0.7 + traits.extraversion * 0.6).clamp(0.7, 1.3);

        let dv = base_dv * volatility * a_modifier;
        let da = base_da * volatility * e_arousal_mul;

        // 5. Stability gate.
        let (dv, da) = if self.state.stability < STABILITY_GATE {
            (dv * 0.5, da * 0.5)
        } else {
            (dv, da)
        };

        // 6. EMA smoothing.
        let alpha = if self.state.last_update_ms == 0 {
            1.0
        } else if elapsed_ms > 0 {
            1.0 - (-(elapsed_ms as f32) / EMA_TAU_MS).exp()
        } else {
            1.0
        };

        let target_v = self.state.mood.valence + dv;
        let target_a = self.state.mood.arousal + da;

        let old_valence = self.state.mood.valence;
        let old_arousal = self.state.mood.arousal;

        self.state.mood.valence = (old_valence + alpha * (target_v - old_valence)).clamp(-1.0, 1.0);
        self.state.mood.arousal = (old_arousal + alpha * (target_a - old_arousal)).clamp(-1.0, 1.0);
        self.state.mood.dominance = self.state.mood.dominance.clamp(0.0, 1.0);

        // 7. Classify emotion.
        self.state.emotion = Self::classify_emotion(&self.state.mood);

        // 8. Major change detection → cooldown.
        let delta_magnitude = ((self.state.mood.valence - old_valence).powi(2)
            + (self.state.mood.arousal - old_arousal).powi(2))
        .sqrt();

        if delta_magnitude > MAJOR_CHANGE_THRESHOLD {
            self.state.cooldown_until_ms = now_ms + COOLDOWN_DURATION_MS;
            tracing::info!(
                delta = delta_magnitude,
                "major mood change (personality path) — cooldown set"
            );
        }

        // Update stability.
        if delta_magnitude > 0.1 {
            self.state.stability = (self.state.stability - 0.05).clamp(0.0, 1.0);
        } else {
            self.state.stability = (self.state.stability + 0.01).clamp(0.0, 1.0);
        }

        self.state.last_update_ms = now_ms;

        tracing::debug!(
            valence = self.state.mood.valence,
            arousal = self.state.mood.arousal,
            emotion = ?self.state.emotion,
            volatility,
            "personality-influenced mood update"
        );
    }

    /// Compute a personality-influenced decay rate multiplier.
    ///
    /// - High **Neuroticism** → faster decay (moods don't persist)
    /// - High **Extraversion** + positive valence → slower decay
    /// - High **Extraversion** + negative valence → faster decay
    ///
    /// Returns a multiplier > 0.0 applied to the base half-life denominator,
    /// so values > 1.0 mean *faster* decay and < 1.0 mean *slower* decay.
    pub fn personality_decay_rate(traits: &OceanTraits, current_valence: f32) -> f32 {
        let n_factor = 0.8 + traits.neuroticism * 0.4; // [0.8, 1.2]

        // Extraversion: slows positive-mood decay, speeds negative-mood decay.
        let sign = if current_valence >= 0.0 { -1.0 } else { 1.0 };
        let e_factor = 1.0 + sign * (traits.extraversion - 0.5) * 0.3; // ~[0.85, 1.15]

        (n_factor * e_factor).clamp(0.5, 2.0)
    }

    /// Produce a structured one-line log entry for the current mood state.
    ///
    /// Useful for tracing pipelines and audit logs.
    pub fn mood_log_entry(&self) -> String {
        format!(
            "mood[v={:.3} a={:.3} d={:.3}] emotion={:?} stability={:.2} cooldown_until={}",
            self.state.mood.valence,
            self.state.mood.arousal,
            self.state.mood.dominance,
            self.state.emotion,
            self.state.stability,
            self.state.cooldown_until_ms,
        )
    }
}

// ---------------------------------------------------------------------------
// Stress Accumulator
// ---------------------------------------------------------------------------

/// Stress half-life in milliseconds (10 minutes).
const STRESS_HALF_LIFE_MS: f32 = 600_000.0;

/// Stress warning threshold — triggers a tracing::warn! when exceeded.
const STRESS_WARNING_THRESHOLD: f32 = 0.7;

/// Sliding window for counting recent negative events (5 minutes).
const STRESS_WINDOW_MS: u64 = 300_000;

/// Tracks accumulated stress from negative events, modulated by
/// personality (Neuroticism).
///
/// Stress increases on negative mood events and decays over time with
/// a configurable half-life.  High-Neuroticism personalities accumulate
/// stress faster.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StressAccumulator {
    /// Current stress level in \[0.0, 1.0\].
    pub stress_level: f32,
    /// Count of negative events within the current sliding window.
    pub recent_negatives: u32,
    /// Start of the current sliding window (epoch ms).
    pub window_start_ms: u64,
    /// Peak stress level ever observed.
    pub peak_stress: f32,
    /// Timestamp of last stress update.
    last_update_ms: u64,
}

impl StressAccumulator {
    /// Create a new stress accumulator with zero stress.
    pub fn new() -> Self {
        Self {
            stress_level: 0.0,
            recent_negatives: 0,
            window_start_ms: 0,
            peak_stress: 0.0,
            last_update_ms: 0,
        }
    }

    /// Accumulate stress from a mood event.
    ///
    /// Only negative events increase stress.  The increment is scaled by
    /// `neuroticism`: `Δstress = 0.05 + neuroticism * 0.10`, so high-N
    /// personalities gain stress roughly twice as fast as low-N ones.
    pub fn accumulate(&mut self, event: &MoodEvent, now_ms: u64, neuroticism: f32) {
        // Reset sliding window if it has expired.
        if now_ms.saturating_sub(self.window_start_ms) > STRESS_WINDOW_MS {
            self.recent_negatives = 0;
            self.window_start_ms = now_ms;
        }

        // Decay first.
        self.apply_decay(now_ms);

        // Only negative events add stress.
        let is_negative = matches!(
            event,
            MoodEvent::TaskFailed
                | MoodEvent::UserFrustrated
                | MoodEvent::Criticism
                | MoodEvent::VoiceStressDetected { .. }
                | MoodEvent::VoiceFatigueDetected { .. }
        );

        if is_negative {
            let n = neuroticism.clamp(0.0, 1.0);
            let delta = 0.05 + n * 0.10;
            self.stress_level = (self.stress_level + delta).clamp(0.0, 1.0);
            self.recent_negatives += 1;

            if self.stress_level > self.peak_stress {
                self.peak_stress = self.stress_level;
            }

            if self.stress_level > STRESS_WARNING_THRESHOLD {
                tracing::warn!(
                    stress = self.stress_level,
                    recent_negatives = self.recent_negatives,
                    "stress level exceeds warning threshold"
                );
            }
        }

        self.last_update_ms = now_ms;
    }

    /// Apply time-based stress decay.
    ///
    /// Uses exponential decay with a 10-minute half-life.
    pub fn apply_decay(&mut self, now_ms: u64) {
        if self.last_update_ms == 0 || now_ms <= self.last_update_ms {
            return;
        }
        let elapsed = (now_ms - self.last_update_ms) as f32;
        let decay = 0.5_f32.powf(elapsed / STRESS_HALF_LIFE_MS);
        self.stress_level = (self.stress_level * decay).clamp(0.0, 1.0);
        self.last_update_ms = now_ms;
    }

    /// Return the current stress level.
    pub fn current_stress(&self) -> f32 {
        self.stress_level
    }
}

impl Default for StressAccumulator {
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
    fn test_initial_state_is_calm() {
        let engine = AffectiveEngine::new();
        let s = engine.current_state();
        assert_eq!(s.emotion, EmotionLabel::Calm);
        assert!((s.mood.valence).abs() < f32::EPSILON);
        assert!((s.stability - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn test_positive_event_increases_valence() {
        let mut engine = AffectiveEngine::new();
        engine.process_event(MoodEvent::UserHappy, 1_000);
        let s = engine.current_state();
        assert!(s.mood.valence > 0.0, "valence should be positive");
        assert!(s.mood.arousal > 0.0, "arousal should be positive");
    }

    #[test]
    fn test_decay_toward_neutral() {
        let mut engine = AffectiveEngine::new();
        engine.process_event(MoodEvent::UserHappy, 1_000);
        let v1 = engine.current_state().mood.valence;

        // Process a neutral-ish event after long time — decay should pull toward 0.
        engine.process_event(MoodEvent::TaskSucceeded, 600_000); // 10 min later
        let v2 = engine.current_state().mood.valence;

        // After decay + small positive event, valence should still be smaller
        // than v1 because 10 min elapsed with 5 min half-life → ~4 half-lives → heavy decay.
        // Actually 600s/300s = 2 half-lives → decay = 0.25
        // v1 * 0.25 + small event ~ still less than v1
        assert!(v2 < v1, "v2={} should be < v1={}", v2, v1);
    }

    #[test]
    fn test_cooldown_blocks_update() {
        let mut engine = AffectiveEngine::new();

        // Force a major mood change to trigger cooldown.
        // UserHappy gives +0.20 valence, +0.10 arousal.
        // magnitude = sqrt(0.20^2 + 0.10^2) = sqrt(0.05) ≈ 0.224 — not enough for 0.3.
        // Need multiple rapid events or direct state manipulation.
        // Let's set cooldown manually for the test.
        engine.state.cooldown_until_ms = 100_000;

        let valence_before = engine.state.mood.valence;
        engine.process_event(MoodEvent::UserHappy, 50_000);
        let valence_after = engine.state.mood.valence;

        assert!(
            (valence_before - valence_after).abs() < f32::EPSILON,
            "mood should not change during cooldown"
        );
    }

    #[test]
    fn test_stability_gate_halves_effect() {
        let mut engine = AffectiveEngine::new();
        engine.state.stability = 0.5; // Below gate threshold of 0.6

        engine.process_event(MoodEvent::UserHappy, 1_000);
        let v_low_stability = engine.current_state().mood.valence;

        let mut engine2 = AffectiveEngine::new();
        engine2.state.stability = 1.0; // Above gate threshold

        engine2.process_event(MoodEvent::UserHappy, 1_000);
        let v_high_stability = engine2.current_state().mood.valence;

        // Low stability should produce roughly half the effect.
        assert!(
            v_low_stability < v_high_stability,
            "low_stab={} >= high_stab={}",
            v_low_stability,
            v_high_stability
        );
    }

    #[test]
    fn test_emotion_classification() {
        // Joy
        assert_eq!(
            AffectiveEngine::classify_emotion(&MoodVAD {
                valence: 0.5,
                arousal: 0.3,
                dominance: 0.6
            }),
            EmotionLabel::Joy
        );

        // Sadness
        assert_eq!(
            AffectiveEngine::classify_emotion(&MoodVAD {
                valence: -0.4,
                arousal: -0.2,
                dominance: 0.3
            }),
            EmotionLabel::Sadness
        );

        // Anger
        assert_eq!(
            AffectiveEngine::classify_emotion(&MoodVAD {
                valence: -0.3,
                arousal: 0.5,
                dominance: 0.7
            }),
            EmotionLabel::Anger
        );

        // Fear
        assert_eq!(
            AffectiveEngine::classify_emotion(&MoodVAD {
                valence: -0.3,
                arousal: 0.5,
                dominance: 0.1
            }),
            EmotionLabel::Fear
        );

        // Calm
        assert_eq!(
            AffectiveEngine::classify_emotion(&MoodVAD {
                valence: 0.0,
                arousal: 0.0,
                dominance: 0.5
            }),
            EmotionLabel::Calm
        );

        // Curiosity
        assert_eq!(
            AffectiveEngine::classify_emotion(&MoodVAD {
                valence: 0.2,
                arousal: 0.3,
                dominance: 0.6
            }),
            EmotionLabel::Curiosity
        );
    }

    #[test]
    fn test_silence_under_threshold_no_effect() {
        let mut engine = AffectiveEngine::new();
        engine.process_event(
            MoodEvent::Silence {
                duration_ms: 100_000,
            },
            1_000,
        );
        let s = engine.current_state();
        // 100s < 300s, no effect
        assert!((s.mood.valence).abs() < f32::EPSILON);
    }

    #[test]
    fn test_silence_over_threshold_negative() {
        let mut engine = AffectiveEngine::new();
        engine.process_event(
            MoodEvent::Silence {
                duration_ms: 400_000,
            },
            1_000,
        );
        let s = engine.current_state();
        assert!(s.mood.valence < 0.0, "silence should lower valence");
        assert!(s.mood.arousal < 0.0, "silence should lower arousal");
    }

    #[test]
    fn test_mood_context_string_default() {
        let engine = AffectiveEngine::new();
        let ctx = engine.mood_context_string();
        assert!(
            ctx.contains("neutral"),
            "default mood should be neutral: {}",
            ctx
        );
        assert!(
            ctx.contains("Calm"),
            "default emotion should be Calm: {}",
            ctx
        );
    }

    #[test]
    fn test_mood_context_string_after_positive() {
        let mut engine = AffectiveEngine::new();
        engine.process_event(MoodEvent::UserHappy, 1_000);
        let ctx = engine.mood_context_string();
        // With positive valence, should say "positive" or similar
        assert!(!ctx.is_empty());
    }

    #[test]
    fn test_urgency_modifier_bounds() {
        let engine = AffectiveEngine::new();
        let u = engine.urgency_modifier();
        assert!(u >= 0.8 && u <= 1.2, "urgency_modifier={}", u);
    }

    #[test]
    fn test_urgency_modifier_high_arousal() {
        let mut engine = AffectiveEngine::new();
        // Force high arousal
        engine.state.mood.arousal = 0.8;
        let u = engine.urgency_modifier();
        assert!(u > 1.0, "high arousal should increase urgency, got {}", u);
    }

    #[test]
    fn test_tone_modifier_bounds() {
        let engine = AffectiveEngine::new();
        let t = engine.tone_modifier();
        assert!(t >= -1.0 && t <= 1.0, "tone_modifier={}", t);
    }

    // -----------------------------------------------------------------------
    // New tests for personality-influenced mood + stress + MoodModifier
    // -----------------------------------------------------------------------

    fn default_traits() -> OceanTraits {
        OceanTraits::DEFAULT
    }

    fn high_neuroticism_traits() -> OceanTraits {
        OceanTraits {
            openness: 0.5,
            conscientiousness: 0.5,
            extraversion: 0.5,
            agreeableness: 0.5,
            neuroticism: 0.9,
        }
    }

    fn low_neuroticism_traits() -> OceanTraits {
        OceanTraits {
            openness: 0.5,
            conscientiousness: 0.5,
            extraversion: 0.5,
            agreeableness: 0.5,
            neuroticism: 0.1,
        }
    }

    #[test]
    fn test_personality_event_high_neuroticism_more_volatile() {
        let mut engine_high_n = AffectiveEngine::new();
        engine_high_n.process_event_with_personality(
            MoodEvent::TaskFailed,
            1_000,
            &high_neuroticism_traits(),
        );
        let v_high = engine_high_n.current_state().mood.valence;

        let mut engine_low_n = AffectiveEngine::new();
        engine_low_n.process_event_with_personality(
            MoodEvent::TaskFailed,
            1_000,
            &low_neuroticism_traits(),
        );
        let v_low = engine_low_n.current_state().mood.valence;

        // High N should have MORE negative valence (more volatile).
        assert!(
            v_high < v_low,
            "high-N valence {} should be more negative than low-N {}",
            v_high,
            v_low
        );
    }

    #[test]
    fn test_personality_event_agreeableness_dampens_criticism() {
        let high_a = OceanTraits {
            agreeableness: 0.9,
            ..default_traits()
        };
        let low_a = OceanTraits {
            agreeableness: 0.1,
            ..default_traits()
        };

        let mut e1 = AffectiveEngine::new();
        e1.process_event_with_personality(MoodEvent::Criticism, 1_000, &high_a);
        let v_high_a = e1.current_state().mood.valence;

        let mut e2 = AffectiveEngine::new();
        e2.process_event_with_personality(MoodEvent::Criticism, 1_000, &low_a);
        let v_low_a = e2.current_state().mood.valence;

        // High A should be LESS affected by criticism (less negative).
        assert!(
            v_high_a > v_low_a,
            "high-A {} should be less negative than low-A {} on criticism",
            v_high_a,
            v_low_a
        );
    }

    #[test]
    fn test_personality_event_agreeableness_amplifies_compliment() {
        let high_a = OceanTraits {
            agreeableness: 0.9,
            ..default_traits()
        };
        let low_a = OceanTraits {
            agreeableness: 0.1,
            ..default_traits()
        };

        let mut e1 = AffectiveEngine::new();
        e1.process_event_with_personality(MoodEvent::Compliment, 1_000, &high_a);
        let v_high_a = e1.current_state().mood.valence;

        let mut e2 = AffectiveEngine::new();
        e2.process_event_with_personality(MoodEvent::Compliment, 1_000, &low_a);
        let v_low_a = e2.current_state().mood.valence;

        // High A should be MORE affected by compliments (more positive).
        assert!(
            v_high_a > v_low_a,
            "high-A {} should be more positive than low-A {} on compliment",
            v_high_a,
            v_low_a
        );
    }

    #[test]
    fn test_personality_decay_rate_high_neuroticism_faster() {
        let high_n = high_neuroticism_traits();
        let low_n = low_neuroticism_traits();
        let rate_high = AffectiveEngine::personality_decay_rate(&high_n, 0.5);
        let rate_low = AffectiveEngine::personality_decay_rate(&low_n, 0.5);
        // Higher N → higher decay rate multiplier → faster decay.
        assert!(
            rate_high > rate_low,
            "high-N rate {} should > low-N rate {}",
            rate_high,
            rate_low
        );
    }

    #[test]
    fn test_personality_decay_rate_positive() {
        let traits = default_traits();
        let rate = AffectiveEngine::personality_decay_rate(&traits, 0.5);
        assert!(rate > 0.0, "decay rate must be positive: {}", rate);
        assert!(rate <= 2.0, "decay rate clamped to 2.0: {}", rate);
    }

    #[test]
    fn test_mood_modifier_default_state() {
        let engine = AffectiveEngine::new();
        let m = engine.get_mood_modifier();
        // Default: valence=0, arousal=0, dominance=0.5
        assert!((m.urgency - 1.0).abs() < 0.01, "urgency={}", m.urgency);
        assert!((m.tone - 0.0).abs() < 0.01, "tone={}", m.tone);
        assert!(m.patience >= 0.4, "patience={}", m.patience);
        assert!(m.empathy_boost < 0.01, "empathy_boost={}", m.empathy_boost);
    }

    #[test]
    fn test_mood_modifier_after_negative_event() {
        let mut engine = AffectiveEngine::new();
        engine.process_event(MoodEvent::UserFrustrated, 1_000);
        let m = engine.get_mood_modifier();
        // Negative valence → tone < 0, empathy_boost > 0
        assert!(m.tone < 0.0, "tone should be negative: {}", m.tone);
        assert!(
            m.empathy_boost > 0.0,
            "empathy should increase: {}",
            m.empathy_boost
        );
    }

    #[test]
    fn test_mood_log_entry_not_empty() {
        let engine = AffectiveEngine::new();
        let log = engine.mood_log_entry();
        assert!(log.contains("mood[v="));
        assert!(log.contains("emotion="));
        assert!(log.contains("stability="));
    }

    // -- Stress Accumulator tests --

    #[test]
    fn test_stress_accumulator_initial() {
        let sa = StressAccumulator::new();
        assert!((sa.current_stress() - 0.0).abs() < f32::EPSILON);
        assert_eq!(sa.recent_negatives, 0);
        assert!((sa.peak_stress - 0.0).abs() < f32::EPSILON);
    }

    #[test]
    fn test_stress_increases_on_negative() {
        let mut sa = StressAccumulator::new();
        sa.accumulate(&MoodEvent::TaskFailed, 1_000, 0.5);
        assert!(
            sa.current_stress() > 0.0,
            "stress should increase: {}",
            sa.current_stress()
        );
        assert_eq!(sa.recent_negatives, 1);
    }

    #[test]
    fn test_stress_higher_with_high_neuroticism() {
        let mut sa_high = StressAccumulator::new();
        sa_high.accumulate(&MoodEvent::Criticism, 1_000, 0.9);
        let stress_high = sa_high.current_stress();

        let mut sa_low = StressAccumulator::new();
        sa_low.accumulate(&MoodEvent::Criticism, 1_000, 0.1);
        let stress_low = sa_low.current_stress();

        assert!(
            stress_high > stress_low,
            "high-N stress {} > low-N stress {}",
            stress_high,
            stress_low
        );
    }

    #[test]
    fn test_stress_no_increase_on_positive() {
        let mut sa = StressAccumulator::new();
        sa.accumulate(&MoodEvent::TaskSucceeded, 1_000, 0.5);
        assert!(
            (sa.current_stress() - 0.0).abs() < f32::EPSILON,
            "positive event should not increase stress"
        );
    }

    #[test]
    fn test_stress_decay_over_time() {
        let mut sa = StressAccumulator::new();
        sa.accumulate(&MoodEvent::TaskFailed, 1_000, 0.5);
        let before = sa.current_stress();

        // 10 minutes later (one half-life)
        sa.apply_decay(601_000);
        let after = sa.current_stress();

        assert!(
            after < before,
            "stress should decay: before={} after={}",
            before,
            after
        );
        // Should be roughly half after one half-life.
        assert!(
            (after - before * 0.5).abs() < 0.01,
            "after={}, expected ~{}",
            after,
            before * 0.5
        );
    }

    #[test]
    fn test_stress_peak_tracked() {
        let mut sa = StressAccumulator::new();
        sa.accumulate(&MoodEvent::TaskFailed, 1_000, 0.9);
        sa.accumulate(&MoodEvent::Criticism, 2_000, 0.9);
        let peak = sa.peak_stress;
        assert!(peak > 0.0, "peak should be tracked");

        // Decay stress.
        sa.apply_decay(700_000);
        // Peak should remain the same even after decay.
        assert!(
            (sa.peak_stress - peak).abs() < f32::EPSILON,
            "peak should not decrease with decay"
        );
    }

    #[test]
    fn test_stress_clamped_to_one() {
        let mut sa = StressAccumulator::new();
        // Pile on many negative events with max neuroticism.
        for i in 0..50 {
            sa.accumulate(&MoodEvent::TaskFailed, 1_000 + i * 100, 1.0);
        }
        assert!(
            sa.current_stress() <= 1.0,
            "stress should be clamped: {}",
            sa.current_stress()
        );
    }

    // -----------------------------------------------------------------------
    // Additional integration tests for Team 2 wiring
    // -----------------------------------------------------------------------

    #[test]
    fn test_urgency_modifier_stress_response() {
        let mut engine = AffectiveEngine::new();
        // High arousal should increase urgency
        engine.state.mood.arousal = 0.8;
        let u = engine.urgency_modifier();
        assert!(u > 1.0, "high arousal should give urgency > 1.0, got {}", u);
    }

    #[test]
    fn test_tone_modifier_stress_response() {
        let mut engine = AffectiveEngine::new();
        // Positive valence should give positive tone
        engine.state.mood.valence = 0.5;
        let t = engine.tone_modifier();
        assert!(
            t > 0.0,
            "positive valence should give positive tone, got {}",
            t
        );

        // Negative valence should give negative tone
        engine.state.mood.valence = -0.5;
        let t2 = engine.tone_modifier();
        assert!(
            t2 < 0.0,
            "negative valence should give negative tone, got {}",
            t2
        );
    }

    #[test]
    fn test_mood_modifier_comprehensive() {
        let mut engine = AffectiveEngine::new();
        // Set specific mood state
        engine.state.mood.valence = 0.3;
        engine.state.mood.arousal = 0.4;

        let m = engine.get_mood_modifier();

        // Verify all fields are populated
        assert!(m.urgency >= 0.8 && m.urgency <= 1.2);
        assert!(m.tone >= -1.0 && m.tone <= 1.0);
        assert!(m.patience >= 0.0 && m.patience <= 1.0);
        assert!(m.empathy_boost >= 0.0 && m.empathy_boost <= 1.0);
        assert!(m.creativity_boost >= 0.0 && m.creativity_boost <= 1.0);
    }

    #[test]
    fn test_mood_modifier_negative_valence_empathy() {
        let mut engine = AffectiveEngine::new();
        engine.state.mood.valence = -0.5;

        let m = engine.get_mood_modifier();

        // Negative valence should boost empathy
        assert!(
            m.empathy_boost > 0.0,
            "negative valence should boost empathy"
        );
    }

    #[test]
    fn test_process_event_with_personality_full_integration() {
        let mut engine = AffectiveEngine::new();

        let traits = OceanTraits {
            openness: 0.7,
            conscientiousness: 0.6,
            extraversion: 0.8,
            agreeableness: 0.9,
            neuroticism: 0.3,
        };

        // Process multiple events
        engine.process_event_with_personality(MoodEvent::UserHappy, 1_000, &traits);
        engine.process_event_with_personality(MoodEvent::Compliment, 2_000, &traits);

        let state = engine.current_state();

        // After positive events with high agreeableness, valence should be positive
        assert!(
            state.mood.valence > 0.0,
            "valence should be positive after happy events"
        );
    }

    #[test]
    fn test_cooldown_prevents_rapid_updates() {
        let mut engine = AffectiveEngine::new();

        // First event
        engine.process_event(MoodEvent::UserHappy, 1_000);

        // Try immediate second event - should be blocked by cooldown
        // Force a major change to trigger cooldown
        engine.state.cooldown_until_ms = 100_000; // Set cooldown

        let valence_before = engine.state.mood.valence;
        engine.process_event(MoodEvent::UserHappy, 50_000);
        let valence_after = engine.state.mood.valence;

        assert!(
            (valence_before - valence_after).abs() < f32::EPSILON,
            "mood should not change during cooldown"
        );
    }

    #[test]
    fn test_mood_context_string_comprehensive() {
        let mut engine = AffectiveEngine::new();

        // Test with positive mood
        engine.state.mood.valence = 0.5;
        engine.state.mood.arousal = 0.5;
        engine.state.mood.dominance = 0.7;
        engine.state.emotion = EmotionLabel::Joy;

        let ctx = engine.mood_context_string();

        assert!(ctx.contains("positive") || ctx.contains("Joy"));
    }

    #[test]
    fn test_personality_decay_rate_integration() {
        let traits_neuro = OceanTraits {
            neuroticism: 0.9,
            ..OceanTraits::DEFAULT
        };
        let traits_stable = OceanTraits {
            neuroticism: 0.1,
            ..OceanTraits::DEFAULT
        };

        let rate_neuro = AffectiveEngine::personality_decay_rate(&traits_neuro, 0.5);
        let rate_stable = AffectiveEngine::personality_decay_rate(&traits_stable, 0.5);

        // High neuroticism should have faster decay
        assert!(
            rate_neuro > rate_stable,
            "high N should decay faster: {} vs {}",
            rate_neuro,
            rate_stable
        );
    }

    #[test]
    fn test_multiple_mood_events_sequence() {
        let mut engine = AffectiveEngine::new();

        // Simulate a sequence of interactions
        engine.process_event(MoodEvent::UserHappy, 1_000);
        let v1 = engine.current_state().mood.valence;

        engine.process_event(MoodEvent::Compliment, 2_000);
        let v2 = engine.current_state().mood.valence;

        engine.process_event(MoodEvent::TaskSucceeded, 3_000);
        let v3 = engine.current_state().mood.valence;

        // Each positive event should increase or maintain valence
        assert!(v2 >= v1 - 0.01, "v2 {} should be >= v1 {}", v2, v1);
        assert!(v3 >= v2 - 0.01, "v3 {} should be >= v2 {}", v3, v2);
    }

    #[test]
    fn test_negative_event_sequence() {
        let mut engine = AffectiveEngine::new();

        engine.process_event(MoodEvent::TaskFailed, 1_000);
        let v1 = engine.current_state().mood.valence;

        engine.process_event(MoodEvent::Criticism, 2_000);
        let v2 = engine.current_state().mood.valence;

        // Negative events should decrease valence
        assert!(
            v1 < 0.0 || v1 < 0.1,
            "v1 {} should be low after failure",
            v1
        );
        assert!(v2 <= v1 + 0.01, "v2 {} should be <= v1 {}", v2, v1);
    }

    #[test]
    fn test_stress_accumulator_multiple_negative_events() {
        let mut sa = StressAccumulator::new();

        // Multiple negative events should accumulate stress
        sa.accumulate(&MoodEvent::TaskFailed, 1_000, 0.5);
        let s1 = sa.current_stress();

        sa.accumulate(&MoodEvent::Criticism, 2_000, 0.5);
        let s2 = sa.current_stress();

        sa.accumulate(&MoodEvent::UserFrustrated, 3_000, 0.5);
        let s3 = sa.current_stress();

        assert!(s2 > s1, "stress should increase: {} > {}", s2, s1);
        assert!(s3 > s2, "stress should increase: {} > {}", s3, s2);
    }

    #[test]
    fn test_stress_sliding_window() {
        let mut sa = StressAccumulator::new();

        // Events within 5-minute window should count
        sa.accumulate(&MoodEvent::TaskFailed, 1_000, 0.5);
        sa.accumulate(&MoodEvent::Criticism, 2_000, 0.5);

        assert_eq!(sa.recent_negatives, 2, "should have 2 recent negatives");

        // After window expires, count should reset
        sa.accumulate(&MoodEvent::TaskFailed, 400_000, 0.5); // ~6.5 min later

        // Window should have reset
        assert!(
            sa.recent_negatives <= 1,
            "recent count should reset after window"
        );
    }

    // -----------------------------------------------------------------------
    // ResponseStyleModifier tests (Team 2 wiring)
    // -----------------------------------------------------------------------

    #[test]
    fn test_response_style_default() {
        let engine = AffectiveEngine::new();
        let rs = engine.response_style_modifier();

        assert!(
            (rs.shorten_factor - 1.0).abs() < 0.01,
            "default should not shorten"
        );
        assert!(
            (rs.emoji_boost).abs() < 0.01,
            "default should have no emoji boost"
        );
        assert!(
            (rs.empathy_level).abs() < 0.01,
            "default should have no empathy"
        );
    }

    #[test]
    fn test_response_style_shorten_high_arousal() {
        let mut engine = AffectiveEngine::new();
        engine.state.mood.arousal = 0.8;
        engine.state.mood.valence = 0.2;

        let rs = engine.response_style_modifier();

        assert!(
            rs.shorten_factor < 1.0,
            "high arousal should shorten: {}",
            rs.shorten_factor
        );
    }

    #[test]
    fn test_response_style_shorten_negative_valence() {
        let mut engine = AffectiveEngine::new();
        engine.state.mood.arousal = 0.2;
        engine.state.mood.valence = -0.5;

        let rs = engine.response_style_modifier();

        assert!(
            rs.shorten_factor < 1.0,
            "negative valence should shorten: {}",
            rs.shorten_factor
        );
    }

    #[test]
    fn test_response_style_emoji_boost_positive_mood() {
        let mut engine = AffectiveEngine::new();
        engine.state.mood.valence = 0.5;
        engine.state.mood.arousal = 0.3;

        let rs = engine.response_style_modifier();

        assert!(
            rs.emoji_boost > 0.0,
            "positive mood should add emojis: {}",
            rs.emoji_boost
        );
    }

    #[test]
    fn test_response_style_emoji_no_boost_low_arousal() {
        let mut engine = AffectiveEngine::new();
        engine.state.mood.valence = 0.5;
        engine.state.mood.arousal = 0.05; // Very low arousal

        let rs = engine.response_style_modifier();

        assert!(
            rs.emoji_boost < 0.1,
            "low arousal should not add emojis: {}",
            rs.emoji_boost
        );
    }

    #[test]
    fn test_response_style_empathy_fear() {
        let mut engine = AffectiveEngine::new();
        engine.state.mood.valence = -0.4;
        engine.state.mood.arousal = 0.5;
        engine.state.mood.dominance = 0.1;
        engine.state.emotion = EmotionLabel::Fear;

        let rs = engine.response_style_modifier();

        assert!(
            rs.empathy_level > 0.3,
            "fear should trigger empathy: {}",
            rs.empathy_level
        );
    }

    #[test]
    fn test_response_style_empathy_sadness() {
        let mut engine = AffectiveEngine::new();
        engine.state.mood.valence = -0.5;
        engine.state.mood.arousal = -0.2;
        engine.state.mood.dominance = 0.3;
        engine.state.emotion = EmotionLabel::Sadness;

        let rs = engine.response_style_modifier();

        assert!(
            rs.empathy_level > 0.3,
            "sadness should trigger empathy: {}",
            rs.empathy_level
        );
    }

    #[test]
    fn test_response_style_warmth_joy() {
        let mut engine = AffectiveEngine::new();
        engine.state.mood.valence = 0.6;
        engine.state.mood.arousal = 0.4;
        engine.state.mood.dominance = 0.6;
        engine.state.emotion = EmotionLabel::Joy;

        let rs = engine.response_style_modifier();

        // warmth = v*0.6 + (1-d)*0.2 = 0.6*0.6 + 0.4*0.2 = 0.36 + 0.08 = 0.44
        assert!(
            rs.warmth_level > 0.3,
            "joy should have some warmth: {}",
            rs.warmth_level
        );
    }

    #[test]
    fn test_response_style_directness_high_arousal() {
        let mut engine = AffectiveEngine::new();
        engine.state.mood.arousal = 0.7;
        engine.state.mood.valence = 0.0;

        let rs = engine.response_style_modifier();

        assert!(
            rs.directness_level > 0.3,
            "high arousal should increase directness: {}",
            rs.directness_level
        );
    }

    #[test]
    fn test_response_style_all_stressed() {
        let mut engine = AffectiveEngine::new();
        engine.state.mood.valence = -0.4;
        engine.state.mood.arousal = 0.8;
        engine.state.mood.dominance = 0.5;
        engine.state.emotion = EmotionLabel::Anger;

        let rs = engine.response_style_modifier();

        // Stressed: shorten, no emojis, high empathy
        assert!(
            rs.shorten_factor < 0.8,
            "stressed should shorten significantly"
        );
        assert!(rs.emoji_boost < 0.1, "stressed should have no emojis");
        assert!(rs.empathy_level > 0.3, "stressed should have empathy");
    }

    #[test]
    fn test_response_style_happy_engaged() {
        let mut engine = AffectiveEngine::new();
        engine.state.mood.valence = 0.5;
        engine.state.mood.arousal = 0.4;
        engine.state.mood.dominance = 0.6;
        engine.state.emotion = EmotionLabel::Joy;

        let rs = engine.response_style_modifier();

        // Happy + engaged: emojis, warmth, not shortened
        // warmth = v*0.6 + (1-d)*0.2 = 0.5*0.6 + 0.4*0.2 = 0.30 + 0.08 = 0.38
        assert!(rs.shorten_factor > 0.9, "happy should not shorten");
        assert!(rs.emoji_boost > 0.1, "happy should have emojis");
        assert!(
            rs.warmth_level > 0.3,
            "happy should have some warmth: {}",
            rs.warmth_level
        );
    }
}
