//! Audio input/output via Oboe FFI (Android) with desktop mocks.
//!
//! Provides low-latency audio capture and playback through Android's Oboe library.
//! On non-Android platforms, a mock implementation is provided for testing.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

// ---------------------------------------------------------------------------
// Error type (voice-local, convertible to AuraError in mod.rs)
// ---------------------------------------------------------------------------

#[derive(Debug, thiserror::Error)]
pub enum AudioError {
    #[error("audio stream not initialized")]
    StreamNotInitialized,
    #[error("audio stream already running")]
    StreamAlreadyRunning,
    #[error("oboe FFI error: {0}")]
    OboeError(String),
    #[error("buffer overflow — consumer too slow")]
    BufferOverflow,
    #[error("invalid sample rate: {0}")]
    InvalidSampleRate(u32),
    #[error("invalid buffer size: {0}")]
    InvalidBufferSize(usize),
}

pub type AudioResult<T> = Result<T, AudioError>;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Default sample rate for STT pipeline (16 kHz).
pub const DEFAULT_SAMPLE_RATE: u32 = 16_000;

/// Default mono channel.
pub const DEFAULT_CHANNELS: u8 = 1;

/// 30 ms frame at 16 kHz = 480 samples.
pub const DEFAULT_FRAME_SIZE: usize = 480;

/// Max ring buffer: 10 seconds at 16 kHz = 160 000 samples.
pub const MAX_BUFFER_SAMPLES: usize = 160_000;

/// Playback buffer high-water mark (samples). When exceeded, oldest samples are dropped.
pub const PLAYBACK_HWM: usize = 160_000;

// ---------------------------------------------------------------------------
// Lock-free ring buffer for audio capture
// ---------------------------------------------------------------------------

/// Single-producer single-consumer ring buffer backed by a fixed-size slice.
///
/// The write side is driven by the Oboe callback (realtime thread) and the
/// read side is consumed by the processing pipeline on a normal thread.
pub struct RingBuffer {
    data: Box<[i16]>,
    capacity: usize,
    write_pos: AtomicUsize,
    read_pos: AtomicUsize,
}

impl RingBuffer {
    pub fn new(capacity: usize) -> Self {
        assert!(
            capacity > 0 && capacity.is_power_of_two(),
            "capacity must be power of two"
        );
        Self {
            data: vec![0i16; capacity].into_boxed_slice(),
            capacity,
            write_pos: AtomicUsize::new(0),
            read_pos: AtomicUsize::new(0),
        }
    }

    /// Number of samples available for reading.
    pub fn available(&self) -> usize {
        let w = self.write_pos.load(Ordering::Acquire);
        let r = self.read_pos.load(Ordering::Acquire);
        w.wrapping_sub(r)
    }

    /// Write samples into the ring buffer (producer side). Returns the number
    /// of samples actually written. Samples that would overflow are **dropped**.
    pub fn write(&self, samples: &[i16]) -> usize {
        let w = self.write_pos.load(Ordering::Relaxed);
        let r = self.read_pos.load(Ordering::Acquire);
        let free = self.capacity - w.wrapping_sub(r);
        let to_write = samples.len().min(free);

        for i in 0..to_write {
            let idx = (w + i) & (self.capacity - 1);
            // SAFETY: idx is always < capacity because of the mask.
            unsafe {
                let ptr = self.data.as_ptr() as *mut i16;
                ptr.add(idx).write(samples[i]);
            }
        }
        self.write_pos.store(w + to_write, Ordering::Release);
        to_write
    }

    /// Read samples from the ring buffer (consumer side). Returns the number
    /// of samples actually read.
    pub fn read(&self, out: &mut [i16]) -> usize {
        let w = self.write_pos.load(Ordering::Acquire);
        let r = self.read_pos.load(Ordering::Relaxed);
        let avail = w.wrapping_sub(r);
        let to_read = out.len().min(avail);

        for i in 0..to_read {
            let idx = (r + i) & (self.capacity - 1);
            unsafe {
                let ptr = self.data.as_ptr();
                out[i] = ptr.add(idx).read();
            }
        }
        self.read_pos.store(r + to_read, Ordering::Release);
        to_read
    }
}

// ---------------------------------------------------------------------------
// AudioInputStream / AudioOutputStream
// ---------------------------------------------------------------------------

/// Wraps a ring buffer fed by the Oboe capture callback.
pub struct AudioInputStream {
    pub(crate) ring: Arc<RingBuffer>,
    pub(crate) running: Arc<AtomicBool>,
}

/// Wraps a playback queue drained by the Oboe playback callback.
pub struct AudioOutputStream {
    pub(crate) buffer: VecDeque<i16>,
    pub(crate) running: Arc<AtomicBool>,
}

impl AudioOutputStream {
    pub fn enqueue(&mut self, samples: &[i16]) {
        // Enforce high-water mark to prevent unbounded growth.
        if self.buffer.len() + samples.len() > PLAYBACK_HWM {
            let excess = (self.buffer.len() + samples.len()) - PLAYBACK_HWM;
            self.buffer.drain(..excess);
        }
        self.buffer.extend(samples.iter().copied());
    }

    pub fn drain(&mut self, out: &mut [i16]) -> usize {
        let n = out.len().min(self.buffer.len());
        for (i, sample) in self.buffer.drain(..n).enumerate() {
            out[i] = sample;
        }
        n
    }

    pub fn pending(&self) -> usize {
        self.buffer.len()
    }
}

// ---------------------------------------------------------------------------
// Oboe FFI (Android only)
// ---------------------------------------------------------------------------

#[cfg(target_os = "android")]
mod oboe_ffi {
    use std::os::raw::{c_int, c_void};

    extern "C" {
        pub fn oboe_create_input_stream(
            sample_rate: c_int,
            channels: c_int,
            frames_per_buffer: c_int,
            callback: extern "C" fn(*mut c_void, *const i16, c_int),
            user_data: *mut c_void,
        ) -> *mut c_void;

        pub fn oboe_create_output_stream(
            sample_rate: c_int,
            channels: c_int,
            frames_per_buffer: c_int,
            callback: extern "C" fn(*mut c_void, *mut i16, c_int),
            user_data: *mut c_void,
        ) -> *mut c_void;

        pub fn oboe_start_stream(stream: *mut c_void) -> c_int;
        pub fn oboe_stop_stream(stream: *mut c_void) -> c_int;
        pub fn oboe_destroy_stream(stream: *mut c_void);
    }
}

// ---------------------------------------------------------------------------
// AudioIo — main public interface
// ---------------------------------------------------------------------------

pub struct AudioIo {
    pub(crate) input: Option<AudioInputStream>,
    pub(crate) output: Option<AudioOutputStream>,
    pub sample_rate: u32,
    pub channels: u8,
    pub frame_size: usize,
}

impl AudioIo {
    /// Create a new `AudioIo` with default parameters (16 kHz, mono, 30 ms frame).
    pub fn new() -> Self {
        Self {
            input: None,
            output: None,
            sample_rate: DEFAULT_SAMPLE_RATE,
            channels: DEFAULT_CHANNELS,
            frame_size: DEFAULT_FRAME_SIZE,
        }
    }

    /// Create with explicit parameters.
    pub fn with_params(sample_rate: u32, channels: u8, frame_size: usize) -> AudioResult<Self> {
        if sample_rate == 0 || sample_rate > 48_000 {
            return Err(AudioError::InvalidSampleRate(sample_rate));
        }
        if frame_size == 0 || frame_size > MAX_BUFFER_SAMPLES {
            return Err(AudioError::InvalidBufferSize(frame_size));
        }
        Ok(Self {
            input: None,
            output: None,
            sample_rate,
            channels,
            frame_size,
        })
    }

    // -- Capture --------------------------------------------------------

    /// Start audio capture. On Android this opens an Oboe input stream.
    pub fn start_capture(&mut self) -> AudioResult<()> {
        if self
            .input
            .as_ref()
            .map_or(false, |i| i.running.load(Ordering::Relaxed))
        {
            return Err(AudioError::StreamAlreadyRunning);
        }
        let ring = Arc::new(RingBuffer::new(MAX_BUFFER_SAMPLES.next_power_of_two()));
        let running = Arc::new(AtomicBool::new(true));

        #[cfg(target_os = "android")]
        {
            // TODO: call oboe_ffi::oboe_create_input_stream and oboe_ffi::oboe_start_stream
            // The callback writes into `ring`.
        }

        self.input = Some(AudioInputStream { ring, running });
        Ok(())
    }

    /// Stop audio capture.
    pub fn stop_capture(&mut self) -> AudioResult<()> {
        if let Some(ref input) = self.input {
            input.running.store(false, Ordering::Release);
            #[cfg(target_os = "android")]
            {
                // TODO: oboe_ffi::oboe_stop_stream / oboe_ffi::oboe_destroy_stream
            }
        }
        self.input = None;
        Ok(())
    }

    /// Read available samples from the capture ring buffer.
    /// Returns the number of samples actually read.
    pub fn read_samples(&self, out: &mut [i16]) -> usize {
        match &self.input {
            Some(input) => input.ring.read(out),
            None => 0,
        }
    }

    /// Returns true if capture is active.
    pub fn is_capturing(&self) -> bool {
        self.input
            .as_ref()
            .map_or(false, |i| i.running.load(Ordering::Relaxed))
    }

    // -- Playback -------------------------------------------------------

    /// Start audio playback. On Android this opens an Oboe output stream.
    pub fn start_playback(&mut self) -> AudioResult<()> {
        if self
            .output
            .as_ref()
            .map_or(false, |o| o.running.load(Ordering::Relaxed))
        {
            return Err(AudioError::StreamAlreadyRunning);
        }
        let running = Arc::new(AtomicBool::new(true));

        #[cfg(target_os = "android")]
        {
            // TODO: call oboe_ffi::oboe_create_output_stream and oboe_ffi::oboe_start_stream
        }

        self.output = Some(AudioOutputStream {
            buffer: VecDeque::with_capacity(self.frame_size * 100),
            running,
        });
        Ok(())
    }

    /// Stop audio playback.
    pub fn stop_playback(&mut self) -> AudioResult<()> {
        if let Some(ref output) = self.output {
            output.running.store(false, Ordering::Release);
            #[cfg(target_os = "android")]
            {
                // TODO: oboe_ffi::oboe_stop_stream / oboe_ffi::oboe_destroy_stream
            }
        }
        self.output = None;
        Ok(())
    }

    /// Enqueue PCM samples for playback.
    pub fn play_samples(&mut self, samples: &[i16]) -> AudioResult<()> {
        match &mut self.output {
            Some(output) => {
                output.enqueue(samples);
                Ok(())
            }
            None => Err(AudioError::StreamNotInitialized),
        }
    }

    /// Returns true if playback is active.
    pub fn is_playing(&self) -> bool {
        self.output
            .as_ref()
            .map_or(false, |o| o.running.load(Ordering::Relaxed))
    }

    /// Returns number of pending playback samples.
    pub fn playback_pending(&self) -> usize {
        self.output.as_ref().map_or(0, |o| o.buffer.len())
    }
}

impl Default for AudioIo {
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
    fn ring_buffer_basic_read_write() {
        let rb = RingBuffer::new(1024);
        let data = [1i16, 2, 3, 4, 5];
        assert_eq!(rb.write(&data), 5);
        assert_eq!(rb.available(), 5);

        let mut out = [0i16; 5];
        assert_eq!(rb.read(&mut out), 5);
        assert_eq!(out, [1, 2, 3, 4, 5]);
        assert_eq!(rb.available(), 0);
    }

    #[test]
    fn ring_buffer_overflow_drops() {
        let rb = RingBuffer::new(4); // tiny buffer
        let data = [1i16, 2, 3, 4, 5, 6];
        // Only 4 fit
        assert_eq!(rb.write(&data), 4);
        let mut out = [0i16; 6];
        assert_eq!(rb.read(&mut out), 4);
        assert_eq!(&out[..4], &[1, 2, 3, 4]);
    }

    #[test]
    fn ring_buffer_wraparound() {
        let rb = RingBuffer::new(4);
        let data = [10i16, 20, 30];
        assert_eq!(rb.write(&data), 3);

        let mut out = [0i16; 2];
        rb.read(&mut out);
        // consumed 2, write pos=3, read pos=2 → 1 available
        assert_eq!(rb.available(), 1);

        // Write 3 more — wraps around
        let data2 = [40i16, 50, 60];
        assert_eq!(rb.write(&data2), 3);
        assert_eq!(rb.available(), 4);

        let mut out2 = [0i16; 4];
        rb.read(&mut out2);
        assert_eq!(out2, [30, 40, 50, 60]);
    }

    #[test]
    fn audio_io_capture_lifecycle() {
        let mut io = AudioIo::new();
        assert!(!io.is_capturing());
        io.start_capture().unwrap();
        assert!(io.is_capturing());
        io.stop_capture().unwrap();
        assert!(!io.is_capturing());
    }

    #[test]
    fn audio_io_playback_lifecycle() {
        let mut io = AudioIo::new();
        assert!(!io.is_playing());
        io.start_playback().unwrap();
        assert!(io.is_playing());

        let samples = [100i16; 480];
        io.play_samples(&samples).unwrap();
        assert_eq!(io.playback_pending(), 480);

        io.stop_playback().unwrap();
        assert!(!io.is_playing());
    }

    #[test]
    fn audio_io_play_without_stream_errors() {
        let mut io = AudioIo::new();
        let samples = [0i16; 10];
        assert!(io.play_samples(&samples).is_err());
    }

    #[test]
    fn audio_io_invalid_params() {
        assert!(AudioIo::with_params(0, 1, 480).is_err());
        assert!(AudioIo::with_params(96_000, 1, 480).is_err());
        assert!(AudioIo::with_params(16_000, 1, 0).is_err());
    }
}
