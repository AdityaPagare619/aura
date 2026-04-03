//! Signal processing: RNNoise denoising + Acoustic Echo Cancellation.
//!
//! RNNoise operates on 480-sample frames at 48 kHz. Since our pipeline runs
//! at 16 kHz, we resample up → denoise → resample down. On non-Android
//! platforms, a passthrough mock is used.

use std::collections::VecDeque;
use std::sync::Mutex;

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

#[derive(Debug, thiserror::Error)]
pub enum SignalError {
    #[error("RNNoise initialization failed")]
    RnnoiseInitFailed,
    #[error("invalid frame size: expected {expected}, got {got}")]
    InvalidFrameSize { expected: usize, got: usize },
    #[error("AEC reference buffer overflow")]
    AecOverflow,
}

pub type SignalResult<T> = Result<T, SignalError>;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// RNNoise native frame size (480 samples at 48 kHz = 10 ms).
pub const RNNOISE_FRAME_SIZE: usize = 480;

/// Our pipeline sample rate.
// Phase 8 wire point: used by VoiceEngine resampling path on Android target.
#[allow(dead_code)]
const PIPELINE_RATE: u32 = 16_000;

/// RNNoise native sample rate.
// Phase 8 wire point: used by VoiceEngine resampling path on Android target.
#[allow(dead_code)]
const RNNOISE_RATE: u32 = 48_000;

/// Resample factor (48000 / 16000 = 3).
const RESAMPLE_FACTOR: usize = 3;

/// AEC reference buffer max length (2 seconds at 16 kHz).
const AEC_MAX_REF_SAMPLES: usize = 32_000;

// ---------------------------------------------------------------------------
// RNNoise FFI (Android only)
// ---------------------------------------------------------------------------

#[cfg(target_os = "android")]
mod rnnoise_ffi {
    use std::os::raw::c_void;

    extern "C" {
        pub fn rnnoise_create(model: *const c_void) -> *mut c_void;
        pub fn rnnoise_destroy(state: *mut c_void);
        /// Process one frame of 480 float samples. Returns voice probability [0,1].
        pub fn rnnoise_process_frame(state: *mut c_void, out: *mut f32, input: *const f32) -> f32;
    }
}

// ---------------------------------------------------------------------------
// RNNoise Denoiser
// ---------------------------------------------------------------------------

// Phase 8 wire point: upsample_buf and output_buf are used exclusively
// in the Android cfg-gated RNNoise path; on non-Android targets they are
// unreachable dead storage. Annotate the struct to suppress the warning.
#[allow(dead_code)]
pub struct RnnoiseDenoiser {
    #[cfg(target_os = "android")]
    state: Mutex<*mut std::ffi::c_void>,
    /// Reusable buffer for upsampled input (480 samples at 48 kHz).
    upsample_buf: Vec<f32>,
    /// Reusable buffer for denoised output (480 samples at 48 kHz).
    output_buf: Vec<f32>,
}

// SAFETY: The RNNoise state pointer is wrapped in a Mutex, providing interior
// mutability and cross-thread synchronization. The Mutex ensures only one thread
// can dereference the RNNoise C library pointer at a time. The VoiceEngine
// serializes all calls through its async task, and the Mutex provides an
// additional safety guarantee.
unsafe impl Send for RnnoiseDenoiser {}

impl RnnoiseDenoiser {
    /// Create a new RNNoise denoiser instance.
    pub fn new() -> SignalResult<Self> {
        #[cfg(target_os = "android")]
        let state = unsafe {
            // SAFETY: rnnoise_create allocates a new RNNoise state. We pass null
            // for the optional model parameter (uses built-in default). The returned
            // pointer is null-checked; if null, we return an error rather than storing it.
            let s = rnnoise_ffi::rnnoise_create(std::ptr::null());
            if s.is_null() {
                return Err(SignalError::RnnoiseInitFailed);
            }
            s
        };

        Ok(Self {
            #[cfg(target_os = "android")]
            state: Mutex::new(state),
            upsample_buf: vec![0.0f32; RNNOISE_FRAME_SIZE],
            output_buf: vec![0.0f32; RNNOISE_FRAME_SIZE],
        })
    }

    /// Process one frame of 160 i16 samples at 16 kHz.
    /// Returns voice probability in [0.0, 1.0]. The `samples` buffer is
    /// modified in-place with denoised audio.
    ///
    /// Frame size must be exactly 160 samples (10 ms at 16 kHz), which maps
    /// to 480 samples at 48 kHz for RNNoise.
    pub fn process(&mut self, samples: &mut [f32]) -> SignalResult<f32> {
        let expected = RNNOISE_FRAME_SIZE / RESAMPLE_FACTOR; // 160
        if samples.len() != expected {
            return Err(SignalError::InvalidFrameSize {
                expected,
                got: samples.len(),
            });
        }

        #[cfg(target_os = "android")]
        {
            // Upsample 160 → 480 via linear interpolation
            Self::upsample(samples, &mut self.upsample_buf, RESAMPLE_FACTOR);

            // RNNoise expects float in roughly [-32768, 32767] range
            let ptr = *self.state.lock().unwrap();
            let vad_prob = unsafe {
                // SAFETY: ptr was allocated by rnnoise_create and is non-null
                // (checked in new()). upsample_buf and output_buf are Vecs with
                // RNNOISE_FRAME_SIZE capacity, so their pointers are valid for
                // the duration of this call. The Mutex ensures exclusive access.
                rnnoise_ffi::rnnoise_process_frame(
                    ptr,
                    self.output_buf.as_mut_ptr(),
                    self.upsample_buf.as_ptr(),
                )
            };

            // Downsample 480 → 160
            Self::downsample(&self.output_buf, samples, RESAMPLE_FACTOR);
            Ok(vad_prob)
        }

        #[cfg(not(target_os = "android"))]
        {
            // Mock: passthrough with high voice probability
            Ok(0.95)
        }
    }

    /// Simple linear-interpolation upsample by integer factor.
    // Phase 8 wire point: called by RNNoise denoising path on Android target.
    #[allow(dead_code)]
    fn upsample(input: &[f32], output: &mut [f32], factor: usize) {
        let in_len = input.len();
        for i in 0..in_len {
            let next = if i + 1 < in_len {
                input[i + 1]
            } else {
                input[i]
            };
            for j in 0..factor {
                let t = j as f32 / factor as f32;
                output[i * factor + j] = input[i] * (1.0 - t) + next * t;
            }
        }
    }

    /// Downsample by integer factor (simple decimation).
    // Phase 8 wire point: called by RNNoise denoising path on Android target.
    #[allow(dead_code)]
    fn downsample(input: &[f32], output: &mut [f32], factor: usize) {
        for (i, out_sample) in output.iter_mut().enumerate() {
            *out_sample = input[i * factor];
        }
    }
}

impl Drop for RnnoiseDenoiser {
    fn drop(&mut self) {
        #[cfg(target_os = "android")]
        {
            let ptr = *self.state.lock().unwrap();
            if !ptr.is_null() {
                unsafe { rnnoise_ffi::rnnoise_destroy(ptr) };
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Acoustic Echo Canceller
// ---------------------------------------------------------------------------

/// Simple delay-and-subtract AEC.
///
/// When AURA speaks (TTS output), we capture a copy of the output as the
/// "reference signal." The AEC subtracts a delayed, scaled version of that
/// reference from the mic input to reduce self-hearing echo.
pub struct EchoCanceller {
    /// Buffer of recent TTS output samples (reference signal).
    reference_buffer: VecDeque<f32>,
    /// Estimated echo delay in samples.
    delay_samples: usize,
    /// Attenuation factor for the reference signal [0.0, 1.0].
    attenuation: f32,
    /// Whether AEC is active (only during simultaneous capture + playback).
    active: bool,
}

impl EchoCanceller {
    /// Create a new echo canceller with estimated delay.
    ///
    /// `delay_ms` — estimated round-trip speaker→mic delay in milliseconds.
    pub fn new(delay_ms: u32, sample_rate: u32) -> Self {
        let delay_samples = (delay_ms as usize * sample_rate as usize) / 1000;
        Self {
            reference_buffer: VecDeque::with_capacity(AEC_MAX_REF_SAMPLES),
            delay_samples,
            attenuation: 0.7,
            active: false,
        }
    }

    /// Feed TTS output as reference signal.
    pub fn feed_reference(&mut self, samples: &[f32]) -> SignalResult<()> {
        if self.reference_buffer.len() + samples.len() > AEC_MAX_REF_SAMPLES {
            // Drain oldest to make room
            let excess = (self.reference_buffer.len() + samples.len()) - AEC_MAX_REF_SAMPLES;
            self.reference_buffer.drain(..excess);
        }
        self.reference_buffer.extend(samples.iter().copied());
        Ok(())
    }

    /// Process mic input: subtract estimated echo from samples in-place.
    pub fn cancel_echo(&mut self, samples: &mut [f32]) {
        if !self.active || self.reference_buffer.len() <= self.delay_samples {
            return;
        }

        let ref_start = self
            .reference_buffer
            .len()
            .saturating_sub(self.delay_samples + samples.len());
        for (i, sample) in samples.iter_mut().enumerate() {
            if let Some(&ref_sample) = self.reference_buffer.get(ref_start + i) {
                *sample -= ref_sample * self.attenuation;
            }
        }
    }

    pub fn set_active(&mut self, active: bool) {
        self.active = active;
        if !active {
            self.reference_buffer.clear();
        }
    }

    pub fn is_active(&self) -> bool {
        self.active
    }

    pub fn set_attenuation(&mut self, attenuation: f32) {
        self.attenuation = attenuation.clamp(0.0, 1.0);
    }
}

// ---------------------------------------------------------------------------
// Combined SignalProcessor
// ---------------------------------------------------------------------------

/// Bundles denoising and echo cancellation into one processing step.
pub struct SignalProcessor {
    pub rnnoise: RnnoiseDenoiser,
    pub aec: EchoCanceller,
}

impl SignalProcessor {
    /// Create with default 20 ms echo delay at 16 kHz.
    pub fn new() -> SignalResult<Self> {
        Ok(Self {
            rnnoise: RnnoiseDenoiser::new()?,
            aec: EchoCanceller::new(20, super::audio_io::DEFAULT_SAMPLE_RATE),
        })
    }

    /// Process a frame: AEC first, then denoise. Returns voice probability.
    pub fn process_frame(&mut self, samples: &mut [f32]) -> SignalResult<f32> {
        self.aec.cancel_echo(samples);
        self.rnnoise.process(samples)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rnnoise_mock_passthrough() {
        let mut denoiser = RnnoiseDenoiser::new().unwrap();
        let mut frame = vec![0.5f32; 160]; // 10ms at 16kHz
        let prob = denoiser.process(&mut frame).unwrap();
        // Mock returns 0.95
        assert!((prob - 0.95).abs() < 0.01);
    }

    #[test]
    fn rnnoise_rejects_wrong_frame_size() {
        let mut denoiser = RnnoiseDenoiser::new().unwrap();
        let mut frame = vec![0.0f32; 100]; // wrong size
        assert!(denoiser.process(&mut frame).is_err());
    }

    #[test]
    fn aec_subtracts_reference() {
        let mut aec = EchoCanceller::new(0, 16_000); // 0 delay for simple test
        aec.set_active(true);

        // Feed reference
        let reference = vec![1.0f32; 100];
        aec.feed_reference(&reference).unwrap();

        // Process mic input that is identical to reference → should reduce
        let mut mic = vec![1.0f32; 50];
        aec.cancel_echo(&mut mic);

        // After subtraction: 1.0 - 1.0 * 0.7 = 0.3
        for &s in &mic {
            assert!((s - 0.3).abs() < 0.1, "expected ~0.3, got {s}");
        }
    }

    #[test]
    fn aec_inactive_passthrough() {
        let mut aec = EchoCanceller::new(0, 16_000);
        // active is false by default
        let reference = vec![1.0f32; 100];
        aec.feed_reference(&reference).unwrap();

        let mut mic = vec![1.0f32; 50];
        aec.cancel_echo(&mut mic);
        // Should not modify
        for &s in &mic {
            assert!((s - 1.0).abs() < f32::EPSILON);
        }
    }

    #[test]
    fn upsample_downsample_roundtrip() {
        let input = [0.0f32, 1.0, 2.0, 3.0];
        let mut upsampled = vec![0.0f32; 12];
        RnnoiseDenoiser::upsample(&input, &mut upsampled, 3);

        let mut downsampled = vec![0.0f32; 4];
        RnnoiseDenoiser::downsample(&upsampled, &mut downsampled, 3);

        // Decimation picks every 3rd sample from the upsampled data
        for (i, &val) in downsampled.iter().enumerate() {
            assert!(
                (val - input[i]).abs() < 0.01,
                "index {i}: expected {}, got {val}",
                input[i]
            );
        }
    }
}
