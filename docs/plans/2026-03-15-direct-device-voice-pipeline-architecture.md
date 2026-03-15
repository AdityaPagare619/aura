# AURA v4 Direct Device Voice Pipeline — Complete Architecture

> **Agent 7 Deliverable** | Date: 2026-03-15
> **Scope:** Architecture + Design only (no implementation — can't compile)
> **Iron Laws:** LLM = brain, Rust = body. Voice I/O is body. Anti-cloud default. Fast-path structural parsers acceptable for audio format handling and wake word detection.

---

## Table of Contents

- [Part 1: Full Pipeline Architecture + State Diagram](#part-1-full-pipeline-architecture--state-diagram)
- [Part 2: JNI Bridge Design — Android Built-in TTS](#part-2-jni-bridge-design--android-built-in-tts)
- [Part 3: JNI Bridge Design — Android Built-in STT (Optional)](#part-3-jni-bridge-design--android-built-in-stt-optional)
- [Part 4: Latency UX Specification](#part-4-latency-ux-specification)
- [Part 5: Proactive Voice Decision Matrix](#part-5-proactive-voice-decision-matrix)
- [Part 6: Integration Map + Alpha vs Beta Scope Split](#part-6-integration-map--alpha-vs-beta-scope-split)

---

# Part 1: Full Pipeline Architecture + State Diagram

## 1.1 Pipeline Overview

The "Hey AURA" direct device voice pipeline is a 24/7 always-listening loop running on Android. It is the primary way a user interacts with AURA through speech, from wake word detection through to spoken response.

```
                    ┌─────────────────────────────────────────┐
                    │         ANDROID DEVICE (24/7)            │
                    │                                          │
                    │  Oboe AudioStream (16kHz, mono, 30ms)    │
                    │         │                                │
                    │         ▼                                │
                    │  ┌─────────────────┐                     │
                    │  │ Signal Process   │ RNNoise denoise    │
                    │  │ (30ms frames)    │ + AEC cancel       │
                    │  └────────┬────────┘                     │
                    │           │                               │
                    │           ▼                               │
                    │  ┌─────────────────┐                     │
                    │  │  Wake Word KWS   │ sherpa-onnx         │
                    │  │  "Hey AURA"      │ ~5MB, low power    │
                    │  └────────┬────────┘                     │
                    │           │ DETECTED                      │
                    │           ▼                               │
                    │  ┌─────────────────┐                     │
                    │  │ Ack Playback    │ Pre-cached "Mm-hm"  │
                    │  │ (~200ms audio)  │ via Android TTS     │
                    │  └────────┬────────┘                     │
                    │           │                               │
                    │           ▼                               │
                    │  ┌─────────────────┐                     │
                    │  │  VAD + Capture   │ Silero VAD          │
                    │  │  Active Listen   │ Buffer accumulate   │
                    │  └────────┬────────┘                     │
                    │           │ SILENCE (500ms)               │
                    │           ▼                               │
                    │  ┌─────────────────┐                     │
                    │  │  STT Transcribe  │ Zipformer stream    │
                    │  │                  │ + Whisper batch     │
                    │  └────────┬────────┘                     │
                    │           │                               │
                    │           ▼                               │
                    │  ┌─────────────────┐                     │
                    │  │ Biomarkers      │ F0, jitter, energy   │
                    │  │ (parallel)      │ → VoiceMetadata      │
                    │  └────────┬────────┘                     │
                    │           │                               │
                    │           ▼                               │
                    │  ┌─────────────────┐                     │
                    │  │ Processing Cue  │ "Let me think..."   │
                    │  │ (if LLM slow)   │ after 1.5s timeout  │
                    │  └────────┬────────┘                     │
                    │           │                               │
                    │           ▼                               │
                    │      ┌─────────┐                         │
                    │      │  DAEMON  │ LLM processing          │
                    │      │  CORE    │ (3-15 seconds)          │
                    │      └────┬────┘                         │
                    │           │                               │
                    │           ▼                               │
                    │  ┌─────────────────┐                     │
                    │  │  TTS Synthesis   │ Android built-in    │
                    │  │                  │ (or Piper fallback) │
                    │  └────────┬────────┘                     │
                    │           │                               │
                    │           ▼                               │
                    │  ┌─────────────────┐                     │
                    │  │ Audio Playback  │ Oboe output stream   │
                    │  │ + AEC ref feed  │ feeds echo cancel    │
                    │  └────────┬────────┘                     │
                    │           │                               │
                    │           ▼                               │
                    │     Back to Wake Word Listening           │
                    └──────────────────────────────────────────┘
```

## 1.2 Enhanced State Machine

The existing `ModalityStateMachine` in `modality_state_machine.rs` has these states:
`Idle → WakeWordListening → ActiveListening → Processing → Speaking`

**We add three micro-states** (implemented as sub-states, not new top-level states) to handle UX gaps:

```
                        ┌─────────────────────┐
                        │       IDLE           │
                        │  (screen off / quiet) │
                        └──────────┬──────────┘
                                   │ audio_io starts / app foreground
                                   ▼
                    ┌───────────────────────────────┐
                    │     WAKE_WORD_LISTENING        │
                    │  • Signal processing active    │
                    │  • Wake word KWS consuming      │
                    │  • VAD in background (low power)│
                    │  • ~2mW CPU budget              │
                    └──────────────┬────────────────┘
                                   │ wake word detected
                                   │ (2s cooldown enforced)
                                   ▼
                    ┌───────────────────────────────┐
                    │     ACKNOWLEDGING              │  ◄── NEW sub-state
                    │  • Play pre-cached ack audio   │
                    │  • "Mm-hm" / "Yeah?" / chime   │
                    │  • ~200ms duration              │
                    │  • Concurrent: start VAD        │
                    └──────────────┬────────────────┘
                                   │ ack complete (or ack + first speech frame)
                                   ▼
                    ┌───────────────────────────────┐
                    │     ACTIVE_LISTENING           │
                    │  • VAD tracking speech          │
                    │  • Audio buffering to ring buf  │
                    │  • STT streaming (Zipformer)    │
                    │  • Timeout: 8s max silence      │
                    │  • Timeout: 30s max utterance   │
                    └──────────────┬────────────────┘
                                   │ utterance_complete (VAD silence 500ms)
                                   │ OR timeout
                                   ▼
                    ┌───────────────────────────────┐
                    │     PROCESSING                 │
                    │  • STT finalize (Whisper re-tx  │
                    │    if audio ≤ 30s)              │
                    │  • Biomarker extraction          │
                    │  • Send UserCommand::Chat       │
                    │  • Start 1.5s timer             │
                    │  ┌──────────────────────────┐  │
                    │  │  PROCESSING_CUE          │  │  ◄── NEW sub-state
                    │  │  If timer fires before    │  │
                    │  │  LLM response:            │  │
                    │  │  play "Let me think..."   │  │
                    │  │  or "Working on it..."    │  │
                    │  └──────────────────────────┘  │
                    └──────────────┬────────────────┘
                                   │ DaemonResponse received
                                   ▼
                    ┌───────────────────────────────┐
                    │     SPEAKING                   │
                    │  • TTS synthesize response      │
                    │  • Audio playback via Oboe      │
                    │  • AEC reference signal fed     │
                    │  • Barge-in: wake word → cancel  │
                    │    TTS, jump to ACKNOWLEDGING   │
                    └──────────────┬────────────────┘
                                   │ playback complete
                                   │ OR barge-in
                                   ▼
                          WAKE_WORD_LISTENING
                          (loop continues)

    ╔═══════════════════════════════════════════╗
    ║  SPECIAL TRANSITIONS:                     ║
    ║                                           ║
    ║  ANY state + incoming call →  IN_CALL     ║
    ║  IN_CALL + call ends → restore prev state ║
    ║                                           ║
    ║  SPEAKING + wake word → ACKNOWLEDGING     ║
    ║  (barge-in: cancel current TTS)           ║
    ║                                           ║
    ║  ACTIVE_LISTENING + 8s silence → IDLE     ║
    ║  (user didn't say anything)               ║
    ║                                           ║
    ║  ANY state + screen lock → IDLE           ║
    ║  (battery conservation)                   ║
    ╚═══════════════════════════════════════════╝
```

## 1.3 Sub-State Implementation Strategy

The new sub-states (`Acknowledging` and `ProcessingCue`) are NOT new enum variants in `ModalityState`. They are implemented as **flags within the existing states** to avoid breaking the clean state machine:

```rust
// In VoiceEngine (voice/mod.rs), not in the state machine itself:
struct VoiceEngineState {
    /// Set when wake word fires, cleared when ack audio finishes
    ack_playing: bool,
    /// Set when Processing state entered, cleared when LLM responds or cue plays
    processing_cue_timer: Option<Instant>,
    /// Whether processing cue has already been spoken for this turn
    processing_cue_spoken: bool,
}
```

**Rationale:** The modality state machine is pure and clean. UX micro-states are orchestration concerns that belong in VoiceEngine, not in the state machine. The state machine stays at 5+1 states (Idle, WakeWordListening, ActiveListening, Processing, Speaking, InCall).

## 1.4 Frame Processing Loop (Enhanced)

The current `process_frame()` in `voice/mod.rs` processes one 30ms frame at a time. Here's the enhanced flow:

```
process_frame(raw_audio: &[i16]) -> Option<ProcessedUtterance>
    │
    ├── signal_processing.denoise(raw_audio) → clean_audio
    │   └── signal_processing.echo_cancel(clean_audio, tts_ref) → processed
    │
    ├── match current_state:
    │
    │   WakeWordListening:
    │   ├── wake_word.process_frame(processed)
    │   ├── if detected:
    │   │   ├── play_acknowledgment()          // NEW: async, non-blocking
    │   │   ├── state → ActiveListening
    │   │   └── vad.reset()                    // fresh utterance
    │   └── else: return None
    │
    │   ActiveListening:
    │   ├── vad.process_frame(processed)
    │   ├── speech_buffer.extend(processed)
    │   ├── stt.feed_streaming(processed)      // Zipformer real-time
    │   ├── if vad.utterance_complete():
    │   │   ├── final_audio = speech_buffer.drain()
    │   │   ├── transcript = stt.finalize()
    │   │   ├── if audio.len() <= 30s * 16000:
    │   │   │   └── transcript = stt.transcribe_batch(final_audio) // Whisper
    │   │   ├── biomarkers = extract_biomarkers(final_audio)
    │   │   ├── state → Processing
    │   │   ├── start_processing_cue_timer(1.5s)  // NEW
    │   │   └── return Some(ProcessedUtterance { transcript, biomarkers })
    │   └── if timeout(8s no speech OR 30s total):
    │       ├── state → WakeWordListening
    │       └── return None
    │
    │   Processing:
    │   ├── // Frames still flow for AEC reference
    │   ├── if processing_cue_timer.elapsed() > 1.5s && !cue_spoken:
    │   │   ├── play_processing_cue()             // NEW
    │   │   └── cue_spoken = true
    │   └── // Wait for speak() call from VoiceBridge
    │
    │   Speaking:
    │   ├── wake_word.process_frame(processed)    // Barge-in detection
    │   ├── if wake_word detected:
    │   │   ├── cancel_tts_playback()
    │   │   ├── play_acknowledgment()
    │   │   └── state → ActiveListening
    │   └── // else: continue TTS playback
    │
    └── return None
```

## 1.5 Audio Buffer Architecture

```
┌──────────────────────────────────────────────────────────────┐
│                  AUDIO BUFFER STRATEGY                        │
│                                                              │
│  Input Path:                                                 │
│  ┌──────────┐    ┌──────────┐    ┌──────────┐               │
│  │  Oboe    │───►│  SPSC    │───►│ process  │               │
│  │  Callback│    │  Ring    │    │ _frame() │               │
│  │  (30ms)  │    │  Buffer  │    │          │               │
│  └──────────┘    │  4096    │    └──────────┘               │
│                  │  samples  │                               │
│                  └──────────┘                                │
│                                                              │
│  Speech Accumulation (during ActiveListening):               │
│  ┌──────────────────────────────────┐                       │
│  │  Vec<i16> speech_buffer           │                       │
│  │  Max: 30s × 16000 = 480,000      │                       │
│  │  ~960 KB                          │                       │
│  │  Pre-allocated, reused per turn   │                       │
│  └──────────────────────────────────┘                       │
│                                                              │
│  Output Path:                                                │
│  ┌──────────┐    ┌──────────┐    ┌──────────┐               │
│  │  TTS     │───►│  SPSC    │───►│  Oboe    │               │
│  │  Synth   │    │  Ring    │    │  Output  │               │
│  │          │    │  Buffer  │    │  Callback│               │
│  └──────────┘    │  8192    │    └──────────┘               │
│                  │  samples  │                               │
│                  └──────────┘                                │
│                       │                                      │
│                       └──► AEC reference signal              │
│                            (fed back to signal_processing)   │
└──────────────────────────────────────────────────────────────┘
```

## 1.6 Threading Model

```
┌──────────────────────────────────────────────────────────┐
│                  THREAD MODEL                             │
│                                                          │
│  Thread 1: Oboe Audio Input Callback (real-time)         │
│  ├── Priority: AUDIO (highest)                           │
│  ├── Work: Copy 480 samples → input SPSC ring buffer     │
│  └── Rule: NO allocations, NO locks, NO blocking         │
│                                                          │
│  Thread 2: VoiceBridge processing loop (Tokio task)      │
│  ├── Priority: Normal                                    │
│  ├── Work: Read from ring buffer → process_frame()       │
│  │   └── signal processing, VAD, KWS, STT, biomarkers   │
│  ├── Yield: tokio::task::yield_now() between frames      │
│  └── Note: Currently polls. Alpha ships with polling.    │
│      Beta: event-driven via condvar/channel from Oboe.   │
│                                                          │
│  Thread 3: Oboe Audio Output Callback (real-time)        │
│  ├── Priority: AUDIO (highest)                           │
│  ├── Work: Read from output SPSC ring buffer → speaker   │
│  └── Rule: Same as input — zero allocation               │
│                                                          │
│  Thread 4: TTS Synthesis (spawn_blocking or dedicated)   │
│  ├── Priority: Normal                                    │
│  ├── Work: Android TTS JNI calls OR Piper synthesis      │
│  └── Writes PCM to output ring buffer                    │
│                                                          │
│  Main thread: Tokio runtime (daemon core)                │
│  ├── LLM processing, memory, response routing            │
│  └── Sends DaemonResponse to VoiceBridge                 │
└──────────────────────────────────────────────────────────┘
```

## 1.7 Power Budget

| Component | State | CPU Usage | Notes |
|-----------|-------|-----------|-------|
| Oboe input stream | Always | ~1% | Hardware-assisted, callback-driven |
| Signal processing | Always | ~2% | RNNoise per 30ms frame |
| Wake word KWS | WakeWordListening | ~3% | sherpa-onnx small model, 5MB |
| VAD | WakeWordListening (bg) | ~0.5% | Silero tiny, only energy pre-check |
| **Total idle** | **WakeWordListening** | **~6.5%** | **Acceptable for 24/7** |
| STT streaming | ActiveListening | ~15% | Zipformer, short bursts |
| STT batch | Processing | ~25% | Whisper, 1-3 seconds |
| TTS (Android) | Speaking | ~5% | Hardware-accelerated |
| TTS (Piper) | Speaking | ~20% | CPU VITS inference |
| LLM | Processing | ~80% | 3-15 seconds, dominates |

**Battery strategy:** Wake word listening is the only 24/7 cost. At ~6.5% single-core, this is ~1-2% of battery per hour on modern SoCs. Acceptable. If user enables "battery saver," disable wake word and require manual activation (tap-to-talk).

---

# Part 2: JNI Bridge Design — Android Built-in TTS

## 2.1 Design Philosophy

Android's built-in `TextToSpeech` is the **DEFAULT** TTS for AURA v4 because:
- **Free** — no model download, no 30MB Piper weight
- **Fast** — hardware-accelerated on most devices, <500ms first-word latency
- **Already on the phone** — zero setup cost
- **Mature** — supports 40+ languages, SSML, pitch/rate control

Piper VITS and eSpeak-NG remain as **OPTIONAL alternatives** for:
- Privacy absolutists who want zero Android API calls
- Custom voice personality (Piper can be fine-tuned)
- Offline-first when Android TTS engine requires network (some engines do)

## 2.2 Architecture: JNI Callback Pattern

Android `TextToSpeech` is an async Java API. You call `speak()` or `synthesizeToFile()` and get callbacks. The JNI bridge must handle this async nature from Rust.

**Two synthesis modes:**

1. **Direct playback** (Alpha) — Let Android TTS play through device speaker directly. Simplest. We lose AEC reference signal.
2. **PCM capture** (Beta) — Use `synthesizeToStream()` (API 21+) to capture PCM, feed through our output ring buffer. Enables AEC, mixing, volume control.

```
┌───────────────────────────────────────────────────────────┐
│                  JNI TTS BRIDGE                            │
│                                                           │
│  RUST SIDE                        │  KOTLIN/JAVA SIDE     │
│                                   │                       │
│  AndroidTts {                     │  AuraTtsEngine {      │
│    jni_env: *mut JNIEnv,          │    tts: TextToSpeech  │
│    tts_object: jobject,           │    callback: Callback │
│    state: AtomicU8,               │                       │
│    pcm_sender: Sender<Vec<i16>>,  │    onInit(status)     │
│  }                                │    onStart(uttId)     │
│                                   │    onDone(uttId)      │
│  fn speak(text, priority)         │    onError(uttId)     │
│    → JNI call to                  │                       │
│      AuraTtsEngine.speak()        │    synthesize(text)   │
│                                   │      → tts.speak()    │
│  fn synthesize_to_pcm(text)       │                       │
│    → JNI call to                  │    synthesizeToPcm()  │
│      AuraTtsEngine                │      → tts.synth      │
│      .synthesizeToPcm()           │        ToStream()     │
│    ← PCM data via callback        │      → write to       │
│      through pcm_sender           │        AudioTrack     │
│                                   │        OR callback    │
│  fn stop()                        │                       │
│    → JNI call to                  │    stop()             │
│      AuraTtsEngine.stop()         │      → tts.stop()     │
│                                   │                       │
│  fn set_params(pitch, rate, lang) │    setParams(...)     │
│    → JNI calls to                 │      → tts.setPitch() │
│      AuraTtsEngine.setParams()    │      → tts.setRate()  │
│                                   │      → tts.setLang()  │
│  fn is_ready() -> bool            │                       │
│    → check init state             │                       │
│                                   │                       │
│  // Callback from Kotlin:         │                       │
│  #[no_mangle]                     │                       │
│  extern "C" fn on_tts_done(       │                       │
│    env, class, utt_id             │    // Called from      │
│  )                                │    // UtteranceProgress│
│                                   │    // Listener         │
└───────────────────────────────────┴───────────────────────┘
```

## 2.3 Rust-Side Interface

```rust
// voice/android_tts.rs — NEW FILE

/// Android built-in TTS via JNI bridge.
/// This is the DEFAULT TTS engine for AURA v4 on Android.
/// 
/// Design:
/// - Alpha: direct playback mode (Android TTS plays through speaker)
/// - Beta: PCM capture mode (synthesizeToStream → our ring buffer → Oboe output)
///
/// Integration: Called by the existing TtsEngine wrapper in voice/tts.rs
/// as the primary backend, replacing Piper as the default.

use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;

/// TTS initialization state
#[repr(u8)]
enum AndroidTtsState {
    Uninitialized = 0,
    Initializing = 1,
    Ready = 2,
    Error = 3,
    Speaking = 4,
}

/// Parameters mapped from AURA's personality_voice system
pub struct AndroidTtsParams {
    /// Android TTS pitch: 0.25 (low) to 4.0 (high), 1.0 = normal
    /// Mapped from PersonalityVoice mood_hint
    pub pitch: f32,
    /// Android TTS speech rate: 0.25 (slow) to 4.0 (fast), 1.0 = normal
    pub rate: f32,
    /// BCP-47 language tag, e.g. "en-US"
    pub language: String,
}

pub struct AndroidTts {
    /// JNI global reference to the Kotlin AuraTtsEngine object
    /// Stored as raw pointer — valid for lifetime of the Android process
    tts_object: jni::objects::GlobalRef,

    /// Atomic state for lock-free status checks from audio thread
    state: Arc<AtomicU8>,

    /// Channel for receiving PCM data in Beta mode
    /// None in Alpha (direct playback) mode
    pcm_receiver: Option<tokio::sync::mpsc::UnboundedReceiver<Vec<i16>>>,

    /// Current utterance ID (monotonically increasing)
    utterance_counter: u64,

    /// Cached JNI method IDs for hot-path calls (avoid repeated lookups)
    method_speak: jni::objects::JMethodID,
    method_stop: jni::objects::JMethodID,
    method_set_params: jni::objects::JMethodID,
    method_synthesize_to_pcm: jni::objects::JMethodID,
}

impl AndroidTts {
    /// Initialize the Android TTS engine.
    /// Must be called on a thread with JNI env attached.
    /// The `tts_object` is created by Kotlin and passed via JNI.
    pub fn new(env: &mut jni::JNIEnv, tts_object: jni::objects::GlobalRef) -> Result<Self, VoiceError>;

    /// Speak text through device speaker (Alpha mode).
    /// Non-blocking: returns immediately, playback happens async.
    /// Priority levels match existing TtsPriority enum.
    pub fn speak(&mut self, text: &str, priority: TtsPriority) -> Result<u64, VoiceError>;

    /// Synthesize text to PCM samples (Beta mode).
    /// Returns a stream of i16 PCM chunks at the device's native sample rate.
    /// Caller must resample to match Oboe output stream rate if different.
    pub fn synthesize_to_pcm(&mut self, text: &str) -> Result<PcmStream, VoiceError>;

    /// Stop current speech immediately.
    /// Used for barge-in (wake word during Speaking state).
    pub fn stop(&mut self) -> Result<(), VoiceError>;

    /// Update TTS parameters from personality voice system.
    /// Called when mood_hint changes or user adjusts preferences.
    pub fn set_params(&mut self, params: AndroidTtsParams) -> Result<(), VoiceError>;

    /// Check if TTS engine is initialized and ready.
    /// Lock-free: reads atomic state.
    pub fn is_ready(&self) -> bool;

    /// Get the sample rate of the Android TTS engine output.
    /// Typically 22050 or 24000 Hz depending on the engine.
    pub fn output_sample_rate(&self) -> u32;
}

/// Stream of PCM chunks from Android TTS synthesis (Beta mode)
pub struct PcmStream {
    receiver: tokio::sync::mpsc::UnboundedReceiver<Vec<i16>>,
    sample_rate: u32,
    done: bool,
}

impl PcmStream {
    /// Get next chunk of PCM data. Returns None when synthesis is complete.
    pub async fn next_chunk(&mut self) -> Option<Vec<i16>>;
}

// === JNI Callback Entry Points ===
// These are called FROM Kotlin back into Rust when TTS events occur.

/// Called when TTS engine initialization completes.
/// Kotlin: AuraTtsEngine.onInit(status) → native call
#[no_mangle]
pub extern "C" fn Java_com_aura_daemon_AuraTtsEngine_onTtsInitComplete(
    env: jni::JNIEnv,
    _class: jni::objects::JClass,
    status: jni::sys::jint,
);

/// Called when an utterance starts playing.
#[no_mangle]
pub extern "C" fn Java_com_aura_daemon_AuraTtsEngine_onTtsUtteranceStart(
    env: jni::JNIEnv,
    _class: jni::objects::JClass,
    utterance_id: jni::objects::JString,
);

/// Called when an utterance finishes playing.
#[no_mangle]
pub extern "C" fn Java_com_aura_daemon_AuraTtsEngine_onTtsUtteranceDone(
    env: jni::JNIEnv,
    _class: jni::objects::JClass,
    utterance_id: jni::objects::JString,
);

/// Called when a TTS error occurs.
#[no_mangle]
pub extern "C" fn Java_com_aura_daemon_AuraTtsEngine_onTtsError(
    env: jni::JNIEnv,
    _class: jni::objects::JClass,
    utterance_id: jni::objects::JString,
    error_code: jni::sys::jint,
);

/// Called with PCM data chunks during synthesizeToStream (Beta mode).
#[no_mangle]
pub extern "C" fn Java_com_aura_daemon_AuraTtsEngine_onTtsPcmData(
    env: jni::JNIEnv,
    _class: jni::objects::JClass,
    pcm_data: jni::objects::JByteArray,
    sample_rate: jni::sys::jint,
);
```

## 2.4 Kotlin-Side Interface

```kotlin
// app/src/main/java/com/aura/daemon/AuraTtsEngine.kt — NEW FILE

package com.aura.daemon

import android.content.Context
import android.media.AudioAttributes
import android.os.Bundle
import android.speech.tts.TextToSpeech
import android.speech.tts.UtteranceProgressListener
import android.speech.tts.Voice
import java.util.Locale

/**
 * Kotlin wrapper around Android's built-in TextToSpeech.
 * Created by NativeBridge.init() and passed to Rust via JNI.
 *
 * Lifecycle:
 *   1. NativeBridge creates AuraTtsEngine in init()
 *   2. AuraTtsEngine.initialize() → TextToSpeech constructor (async)
 *   3. onInit callback → notify Rust via JNI
 *   4. Rust calls speak()/stop()/setParams() via JNI
 *   5. UtteranceProgressListener callbacks → notify Rust via JNI
 *   6. NativeBridge.shutdown() → AuraTtsEngine.destroy()
 */
class AuraTtsEngine(private val context: Context) {

    private var tts: TextToSpeech? = null
    private var isReady = false

    // === Native callbacks (implemented in Rust) ===
    private external fun onTtsInitComplete(status: Int)
    private external fun onTtsUtteranceStart(utteranceId: String)
    private external fun onTtsUtteranceDone(utteranceId: String)
    private external fun onTtsError(utteranceId: String, errorCode: Int)
    private external fun onTtsPcmData(pcmData: ByteArray, sampleRate: Int)

    /**
     * Initialize the Android TTS engine.
     * This is async — the engine is NOT ready until onInit fires.
     */
    fun initialize() {
        tts = TextToSpeech(context) { status ->
            isReady = (status == TextToSpeech.SUCCESS)
            if (isReady) {
                setupEngine()
            }
            onTtsInitComplete(status)
        }
    }

    private fun setupEngine() {
        tts?.let { engine ->
            // Set audio attributes for USAGE_ASSISTANT
            // This gives proper audio focus behavior
            val attrs = AudioAttributes.Builder()
                .setUsage(AudioAttributes.USAGE_ASSISTANT)
                .setContentType(AudioAttributes.CONTENT_TYPE_SPEECH)
                .build()
            engine.setAudioAttributes(attrs)

            // Set default language
            engine.language = Locale.US

            // Register utterance progress listener
            engine.setOnUtteranceProgressListener(object : UtteranceProgressListener() {
                override fun onStart(utteranceId: String) {
                    onTtsUtteranceStart(utteranceId)
                }
                override fun onDone(utteranceId: String) {
                    onTtsUtteranceDone(utteranceId)
                }
                @Deprecated("Deprecated in Java")
                override fun onError(utteranceId: String) {
                    onTtsError(utteranceId, -1)
                }
                override fun onError(utteranceId: String, errorCode: Int) {
                    onTtsError(utteranceId, errorCode)
                }
            })
        }
    }

    /**
     * Speak text through the device speaker.
     * Called from Rust via JNI.
     * @param text The text to speak
     * @param utteranceId Unique ID for tracking this utterance
     * @param queueMode QUEUE_FLUSH (interrupt) or QUEUE_ADD (append)
     */
    fun speak(text: String, utteranceId: String, queueMode: Int) {
        val params = Bundle().apply {
            putString(TextToSpeech.Engine.KEY_PARAM_UTTERANCE_ID, utteranceId)
            // Stream type is set via AudioAttributes in setupEngine()
        }
        tts?.speak(text, queueMode, params, utteranceId)
    }

    /**
     * Stop all current and queued speech.
     * Called from Rust for barge-in handling.
     */
    fun stop() {
        tts?.stop()
    }

    /**
     * Update TTS parameters.
     * @param pitch 0.25 to 4.0 (1.0 = normal)
     * @param rate 0.25 to 4.0 (1.0 = normal)
     * @param languageTag BCP-47 tag, e.g. "en-US"
     */
    fun setParams(pitch: Float, rate: Float, languageTag: String) {
        tts?.let { engine ->
            engine.setPitch(pitch)
            engine.setSpeechRate(rate)
            val locale = Locale.forLanguageTag(languageTag)
            engine.language = locale
        }
    }

    /**
     * Clean shutdown. Called from NativeBridge.shutdown().
     */
    fun destroy() {
        tts?.stop()
        tts?.shutdown()
        tts = null
        isReady = false
    }
}
```

## 2.5 Integration with Existing TTS Module

The existing `voice/tts.rs` has `TextToSpeech` struct with Piper + eSpeak backends. The Android built-in TTS slots in as the **primary** backend:

```
TextToSpeech (voice/tts.rs)
├── Backend priority (Alpha):
│   1. AndroidTts (default, free, fast)    ← NEW
│   2. PiperTts (optional, custom voice)
│   3. EspeakTts (fallback, tiny)
│
├── synthesize(text) → chooses backend:
│   if android_tts.is_ready():
│       android_tts.speak(text)            // Alpha: direct playback
│   elif piper_tts.is_loaded():
│       piper_tts.synthesize(text)         // PCM → ring buffer → Oboe
│   else:
│       espeak_tts.synthesize(text)        // PCM → ring buffer → Oboe
│
└── synthesize_streaming(text) → Beta:
    if android_tts.is_ready():
        android_tts.synthesize_to_pcm(text) // PCM stream → ring buffer
    else:
        piper_tts.synthesize_streaming(text)
```

## 2.6 Personality Voice Mapping

The existing `personality_voice.rs` maps mood_hint (0.0-1.0) to TTS parameters. For Android TTS:

| AURA mood_hint | Android pitch | Android rate | Feel |
|----------------|--------------|-------------|------|
| 0.0 (sad/low) | 0.85 | 0.85 | Slower, deeper |
| 0.3 (calm) | 0.95 | 0.92 | Slightly subdued |
| 0.5 (neutral) | 1.0 | 1.0 | Normal |
| 0.7 (engaged) | 1.05 | 1.05 | Slightly brighter |
| 1.0 (excited) | 1.15 | 1.12 | Brighter, faster |

Range is deliberately narrow (0.85-1.15) to avoid sounding robotic. Android TTS gets uncanny at extreme values.

---

# Part 3: JNI Bridge Design — Android Built-in STT (Optional)

## 3.1 Why Optional

AURA v4 already has a strong STT stack:
- **Zipformer streaming** (~30MB) — low latency, real-time
- **Whisper batch** (~75MB) — high accuracy, re-transcription

Android's `SpeechRecognizer` has drawbacks:
- Requires `RECORD_AUDIO` permission (we already have this via Oboe)
- Many implementations send audio to Google's cloud (violates anti-cloud iron law)
- Less control over VAD timing, utterance boundaries
- Can't extract biomarkers from the audio if Android consumes the mic

**Verdict:** Android STT is a **deferred Beta feature**, useful only as a fallback when on-device models aren't loaded (cold start, low memory). The primary STT path is always Zipformer + Whisper.

## 3.2 Rust-Side Interface (Deferred)

```rust
// voice/android_stt.rs — DEFERRED TO BETA

/// Android built-in STT via SpeechRecognizer JNI bridge.
/// OPTIONAL: Only used as fallback when on-device models aren't available.
/// 
/// WARNING: Many Android STT implementations are cloud-based.
/// AURA must check `SpeechRecognizer.isOnDeviceRecognitionAvailable()`
/// and refuse to use cloud-based recognition by default.

pub struct AndroidStt {
    /// JNI reference to Kotlin wrapper
    stt_object: jni::objects::GlobalRef,

    /// Whether on-device recognition is available
    is_on_device: bool,

    /// Channel for receiving transcription results
    result_receiver: tokio::sync::mpsc::UnboundedReceiver<SttResult>,
}

pub struct SttResult {
    pub text: String,
    pub confidence: f32,
    pub is_final: bool,
}

impl AndroidStt {
    /// Only initialize if on-device recognition is available.
    /// Returns Err if only cloud STT is available (anti-cloud policy).
    pub fn new_on_device_only(env: &mut jni::JNIEnv, context: jobject) -> Result<Self, VoiceError>;

    /// Start listening. Android STT takes control of the microphone.
    /// IMPORTANT: Must stop Oboe input stream first to avoid conflicts.
    pub fn start_listening(&mut self) -> Result<(), VoiceError>;

    /// Stop listening and get final result.
    pub fn stop_listening(&mut self) -> Result<(), VoiceError>;

    /// Get next partial or final result.
    pub async fn next_result(&mut self) -> Option<SttResult>;

    /// Check if on-device recognition is available.
    pub fn is_available(&self) -> bool;
}
```

## 3.3 Kotlin-Side Interface (Deferred)

```kotlin
// app/src/main/java/com/aura/daemon/AuraSttEngine.kt — DEFERRED TO BETA

package com.aura.daemon

import android.content.Context
import android.content.Intent
import android.os.Bundle
import android.speech.RecognitionListener
import android.speech.RecognizerIntent
import android.speech.SpeechRecognizer

/**
 * OPTIONAL fallback STT using Android's built-in SpeechRecognizer.
 * Only activated when:
 *   1. On-device recognition is available (checked at init)
 *   2. Zipformer/Whisper models are not loaded
 *   3. User hasn't disabled Android STT in preferences
 */
class AuraSttEngine(private val context: Context) {

    private var recognizer: SpeechRecognizer? = null
    private var isOnDevice = false

    // Native callbacks
    private external fun onSttResult(text: String, confidence: Float, isFinal: Boolean)
    private external fun onSttError(errorCode: Int)
    private external fun onSttReady()

    fun initialize(): Boolean {
        // CRITICAL: Check for on-device availability first
        isOnDevice = SpeechRecognizer.isOnDeviceRecognitionAvailable(context)
        if (!isOnDevice) {
            // Anti-cloud policy: refuse to initialize cloud STT
            return false
        }

        recognizer = SpeechRecognizer.createOnDeviceSpeechRecognizer(context)
        recognizer?.setRecognitionListener(object : RecognitionListener {
            override fun onResults(results: Bundle) {
                val matches = results.getStringArrayList(SpeechRecognizer.RESULTS_RECOGNITION)
                val confidences = results.getFloatArray(SpeechRecognizer.CONFIDENCE_SCORES)
                if (!matches.isNullOrEmpty()) {
                    onSttResult(matches[0], confidences?.get(0) ?: 0.8f, true)
                }
            }
            override fun onPartialResults(partialResults: Bundle) {
                val matches = partialResults.getStringArrayList(SpeechRecognizer.RESULTS_RECOGNITION)
                if (!matches.isNullOrEmpty()) {
                    onSttResult(matches[0], 0.5f, false)
                }
            }
            override fun onError(error: Int) { onSttError(error) }
            override fun onReadyForSpeech(params: Bundle) { onSttReady() }
            // ... other required overrides (no-op) ...
            override fun onBeginningOfSpeech() {}
            override fun onRmsChanged(rmsdB: Float) {}
            override fun onBufferReceived(buffer: ByteArray?) {}
            override fun onEndOfSpeech() {}
            override fun onEvent(eventType: Int, params: Bundle?) {}
        })
        return true
    }

    fun startListening() {
        val intent = Intent(RecognizerIntent.ACTION_RECOGNIZE_SPEECH).apply {
            putExtra(RecognizerIntent.EXTRA_LANGUAGE_MODEL, RecognizerIntent.LANGUAGE_MODEL_FREE_FORM)
            putExtra(RecognizerIntent.EXTRA_PARTIAL_RESULTS, true)
            putExtra(RecognizerIntent.EXTRA_MAX_RESULTS, 1)
            putExtra(RecognizerIntent.EXTRA_LANGUAGE, "en-US")
            // Request on-device only
            putExtra(RecognizerIntent.EXTRA_PREFER_OFFLINE, true)
        }
        recognizer?.startListening(intent)
    }

    fun stopListening() { recognizer?.stopListening() }
    fun destroy() { recognizer?.destroy(); recognizer = null }
}
```

---

# Part 4: Latency UX Specification

## 4.1 Honest Latency Budget

The user says "Hey AURA, what's the weather?" — here's what actually happens:

| Phase | Duration | Cumulative | What's Happening |
|-------|----------|------------|-----------------|
| Wake word detection | ~100ms | 100ms | sherpa-onnx KWS triggers |
| **Acknowledgment playback** | ~200ms | 300ms | "Mm-hm" plays (pre-cached) |
| User speaks | 1-5s | 1-5.3s | VAD tracks, STT streams |
| Silence detection | 500ms | 1.5-5.8s | VAD confirms utterance end |
| STT finalization | 200-800ms | 1.7-6.6s | Zipformer final + optional Whisper |
| Biomarker extraction | ~50ms | 1.75-6.65s | Parallel with STT finalize |
| **[GAP STARTS HERE]** | | | |
| LLM processing | 3-15s | 4.75-21.65s | On-device LLM inference |
| **Processing cue** | at +1.5s | | "Let me think..." if LLM > 1.5s |
| TTS synthesis | 300-800ms | 5.05-22.45s | Android TTS or Piper |
| **First word of response** | | 5-22s | **Honest total latency** |

**This is slow.** A 5-22 second response time is not conversational. We must be honest about this and design UX to make it tolerable, not pretend it's fast.

## 4.2 Pre-Cached Acknowledgment Phrases

Synthesized at app startup via Android TTS (or Piper), stored as PCM in memory. Played instantly on wake word detection.

```
Category: Wake Word Acknowledgment
Played: Immediately on wake word detection
Duration: 100-300ms each
Storage: Pre-synthesized PCM in memory (~50KB total)

Phrases (rotated randomly):
  - "Mm-hm"
  - "Yeah?"  
  - "I'm here"
  - "Listening"
  - [tone/chime option — 200ms sine sweep, no TTS needed]

Selection logic:
  - Default: chime (most universal, no language dependency)
  - If user prefers verbal: random from phrase list
  - Never repeat the same phrase twice in a row
  - Personality influence: mood_hint > 0.7 → more energetic variants
```

## 4.3 Processing Indicator Phrases

Played if LLM takes more than 1.5 seconds after STT completes.

```
Category: Processing Cue  
Trigger: 1.5 seconds after entering Processing state with no LLM response
Duration: 500-1000ms
Played: Maximum ONCE per turn

Phrases (rotated):
  - "Let me think about that..."
  - "Working on it..."
  - "Give me a moment..."
  - "Hmm, let me check..."
  - "One second..."

Selection logic:
  - Varies by query complexity (simple = "one second", complex = "let me think")
  - Personality influence: casual mood → "Hmm..." / formal → "One moment..."
  - If previous response was also slow → different phrase than last time

Implementation:
  - Timer starts when Processing state entered
  - Timer cancelled if LLM response arrives before 1.5s
  - If timer fires → synthesize & play cue phrase
  - Pre-cache top 2-3 phrases at startup for instant playback
  - Never play cue if user is in "quiet mode" preference
```

## 4.4 Timeout Handling

```
Timeout: Active Listening — No Speech
  Trigger: 8 seconds in ActiveListening with no speech detected by VAD
  Action:  
    - Play: "I didn't catch anything. Say 'Hey AURA' when you're ready."
    - Transition: → WakeWordListening
    - Rationale: User may have accidentally triggered wake word

Timeout: Active Listening — Max Duration
  Trigger: 30 seconds of continuous speech
  Action:
    - Play soft tone (don't interrupt, just signal)
    - Complete utterance processing with what we have
    - STT may lose accuracy on very long utterances anyway
    - Transition: → Processing with accumulated audio

Timeout: Processing — LLM Unresponsive
  Trigger: 30 seconds in Processing with no DaemonResponse
  Action:
    - Play: "I'm having trouble processing that. Could you try again?"
    - Log error: LLM timeout
    - Transition: → WakeWordListening
    - This likely means the LLM is stuck or crashed

Timeout: Speaking — TTS Error
  Trigger: TTS synthesis fails or takes > 10 seconds
  Action:
    - Fall back to next TTS backend (Android → Piper → eSpeak)
    - If all fail: play error tone
    - Transition: → WakeWordListening
```

## 4.5 Interruption (Barge-In) Handling

```
Scenario: User says "Hey AURA" while AURA is speaking

Detection:
  - Wake word KWS runs DURING Speaking state (already in the design)
  - AEC reduces AURA's own voice from the mic signal
  - KWS only triggers on clean signal after AEC

Action sequence:
  1. Immediately: stop TTS playback (AndroidTts.stop() or clear output ring buffer)
  2. Within 50ms: play acknowledgment ("Yeah?")
  3. Transition: Speaking → Acknowledging → ActiveListening
  4. Previous response is abandoned (not resumed)

Edge cases:
  - Double barge-in: user says "Hey AURA" twice rapidly → 2s cooldown prevents
  - Barge-in during ack: ignore (ack is only 200ms, too short to interrupt)
  - Barge-in during processing cue: stop cue, start new listening cycle

AEC quality note:
  In Alpha (Android TTS direct playback), we don't have the TTS output signal
  as AEC reference. This means barge-in detection during Speaking will be
  LESS RELIABLE in Alpha. This is a known limitation.
  Beta (PCM capture mode) fixes this by routing TTS through our output buffer.
```

## 4.6 Streaming LLM→TTS (Beta Feature)

```
Goal: Start speaking before the full LLM response is generated.
Target: Reduce perceived latency by 2-5 seconds.

Architecture (Beta):
  1. LLM generates tokens one at a time
  2. Sentence boundary detector accumulates tokens
  3. On first complete sentence: send to TTS immediately
  4. TTS synthesizes first sentence while LLM generates second
  5. Playback starts as soon as first sentence TTS completes

  LLM tokens → [sentence accumulator] → TTS queue → playback
                     "The weather"
                     "The weather in"
                     "The weather in Delhi"
                     "The weather in Delhi is" 
                     "The weather in Delhi is 34°C." ← SENTENCE COMPLETE
                         │
                         ▼
                     TTS.synthesize("The weather in Delhi is 34°C.")
                         │
                         ▼
                     Start playback (while LLM continues generating)

Sentence detection heuristic:
  - Period, exclamation, question mark followed by space or end
  - Minimum 4 words (don't flush single-word fragments)
  - Maximum 2 seconds accumulation (flush regardless for responsiveness)

Why Beta:
  - Requires LLM streaming support (current daemon sends complete response)
  - Requires TTS queue management (synthesize while playing)
  - Requires careful pause/resume between sentences
  - Alpha ships with complete-response-then-speak model
```

---

# Part 5: Proactive Voice Decision Matrix

## 5.1 When AURA Speaks Unprompted

AURA is not just a responder — it's a companion. There are situations where AURA should initiate speech. This requires careful design to avoid being annoying.

## 5.2 Decision Matrix

| Trigger | Priority | When to Speak | When to Stay Silent | Audio Focus |
|---------|----------|---------------|--------------------|----|
| **Urgent Notification** | Critical | Always (alarm, emergency, timer) | Never silent for Critical | `AUDIOFOCUS_GAIN_TRANSIENT_MAY_DUCK` |
| **Reminder** | High | User is idle (no active app audio), screen on or recently on | Phone call active, media playing, DND mode | `AUDIOFOCUS_GAIN_TRANSIENT` |
| **Proactive Insight** | Medium | User idle > 5 min, quiet environment, daytime hours | Late night (11pm-7am), DND, battery < 15%, user recently dismissed | `AUDIOFOCUS_GAIN_TRANSIENT_MAY_DUCK` |
| **Ambient Comment** | Low | User explicitly enabled "ambient mode", idle > 15 min | Default OFF, any active audio, any user interaction in last 5 min | `AUDIOFOCUS_GAIN_TRANSIENT_MAY_DUCK` |
| **Learning/Growth** | Low | After completing a conversation, natural pause | Never interrupt, only in gap after user interaction | None — use notification instead |

## 5.3 Proactive Voice Initiation Flow

```
ProactiveVoiceEvent arrives (from daemon core / scheduler / notifications)
    │
    ├── Check: Is user in a call? → BLOCK (always)
    │
    ├── Check: Is DND mode active? → BLOCK unless Critical priority
    │
    ├── Check: Is media playing?
    │   ├── Critical: Duck media volume, speak
    │   ├── High: Wait up to 30s for media pause, then duck
    │   ├── Medium/Low: BLOCK
    │   └── After speaking: restore media volume
    │
    ├── Check: Is it quiet hours? (user-configurable, default 11pm-7am)
    │   ├── Critical: Speak (lower volume: 40% of normal)
    │   ├── All others: BLOCK → queue for morning
    │   └── Morning delivery: "While you were sleeping..."
    │
    ├── Check: Battery level
    │   ├── < 15%: Only Critical and High
    │   ├── < 5%: Only Critical
    │   └── Normal: All priorities
    │
    ├── Check: User dismissal history
    │   ├── User dismissed last 3 proactive messages: BLOCK for 2 hours
    │   ├── User dismissed > 5 today: BLOCK proactive for rest of day
    │   └── Adaptive: reduce frequency based on dismissal rate
    │
    └── APPROVED → Request Audio Focus → Speak
```

## 5.4 Audio Focus Handling

```kotlin
// Integrated into AuraTtsEngine

fun requestAudioFocusAndSpeak(text: String, priority: VoicePriority) {
    val focusType = when (priority) {
        VoicePriority.CRITICAL -> AudioManager.AUDIOFOCUS_GAIN_TRANSIENT
        VoicePriority.HIGH -> AudioManager.AUDIOFOCUS_GAIN_TRANSIENT
        VoicePriority.NORMAL -> AudioManager.AUDIOFOCUS_GAIN_TRANSIENT_MAY_DUCK
        VoicePriority.LOW -> AudioManager.AUDIOFOCUS_GAIN_TRANSIENT_MAY_DUCK
    }

    val focusRequest = AudioFocusRequest.Builder(focusType)
        .setAudioAttributes(assistantAudioAttributes)
        .setAcceptsDelayedFocusGain(priority != VoicePriority.CRITICAL)
        .setOnAudioFocusChangeListener { focusChange ->
            when (focusChange) {
                AudioManager.AUDIOFOCUS_LOSS -> stop() // Another app took focus
                AudioManager.AUDIOFOCUS_LOSS_TRANSIENT -> pause()
                AudioManager.AUDIOFOCUS_GAIN -> resume()
            }
        }
        .build()

    val result = audioManager.requestAudioFocus(focusRequest)
    when (result) {
        AudioManager.AUDIOFOCUS_REQUEST_GRANTED -> speak(text)
        AudioManager.AUDIOFOCUS_REQUEST_DELAYED -> queueForLater(text)
        AudioManager.AUDIOFOCUS_REQUEST_FAILED -> {
            // Can't get focus — fall back to notification
            showNotificationInstead(text)
        }
    }
}
```

## 5.5 Volume and Timing Considerations

```
Volume:
  - Proactive speech: 70% of user-set media volume (never startling)
  - Urgent/Critical: 90% of user-set volume
  - Night mode (if somehow allowed): 40% of user-set volume
  - Fade in: 200ms ramp-up (never abrupt start)
  - Fade out: 100ms ramp-down

Timing:
  - Minimum gap between proactive utterances: 5 minutes
  - After user dismisses: 30 minute cooldown
  - Proactive messages queue if rapid-fire (max 3 in queue, oldest dropped)
  - Queue is flushed at next natural interaction point

Pre-speech cue:
  - Before any proactive speech: play a distinctive soft tone (300ms)
  - This gives user a moment to prepare / mute / dismiss
  - Different tone from wake word acknowledgment (users must distinguish)
  - Pattern: two soft notes, ascending (da-dum)
  
  Proactive tone: ♪♪ (da-dum, 300ms) → 200ms pause → "You have a reminder..."
  Wake word ack:  ♪ (single chime, 200ms) → immediate listening
```

## 5.6 User Preference Controls

```
Settings (stored in AURA's preference system):

voice_proactive_enabled: bool (default: true)
voice_proactive_priority_threshold: Priority (default: Medium)
voice_quiet_hours_start: TimeOfDay (default: 23:00)
voice_quiet_hours_end: TimeOfDay (default: 07:00)
voice_proactive_volume_percent: u8 (default: 70)
voice_ambient_mode: bool (default: false)
voice_max_proactive_per_hour: u8 (default: 4)
voice_dismissal_cooldown_minutes: u16 (default: 30)
```

---

# Part 6: Integration Map + Alpha vs Beta Scope Split

## 6.1 Integration Map

How the new voice pipeline components connect to existing infrastructure:

```
┌─────────────────────────────────────────────────────────────────┐
│                    EXISTING INFRASTRUCTURE                       │
│                                                                  │
│  main_loop.rs:1196-1203                                         │
│  ┌──────────────────────────────────┐                           │
│  │  let voice = VoiceEngine::new(); │ ← Add AndroidTts init    │
│  │  let bridge = VoiceBridge::new();│                           │
│  │  spawn_bridge(bridge);           │                           │
│  └──────────────────────────────────┘                           │
│           │                    ▲                                 │
│           │                    │ DaemonResponse { text, mood }   │
│           ▼                    │                                 │
│  ┌────────────────────┐  ┌─────────────────────┐                │
│  │  VoiceBridge        │  │  Response Router     │                │
│  │  voice_bridge.rs    │  │  (daemon_core)       │                │
│  │                     │  │                      │                │
│  │  ProcessedUtterance │──►  UserCommand::Chat   │                │
│  │  { text, biomarks } │  │  { text, metadata }  │                │
│  │                     │◄──  DaemonResponse      │                │
│  │  speak(response)    │  │  { text, mood_hint } │                │
│  └─────────┬───────────┘  └──────────────────────┘                │
│            │                                                      │
│            ▼                                                      │
│  ┌──────────────────────────────────────────────────────────┐    │
│  │  VoiceEngine (voice/mod.rs)                               │    │
│  │                                                           │    │
│  │  EXISTING:                    │  NEW ADDITIONS:            │    │
│  │  • process_frame() loop       │  • ack_playing flag        │    │
│  │  • speak() method             │  • processing_cue_timer    │    │
│  │  • modality state machine     │  • play_acknowledgment()   │    │
│  │  • signal processing          │  • play_processing_cue()   │    │
│  │  • wake word detection        │  • android_tts integration │    │
│  │  • VAD tracking               │  • proactive voice check   │    │
│  │  • STT transcription          │                            │    │
│  │  • biomarker extraction       │                            │    │
│  │  • personality voice          │                            │    │
│  └────────────┬──────────────────┴────────────────────────────┘    │
│               │                                                    │
│               ▼                                                    │
│  ┌────────────────────────────────────────────────────────────┐   │
│  │  TTS Backend Selection (voice/tts.rs — MODIFIED)           │   │
│  │                                                            │   │
│  │  EXISTING:              │  NEW:                             │   │
│  │  • PiperTts             │  • AndroidTts (DEFAULT)           │   │
│  │  • EspeakTts            │  • Backend priority selection     │   │
│  │  • TtsPriority enum     │  • Pre-cached ack phrases         │   │
│  │  • synthesize()         │  • Proactive voice scheduling     │   │
│  │  • synthesize_streaming │  │                                │   │
│  └────────────────────────┴────────────────────────────────────┘   │
│                                                                    │
│  ┌────────────────────────────────────────────────────────────┐   │
│  │  JNI Layer (lib.rs — EXTENDED)                             │   │
│  │                                                            │   │
│  │  EXISTING:              │  NEW:                             │   │
│  │  • NativeBridge.init()  │  • Pass AuraTtsEngine to Rust     │   │
│  │  • NativeBridge.run()   │  • TTS callback JNI entry points  │   │
│  │  • NativeBridge.shutdown│  • Audio focus management         │   │
│  │  • cancel flag          │  • (Beta: AuraSttEngine)          │   │
│  └────────────────────────┴────────────────────────────────────┘   │
│                                                                    │
│  ┌────────────────────────────────────────────────────────────┐   │
│  │  Kotlin App Layer (NativeBridge.kt — EXTENDED)             │   │
│  │                                                            │   │
│  │  EXISTING:              │  NEW:                             │   │
│  │  • init(context)        │  • Create AuraTtsEngine           │   │
│  │  • run()                │  • Pass to Rust via JNI           │   │
│  │  • shutdown()           │  • Audio focus request/abandon     │   │
│  │  • System.loadLibrary   │  • Proactive voice permissions     │   │
│  └────────────────────────┴────────────────────────────────────┘   │
└────────────────────────────────────────────────────────────────────┘
```

## 6.2 New Files Required

| File | Purpose | Priority |
|------|---------|----------|
| `voice/android_tts.rs` | Rust-side Android TTS JNI bridge | Alpha |
| `AuraTtsEngine.kt` | Kotlin Android TTS wrapper | Alpha |
| `voice/ack_cache.rs` | Pre-cached acknowledgment audio management | Alpha |
| `voice/proactive_voice.rs` | Proactive voice decision engine | Beta |
| `voice/android_stt.rs` | Rust-side Android STT JNI bridge | Beta |
| `AuraSttEngine.kt` | Kotlin Android STT wrapper | Beta |
| `voice/streaming_tts.rs` | LLM streaming → sentence → TTS pipeline | Beta |

## 6.3 Modified Files

| File | Changes | Priority |
|------|---------|----------|
| `voice/mod.rs` | Add ack_playing, processing_cue_timer, play_acknowledgment(), play_processing_cue(), integrate AndroidTts | Alpha |
| `voice/tts.rs` | Add AndroidTts as primary backend, backend selection logic | Alpha |
| `lib.rs` | Add TTS callback JNI entry points | Alpha |
| `bridge/voice_bridge.rs` | Add proactive voice event handling, processing cue timer | Alpha |
| `voice/personality_voice.rs` | Add Android TTS parameter mapping (pitch/rate) | Alpha |
| `voice/modality_state_machine.rs` | No changes needed (sub-states live in VoiceEngine) | — |

## 6.4 Alpha vs Beta Scope Split

### Alpha (Ship Now)

**Core "Hey AURA" → Listen → Think → Speak loop:**

| Feature | Details | Complexity |
|---------|---------|------------|
| Android built-in TTS (direct playback) | `AuraTtsEngine.kt` + `android_tts.rs`, speak() mode only | Medium |
| Wake word acknowledgment | Pre-cache 3-5 phrases at startup, play on detection | Low |
| Processing cue | 1.5s timer, play "Let me think..." if LLM slow | Low |
| Timeout handling | All 4 timeout scenarios (no-speech, max-duration, LLM, TTS) | Low |
| Barge-in (basic) | Wake word during Speaking → stop TTS, restart listen | Medium |
| Backend priority | Android TTS > Piper > eSpeak fallback chain | Low |
| Personality voice → Android TTS | mood_hint → pitch/rate mapping | Low |
| Power management | Disable wake word on screen lock if battery saver | Low |

**Alpha limitations (known, accepted):**
- No AEC during TTS playback (Android plays directly, we don't have reference signal)
- Barge-in less reliable during speaking (no AEC reference)
- No streaming LLM→TTS (full response, then speak)
- No proactive voice initiation (respond only)
- Polling-based frame processing (not event-driven)

### Beta (Next Phase)

| Feature | Details | Complexity |
|---------|---------|------------|
| PCM capture mode | `synthesizeToStream()` → ring buffer → Oboe, enables AEC during TTS | High |
| Streaming LLM→TTS | Sentence boundary detection, queue-based TTS during generation | High |
| Proactive voice | Decision matrix, audio focus, quiet hours, dismissal tracking | Medium |
| Android STT fallback | On-device only, cold-start fallback | Medium |
| Event-driven frame processing | Condvar/channel from Oboe callback → VoiceBridge | Medium |
| Voice persona selection | Multiple pre-configured Android TTS voices | Low |
| Multi-language | Language detection → switch Android TTS language | Medium |

### Deferred (Future)

| Feature | Details | Rationale |
|---------|---------|-----------|
| Custom wake word | User-defined wake phrases | Requires KWS retraining |
| Voice cloning | Clone user-preferred voice for TTS | Privacy + compute intensive |
| Continuous conversation | No wake word needed after first interaction | Complex UX, battery cost |
| Whisper word-level timestamps | Align biomarkers to specific words | Not needed for MVP |
| Multi-speaker detection | Distinguish owner from others | Requires speaker embeddings |

## 6.5 Dependency Graph for Implementation

```
Alpha implementation order:

1. AuraTtsEngine.kt + android_tts.rs (JNI bridge)
   └── No dependencies, standalone
   
2. voice/ack_cache.rs (pre-cached ack phrases)
   └── Depends on: (1) TTS bridge working
   
3. voice/tts.rs modifications (backend priority)
   └── Depends on: (1) AndroidTts struct
   
4. voice/mod.rs enhancements (ack_playing, cue timer)
   └── Depends on: (2) ack cache, (3) TTS backend
   
5. lib.rs JNI entry points
   └── Depends on: (1) callback function signatures
   
6. bridge/voice_bridge.rs (processing cue timer)
   └── Depends on: (4) VoiceEngine changes
   
7. voice/personality_voice.rs (Android TTS params)
   └── Depends on: (1) AndroidTts.set_params()
   
8. Integration testing
   └── Depends on: all above
   
Parallelizable: (1) and (2) can start together
                (3) and (5) can proceed in parallel after (1)
                (7) is independent after (1)
```

## 6.6 Memory Impact

```
Alpha additions to voice pipeline memory:

Pre-cached ack phrases:
  5 phrases × ~10KB PCM each = ~50KB

AndroidTts object:
  JNI references + state = ~1KB
  (Android TTS engine memory is managed by Android, not us)

Processing cue phrases:
  3 phrases × ~20KB PCM each = ~60KB

Total Alpha addition: ~111KB
(Negligible vs existing ~143MB voice pipeline)

Beta additions:
  PCM capture buffer: ~192KB (2s at 48kHz stereo)
  Sentence accumulator: ~4KB
  Proactive voice queue: ~10KB
  
Total Beta addition: ~206KB
```

## 6.7 Risk Assessment

| Risk | Likelihood | Impact | Mitigation |
|------|-----------|--------|------------|
| Android TTS not initialized when first wake word fires | Medium | User gets no response | Fall back to Piper/eSpeak, pre-init TTS at app start |
| JNI callback from TTS arrives on wrong thread | High | Crash or deadlock | Use `GlobalRef`, always check JNI env attachment |
| AEC ineffective without TTS reference signal (Alpha) | Certain | Barge-in unreliable during speaking | Document limitation, fix in Beta with PCM capture |
| User perceives 5-22s latency as broken | High | User abandons voice | Ack + processing cue buy time, set expectations in onboarding |
| Wake word false positives drain battery | Medium | User trust erodes | 2s cooldown, sensitivity tuning, "Hey AURA" (2-word) preferred |
| Android TTS quality varies by device/manufacturer | Medium | Inconsistent experience | Test on major OEMs, fall back to Piper if quality too low |

---

## Appendix A: Glossary

| Term | Definition |
|------|-----------|
| KWS | Keyword Spotting — wake word detection via sherpa-onnx |
| VAD | Voice Activity Detection — Silero ONNX model |
| STT | Speech-to-Text — Zipformer (streaming) + Whisper (batch) |
| TTS | Text-to-Speech — Android built-in (default) + Piper + eSpeak |
| AEC | Acoustic Echo Cancellation — removes AURA's own voice from mic |
| SPSC | Single-Producer Single-Consumer lock-free ring buffer |
| Barge-in | User interrupts AURA while it's speaking |
| Ack | Acknowledgment — brief audio response to wake word |
| PCM | Pulse Code Modulation — raw audio samples (i16, 16-bit) |
| JNI | Java Native Interface — Rust ↔ Kotlin bridge layer |

## Appendix B: Related Files Quick Reference

| File | Path | Lines |
|------|------|-------|
| VoiceEngine | `crates/aura-daemon/src/voice/mod.rs` | 606 |
| TTS | `crates/aura-daemon/src/voice/tts.rs` | 544 |
| STT | `crates/aura-daemon/src/voice/stt.rs` | 518 |
| VAD | `crates/aura-daemon/src/voice/vad.rs` | 467 |
| Audio I/O | `crates/aura-daemon/src/voice/audio_io.rs` | 450 |
| Wake Word | `crates/aura-daemon/src/voice/wake_word.rs` | 303 |
| Signal Processing | `crates/aura-daemon/src/voice/signal_processing.rs` | 363 |
| Modality SM | `crates/aura-daemon/src/voice/modality_state_machine.rs` | 381 |
| Personality Voice | `crates/aura-daemon/src/voice/personality_voice.rs` | 322 |
| Voice Bridge | `crates/aura-daemon/src/bridge/voice_bridge.rs` | 277 |
| JNI Entry | `crates/aura-daemon/src/lib.rs` | 208 |
| Main Loop | `crates/aura-daemon/src/daemon_core/main_loop.rs` | ~1229 |
| Channels | `crates/aura-daemon/src/daemon_core/channels.rs` | ~149 |
