//! Voice biomarker extraction for emotional signal detection.
//!
//! Extracts: fundamental frequency (F0), jitter, shimmer, speech rate,
//! energy, and pause ratio from raw audio. These feed into AURA's Amygdala
//! module for real-time emotion detection.

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

#[derive(Debug, thiserror::Error)]
pub enum BiomarkerError {
    #[error("insufficient audio: need at least {min_samples} samples, got {got}")]
    InsufficientAudio { min_samples: usize, got: usize },
    #[error("invalid sample rate: {0}")]
    InvalidSampleRate(u32),
}

pub type BiomarkerResult<T> = Result<T, BiomarkerError>;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Minimum audio length for reliable biomarker extraction (100ms at 16kHz).
const MIN_SAMPLES: usize = 1_600;

/// Human voice F0 range.
const F0_MIN_HZ: f32 = 50.0;
const F0_MAX_HZ: f32 = 500.0;

/// Default sample rate.
const DEFAULT_SAMPLE_RATE: u32 = 16_000;

// ---------------------------------------------------------------------------
// Biomarker data
// ---------------------------------------------------------------------------

/// Extracted voice biomarkers from a speech segment.
#[derive(Debug, Clone, Default)]
pub struct VoiceBiomarkers {
    /// Fundamental frequency (pitch) in Hz. 0 if unvoiced.
    pub fundamental_freq_hz: f32,
    /// Jitter: pitch instability (%) — stress/anxiety indicator.
    pub jitter_percent: f32,
    /// Shimmer: amplitude instability (%) — fatigue indicator.
    pub shimmer_percent: f32,
    /// Estimated speech rate in words per minute.
    pub speech_rate_wpm: f32,
    /// RMS energy in dB.
    pub energy_db: f32,
    /// Ratio of silence to total duration [0.0, 1.0].
    pub pause_ratio: f32,
}

/// Emotional signal derived from biomarkers for the Amygdala.
#[derive(Debug, Clone, Default)]
pub struct EmotionalSignal {
    /// Arousal (low=calm, high=excited/stressed) [0.0, 1.0].
    pub arousal: f32,
    /// Valence (low=negative, high=positive) [0.0, 1.0].
    pub valence: f32,
    /// Stress level [0.0, 1.0].
    pub stress: f32,
    /// Fatigue level [0.0, 1.0].
    pub fatigue: f32,
    /// Confidence in this emotional reading [0.0, 1.0].
    pub confidence: f32,
}

// ---------------------------------------------------------------------------
// BiomarkerExtractor
// ---------------------------------------------------------------------------

pub struct BiomarkerExtractor {
    sample_rate: u32,
}

impl BiomarkerExtractor {
    pub fn new(sample_rate: u32) -> BiomarkerResult<Self> {
        if sample_rate == 0 || sample_rate > 48_000 {
            return Err(BiomarkerError::InvalidSampleRate(sample_rate));
        }
        Ok(Self { sample_rate })
    }

    pub fn with_default_rate() -> Self {
        Self {
            sample_rate: DEFAULT_SAMPLE_RATE,
        }
    }

    /// Extract all biomarkers from a PCM audio segment.
    pub fn extract(&self, samples: &[i16]) -> BiomarkerResult<VoiceBiomarkers> {
        if samples.len() < MIN_SAMPLES {
            return Err(BiomarkerError::InsufficientAudio {
                min_samples: MIN_SAMPLES,
                got: samples.len(),
            });
        }

        let f0 = self.extract_f0(samples);
        let jitter = self.extract_jitter(samples);
        let shimmer = self.extract_shimmer(samples);
        let energy_db = self.compute_energy_db(samples);
        let pause_ratio = self.compute_pause_ratio(samples);

        // Estimate speech rate: very rough heuristic based on energy envelope
        // zero-crossings correlate loosely with syllable rate.
        let speech_rate_wpm = self.estimate_speech_rate(samples);

        Ok(VoiceBiomarkers {
            fundamental_freq_hz: f0,
            jitter_percent: jitter,
            shimmer_percent: shimmer,
            speech_rate_wpm,
            energy_db,
            pause_ratio,
        })
    }

    /// Extract fundamental frequency (F0) via autocorrelation.
    ///
    /// Autocorrelation finds the period of the strongest repeating pattern
    /// in the signal, which corresponds to the vocal cord vibration rate.
    fn extract_f0(&self, samples: &[i16]) -> f32 {
        let float_samples: Vec<f32> = samples.iter().map(|&s| s as f32).collect();

        // Lag range corresponding to F0_MIN_HZ..F0_MAX_HZ
        let min_lag = (self.sample_rate as f32 / F0_MAX_HZ) as usize;
        let max_lag = (self.sample_rate as f32 / F0_MIN_HZ) as usize;
        let max_lag = max_lag.min(float_samples.len() / 2);

        if min_lag >= max_lag || max_lag >= float_samples.len() {
            return 0.0;
        }

        // Compute normalized autocorrelation
        let mut best_lag = 0;
        let mut best_corr = f32::NEG_INFINITY;
        let window = &float_samples[..max_lag * 2];

        // Energy of the first window for normalization
        let energy: f32 = window[..max_lag].iter().map(|&x| x * x).sum();
        if energy < 1e-6 {
            return 0.0; // silence
        }

        for lag in min_lag..max_lag {
            let mut corr = 0.0f32;
            for i in 0..max_lag {
                if i + lag < window.len() {
                    corr += window[i] * window[i + lag];
                }
            }
            let normalized = corr / energy;
            if normalized > best_corr {
                best_corr = normalized;
                best_lag = lag;
            }
        }

        if best_lag == 0 || best_corr < 0.3 {
            return 0.0; // unvoiced
        }

        // Octave-error correction: prefer the shortest lag (highest pitch)
        // whose correlation is within 5% of the best.  Autocorrelation of
        // periodic signals peaks at every multiple of the fundamental
        // period, so without this bias we'd pick a sub-harmonic.
        let threshold = best_corr * 0.95;
        for lag in min_lag..best_lag {
            let mut corr = 0.0f32;
            for i in 0..max_lag {
                if i + lag < window.len() {
                    corr += window[i] * window[i + lag];
                }
            }
            let normalized = corr / energy;
            if normalized >= threshold {
                return self.sample_rate as f32 / lag as f32;
            }
        }

        self.sample_rate as f32 / best_lag as f32
    }

    /// Extract jitter: variation in F0 period between consecutive cycles.
    ///
    /// High jitter (>1.0%) indicates vocal stress or pathology.
    fn extract_jitter(&self, samples: &[i16]) -> f32 {
        let periods = self.find_pitch_periods(samples);
        if periods.len() < 3 {
            return 0.0;
        }

        // Jitter (local) = mean |T_i - T_{i+1}| / mean(T)
        let mean_period: f32 = periods.iter().sum::<f32>() / periods.len() as f32;
        if mean_period < 1e-6 {
            return 0.0;
        }

        let jitter_sum: f32 = periods.windows(2).map(|w| (w[0] - w[1]).abs()).sum();
        let jitter_local = jitter_sum / (periods.len() - 1) as f32;

        (jitter_local / mean_period) * 100.0
    }

    /// Extract shimmer: variation in amplitude between consecutive cycles.
    ///
    /// High shimmer (>3.0%) indicates vocal fatigue or breathiness.
    fn extract_shimmer(&self, samples: &[i16]) -> f32 {
        let amplitudes = self.find_cycle_amplitudes(samples);
        if amplitudes.len() < 3 {
            return 0.0;
        }

        let mean_amp: f32 = amplitudes.iter().sum::<f32>() / amplitudes.len() as f32;
        if mean_amp < 1e-6 {
            return 0.0;
        }

        let shimmer_sum: f32 = amplitudes.windows(2).map(|w| (w[0] - w[1]).abs()).sum();
        let shimmer_local = shimmer_sum / (amplitudes.len() - 1) as f32;

        (shimmer_local / mean_amp) * 100.0
    }

    /// Compute RMS energy in dB.
    fn compute_energy_db(&self, samples: &[i16]) -> f32 {
        if samples.is_empty() {
            return -100.0;
        }
        let sum_sq: f64 = samples.iter().map(|&s| (s as f64) * (s as f64)).sum();
        let rms = (sum_sq / samples.len() as f64).sqrt();
        if rms < 1.0 {
            return -100.0;
        }
        20.0 * (rms as f32).log10()
    }

    /// Compute ratio of silent frames to total frames.
    fn compute_pause_ratio(&self, samples: &[i16]) -> f32 {
        let frame_size = (self.sample_rate as usize) / 100; // 10ms frames
        if frame_size == 0 {
            return 0.0;
        }

        let silence_threshold = 500i16; // ~-36 dB
        let total_frames = samples.len() / frame_size;
        if total_frames == 0 {
            return 0.0;
        }

        let silent_frames = samples
            .chunks(frame_size)
            .filter(|chunk| {
                let max_abs = chunk.iter().map(|&s| s.unsigned_abs()).max().unwrap_or(0);
                max_abs < silence_threshold as u16
            })
            .count();

        silent_frames as f32 / total_frames as f32
    }

    /// Rough speech rate estimation from energy envelope zero-crossing rate.
    fn estimate_speech_rate(&self, samples: &[i16]) -> f32 {
        // Compute energy envelope (10ms windows)
        let frame_size = self.sample_rate as usize / 100;
        if frame_size == 0 {
            return 0.0;
        }

        let envelope: Vec<f32> = samples
            .chunks(frame_size)
            .map(|chunk| {
                let sum: f32 = chunk.iter().map(|&s| (s as f32).abs()).sum();
                sum / chunk.len() as f32
            })
            .collect();

        if envelope.len() < 2 {
            return 0.0;
        }

        // Count transitions from below to above median (≈ syllable onsets)
        let median = {
            let mut sorted = envelope.clone();
            sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            sorted[sorted.len() / 2]
        };

        let transitions = envelope
            .windows(2)
            .filter(|w| w[0] < median && w[1] >= median)
            .count();

        // Duration in seconds
        let duration_s = samples.len() as f32 / self.sample_rate as f32;
        if duration_s < 0.1 {
            return 0.0;
        }

        // Syllables per second → ~3 syllables per word average
        let syllables_per_sec = transitions as f32 / duration_s;
        let words_per_min = (syllables_per_sec / 3.0) * 60.0;

        words_per_min.clamp(0.0, 300.0)
    }

    // -- Helpers ---------------------------------------------------------

    /// Find pitch periods (in samples) using zero-crossing detection.
    fn find_pitch_periods(&self, samples: &[i16]) -> Vec<f32> {
        let mut periods = Vec::new();
        let mut last_crossing = None;

        for i in 1..samples.len() {
            // Positive zero crossing
            if samples[i - 1] <= 0 && samples[i] > 0 {
                if let Some(last) = last_crossing {
                    let period = (i - last) as f32;
                    // Filter to human voice range
                    let freq = self.sample_rate as f32 / period;
                    if (F0_MIN_HZ..=F0_MAX_HZ).contains(&freq) {
                        periods.push(period);
                    }
                }
                last_crossing = Some(i);
            }
        }

        periods
    }

    /// Find peak amplitude per pitch cycle.
    fn find_cycle_amplitudes(&self, samples: &[i16]) -> Vec<f32> {
        let mut amplitudes = Vec::new();
        let mut last_crossing = 0usize;

        for i in 1..samples.len() {
            if samples[i - 1] <= 0 && samples[i] > 0 {
                if last_crossing > 0 && i > last_crossing {
                    let cycle = &samples[last_crossing..i];
                    let peak = cycle
                        .iter()
                        .map(|&s| s.unsigned_abs() as f32)
                        .fold(0.0f32, f32::max);
                    if peak > 0.0 {
                        amplitudes.push(peak);
                    }
                }
                last_crossing = i;
            }
        }

        amplitudes
    }
}

// ---------------------------------------------------------------------------
// Emotional signal mapping
// ---------------------------------------------------------------------------

impl VoiceBiomarkers {
    /// Map biomarkers to an emotional signal for the Amygdala.
    pub fn to_emotional_signal(&self) -> EmotionalSignal {
        // Arousal: driven by F0 height, energy, and speech rate
        let f0_norm = if self.fundamental_freq_hz > 0.0 {
            ((self.fundamental_freq_hz - 100.0) / 200.0).clamp(0.0, 1.0)
        } else {
            0.5
        };
        let energy_norm = ((self.energy_db + 40.0) / 60.0).clamp(0.0, 1.0);
        let rate_norm = (self.speech_rate_wpm / 200.0).clamp(0.0, 1.0);
        let arousal = (f0_norm * 0.4 + energy_norm * 0.3 + rate_norm * 0.3).clamp(0.0, 1.0);

        // Valence: harder to determine from acoustics alone.
        // Higher pitch + higher energy → more positive (rough heuristic).
        // High jitter/shimmer → negative.
        let instability = ((self.jitter_percent / 2.0) + (self.shimmer_percent / 5.0)).min(1.0);
        let valence = (0.5 + f0_norm * 0.2 + energy_norm * 0.1 - instability * 0.3).clamp(0.0, 1.0);

        // Stress: jitter + speech rate + high F0
        let stress = (self.jitter_percent / 3.0 + rate_norm * 0.3 + f0_norm * 0.2).clamp(0.0, 1.0);

        // Fatigue: shimmer + low energy + slow speech + high pause ratio
        let fatigue = (self.shimmer_percent / 5.0
            + (1.0 - energy_norm) * 0.3
            + (1.0 - rate_norm) * 0.2
            + self.pause_ratio * 0.2)
            .clamp(0.0, 1.0);

        // Confidence: based on F0 detection + sufficient energy
        let has_f0 = if self.fundamental_freq_hz > 0.0 {
            0.5
        } else {
            0.0
        };
        let has_energy = if self.energy_db > -30.0 { 0.3 } else { 0.0 };
        let has_speech = if self.pause_ratio < 0.8 { 0.2 } else { 0.0 };
        let confidence = has_f0 + has_energy + has_speech;

        EmotionalSignal {
            arousal,
            valence,
            stress,
            fatigue,
            confidence,
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Generate a sine wave at a given frequency.
    fn sine_wave(freq_hz: f32, sample_rate: u32, duration_ms: u32) -> Vec<i16> {
        let num_samples = (sample_rate as usize * duration_ms as usize) / 1000;
        (0..num_samples)
            .map(|i| {
                let t = i as f32 / sample_rate as f32;
                (10_000.0 * (2.0 * std::f32::consts::PI * freq_hz * t).sin()) as i16
            })
            .collect()
    }

    #[test]
    fn f0_extraction_sine() {
        let extractor = BiomarkerExtractor::with_default_rate();
        // 200 Hz sine wave, 500ms
        let samples = sine_wave(200.0, 16_000, 500);
        let bio = extractor.extract(&samples).unwrap();
        // F0 should be close to 200 Hz (allow 20% tolerance for autocorrelation)
        assert!(
            (bio.fundamental_freq_hz - 200.0).abs() < 40.0,
            "F0 = {}, expected ~200",
            bio.fundamental_freq_hz
        );
    }

    #[test]
    fn silence_biomarkers() {
        let extractor = BiomarkerExtractor::with_default_rate();
        let samples = vec![0i16; 16_000]; // 1 second of silence
        let bio = extractor.extract(&samples).unwrap();
        assert_eq!(bio.fundamental_freq_hz, 0.0);
        assert!(bio.energy_db < -50.0);
        assert!((bio.pause_ratio - 1.0).abs() < 0.1); // all silence
    }

    #[test]
    fn insufficient_audio_rejected() {
        let extractor = BiomarkerExtractor::with_default_rate();
        let short = vec![0i16; 100];
        assert!(extractor.extract(&short).is_err());
    }

    #[test]
    fn emotional_signal_high_arousal() {
        let bio = VoiceBiomarkers {
            fundamental_freq_hz: 300.0, // high pitch
            jitter_percent: 0.5,
            shimmer_percent: 1.0,
            speech_rate_wpm: 180.0, // fast
            energy_db: 20.0,        // loud
            pause_ratio: 0.1,       // few pauses
        };
        let emo = bio.to_emotional_signal();
        assert!(emo.arousal > 0.6, "arousal = {}", emo.arousal);
        assert!(emo.confidence > 0.5);
    }

    #[test]
    fn emotional_signal_fatigued() {
        let bio = VoiceBiomarkers {
            fundamental_freq_hz: 120.0,
            jitter_percent: 0.3,
            shimmer_percent: 4.0,  // high shimmer
            speech_rate_wpm: 60.0, // slow
            energy_db: -20.0,      // quiet
            pause_ratio: 0.6,      // many pauses
        };
        let emo = bio.to_emotional_signal();
        assert!(emo.fatigue > 0.4, "fatigue = {}", emo.fatigue);
    }

    #[test]
    fn energy_db_calculation() {
        let extractor = BiomarkerExtractor::with_default_rate();
        // Full-scale sine: peak 32767 → RMS ≈ 23170 → ~87 dB
        let samples = sine_wave(440.0, 16_000, 200);
        let bio = extractor.extract(&samples).unwrap();
        assert!(
            bio.energy_db > 60.0 && bio.energy_db < 100.0,
            "energy = {} dB",
            bio.energy_db
        );
    }
}
