//! aura-llama-sys: FFI bindings to llama.cpp and GGUF metadata parser.
//!
//! This crate provides the interface between AURA's Rust inference engine and
//! the llama.cpp C library, plus a pure-Rust GGUF header parser that reads
//! model capabilities without loading weights.
//!
//! # Architecture
//!
//! - **Android (ARM64)**: Runtime dynamic loading of `libllama.so` via `libloading`. The shared
//!   library is expected at a path supplied by the caller (typically extracted from the APK's
//!   native libs directory).
//! - **Desktop (host builds)**: A smart stub that generates plausible English text using a simple
//!   bigram model. This lets the full 6-layer teacher stack exercise real code paths during
//!   development and testing.
//!
//! # Safety
//!
//! All FFI calls go through the `LlamaBackend` trait. On Android, the `FfiBackend`
//! implementation uses raw pointers obtained from `libloading`. On desktop, the
//! `StubBackend` implementation uses safe Rust only.

// FFI crate: all unsafe blocks are documented with SAFETY comments.
// This crate requires unsafe for llama.cpp C FFI bindings.
#![allow(unsafe_code)]

pub mod gguf_meta;
pub mod server_http_backend;
use std::{collections::HashMap, sync::Mutex};

pub use gguf_meta::{parse_from_reader, parse_gguf_meta, GgufError, GgufMeta};
use serde::{Deserialize, Serialize};
#[allow(unused_imports)]
use tracing::{debug, error, info, trace, warn};

// ─── Core types ─────────────────────────────────────────────────────────────

/// Opaque model handle returned by llama_load_model_from_file.
#[repr(C)]
pub struct LlamaModel {
    _opaque: [u8; 0],
}

/// Opaque context handle returned by llama_new_context_with_model.
#[repr(C)]
pub struct LlamaContext {
    _opaque: [u8; 0],
}

/// Token ID type — matches llama.cpp's llama_token (i32).
pub type LlamaToken = i32;

/// Special token IDs used by the stub backend.
/// Real llama.cpp provides these from the loaded model vocabulary.
pub mod special_tokens {
    use super::LlamaToken;

    /// Beginning of sequence.
    pub const BOS: LlamaToken = 1;
    /// End of sequence.
    pub const EOS: LlamaToken = 2;
    /// Unknown token.
    pub const UNK: LlamaToken = 0;
}

/// Model parameters for loading.
#[repr(C)]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlamaModelParams {
    /// Number of layers to offload to GPU (0 = CPU only).
    pub n_gpu_layers: i32,
    /// Use memory-mapped files for model loading.
    pub use_mmap: bool,
    /// Lock model in RAM (prevent swapping).
    pub use_mlock: bool,
}

impl Default for LlamaModelParams {
    fn default() -> Self {
        Self {
            n_gpu_layers: 0,
            use_mmap: true,
            use_mlock: false,
        }
    }
}

/// Context parameters for inference.
#[repr(C)]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlamaContextParams {
    /// Context window size (in tokens).
    pub n_ctx: u32,
    /// Batch size for prompt processing.
    pub n_batch: u32,
    /// Number of threads for generation.
    pub n_threads: u32,
    /// Random seed.
    pub seed: u32,
}

impl Default for LlamaContextParams {
    fn default() -> Self {
        Self {
            n_ctx: 2048,
            n_batch: 512,
            n_threads: 4,
            seed: 0xA0BA,
        }
    }
}

/// Sampling parameters for token generation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SamplingParams {
    /// Temperature for softmax sampling (0.0 = greedy, higher = more random).
    pub temperature: f32,
    /// Top-p (nucleus) sampling cutoff.
    pub top_p: f32,
    /// Top-k sampling cutoff.
    pub top_k: i32,
    /// Repetition penalty (1.0 = no penalty).
    pub repeat_penalty: f32,
    /// Maximum number of tokens to generate.
    pub max_tokens: u32,
    /// Optional compiled GBNF grammar for constrained decoding (Layer 0).
    ///
    /// When set, the grammar masks invalid tokens BEFORE temperature/sampling,
    /// ensuring every generated token is grammar-valid. This replaces the
    /// post-hoc 0.7× confidence penalty with hard structural enforcement.
    ///
    /// The string is a compiled GBNF grammar (see `grammar.rs` for definitions).
    /// On the FfiBackend, this is passed to `llama_sampler_init_grammar()`.
    /// On the StubBackend, this field is ignored (no-op).
    pub grammar_gbnf: Option<String>,
}

impl Default for SamplingParams {
    fn default() -> Self {
        Self {
            temperature: 0.6,
            top_p: 0.9,
            top_k: 40,
            repeat_penalty: 1.1,
            max_tokens: 512,
            grammar_gbnf: None,
        }
    }
}

// ─── Backend trait ──────────────────────────────────────────────────────────

/// Result type for all backend operations.
pub type BackendResult<T> = Result<T, BackendError>;

/// Errors from the llama backend.
#[derive(Debug, thiserror::Error)]
pub enum BackendError {
    /// Failed to load the shared library.
    #[error("library load failed: {0}")]
    LibraryLoad(String),
    /// Failed to resolve a symbol in the shared library.
    #[error("symbol resolution failed: {0}")]
    SymbolResolution(String),
    /// Model file not found or failed to load.
    #[error("model load failed: {0}")]
    ModelLoad(String),
    /// Context creation failed.
    #[error("context creation failed: {0}")]
    ContextCreation(String),
    /// Tokenization error.
    #[error("tokenization failed: {0}")]
    Tokenization(String),
    /// Token generation error.
    #[error("generation failed: {0}")]
    Generation(String),
    /// Detokenization error.
    #[error("detokenization failed: {0}")]
    Detokenization(String),
    /// Backend is in stub mode — not a real error, but callers may want to know.
    #[error("stub backend: {0}")]
    StubMode(String),
}

/// Unified backend interface for both real FFI and stub implementations.
///
/// This trait abstracts the llama.cpp operations so that the inference engine
/// can work identically on Android (real model) and desktop (stub model).
pub trait LlamaBackend: Send + Sync {
    /// Returns `true` if this is a stub/testing backend.
    fn is_stub(&self) -> bool;

    /// Load a model from a GGUF file. Returns opaque model and context pointers.
    ///
    /// # Safety
    /// The returned pointers are only valid until `free_model()` is called.
    fn load_model(
        &self,
        path: &str,
        model_params: &LlamaModelParams,
        ctx_params: &LlamaContextParams,
    ) -> BackendResult<(*mut LlamaModel, *mut LlamaContext)>;

    /// Free a previously loaded model and its context.
    ///
    /// # Safety
    /// Must only be called with pointers returned by `load_model()`.
    fn free_model(&self, model: *mut LlamaModel, ctx: *mut LlamaContext);

    /// Tokenize text into token IDs.
    fn tokenize(&self, ctx: *mut LlamaContext, text: &str) -> BackendResult<Vec<LlamaToken>>;

    /// Decode token IDs back to text.
    fn detokenize(&self, ctx: *mut LlamaContext, tokens: &[LlamaToken]) -> BackendResult<String>;

    /// Generate the next token given context of previous tokens and sampling parameters.
    ///
    /// This is the core inference step. On Android it calls `llama_decode` + sampling.
    /// On desktop stubs it uses a bigram model to produce plausible tokens.
    fn sample_next(
        &self,
        ctx: *mut LlamaContext,
        tokens: &[LlamaToken],
        params: &SamplingParams,
    ) -> BackendResult<LlamaToken>;

    /// Get the EOS (end-of-sequence) token ID for the loaded model.
    fn eos_token(&self) -> LlamaToken;

    /// Get the BOS (beginning-of-sequence) token ID for the loaded model.
    fn bos_token(&self) -> LlamaToken;

    /// Evaluate/process a batch of tokens (prompt ingestion).
    ///
    /// Must be called before `sample_next()` to feed the prompt.
    fn eval(&self, ctx: *mut LlamaContext, tokens: &[LlamaToken]) -> BackendResult<()>;

    /// Get the log-probability of a specific token given the current logits state.
    ///
    /// Must be called after `sample_next()` or `eval()` while the logits buffer
    /// is still valid (before the next decode call).
    ///
    /// Computes `log_softmax(logit[token])` = `logit[token] - log(sum(exp(logits)))`.
    ///
    /// Returns `None` on stub backends or if logits are unavailable.
    fn get_token_logprob(&self, ctx: *mut LlamaContext, token: LlamaToken) -> Option<f32>;

    // ── Grammar-constrained decoding (Layer 0) ─────────────────────────

    /// Activate a GBNF grammar for constrained decoding.
    ///
    /// When active, `sample_next()` masks invalid tokens BEFORE temperature
    /// and sampling, enforcing structural correctness at the token level.
    /// No-op on stub backends.
    fn set_grammar(&self, _grammar_gbnf: &str) -> BackendResult<()> {
        Ok(()) // Default: no-op (stub backends)
    }

    /// Clear the active grammar sampler, freeing its resources.
    /// Must be called after generation completes.
    fn clear_grammar(&self) {
        // Default: no-op (stub backends)
    }

    /// Notify the grammar that a token was accepted, advancing its state.
    /// Must be called after each successful `sample_next()` when grammar is active.
    fn accept_grammar_token(&self, _token: LlamaToken) {
        // Default: no-op (stub backends)
    }
}

// ─── Stub backend (desktop / host builds) ───────────────────────────────────

/// A simple bigram-based text generator for desktop testing.
///
/// Instead of returning empty strings or zeros, this generates plausible
/// English-like output so the full inference pipeline can be exercised.
///
/// The stub uses a vocabulary of common words and bigram transitions built
/// from a small hardcoded corpus.
pub struct StubBackend {
    /// Bigram transition table: given a token, what tokens can follow?
    bigrams: HashMap<LlamaToken, Vec<(LlamaToken, f32)>>,
    /// Vocabulary: token ID → word string.
    vocab: Vec<String>,
    /// Reverse vocabulary: word → token ID.
    word_to_token: HashMap<String, LlamaToken>,
    /// RNG state (simple xorshift for reproducibility).
    rng_state: Mutex<u64>,
}

/// Build the stub vocabulary and bigram transition tables.
/// Returns (vocab, word_to_token, bigrams) for the StubBackend.
#[allow(clippy::type_complexity)]
fn build_stub_bigrams() -> (
    Vec<String>,
    HashMap<String, LlamaToken>,
    HashMap<LlamaToken, Vec<(LlamaToken, f32)>>,
) {
    let words: Vec<&str> = vec![
        "<s>",       // 0
        "</s>",      // 1
        "<unk>",     // 2
        "the",       // 3
        "I",         // 4
        "will",      // 5
        "to",        // 6
        "a",         // 7
        "and",       // 8
        "is",        // 9
        "open",      // 10
        "tap",       // 11
        "on",        // 12
        "button",    // 13
        "navigate",  // 14
        "screen",    // 15
        "then",      // 16
        "click",     // 17
        "scroll",    // 18
        "down",      // 19
        "find",      // 20
        "select",    // 21
        "option",    // 22
        "enable",    // 23
        "disable",   // 24
        "toggle",    // 25
        "menu",      // 26
        "search",    // 27
        "for",       // 28
        "type",      // 29
        "text",      // 30
        "in",        // 31
        "field",     // 32
        "wait",      // 33
        "until",     // 34
        "appears",   // 35
        "confirm",   // 36
        "action",    // 37
        "done",      // 38
        "next",      // 39
        "back",      // 40
        "home",      // 41
        "wifi",      // 42
        "bluetooth", // 43
        "display",   // 44
        "this",      // 45
        "plan",      // 46
        "step",      // 47
        "first",     // 48
        "second",    // 49
        "ok",        // 50
        "yes",       // 51
        "no",        // 52
        "let",       // 53
        "me",        // 54
        "help",      // 55
        "you",       // 56
        "with",      // 57
        "that",      // 58
        "sure",      // 59
        "here",      // 60
        "go",        // 61
        "check",     // 62
        "settings",  // 63
        "app",       // 64
        "it",        // 65
        "off",       // 66
        "if",        // 67
    ];

    let vocab: Vec<String> = words.iter().map(|w| w.to_string()).collect();
    let mut word_to_token = HashMap::new();
    for (i, w) in words.iter().enumerate() {
        word_to_token.insert(w.to_string(), i as LlamaToken);
    }

    // Build bigram transitions (word flows that make sense for AURA's domain)
    let mut bigrams: HashMap<LlamaToken, Vec<(LlamaToken, f32)>> = HashMap::new();

    // Helper closure to add transitions
    let mut add = |from: &str, transitions: &[(&str, f32)]| {
        if let Some(&from_id) = word_to_token.get(from) {
            let entries: Vec<(LlamaToken, f32)> = transitions
                .iter()
                .filter_map(|(to, prob)| word_to_token.get(*to).map(|&to_id| (to_id, *prob)))
                .collect();
            bigrams.insert(from_id, entries);
        }
    };

    // Domain-specific bigram transitions
    add(
        "<s>",
        &[
            ("I", 0.3),
            ("the", 0.1),
            ("first", 0.15),
            ("let", 0.15),
            ("sure", 0.1),
            ("ok", 0.05),
            ("open", 0.15),
        ],
    );
    add("I", &[("will", 0.7), ("can", 0.1), ("open", 0.2)]); // "can" not in vocab, will be filtered
    add(
        "will",
        &[
            ("open", 0.3),
            ("navigate", 0.2),
            ("tap", 0.15),
            ("scroll", 0.1),
            ("find", 0.1),
            ("select", 0.1),
            ("check", 0.05),
        ],
    );
    add(
        "open",
        &[("the", 0.4), ("settings", 0.3), ("app", 0.2), ("menu", 0.1)],
    );
    add(
        "the",
        &[
            ("settings", 0.2),
            ("app", 0.15),
            ("button", 0.1),
            ("screen", 0.1),
            ("menu", 0.1),
            ("option", 0.1),
            ("text", 0.1),
            ("home", 0.05),
            ("display", 0.05),
            ("search", 0.05),
        ],
    );
    add(
        "settings",
        &[
            ("app", 0.3),
            ("screen", 0.2),
            ("menu", 0.2),
            ("and", 0.15),
            ("</s>", 0.15),
        ],
    );
    add(
        "app",
        &[
            ("and", 0.3),
            ("settings", 0.2),
            ("</s>", 0.2),
            ("then", 0.15),
            ("screen", 0.15),
        ],
    );
    add(
        "and",
        &[
            ("tap", 0.2),
            ("click", 0.15),
            ("select", 0.15),
            ("scroll", 0.1),
            ("navigate", 0.1),
            ("find", 0.1),
            ("enable", 0.1),
            ("confirm", 0.1),
        ],
    );
    add("tap", &[("on", 0.5), ("the", 0.3), ("button", 0.2)]);
    add(
        "on",
        &[
            ("the", 0.4),
            ("button", 0.2),
            ("screen", 0.15),
            ("wifi", 0.1),
            ("bluetooth", 0.05),
            ("display", 0.05),
            ("a", 0.05),
        ],
    );
    add(
        "button",
        &[
            ("and", 0.2),
            ("to", 0.2),
            ("then", 0.2),
            ("</s>", 0.2),
            ("on", 0.1),
            ("in", 0.1),
        ],
    );
    add("navigate", &[("to", 0.7), ("back", 0.2), ("home", 0.1)]);
    add(
        "to",
        &[
            ("the", 0.3),
            ("settings", 0.15),
            ("home", 0.1),
            ("find", 0.1),
            ("open", 0.1),
            ("confirm", 0.1),
            ("a", 0.05),
            ("navigate", 0.05),
            ("select", 0.05),
        ],
    );
    add(
        "screen",
        &[
            ("and", 0.3),
            ("then", 0.2),
            ("</s>", 0.2),
            ("to", 0.15),
            ("with", 0.15),
        ],
    );
    add(
        "then",
        &[
            ("tap", 0.2),
            ("click", 0.15),
            ("select", 0.15),
            ("scroll", 0.1),
            ("navigate", 0.1),
            ("find", 0.1),
            ("wait", 0.1),
            ("confirm", 0.1),
        ],
    );
    add("click", &[("on", 0.5), ("the", 0.3), ("button", 0.2)]);
    add("scroll", &[("down", 0.6), ("to", 0.2), ("until", 0.2)]);
    add(
        "down",
        &[("to", 0.3), ("and", 0.3), ("until", 0.2), ("</s>", 0.2)],
    );
    add(
        "find",
        &[
            ("the", 0.4),
            ("a", 0.2),
            ("settings", 0.15),
            ("wifi", 0.1),
            ("bluetooth", 0.05),
            ("option", 0.1),
        ],
    );
    add(
        "select",
        &[
            ("the", 0.3),
            ("a", 0.2),
            ("option", 0.2),
            ("wifi", 0.1),
            ("bluetooth", 0.1),
            ("it", 0.1),
        ],
    );
    add(
        "option",
        &[
            ("and", 0.3),
            ("then", 0.2),
            ("to", 0.15),
            ("</s>", 0.2),
            ("in", 0.15),
        ],
    );
    add(
        "enable",
        &[
            ("wifi", 0.2),
            ("bluetooth", 0.2),
            ("the", 0.2),
            ("it", 0.2),
            ("toggle", 0.2),
        ],
    );
    add(
        "disable",
        &[
            ("wifi", 0.2),
            ("bluetooth", 0.2),
            ("the", 0.2),
            ("it", 0.2),
            ("toggle", 0.2),
        ],
    );
    add(
        "toggle",
        &[
            ("wifi", 0.2),
            ("bluetooth", 0.2),
            ("the", 0.2),
            ("on", 0.2),
            ("off", 0.2),
        ],
    );
    add(
        "menu",
        &[
            ("and", 0.3),
            ("button", 0.2),
            ("option", 0.2),
            ("</s>", 0.15),
            ("screen", 0.15),
        ],
    );
    add("search", &[("for", 0.6), ("the", 0.2), ("in", 0.2)]);
    add(
        "for",
        &[
            ("the", 0.3),
            ("a", 0.2),
            ("wifi", 0.1),
            ("settings", 0.15),
            ("bluetooth", 0.1),
            ("it", 0.15),
        ],
    );
    add(
        "type",
        &[("text", 0.3), ("in", 0.3), ("the", 0.2), ("a", 0.2)],
    );
    add(
        "text",
        &[("in", 0.3), ("field", 0.3), ("and", 0.2), ("</s>", 0.2)],
    );
    add(
        "in",
        &[
            ("the", 0.4),
            ("a", 0.2),
            ("settings", 0.15),
            ("field", 0.15),
            ("search", 0.1),
        ],
    );
    add(
        "field",
        &[("and", 0.3), ("then", 0.2), ("</s>", 0.3), ("to", 0.2)],
    );
    add("wait", &[("until", 0.5), ("for", 0.3), ("a", 0.2)]);
    add(
        "until",
        &[("the", 0.3), ("it", 0.3), ("a", 0.2), ("done", 0.2)],
    );
    add(
        "appears",
        &[("and", 0.3), ("then", 0.3), ("on", 0.2), ("</s>", 0.2)],
    );
    add(
        "confirm",
        &[("the", 0.3), ("action", 0.3), ("and", 0.2), ("</s>", 0.2)],
    );
    add(
        "action",
        &[
            ("and", 0.3),
            ("then", 0.2),
            ("is", 0.2),
            ("done", 0.15),
            ("</s>", 0.15),
        ],
    );
    add(
        "done",
        &[("</s>", 0.5), ("and", 0.2), ("then", 0.15), ("next", 0.15)],
    );
    add(
        "next",
        &[
            ("step", 0.3),
            ("screen", 0.2),
            ("button", 0.2),
            ("option", 0.15),
            ("is", 0.15),
        ],
    );
    add(
        "back",
        &[("to", 0.4), ("and", 0.2), ("button", 0.2), ("home", 0.2)],
    );
    add(
        "home",
        &[
            ("screen", 0.4),
            ("button", 0.2),
            ("and", 0.2),
            ("</s>", 0.2),
        ],
    );
    add(
        "wifi",
        &[
            ("settings", 0.2),
            ("option", 0.2),
            ("toggle", 0.15),
            ("and", 0.15),
            ("</s>", 0.15),
            ("on", 0.15),
        ],
    );
    add(
        "bluetooth",
        &[
            ("settings", 0.2),
            ("option", 0.2),
            ("toggle", 0.15),
            ("and", 0.15),
            ("</s>", 0.15),
            ("on", 0.15),
        ],
    );
    add(
        "display",
        &[
            ("settings", 0.3),
            ("screen", 0.3),
            ("option", 0.2),
            ("</s>", 0.2),
        ],
    );
    add(
        "a",
        &[
            ("button", 0.15),
            ("screen", 0.1),
            ("menu", 0.1),
            ("text", 0.1),
            ("field", 0.1),
            ("search", 0.1),
            ("plan", 0.1),
            ("step", 0.1),
            ("option", 0.15),
        ],
    );
    add(
        "this",
        &[
            ("is", 0.5),
            ("screen", 0.2),
            ("option", 0.15),
            ("step", 0.15),
        ],
    );
    add(
        "is",
        &[
            ("the", 0.2),
            ("a", 0.2),
            ("done", 0.15),
            ("here", 0.15),
            ("to", 0.15),
            ("on", 0.15),
        ],
    );
    add(
        "plan",
        &[("is", 0.3), ("to", 0.3), ("step", 0.2), ("</s>", 0.2)],
    );
    add(
        "step",
        &[
            ("is", 0.2),
            ("to", 0.2),
            ("open", 0.15),
            ("navigate", 0.15),
            ("tap", 0.15),
            ("find", 0.15),
        ],
    );
    add(
        "first",
        &[
            ("open", 0.2),
            ("navigate", 0.2),
            ("I", 0.15),
            ("tap", 0.1),
            ("find", 0.1),
            ("scroll", 0.1),
            ("step", 0.15),
        ],
    );
    add(
        "second",
        &[
            ("step", 0.3),
            ("tap", 0.2),
            ("navigate", 0.2),
            ("open", 0.15),
            ("find", 0.15),
        ],
    );
    add("let", &[("me", 0.9), ("the", 0.1)]);
    add(
        "me",
        &[
            ("help", 0.4),
            ("open", 0.2),
            ("navigate", 0.15),
            ("find", 0.15),
            ("check", 0.1),
        ],
    );
    add("help", &[("you", 0.6), ("with", 0.4)]);
    add(
        "you",
        &[
            ("with", 0.4),
            ("to", 0.2),
            ("open", 0.15),
            ("find", 0.15),
            ("navigate", 0.1),
        ],
    );
    add(
        "with",
        &[("the", 0.3), ("that", 0.3), ("this", 0.2), ("a", 0.2)],
    );
    add(
        "that",
        &[
            ("and", 0.2),
            ("is", 0.2),
            ("option", 0.15),
            ("button", 0.15),
            ("screen", 0.15),
            ("</s>", 0.15),
        ],
    );
    add(
        "sure",
        &[("I", 0.4), ("let", 0.3), ("here", 0.15), ("</s>", 0.15)],
    );
    add(
        "here",
        &[("is", 0.4), ("go", 0.2), ("</s>", 0.2), ("and", 0.2)],
    );
    add(
        "go",
        &[("to", 0.5), ("back", 0.2), ("home", 0.15), ("and", 0.15)],
    );
    add(
        "check",
        &[
            ("the", 0.3),
            ("settings", 0.2),
            ("wifi", 0.15),
            ("bluetooth", 0.1),
            ("if", 0.25),
        ],
    );
    add(
        "ok",
        &[("I", 0.3), ("let", 0.3), ("sure", 0.2), ("first", 0.2)],
    );
    add(
        "yes",
        &[("I", 0.4), ("the", 0.2), ("sure", 0.2), ("that", 0.2)],
    );
    add(
        "no",
        &[("I", 0.2), ("the", 0.2), ("that", 0.3), ("this", 0.3)],
    );

    (vocab, word_to_token, bigrams)
}

impl StubBackend {
    /// Create a new stub backend with a built-in mini vocabulary.
    pub fn new(seed: u64) -> Self {
        let (vocab, word_to_token, bigrams) = build_stub_bigrams();

        Self {
            bigrams,
            vocab,
            word_to_token,
            rng_state: Mutex::new(seed),
        }
    }

    /// Fast xorshift64 PRNG — returns value in [0.0, 1.0).
    fn next_random(&self) -> f64 {
        let mut state = self.rng_state.lock().unwrap_or_else(|e| e.into_inner());
        let mut s = *state;
        if s == 0 {
            s = 0xDEADBEEF;
        }
        s ^= s << 13;
        s ^= s >> 7;
        s ^= s << 17;
        *state = s;
        (s as f64) / (u64::MAX as f64)
    }

    /// Select next token using weighted sampling from bigram table.
    fn sample_bigram(&self, last_token: LlamaToken, temperature: f32) -> LlamaToken {
        let transitions = match self.bigrams.get(&last_token) {
            Some(t) if !t.is_empty() => t,
            _ => {
                // Fallback: pick a random content word (skip special tokens)
                let idx = ((self.next_random() * (self.vocab.len() - 3) as f64) as usize) + 3;
                return idx as LlamaToken;
            }
        };

        // Apply temperature to probabilities
        let temp = temperature.max(0.01) as f64;
        let scaled: Vec<f64> = transitions
            .iter()
            .map(|(_, p)| (*p as f64 / temp).exp())
            .collect();
        let sum: f64 = scaled.iter().sum();
        let normalized: Vec<f64> = scaled.iter().map(|p| p / sum).collect();

        // Weighted random selection
        let r = self.next_random();
        let mut cumulative = 0.0;
        for (i, prob) in normalized.iter().enumerate() {
            cumulative += prob;
            if r < cumulative {
                return transitions[i].0;
            }
        }

        // Fallback to last transition
        transitions
            .last()
            .map(|(t, _)| *t)
            .unwrap_or(special_tokens::EOS)
    }

    /// Apply repetition penalty: reduce probability of recently seen tokens.
    fn apply_repetition_penalty(
        &self,
        last_token: LlamaToken,
        recent_tokens: &[LlamaToken],
        params: &SamplingParams,
    ) -> LlamaToken {
        let candidate = self.sample_bigram(last_token, params.temperature);

        // If the candidate was recently generated and penalty > 1.0, re-sample up to 3 times
        if params.repeat_penalty > 1.0 && recent_tokens.contains(&candidate) {
            for _ in 0..3 {
                let alt = self.sample_bigram(last_token, params.temperature * 1.2);
                if !recent_tokens.contains(&alt) {
                    return alt;
                }
            }
        }

        candidate
    }
}

impl LlamaBackend for StubBackend {
    fn is_stub(&self) -> bool {
        true
    }

    fn load_model(
        &self,
        path: &str,
        _model_params: &LlamaModelParams,
        _ctx_params: &LlamaContextParams,
    ) -> BackendResult<(*mut LlamaModel, *mut LlamaContext)> {
        info!(path, "stub: simulating model load");

        // LLM-MED-4: Sentinel pointers use `std::ptr::dangling_mut()` which returns
        // a well-aligned, non-null pointer that is guaranteed never to alias valid
        // allocations. This is preferred over raw integer casts (0x1, 0x2) which have
        // no alignment guarantees and technically invoke UB under strict provenance.
        let model_ptr = std::ptr::dangling_mut::<LlamaModel>();
        let ctx_ptr = std::ptr::dangling_mut::<LlamaContext>();

        debug!("stub: model loaded (sentinel pointers)");
        Ok((model_ptr, ctx_ptr))
    }

    fn free_model(&self, _model: *mut LlamaModel, _ctx: *mut LlamaContext) {
        debug!("stub: model freed (no-op)");
    }

    fn tokenize(&self, _ctx: *mut LlamaContext, text: &str) -> BackendResult<Vec<LlamaToken>> {
        // Simple whitespace tokenizer: split on spaces, map words to token IDs.
        let mut tokens = vec![special_tokens::BOS];

        for word in text.split_whitespace() {
            let lower = word.to_lowercase();
            // Strip punctuation for matching
            let clean: String = lower.chars().filter(|c| c.is_alphanumeric()).collect();
            if let Some(&token_id) = self.word_to_token.get(&clean) {
                tokens.push(token_id);
            } else if let Some(&token_id) = self.word_to_token.get(word) {
                tokens.push(token_id);
            } else {
                // Unknown word → use UNK but also record position for approximate count
                tokens.push(special_tokens::UNK);
            }
        }

        trace!(token_count = tokens.len(), "stub: tokenized text");
        Ok(tokens)
    }

    fn detokenize(&self, _ctx: *mut LlamaContext, tokens: &[LlamaToken]) -> BackendResult<String> {
        let words: Vec<&str> = tokens
            .iter()
            .filter_map(|&t| {
                let idx = t as usize;
                if idx < self.vocab.len() && t > 2 {
                    // Skip special tokens (0=UNK, 1=BOS, 2=EOS)
                    Some(self.vocab[idx].as_str())
                } else {
                    None
                }
            })
            .collect();

        let text = words.join(" ");
        trace!(text_len = text.len(), "stub: detokenized tokens");
        Ok(text)
    }

    fn sample_next(
        &self,
        _ctx: *mut LlamaContext,
        tokens: &[LlamaToken],
        params: &SamplingParams,
    ) -> BackendResult<LlamaToken> {
        let last_token = tokens.last().copied().unwrap_or(special_tokens::BOS);

        // Use the last N tokens for repetition penalty window
        let window_size = 16.min(tokens.len());
        let recent = &tokens[tokens.len() - window_size..];

        let next = self.apply_repetition_penalty(last_token, recent, params);
        trace!(last = last_token, next, "stub: sampled next token");
        Ok(next)
    }

    fn eos_token(&self) -> LlamaToken {
        special_tokens::EOS
    }

    fn bos_token(&self) -> LlamaToken {
        special_tokens::BOS
    }

    fn eval(&self, _ctx: *mut LlamaContext, tokens: &[LlamaToken]) -> BackendResult<()> {
        trace!(token_count = tokens.len(), "stub: eval (no-op)");
        Ok(())
    }

    fn get_token_logprob(&self, _ctx: *mut LlamaContext, _token: LlamaToken) -> Option<f32> {
        // Stub backend has no real logits — return None so confidence
        // estimation falls back to the uninformative 0.5 prior.
        None
    }
}

/// Position type for the batch API — matches llama.cpp's `llama_pos` (i32).
pub type LlamaPos = i32;

/// Sequence ID type — matches llama.cpp's `llama_seq_id` (i32).
pub type LlamaSeqId = i32;

// ─── Opaque types for grammar-constrained sampling ──────────────────────────

/// Opaque sampler chain — wraps llama.cpp's `struct llama_sampler`.
/// Created by `llama_sampler_init_grammar()`, freed by `llama_sampler_free()`.
#[repr(C)]
pub struct LlamaSampler {
    _opaque: [u8; 0],
}

/// A single token candidate with its probability/logit for sampler operations.
/// Matches `llama_token_data` in llama.cpp.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct LlamaTokenData {
    /// Token ID.
    pub id: LlamaToken,
    /// Log-odds (logit) for this token.
    pub logit: f32,
    /// Probability (may be 0.0 before softmax).
    pub p: f32,
}

/// Array of token candidates passed to sampler functions.
/// Matches `llama_token_data_array` in llama.cpp.
#[repr(C)]
pub struct LlamaTokenDataArray {
    /// Pointer to the array of candidates.
    pub data: *mut LlamaTokenData,
    /// Number of candidates.
    pub size: usize,
    /// Whether the array is already sorted by logit (descending).
    pub sorted: bool,
}

/// Batch of tokens for the modern llama.cpp batch API.
///
/// Corresponds to `struct llama_batch` in llama.cpp (post-0.x batch API).
/// Pass to `llama_decode` instead of the old (tokens, n_tokens, n_past) signature.
#[repr(C)]
pub struct LlamaBatch {
    /// Number of tokens in this batch.
    pub n_tokens: i32,
    /// Token IDs (size: n_tokens). NULL if using embeddings.
    pub token: *mut LlamaToken,
    /// Embeddings (size: n_tokens * n_embd). NULL if using token IDs.
    pub embd: *mut f32,
    /// Token positions (size: n_tokens).
    pub pos: *mut LlamaPos,
    /// Number of sequence IDs per token (size: n_tokens).
    pub n_seq_id: *mut i32,
    /// Sequence IDs for each token (size: n_tokens, each entry is a pointer).
    pub seq_id: *mut *mut LlamaSeqId,
    /// Which tokens to compute logits for (size: n_tokens). 1 = yes, 0 = no.
    pub logits: *mut i8,
    // --- Convenience fields (used when all tokens share the same pos/seq_id) ---
    /// Starting position when not using per-token `pos`.
    pub all_pos_0: LlamaPos,
    /// Position stride when not using per-token `pos`.
    pub all_pos_1: LlamaPos,
    /// Sequence ID when not using per-token `seq_id`.
    pub all_seq_id: LlamaSeqId,
}

#[cfg(target_os = "android")]
extern "C" {
    fn llama_load_model_from_file(
        path: *const std::ffi::c_char,
        params: LlamaModelParams,
    ) -> *mut LlamaModel;
    fn llama_new_context_with_model(
        model: *mut LlamaModel,
        params: LlamaContextParams,
    ) -> *mut LlamaContext;
    pub fn llama_free_model(model: *mut LlamaModel);
    pub fn llama_free(ctx: *mut LlamaContext);
    fn llama_tokenize(
        ctx: *mut LlamaContext,
        text: *const std::ffi::c_char,
        tokens: *mut LlamaToken,
        n_max_tokens: i32,
        add_bos: bool,
    ) -> i32;
    fn llama_token_to_piece(
        model: *mut LlamaModel,
        token: LlamaToken,
        buf: *mut std::ffi::c_char,
        length: i32,
    ) -> i32;
    /// Modern batch API: construct a simple single-sequence batch from a token slice.
    ///
    /// Replaces the old `llama_decode(ctx, tokens, n_tokens, n_past)` call.
    /// Use: `let batch = llama_batch_get_one(tokens_ptr, n_tokens, pos_0, seq_id);`
    ///      `llama_decode(ctx, batch);`
    fn llama_batch_get_one(
        tokens: *mut LlamaToken,
        n_tokens: i32,
        pos_0: LlamaPos,
        seq_id: LlamaSeqId,
    ) -> LlamaBatch;
    /// Evaluate a batch of tokens.
    ///
    /// Modern signature: takes a `LlamaBatch` (batch API), not raw token pointer.
    /// Returns 0 on success, non-zero on error.
    fn llama_decode(ctx: *mut LlamaContext, batch: LlamaBatch) -> i32;
    fn llama_get_logits(ctx: *mut LlamaContext) -> *mut f32;
    fn llama_n_vocab(model: *mut LlamaModel) -> i32;
    fn llama_token_eos(model: *mut LlamaModel) -> LlamaToken;
    fn llama_token_bos(model: *mut LlamaModel) -> LlamaToken;

    // ── Grammar-constrained sampling (GBNF Layer 0) ────────────────────
    //
    // These functions integrate llama.cpp's grammar-constrained sampling,
    // allowing GBNF grammars to mask invalid tokens BEFORE softmax/sampling.
    //
    // Flow: compile grammar → init sampler → for each token: apply grammar
    //       mask to logits → sample → accept token into sampler → loop.
    //
    // Phase 3 wiring: these are available in llama.cpp ≥ b2200.
    // If the linked version is older, the build will fail at link time,
    // which is the correct failure mode (compile-time, not runtime).

    /// Create a grammar-constrained sampler from a GBNF grammar string.
    ///
    /// `model`       — the loaded model (needed for vocab mapping).
    /// `grammar_str` — null-terminated GBNF grammar string.
    /// `grammar_root`— null-terminated name of the root rule (usually "root").
    ///
    /// Returns an opaque `*mut LlamaSampler` that must be freed with
    /// `llama_sampler_free()`.
    fn llama_sampler_init_grammar(
        model: *mut LlamaModel,
        grammar_str: *const std::ffi::c_char,
        grammar_root: *const std::ffi::c_char,
    ) -> *mut LlamaSampler;

    /// Apply all samplers in the chain (including grammar) to modify logits
    /// in-place. Call this BEFORE reading logits for manual sampling.
    fn llama_sampler_apply(smpl: *mut LlamaSampler, cur: *mut LlamaTokenDataArray);

    /// Notify the grammar sampler that a token was accepted, advancing its
    /// internal state machine so the next `apply` call masks correctly.
    fn llama_sampler_accept(smpl: *mut LlamaSampler, token: LlamaToken);

    /// Free a sampler chain (including grammar state).
    fn llama_sampler_free(smpl: *mut LlamaSampler);
}

/// Statically-linked llama.cpp backend for Android.
///
/// Uses standard `extern "C"` blocks, as `build.rs` compiles `llama.cpp` directly.
#[cfg(target_os = "android")]
pub struct FfiBackend {
    model_ptr: std::sync::Mutex<Option<*mut LlamaModel>>,
    /// Context pointer — stored so `Drop` can free it if the caller
    /// doesn't call `free_model()` explicitly. Without this, the context
    /// (which can be 500MB–4GB) leaks when the struct is dropped.
    ctx_ptr: std::sync::Mutex<Option<*mut LlamaContext>>,
    /// Tracks the number of tokens already evaluated in the context,
    /// so multi-turn conversations advance the position instead of
    /// always overwriting position 0.
    n_past: std::sync::Mutex<i32>,
    /// Active grammar sampler for constrained decoding (Layer 0).
    ///
    /// Set by `set_grammar()` before generation begins, cleared after
    /// generation completes. When `Some`, `sample_next()` applies the
    /// grammar mask to logits BEFORE temperature/top-k/top-p, ensuring
    /// every emitted token satisfies the GBNF grammar.
    grammar_sampler: std::sync::Mutex<Option<*mut LlamaSampler>>,
}

// SAFETY: FfiBackend is Send + Sync because every field containing a raw pointer
// (model_ptr, ctx_ptr, grammar_sampler) is wrapped in a std::sync::Mutex, which
// provides both interior mutability and cross-thread synchronization. The Mutex
// guards ensure that only one thread can dereference the C pointers at a time,
// and the remaining fields (n_past) are also Mutex-wrapped. The underlying
// llama.cpp library is safe for single-threaded per-context use, which the Mutex
// guards enforce.
#[cfg(target_os = "android")]
unsafe impl Send for FfiBackend {}
#[cfg(target_os = "android")]
unsafe impl Sync for FfiBackend {}

#[cfg(target_os = "android")]
impl FfiBackend {
    pub fn new(_lib_path: &str) -> BackendResult<Self> {
        info!("Statically-linked FFI backend initialized (lib_path argument ignored)");
        Ok(Self {
            model_ptr: std::sync::Mutex::new(None),
            ctx_ptr: std::sync::Mutex::new(None),
            n_past: std::sync::Mutex::new(0),
            grammar_sampler: std::sync::Mutex::new(None),
        })
    }

    /// Compile and activate a GBNF grammar for constrained decoding.
    ///
    /// Must be called BEFORE the generation loop. The grammar sampler remains
    /// active until `clear_grammar()` is called (typically after generation).
    ///
    /// # Safety
    /// Requires a loaded model (model_ptr must be Some).
    pub fn set_grammar(&self, grammar_gbnf: &str) -> BackendResult<()> {
        let model_guard = self.model_ptr.lock().unwrap_or_else(|e| e.into_inner());
        let model = model_guard
            .ok_or_else(|| BackendError::Generation("no model loaded for grammar init".into()))?;

        let grammar_cstr = std::ffi::CString::new(grammar_gbnf)
            .map_err(|e| BackendError::Generation(format!("grammar contains null byte: {}", e)))?;
        let root_cstr = std::ffi::CString::new("root")
            .map_err(|e| BackendError::Generation(format!("root rule CString failed: {}", e)))?;

        let sampler =
            // SAFETY: model is valid, grammar_cstr/root_cstr are valid CStrings.
            unsafe { llama_sampler_init_grammar(model, grammar_cstr.as_ptr(), root_cstr.as_ptr()) };

        if sampler.is_null() {
            return Err(BackendError::Generation(
                "llama_sampler_init_grammar returned null — grammar compilation failed".into(),
            ));
        }

        let mut grammar_guard = self
            .grammar_sampler
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        // Free any existing grammar sampler before replacing
        if let Some(old) = grammar_guard.take() {
            // SAFETY: old was allocated by llama_sampler_init_grammar.
            unsafe { llama_sampler_free(old) };
        }
        *grammar_guard = Some(sampler);

        debug!("GBNF grammar sampler activated for constrained decoding");
        Ok(())
    }

    /// Clear the active grammar sampler, freeing its resources.
    ///
    /// Called after generation completes (success or failure) to avoid
    /// leaking the grammar state.
    pub fn clear_grammar(&self) {
        let mut grammar_guard = self
            .grammar_sampler
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if let Some(sampler) = grammar_guard.take() {
            // SAFETY: sampler was allocated by llama_sampler_init_grammar.
            unsafe { llama_sampler_free(sampler) };
            debug!("GBNF grammar sampler cleared");
        }
    }
}

#[cfg(target_os = "android")]
impl Drop for FfiBackend {
    fn drop(&mut self) {
        // Free grammar sampler FIRST — it may reference model internals.
        self.clear_grammar();

        // Free context NEXT — it references the model internally,
        // so freeing the model first would leave a dangling pointer
        // in the context during llama_free().
        let mut ctx_guard = self.ctx_ptr.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(ctx) = ctx_guard.take() {
            if !ctx.is_null() {
                // SAFETY: ctx was allocated by llama_new_context_with_model, null-checked.
                unsafe { llama_free(ctx) };
                debug!("FfiBackend::drop freed context pointer");
            }
        }

        let mut guard = self.model_ptr.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(model) = guard.take() {
            if !model.is_null() {
                // SAFETY: model was allocated by llama_load_model_from_file, null-checked.
                unsafe { llama_free_model(model) };
                debug!("FfiBackend::drop freed model pointer");
            }
        }
    }
}

#[cfg(target_os = "android")]
impl LlamaBackend for FfiBackend {
    fn is_stub(&self) -> bool {
        false
    }

    fn load_model(
        &self,
        path: &str,
        model_params: &LlamaModelParams,
        ctx_params: &LlamaContextParams,
    ) -> BackendResult<(*mut LlamaModel, *mut LlamaContext)> {
        use std::ffi::CString;

        let c_path = CString::new(path)
            .map_err(|e| BackendError::ModelLoad(format!("invalid path: {}", e)))?;

        // SAFETY: c_path is a valid CString. llama_load_model_from_file allocates a model.
        let model = unsafe { llama_load_model_from_file(c_path.as_ptr(), model_params.clone()) };
        if model.is_null() {
            return Err(BackendError::ModelLoad(format!(
                "llama_load_model_from_file returned null for: {}",
                path
            )));
        }

        // SAFETY: model is non-null from llama_load_model_from_file.
        let ctx = unsafe { llama_new_context_with_model(model, ctx_params.clone()) };
        if ctx.is_null() {
            // SAFETY: model was just loaded, must be freed on ctx creation failure.
            unsafe { llama_free_model(model) };
            return Err(BackendError::ContextCreation(
                "llama_new_context_with_model returned null".into(),
            ));
        }

        *self.model_ptr.lock().unwrap_or_else(|e| e.into_inner()) = Some(model);
        *self.ctx_ptr.lock().unwrap_or_else(|e| e.into_inner()) = Some(ctx);

        info!(path, "model loaded via static FFI");
        Ok((model, ctx))
    }

    fn free_model(&self, model: *mut LlamaModel, ctx: *mut LlamaContext) {
        if !ctx.is_null() {
            // SAFETY: ctx is non-null, allocated by llama_new_context_with_model.
            unsafe { llama_free(ctx) };
        }
        if !model.is_null() {
            // SAFETY: model is non-null, allocated by llama_load_model_from_file.
            unsafe { llama_free_model(model) };
        }
        *self.model_ptr.lock().unwrap_or_else(|e| e.into_inner()) = None;
        *self.ctx_ptr.lock().unwrap_or_else(|e| e.into_inner()) = None;
        *self.n_past.lock().unwrap_or_else(|e| e.into_inner()) = 0;
        debug!("model freed via static FFI");
    }

    fn tokenize(&self, ctx: *mut LlamaContext, text: &str) -> BackendResult<Vec<LlamaToken>> {
        use std::ffi::CString;

        let c_text = CString::new(text)
            .map_err(|e| BackendError::Tokenization(format!("invalid text: {}", e)))?;

        // First call to get required buffer size
        // SAFETY: ctx is valid, c_text is valid CString, null_mut() for size query.
        let n_tokens =
            unsafe { llama_tokenize(ctx, c_text.as_ptr(), std::ptr::null_mut(), 0, true) };
        if n_tokens < 0 {
            return Err(BackendError::Tokenization(format!(
                "llama_tokenize size query failed: {}",
                n_tokens
            )));
        }

        let mut tokens = vec![0i32; n_tokens as usize];
        // SAFETY: ctx is valid, tokens has n_tokens capacity, n_tokens from prior query.
        let result =
            unsafe { llama_tokenize(ctx, c_text.as_ptr(), tokens.as_mut_ptr(), n_tokens, true) };
        if result < 0 {
            return Err(BackendError::Tokenization(format!(
                "llama_tokenize failed: {}",
                result
            )));
        }

        tokens.truncate(result as usize);
        trace!(token_count = tokens.len(), "tokenized via static FFI");
        Ok(tokens)
    }

    fn detokenize(&self, _ctx: *mut LlamaContext, tokens: &[LlamaToken]) -> BackendResult<String> {
        let model_guard = self.model_ptr.lock().unwrap_or_else(|e| e.into_inner());
        let model =
            model_guard.ok_or_else(|| BackendError::Detokenization("no model loaded".into()))?;

        let mut result = String::new();
        let mut buf = vec![0u8; 256];

        for &token in tokens {
            // SAFETY: model is valid, buf has 256 bytes capacity.
            let n = unsafe {
                llama_token_to_piece(
                    model,
                    token,
                    buf.as_mut_ptr() as *mut std::ffi::c_char,
                    buf.len() as i32,
                )
            };
            if n > 0 {
                let piece = String::from_utf8_lossy(&buf[..n as usize]);
                result.push_str(&piece);
            }
        }

        trace!(text_len = result.len(), "detokenized via static FFI");
        Ok(result)
    }

    fn sample_next(
        &self,
        ctx: *mut LlamaContext,
        tokens: &[LlamaToken],
        params: &SamplingParams,
    ) -> BackendResult<LlamaToken> {
        let model_guard = self.model_ptr.lock().unwrap_or_else(|e| e.into_inner());
        let model =
            model_guard.ok_or_else(|| BackendError::Generation("no model loaded".into()))?;

        let n_vocab = unsafe { llama_n_vocab(model) } as usize;
        let logits_ptr = unsafe { llama_get_logits(ctx) };
        if logits_ptr.is_null() || n_vocab == 0 {
            return Err(BackendError::Generation(
                "logits pointer is null or vocab size is 0".into(),
            ));
        }

        // ── Layer 0: Grammar-constrained logit masking ──────────────────
        //
        // If a GBNF grammar sampler is active, apply it to the raw logits
        // BEFORE temperature scaling. This zeroes out logits for tokens
        // that would violate the grammar, ensuring structural correctness
        // at the token level — not post-hoc validation.
        {
            let grammar_guard = self
                .grammar_sampler
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if let Some(grammar) = *grammar_guard {
                let logits_slice = unsafe { std::slice::from_raw_parts_mut(logits_ptr, n_vocab) };
                let mut candidates: Vec<LlamaTokenData> = logits_slice
                    .iter()
                    .enumerate()
                    .map(|(i, &logit)| LlamaTokenData {
                        id: i as LlamaToken,
                        logit,
                        p: 0.0,
                    })
                    .collect();
                let mut candidates_array = LlamaTokenDataArray {
                    data: candidates.as_mut_ptr(),
                    size: candidates.len(),
                    sorted: false,
                };
                unsafe {
                    llama_sampler_apply(grammar, &mut candidates_array);
                }
                // Write masked logits back so temperature/top-k/top-p
                // operate on grammar-constrained values.
                for candidate in &candidates {
                    let idx = candidate.id as usize;
                    if idx < n_vocab {
                        logits_slice[idx] = candidate.logit;
                    }
                }
            }
        }

        let logits = unsafe { std::slice::from_raw_parts(logits_ptr, n_vocab) };

        // Apply temperature
        let temp = params.temperature.max(0.001);
        let mut probs: Vec<f64> = logits
            .iter()
            .map(|&l| (l as f64 / temp as f64).exp())
            .collect();

        // Apply repetition penalty
        if params.repeat_penalty > 1.0 {
            let window = 64.min(tokens.len());
            for &tok in &tokens[tokens.len() - window..] {
                let idx = tok as usize;
                if idx < n_vocab {
                    probs[idx] /= params.repeat_penalty as f64;
                }
            }
        }

        // Top-k filtering
        if params.top_k > 0 {
            let k = (params.top_k as usize).min(n_vocab);
            let mut indexed: Vec<(usize, f64)> = probs.iter().copied().enumerate().collect();
            indexed.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
            let threshold = indexed.get(k).map(|(_, p)| *p).unwrap_or(0.0);
            for p in probs.iter_mut() {
                if *p < threshold {
                    *p = 0.0;
                }
            }
        }

        // Normalize
        let sum: f64 = probs.iter().sum();
        if sum <= 0.0 {
            return Err(BackendError::Generation(
                "all probabilities zero after filtering".into(),
            ));
        }
        for p in probs.iter_mut() {
            *p /= sum;
        }

        // Top-p (nucleus) sampling
        if params.top_p < 1.0 {
            let mut sorted: Vec<(usize, f64)> = probs.iter().copied().enumerate().collect();
            sorted.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
            let mut cumsum = 0.0;
            let mut cutoff_idx = sorted.len();
            for (i, (_, p)) in sorted.iter().enumerate() {
                cumsum += p;
                if cumsum >= params.top_p as f64 {
                    cutoff_idx = i + 1;
                    break;
                }
            }
            let allowed: std::collections::HashSet<usize> =
                sorted[..cutoff_idx].iter().map(|(idx, _)| *idx).collect();
            for (i, p) in probs.iter_mut().enumerate() {
                if !allowed.contains(&i) {
                    *p = 0.0;
                }
            }
            // Re-normalize
            let sum: f64 = probs.iter().sum();
            if sum > 0.0 {
                for p in probs.iter_mut() {
                    *p /= sum;
                }
            }
        }

        // Sample from distribution (simple linear scan).
        // Use a proper CSPRNG-seeded PRNG via the `rand` crate so that:
        //   1. Rapid sequential calls don't produce identical tokens (the old
        //      SystemTime::subsec_nanos() was deterministic within the same nanosecond, which is
        //      common under batch decode).
        //   2. The distribution is uniform in [0, 1), not biased by clock quantization artifacts.
        let r: f64 = {
            use rand::Rng;
            rand::thread_rng().gen()
        };

        let mut cumulative = 0.0;
        for (i, &p) in probs.iter().enumerate() {
            cumulative += p;
            if r < cumulative {
                return Ok(i as LlamaToken);
            }
        }

        // Fallback: return the most probable token
        let best = probs
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap_or(std::cmp::Ordering::Equal))
            .map(|(i, _)| i as LlamaToken)
            .unwrap_or(self.eos_token());

        Ok(best)
    }

    fn eos_token(&self) -> LlamaToken {
        let guard = self.model_ptr.lock().unwrap_or_else(|e| e.into_inner());
        match *guard {
            Some(model) => unsafe { llama_token_eos(model) },
            None => special_tokens::EOS,
        }
    }

    fn bos_token(&self) -> LlamaToken {
        let guard = self.model_ptr.lock().unwrap_or_else(|e| e.into_inner());
        match *guard {
            Some(model) => unsafe { llama_token_bos(model) },
            None => special_tokens::BOS,
        }
    }

    fn eval(&self, ctx: *mut LlamaContext, tokens: &[LlamaToken]) -> BackendResult<()> {
        let mut n_past_guard = self.n_past.lock().unwrap_or_else(|e| e.into_inner());
        let past = *n_past_guard;

        // Modern batch API: use llama_batch_get_one to construct the batch,
        // then pass it to llama_decode. The old 4-argument llama_decode
        // (ctx, tokens_ptr, n_tokens, n_past) no longer exists in modern llama.cpp.
        //
        // SAFETY: llama_batch_get_one expects `*mut LlamaToken`. We must pass
        // a mutable buffer because the C API may write through the pointer.
        // Casting `&[T].as_ptr()` to `*mut T` is undefined behavior — the
        // compiler is free to place the slice in read-only memory. Instead we
        // copy into a mutable Vec and hand over `as_mut_ptr()`.
        let mut token_buf = tokens.to_vec();
        let result = unsafe {
            let batch = llama_batch_get_one(
                token_buf.as_mut_ptr(),
                token_buf.len() as i32,
                past, // pos_0: starting position for multi-turn context
                0,    // seq_id: single sequence
            );
            llama_decode(ctx, batch)
        };

        if result != 0 {
            return Err(BackendError::Generation(format!(
                "llama_decode failed with code: {}",
                result
            )));
        }

        *n_past_guard += tokens.len() as i32;
        trace!(
            token_count = tokens.len(),
            n_past = *n_past_guard,
            "eval via FFI"
        );
        Ok(())
    }

    fn get_token_logprob(&self, ctx: *mut LlamaContext, token: LlamaToken) -> Option<f32> {
        let model_guard = self.model_ptr.lock().unwrap_or_else(|e| e.into_inner());
        let model = (*model_guard)?;

        let n_vocab = unsafe { llama_n_vocab(model) } as usize;
        if n_vocab == 0 {
            return None;
        }

        let logits_ptr = unsafe { llama_get_logits(ctx) };
        if logits_ptr.is_null() {
            return None;
        }

        let token_idx = token as usize;
        if token_idx >= n_vocab {
            return None;
        }

        // SAFETY: logits_ptr is non-null and points to n_vocab floats,
        // valid until the next llama_decode call. token_idx is bounds-checked.
        let logits = unsafe { std::slice::from_raw_parts(logits_ptr, n_vocab) };

        // Numerically stable log_softmax:
        //   log_softmax(x_i) = x_i - log(sum(exp(x_j)))
        //                    = x_i - (max + log(sum(exp(x_j - max))))
        let max_logit = logits.iter().copied().fold(f32::NEG_INFINITY, f32::max);
        if !max_logit.is_finite() {
            return None;
        }

        let sum_exp: f64 = logits.iter().map(|&l| ((l - max_logit) as f64).exp()).sum();

        if sum_exp <= 0.0 {
            return None;
        }

        let log_softmax = (logits[token_idx] - max_logit) as f64 - sum_exp.ln();
        Some(log_softmax as f32)
    }

    fn set_grammar(&self, grammar_gbnf: &str) -> BackendResult<()> {
        // Delegate to the inherent method on FfiBackend
        FfiBackend::set_grammar(self, grammar_gbnf)
    }

    fn clear_grammar(&self) {
        // Delegate to the inherent method on FfiBackend
        FfiBackend::clear_grammar(self)
    }

    fn accept_grammar_token(&self, token: LlamaToken) {
        let grammar_guard = self
            .grammar_sampler
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if let Some(grammar) = *grammar_guard {
            unsafe { llama_sampler_accept(grammar, token) };
        }
    }
}

// ─── Global backend accessor ────────────────────────────────────────────────

use std::sync::{LazyLock, OnceLock};

/// Errors that can occur during backend initialization.
#[derive(Debug, Clone, thiserror::Error)]
pub enum LlamaError {
    /// Backend was not initialized before access.
    #[error("backend not initialized — call init_stub_backend() or init_ffi_backend() first")]
    NotInitialized,
    /// Backend initialization panicked (caught via catch_unwind).
    #[error("backend initialization panicked: {0}")]
    InitializationPanic(String),
    /// Backend is already initialized (cannot re-initialize).
    #[error("backend already initialized")]
    AlreadyInitialized,
    /// Failed to initialize the backend.
    #[error("initialization failed: {0}")]
    InitializationFailed(String),
}

/// Result type for backend access.
pub type BackendAccessResult<T> = Result<T, LlamaError>;

/// Global backend storage with panic safety.
///
/// Uses `LazyLock<OnceLock<Result<...>>>` to:
/// 1. Lazy-initialize on first access (avoids Static Initialization Order Fiasco)
/// 2. Wrap result in Result type for proper error handling
/// 3. Catch panics during initialization to prevent SIGSEGV
static BACKEND: LazyLock<OnceLock<Result<Box<dyn LlamaBackend>, LlamaError>>> =
    LazyLock::new(OnceLock::new);

/// Initialize the global stub backend (desktop/testing).
///
/// # Errors
/// Returns an error if the backend is already initialized.
pub fn init_stub_backend(seed: u64) -> BackendResult<()> {
    // Check if already initialized
    if BACKEND.get().is_some() {
        return Err(BackendError::StubMode("backend already initialized".into()));
    }

    // Wrap initialization in catch_unwind to prevent panics from crashing the binary
    let result =
        std::panic::catch_unwind(|| Box::new(StubBackend::new(seed)) as Box<dyn LlamaBackend>);

    match result {
        Ok(backend) => {
            // OnceLock::set returns Err if already set, Ok(()) if successful
            if BACKEND.set(Ok(backend)).is_err() {
                // Race condition: another thread initialized first
                error!("backend initialized concurrently");
                return Err(BackendError::StubMode("backend already initialized".into()));
            }
            info!("stub backend initialized (seed={})", seed);
            Ok(())
        }
        Err(panic_info) => {
            let message = if let Some(s) = panic_info.downcast_ref::<&str>() {
                s.to_string()
            } else if let Some(s) = panic_info.downcast_ref::<String>() {
                s.clone()
            } else {
                "unknown panic".to_string()
            };
            error!(panic_message = %message, "stub backend initialization panicked");
            Err(BackendError::StubMode(format!(
                "initialization panicked: {}",
                message
            )))
        }
    }
}

/// Initialize the FFI backend for Android (real llama.cpp).
///
/// # Errors
/// Returns an error if the backend is already initialized or if FFI initialization fails.
#[cfg(target_os = "android")]
pub fn init_ffi_backend(lib_path: &str) -> BackendResult<()> {
    // Check if already initialized
    if BACKEND.get().is_some() {
        return Err(BackendError::LibraryLoad(
            "backend already initialized".into(),
        ));
    }

    // Wrap initialization in catch_unwind to prevent panics from crashing the binary
    let result = std::panic::catch_unwind(move || {
        FfiBackend::new(lib_path).map(|b| Box::new(b) as Box<dyn LlamaBackend>)
    });

    match result {
        Ok(Ok(backend)) => {
            // OnceLock::set returns Err if already set, Ok(()) if successful
            if BACKEND.set(Ok(backend)).is_err() {
                error!("backend initialized concurrently");
                return Err(BackendError::LibraryLoad(
                    "backend already initialized".into(),
                ));
            }
            info!("FFI backend initialized (lib_path={})", lib_path);
            Ok(())
        }
        Ok(Err(e)) => {
            error!(error = %e, "FFI backend initialization failed");
            Err(BackendError::LibraryLoad(format!(
                "initialization failed: {}",
                e
            )))
        }
        Err(panic_info) => {
            let message = if let Some(s) = panic_info.downcast_ref::<&str>() {
                s.to_string()
            } else if let Some(s) = panic_info.downcast_ref::<String>() {
                s.clone()
            } else {
                "unknown panic".to_string()
            };
            error!(panic_message = %message, "FFI backend initialization panicked");
            Err(BackendError::LibraryLoad(format!(
                "initialization panicked: {}",
                message
            )))
        }
    }
}

/// Initialize the HTTP backend for connecting to llama-server.
///
/// This backend connects to a running llama-server instance via HTTP,
/// using the OpenAI-compatible /v1/chat/completions endpoint.
///
/// # Arguments
/// * `base_url` - Base URL of llama-server (e.g., "http://localhost:8080")
/// * `model_name` - Model name as recognized by the server
///
/// # Errors
/// Returns an error if the backend is already initialized.
pub fn init_server_backend(base_url: &str, model_name: &str) -> BackendResult<()> {
    // Check if already initialized
    if BACKEND.get().is_some() {
        return Err(BackendError::StubMode("backend already initialized".into()));
    }

    // Wrap initialization in catch_unwind to prevent panics from crashing the binary
    let result = std::panic::catch_unwind(|| {
        Box::new(server_http_backend::ServerHttpBackend::new(
            base_url, model_name,
        )) as Box<dyn LlamaBackend>
    });

    match result {
        Ok(backend) => {
            // LazyLock::set returns Err if already set, Ok(()) if successful
            if BACKEND.set(Ok(backend)).is_err() {
                // Race condition: another thread initialized first
                error!("backend initialized concurrently");
                return Err(BackendError::StubMode("backend already initialized".into()));
            }
            info!(
                "HTTP backend initialized (base_url={}, model={})",
                base_url, model_name
            );
            Ok(())
        }
        Err(panic_info) => {
            let message = if let Some(s) = panic_info.downcast_ref::<&str>() {
                s.to_string()
            } else if let Some(s) = panic_info.downcast_ref::<String>() {
                s.clone()
            } else {
                "unknown panic".to_string()
            };
            error!(panic_message = %message, "HTTP backend initialization panicked");
            Err(BackendError::StubMode(format!(
                "initialization panicked: {}",
                message
            )))
        }
    }
}

/// Get a reference to the initialized backend.
///
/// # Errors
/// Returns `Err(LlamaError::NotInitialized)` if `init_stub_backend()` or
/// `init_ffi_backend()` has not been called.
pub fn backend() -> BackendAccessResult<&'static dyn LlamaBackend> {
    match BACKEND.get() {
        Some(Ok(backend)) => Ok(backend.as_ref()),
        Some(Err(e)) => {
            // Backend failed to initialize previously
            error!(error = %e, "backend in error state");
            Err(LlamaError::InitializationFailed(e.to_string()))
        }
        None => {
            error!("backend() called before initialization");
            Err(LlamaError::NotInitialized)
        }
    }
}

/// Get a reference to the initialized backend (panicking variant for backward compatibility).
///
/// # Panics
/// Panics if backend is not initialized. Use `backend()` for error-safe access.
pub fn backend_unsafe() -> &'static dyn LlamaBackend {
    backend().expect(
        "llama backend not initialized — call init_stub_backend() or init_ffi_backend() first",
    )
}

/// Check whether a backend has been initialized and is ready.
pub fn is_backend_initialized() -> bool {
    matches!(BACKEND.get(), Some(Ok(_)))
}

/// Check if backend is in an error state (failed to initialize).
pub fn is_backend_error() -> bool {
    matches!(BACKEND.get(), Some(Err(_)))
}

/// Get the current backend error, if any.
pub fn backend_error() -> Option<LlamaError> {
    BACKEND.get().and_then(|r| match r {
        Ok(_) => None,
        Err(e) => Some(e.clone()),
    })
}

// ─── Legacy stub module (compatibility shim) ────────────────────────────────
//
// Maintains backward compatibility with existing code that calls
// `aura_llama_sys::stubs::*` directly. These now delegate to the global backend.

#[cfg(not(target_os = "android"))]
pub mod stubs {
    use super::*;

    /// Initialize the stub backend if not already done.
    fn ensure_init() {
        if !is_backend_initialized() {
            let _ = init_stub_backend(0xA0BA);
        }
    }

    /// Stub: load model (returns sentinel pointers via backend).
    pub fn llama_load_model(path: &str, params: &LlamaModelParams) -> *mut LlamaModel {
        ensure_init();
        let ctx_params = LlamaContextParams::default();
        match backend() {
            Ok(b) => match b.load_model(path, params, &ctx_params) {
                Ok((model, _ctx)) => model,
                Err(e) => {
                    error!(error = %e, "stub model load failed");
                    std::ptr::null_mut()
                }
            },
            Err(e) => {
                error!(error = %e, "backend not available");
                std::ptr::null_mut()
            }
        }
    }

    /// Stub: create context (returns sentinel pointer via backend).
    pub fn llama_new_context(
        model: *mut LlamaModel,
        params: &LlamaContextParams,
    ) -> *mut LlamaContext {
        ensure_init();
        // The backend load_model already creates both — return sentinel ctx
        let _ = (model, params);
        std::ptr::dangling_mut::<LlamaContext>()
    }

    /// Stub: free model.
    pub fn llama_free_model(model: *mut LlamaModel) {
        if is_backend_initialized() {
            if let Ok(b) = backend() {
                b.free_model(model, std::ptr::null_mut());
            }
        }
    }

    /// Stub: free context.
    pub fn llama_free_context(ctx: *mut LlamaContext) {
        if is_backend_initialized() {
            if let Ok(b) = backend() {
                b.free_model(std::ptr::null_mut(), ctx);
            }
        }
    }

    /// Stub: tokenize text.
    pub fn llama_tokenize(ctx: *mut LlamaContext, text: &str) -> Vec<LlamaToken> {
        ensure_init();
        match backend() {
            Ok(b) => b.tokenize(ctx, text).unwrap_or_default(),
            Err(e) => {
                error!(error = %e, "backend not available for tokenize");
                vec![]
            }
        }
    }

    /// Stub: decode tokens to text.
    pub fn llama_decode_tokens(ctx: *mut LlamaContext, tokens: &[LlamaToken]) -> String {
        ensure_init();
        match backend() {
            Ok(b) => b.detokenize(ctx, tokens).unwrap_or_default(),
            Err(e) => {
                error!(error = %e, "backend not available for detokenize");
                String::new()
            }
        }
    }
}

// ─── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn test_stub() -> StubBackend {
        StubBackend::new(42)
    }

    #[test]
    fn default_params_are_sane() {
        let mp = LlamaModelParams::default();
        assert_eq!(mp.n_gpu_layers, 0);
        assert!(mp.use_mmap);

        let cp = LlamaContextParams::default();
        assert_eq!(cp.n_ctx, 2048);
        assert!(cp.n_threads > 0);

        let sp = SamplingParams::default();
        assert!(sp.temperature > 0.0 && sp.temperature < 2.0);
        assert!(sp.top_p > 0.0 && sp.top_p <= 1.0);
    }

    #[test]
    fn stub_is_stub() {
        let stub = test_stub();
        assert!(stub.is_stub());
    }

    #[test]
    fn stub_load_model_returns_non_null() {
        let stub = test_stub();
        let result = stub.load_model(
            "test.gguf",
            &LlamaModelParams::default(),
            &LlamaContextParams::default(),
        );
        assert!(result.is_ok());
        let (model, ctx) = result.expect("load should succeed");
        assert!(!model.is_null());
        assert!(!ctx.is_null());
    }

    #[test]
    fn stub_tokenize_produces_tokens() {
        let stub = test_stub();
        let tokens = stub
            .tokenize(std::ptr::null_mut(), "open the settings app")
            .expect("tokenize should succeed");
        assert!(!tokens.is_empty());
        // First token should be BOS
        assert_eq!(tokens[0], special_tokens::BOS);
        // Should have BOS + 4 words = 5 tokens
        assert_eq!(tokens.len(), 5);
    }

    #[test]
    fn stub_tokenize_unknown_words() {
        let stub = test_stub();
        let tokens = stub
            .tokenize(std::ptr::null_mut(), "xyzzy plugh")
            .expect("tokenize should succeed");
        // BOS + 2 UNK tokens
        assert_eq!(tokens.len(), 3);
        assert_eq!(tokens[0], special_tokens::BOS);
        assert_eq!(tokens[1], special_tokens::UNK);
        assert_eq!(tokens[2], special_tokens::UNK);
    }

    #[test]
    fn stub_detokenize_round_trip() {
        let stub = test_stub();
        let tokens = stub
            .tokenize(std::ptr::null_mut(), "open the settings")
            .expect("tokenize should succeed");
        let text = stub
            .detokenize(std::ptr::null_mut(), &tokens)
            .expect("detokenize should succeed");
        // Should contain the original words
        assert!(text.contains("open"));
        assert!(text.contains("the"));
        assert!(text.contains("settings"));
    }

    #[test]
    fn stub_sample_next_produces_valid_token() {
        let stub = test_stub();
        let tokens = vec![special_tokens::BOS];
        let params = SamplingParams::default();
        let next = stub
            .sample_next(std::ptr::null_mut(), &tokens, &params)
            .expect("sample should succeed");
        // Should be a valid token (not negative, within vocab range)
        assert!(next >= 0);
        assert!((next as usize) < stub.vocab.len());
    }

    #[test]
    fn stub_generates_sequence_of_tokens() {
        let stub = test_stub();
        let params = SamplingParams {
            max_tokens: 20,
            temperature: 0.7,
            ..Default::default()
        };

        let mut tokens = vec![special_tokens::BOS];
        let mut generated_count = 0;

        for _ in 0..20 {
            let next = stub
                .sample_next(std::ptr::null_mut(), &tokens, &params)
                .expect("sample should succeed");
            if next == special_tokens::EOS {
                break;
            }
            tokens.push(next);
            generated_count += 1;
        }

        assert!(generated_count > 0, "should generate at least one token");

        let text = stub
            .detokenize(std::ptr::null_mut(), &tokens)
            .expect("detokenize should succeed");
        assert!(!text.is_empty(), "generated text should not be empty");
    }

    #[test]
    fn stub_eos_bos_tokens() {
        let stub = test_stub();
        assert_eq!(stub.eos_token(), special_tokens::EOS);
        assert_eq!(stub.bos_token(), special_tokens::BOS);
    }

    #[test]
    fn stub_eval_succeeds() {
        let stub = test_stub();
        let tokens = vec![special_tokens::BOS, 3, 7]; // BOS, "the", "settings"
        let result = stub.eval(std::ptr::null_mut(), &tokens);
        assert!(result.is_ok());
    }

    #[test]
    fn stub_get_token_logprob_returns_none() {
        let stub = test_stub();
        // Stub backend has no logits — should always return None
        let result = stub.get_token_logprob(std::ptr::null_mut(), 3);
        assert!(result.is_none(), "stub should return None for logprob");
    }

    #[test]
    fn stub_free_model_no_panic() {
        let stub = test_stub();
        let (model, ctx) = stub
            .load_model(
                "test.gguf",
                &LlamaModelParams::default(),
                &LlamaContextParams::default(),
            )
            .expect("load should succeed");
        // Should not panic
        stub.free_model(model, ctx);
    }

    #[test]
    fn stub_repetition_penalty_avoids_repeats() {
        let stub = test_stub();
        let params = SamplingParams {
            repeat_penalty: 2.0,
            temperature: 0.3,
            ..Default::default()
        };

        // Feed a sequence where token 3 ("the") appears many times
        let tokens = vec![3, 3, 3, 3, 3, 3, 3, 3];
        let mut got_different = false;

        // Sample many times — with high penalty, we should sometimes get non-3 tokens
        for _ in 0..50 {
            let next = stub
                .sample_next(std::ptr::null_mut(), &tokens, &params)
                .expect("sample should succeed");
            if next != 3 {
                got_different = true;
                break;
            }
        }
        assert!(
            got_different,
            "repetition penalty should produce varied tokens"
        );
    }

    #[test]
    fn stub_temperature_zero_is_deterministic() {
        let stub1 = StubBackend::new(42);
        let stub2 = StubBackend::new(42);
        let params = SamplingParams {
            temperature: 0.01, // Near-greedy
            ..Default::default()
        };
        let tokens = vec![special_tokens::BOS];

        let t1 = stub1
            .sample_next(std::ptr::null_mut(), &tokens, &params)
            .expect("sample should succeed");
        let t2 = stub2
            .sample_next(std::ptr::null_mut(), &tokens, &params)
            .expect("sample should succeed");
        assert_eq!(
            t1, t2,
            "same seed + low temperature should produce same result"
        );
    }

    #[test]
    fn stub_detokenize_skips_special_tokens() {
        let stub = test_stub();
        let tokens = vec![special_tokens::BOS, 3, special_tokens::EOS]; // BOS, "the", EOS
        let text = stub
            .detokenize(std::ptr::null_mut(), &tokens)
            .expect("detokenize should succeed");
        assert_eq!(text, "the");
        assert!(!text.contains("<s>"));
        assert!(!text.contains("</s>"));
    }

    #[test]
    fn stub_empty_tokenize() {
        let stub = test_stub();
        let tokens = stub
            .tokenize(std::ptr::null_mut(), "")
            .expect("tokenize should succeed");
        // Just BOS
        assert_eq!(tokens.len(), 1);
        assert_eq!(tokens[0], special_tokens::BOS);
    }

    #[test]
    fn sampling_params_serialize() {
        let params = SamplingParams::default();
        let json = serde_json::to_string(&params).expect("should serialize");
        let restored: SamplingParams = serde_json::from_str(&json).expect("should deserialize");
        assert_eq!(params.temperature, restored.temperature);
        assert_eq!(params.max_tokens, restored.max_tokens);
    }

    #[cfg(not(target_os = "android"))]
    #[test]
    fn legacy_stubs_produce_tokens() {
        // Test backward compatibility
        let tokens = stubs::llama_tokenize(std::ptr::null_mut(), "open settings");
        assert!(!tokens.is_empty());

        let text = stubs::llama_decode_tokens(std::ptr::null_mut(), &tokens);
        // With stub backend, this will detokenize to words
        assert!(!text.is_empty() || tokens.iter().all(|&t| t <= 2)); // empty if all special
    }

    #[cfg(not(target_os = "android"))]
    #[test]
    fn legacy_stub_load_model() {
        let model = stubs::llama_load_model("test.gguf", &LlamaModelParams::default());
        assert!(!model.is_null(), "stub should return non-null sentinel");
    }
}
