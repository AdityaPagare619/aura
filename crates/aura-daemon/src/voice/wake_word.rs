//! Wake word detection ("Hey AURA") via sherpa-onnx keyword spotting.
//!
//! Uses sherpa-onnx's keyword spotter (KWS) for always-on, low-power wake word
//! detection. On non-Android platforms, provides a mock that never triggers
//! (or can be forced for testing).

use std::time::{Duration, Instant};

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

#[derive(Debug, thiserror::Error)]
pub enum WakeWordError {
    #[error("KWS model load failed: {0}")]
    ModelLoadFailed(String),
    #[error("KWS processing error: {0}")]
    ProcessingError(String),
}

pub type WakeWordResult<T> = Result<T, WakeWordError>;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Default sensitivity (0.0 = strict, 1.0 = loose).
pub const DEFAULT_SENSITIVITY: f32 = 0.5;

/// Cooldown between consecutive detections to prevent double-triggers.
pub const DEFAULT_COOLDOWN_MS: u32 = 2_000;

/// Default keywords.
pub const DEFAULT_KEYWORDS: &[&str] = &["hey aura", "aura", "okay aura"];

// ---------------------------------------------------------------------------
// sherpa-onnx KWS FFI (Android only)
// ---------------------------------------------------------------------------

#[cfg(target_os = "android")]
mod sherpa_kws_ffi {
    use std::os::raw::{c_char, c_float, c_int, c_void};

    extern "C" {
        pub fn sherpa_kws_create(
            model_path: *const c_char,
            keywords: *const *const c_char,
            num_keywords: c_int,
            sensitivity: c_float,
        ) -> *mut c_void;

        pub fn sherpa_kws_destroy(state: *mut c_void);

        /// Feed audio samples. Returns keyword index if detected, -1 otherwise.
        pub fn sherpa_kws_process(
            state: *mut c_void,
            samples: *const c_float,
            num_samples: c_int,
        ) -> c_int;

        pub fn sherpa_kws_reset(state: *mut c_void);
    }
}

// ---------------------------------------------------------------------------
// Wake word event
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct WakeWordEvent {
    /// Which keyword was detected.
    pub keyword: String,
    /// Index into the keyword list.
    pub keyword_index: usize,
    /// Confidence score [0.0, 1.0].
    pub confidence: f32,
    /// When the detection occurred.
    pub timestamp: Instant,
}

// ---------------------------------------------------------------------------
// WakeWordDetector
// ---------------------------------------------------------------------------

pub struct WakeWordDetector {
    #[cfg(target_os = "android")]
    kws_state: *mut std::ffi::c_void,

    /// Registered keywords.
    keywords: Vec<String>,
    /// Sensitivity parameter.
    sensitivity: f32,
    /// Minimum interval between detections.
    cooldown: Duration,
    /// When the last detection happened.
    last_detection: Option<Instant>,
    /// Mock trigger for testing.
    #[cfg(not(target_os = "android"))]
    mock_trigger: Option<usize>,
}

unsafe impl Send for WakeWordDetector {}

impl WakeWordDetector {
    /// Create with default keywords and sensitivity.
    pub fn new() -> Self {
        Self::with_keywords(
            DEFAULT_KEYWORDS.iter().map(|s| s.to_string()).collect(),
            DEFAULT_SENSITIVITY,
        )
    }

    /// Create with custom keywords and sensitivity.
    pub fn with_keywords(keywords: Vec<String>, sensitivity: f32) -> Self {
        #[cfg(target_os = "android")]
        let kws_state = {
            // TODO: call sherpa_kws_ffi::sherpa_kws_create with model path
            std::ptr::null_mut()
        };

        Self {
            #[cfg(target_os = "android")]
            kws_state,
            keywords,
            sensitivity: sensitivity.clamp(0.0, 1.0),
            cooldown: Duration::from_millis(DEFAULT_COOLDOWN_MS as u64),
            last_detection: None,
            #[cfg(not(target_os = "android"))]
            mock_trigger: None,
        }
    }

    /// Process one frame of audio samples. Returns a `WakeWordEvent` if a
    /// keyword was detected and cooldown has elapsed.
    pub fn process_frame(&mut self, samples: &[i16]) -> Option<WakeWordEvent> {
        // Check cooldown
        if let Some(last) = self.last_detection {
            if last.elapsed() < self.cooldown {
                return None;
            }
        }

        let detection = self.detect(samples);

        if let Some((keyword_idx, confidence)) = detection {
            if keyword_idx < self.keywords.len() {
                let event = WakeWordEvent {
                    keyword: self.keywords[keyword_idx].clone(),
                    keyword_index: keyword_idx,
                    confidence,
                    timestamp: Instant::now(),
                };
                self.last_detection = Some(Instant::now());
                return Some(event);
            }
        }

        None
    }

    /// Reset internal state (e.g., after handling a wake word).
    pub fn reset(&mut self) {
        self.last_detection = None;

        #[cfg(target_os = "android")]
        if !self.kws_state.is_null() {
            unsafe { sherpa_kws_ffi::sherpa_kws_reset(self.kws_state) };
        }
    }

    /// Get registered keywords.
    pub fn keywords(&self) -> &[String] {
        &self.keywords
    }

    /// Set cooldown duration.
    pub fn set_cooldown(&mut self, cooldown_ms: u32) {
        self.cooldown = Duration::from_millis(cooldown_ms as u64);
    }

    /// For testing: trigger a mock detection of the given keyword index.
    #[cfg(not(target_os = "android"))]
    pub fn mock_set_trigger(&mut self, keyword_index: Option<usize>) {
        self.mock_trigger = keyword_index;
    }

    // -- Internal -------------------------------------------------------

    fn detect(&mut self, samples: &[i16]) -> Option<(usize, f32)> {
        #[cfg(target_os = "android")]
        {
            let float_samples: Vec<f32> = samples.iter().map(|&s| s as f32 / 32768.0).collect();
            let result = unsafe {
                sherpa_kws_ffi::sherpa_kws_process(
                    self.kws_state,
                    float_samples.as_ptr(),
                    float_samples.len() as std::os::raw::c_int,
                )
            };
            if result >= 0 {
                Some((result as usize, self.sensitivity))
            } else {
                None
            }
        }

        #[cfg(not(target_os = "android"))]
        {
            let _ = samples; // suppress unused warning
            self.mock_trigger.take().map(|idx| (idx, 0.9))
        }
    }
}

impl Default for WakeWordDetector {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for WakeWordDetector {
    fn drop(&mut self) {
        #[cfg(target_os = "android")]
        if !self.kws_state.is_null() {
            unsafe { sherpa_kws_ffi::sherpa_kws_destroy(self.kws_state) };
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_keywords() {
        let detector = WakeWordDetector::new();
        assert_eq!(detector.keywords().len(), 3);
        assert_eq!(detector.keywords()[0], "hey aura");
    }

    #[test]
    fn mock_no_trigger_by_default() {
        let mut detector = WakeWordDetector::new();
        let frame = vec![0i16; 512];
        assert!(detector.process_frame(&frame).is_none());
    }

    #[test]
    fn mock_trigger_works() {
        let mut detector = WakeWordDetector::new();
        detector.mock_set_trigger(Some(0)); // trigger "hey aura"

        let frame = vec![0i16; 512];
        let event = detector.process_frame(&frame);
        assert!(event.is_some());
        let event = event.unwrap();
        assert_eq!(event.keyword, "hey aura");
        assert_eq!(event.keyword_index, 0);
    }

    #[test]
    fn cooldown_prevents_double_trigger() {
        let mut detector = WakeWordDetector::new();
        detector.set_cooldown(5_000); // 5 second cooldown

        detector.mock_set_trigger(Some(0));
        let frame = vec![0i16; 512];
        let event = detector.process_frame(&frame);
        assert!(event.is_some());

        // Immediately try again — should be blocked by cooldown
        detector.mock_set_trigger(Some(0));
        let event = detector.process_frame(&frame);
        assert!(event.is_none());
    }

    #[test]
    fn reset_clears_cooldown() {
        let mut detector = WakeWordDetector::new();
        detector.set_cooldown(60_000); // long cooldown

        detector.mock_set_trigger(Some(1));
        let frame = vec![0i16; 512];
        detector.process_frame(&frame); // triggers

        detector.reset(); // clears cooldown

        detector.mock_set_trigger(Some(1));
        let event = detector.process_frame(&frame);
        assert!(event.is_some()); // should trigger despite long cooldown
    }
}
