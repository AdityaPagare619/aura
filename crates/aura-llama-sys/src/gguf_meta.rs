//! GGUF v2/v3 header metadata parser.
//!
//! Reads only the header of a GGUF file — no weights are loaded.
//! This lets AURA auto-detect model capabilities (context length, RAM estimate,
//! architecture, quantization) from whatever GGUF file the user drops in,
//! without committing to any specific filename or hardcoded defaults.
//!
//! # Supported versions
//! - GGUF v2 and v3 (n_tensors/n_kv are u64)
//! - GGUF v1 is deliberately rejected (pre-release, no public models use it)
//!
//! # Usage
//! ```no_run
//! use aura_llama_sys::gguf_meta::parse_gguf_meta;
//!
//! let meta = parse_gguf_meta("/data/models/my-model.gguf").unwrap();
//! println!("arch:         {}", meta.architecture);
//! println!("context:      {}", meta.effective_context());
//! println!("RAM estimate: {} MB", meta.ram_estimate_mb());
//! println!("thinking:     {}", meta.supports_thinking_mode());
//! ```

use std::{
    io::{BufReader, Read, Seek, SeekFrom},
    path::Path,
};

/// Magic bytes at the start of every GGUF file: "GGUF" in little-endian u32.
const GGUF_MAGIC: u32 = 0x46554747;

/// Maximum string length we will read from the header (16 MiB).
const MAX_STRING_BYTES: u64 = 16 * 1024 * 1024;

/// Maximum number of elements in a GGUF array KV value.
const MAX_ARRAY_ELEMENTS: u64 = 16_000_000;

/// Maximum number of KV pairs we will parse (sanity cap).
const MAX_KV_PAIRS: u64 = 100_000;

/// Maximum context length we will advertise on mobile (128K tokens).
/// Users who explicitly want more must override via config.
const MAX_MOBILE_CONTEXT: u32 = 131_072;

// ─── Error type ─────────────────────────────────────────────────────────────

/// Errors that can occur while parsing a GGUF header.
#[derive(Debug, thiserror::Error)]
pub enum GgufError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("not a GGUF file (bad magic: 0x{0:08X})")]
    BadMagic(u32),

    #[error("unsupported GGUF version: {0} (expected 2 or 3)")]
    UnsupportedVersion(u32),

    #[error("string too long: {0} bytes (max {MAX_STRING_BYTES})")]
    StringTooLong(u64),

    #[error("array too large: {0} elements (max {MAX_ARRAY_ELEMENTS})")]
    ArrayTooLarge(u64),

    #[error("too many KV pairs: {0} (max {MAX_KV_PAIRS})")]
    TooManyKv(u64),

    #[error("unknown GGUF value type: {0}")]
    UnknownType(u32),

    #[error("missing required field: {0}")]
    MissingField(&'static str),
}

// ─── GGUF value type codes ───────────────────────────────────────────────────

const GGUF_TYPE_UINT8: u32 = 0;
const GGUF_TYPE_INT8: u32 = 1;
const GGUF_TYPE_UINT16: u32 = 2;
const GGUF_TYPE_INT16: u32 = 3;
const GGUF_TYPE_UINT32: u32 = 4;
const GGUF_TYPE_INT32: u32 = 5;
const GGUF_TYPE_FLOAT32: u32 = 6;
const GGUF_TYPE_BOOL: u32 = 7;
const GGUF_TYPE_STRING: u32 = 8;
const GGUF_TYPE_ARRAY: u32 = 9;
const GGUF_TYPE_UINT64: u32 = 10;
const GGUF_TYPE_INT64: u32 = 11;
const GGUF_TYPE_FLOAT64: u32 = 12;

// ─── GgufMeta ────────────────────────────────────────────────────────────────

/// All metadata extracted from a GGUF header that AURA cares about.
///
/// Fields are `Option` when not present in the file; `effective_*()` methods
/// provide sensible defaults/clamps for missing values.
#[derive(Debug, Clone, Default)]
pub struct GgufMeta {
    // ── Identity ──────────────────────────────────────────────────────────
    /// `general.architecture` — e.g. "llama", "qwen2", "qwen3", "mistral".
    pub architecture: String,
    /// `general.name` — human-readable model name, if present.
    pub general_name: Option<String>,
    /// `general.file_type` — llama.cpp quantization enum (e.g. 15 = Q4_K_M).
    pub file_type: Option<u32>,

    // ── Architecture dims ─────────────────────────────────────────────────
    /// `{arch}.context_length` — trained context window in tokens.
    pub context_length: Option<u32>,
    /// `{arch}.embedding_length` — hidden state / residual stream dimension.
    pub embedding_length: Option<u32>,
    /// `{arch}.block_count` — number of transformer layers.
    pub block_count: Option<u32>,
    /// `{arch}.feed_forward_length` — FFN intermediate dimension.
    pub feed_forward_length: Option<u32>,
    /// `{arch}.attention.head_count` — number of query heads.
    pub attention_head_count: Option<u32>,
    /// `{arch}.attention.head_count_kv` — number of KV heads (GQA).
    pub attention_head_count_kv: Option<u32>,
    /// `{arch}.expert_count` — number of MoE experts, if present.
    pub expert_count: Option<u32>,

    // ── Tokenizer ─────────────────────────────────────────────────────────
    /// `tokenizer.chat_template` — Jinja2 template, used to detect thinking mode.
    pub chat_template: Option<String>,
    /// `tokenizer.ggml.model` — tokenizer model name, e.g. "gpt2", "llama".
    pub tokenizer_model: Option<String>,

    // ── Header counts ─────────────────────────────────────────────────────
    /// Total number of tensors declared in the header.
    pub n_tensors: u64,
    /// Total number of KV metadata pairs parsed.
    pub n_kv: u64,
}

impl GgufMeta {
    // ── Derived helpers ────────────────────────────────────────────────────

    /// Effective context length, clamped to `MAX_MOBILE_CONTEXT` (128K).
    ///
    /// Falls back to 4096 if the field is absent (conservative default).
    pub fn effective_context(&self) -> u32 {
        self.context_length.unwrap_or(4096).min(MAX_MOBILE_CONTEXT)
    }

    /// Estimated RAM usage in MB at the detected quantization level.
    ///
    /// Formula (conservative, GQA-aware):
    ///   weights ≈ (2×emb² + 2×emb×kv_dim + emb×ffn×3) × layers × bits/8 × 1.1
    ///   kv_cache ≈ 2 × layers × heads_kv × head_dim × practical_ctx × sizeof(fp16)
    ///
    /// Uses a practical mobile context (2048 tokens) for KV cache sizing rather
    /// than the model's full context window to match real on-device usage.
    ///
    /// Falls back to rough per-architecture heuristics when dims are missing.
    pub fn ram_estimate_mb(&self) -> u32 {
        let bits = self.quant_bits_per_weight();
        let emb = self.embedding_length.unwrap_or(0) as u64;
        let ffn = self
            .feed_forward_length
            .unwrap_or((emb * 4).min(u32::MAX as u64) as u32) as u64;
        let layers = self.block_count.unwrap_or(0) as u64;

        if emb == 0 || layers == 0 {
            // No dims: fall back to rough size from context_length heuristic
            return self.ram_fallback_mb();
        }

        // GQA-aware attention parameters:
        //   Q projection: emb × emb
        //   K projection: emb × (kv_heads × head_dim)
        //   V projection: emb × (kv_heads × head_dim)
        //   O projection: emb × emb
        // Total: 2 × emb² + 2 × emb × kv_dim
        let head_count_kv = self.attention_head_count_kv.unwrap_or(8) as u64;
        let head_count_q = self.attention_head_count.unwrap_or(32) as u64;
        let head_dim = if head_count_q > 0 {
            emb / head_count_q
        } else {
            64
        };
        let kv_dim = head_count_kv * head_dim;
        let attn_params: u64 = 2 * emb * emb + 2 * emb * kv_dim;
        let ffn_params: u64 = 3 * emb * ffn;
        let params_per_layer: u64 = attn_params + ffn_params;
        let total_params: u64 = params_per_layer * layers + 2 * emb * emb; // + embedding table

        let weight_bytes: u64 = (total_params as f64 * bits as f64 / 8.0) as u64;

        // KV cache: 2 (K+V) × layers × n_kv_heads × head_dim × context × 2 bytes (fp16)
        // Use a practical mobile context (2048) rather than the model's max context,
        // since mobile devices won't allocate the full context window at once.
        let practical_ctx: u64 = (self.effective_context() as u64).min(2048);
        let kv_cache_bytes: u64 = 2 * layers * head_count_kv * head_dim * practical_ctx * 2;

        // 10% overhead for activations, page tables, runtime state
        let total_bytes = (weight_bytes as f64 * 1.1) as u64 + kv_cache_bytes;
        let mb = (total_bytes / (1024 * 1024)) as u32;

        // Clamp to a reasonable range (models don't shrink below ~200 MB or exceed 48 GB)
        mb.max(200).min(49152)
    }

    /// Fallback RAM estimate when architecture dims are not present.
    fn ram_fallback_mb(&self) -> u32 {
        // Use context_length as a very rough size signal
        match self.context_length.unwrap_or(4096) {
            0..=4096 => 1500,
            4097..=8192 => 2500,
            8193..=32768 => 4000,
            _ => 5500,
        }
    }

    /// Bits per weight for the detected quantization type.
    ///
    /// Maps `general.file_type` (llama.cpp `llama_ftype`) to bits/weight.
    /// Returns 4.5 (Q4_K_M equivalent) when file_type is absent.
    pub fn quant_bits_per_weight(&self) -> f32 {
        match self.file_type {
            Some(0) => 32.0,    // F32
            Some(1) => 16.0,    // F16
            Some(2) => 4.0,     // Q4_0
            Some(3) => 4.0,     // Q4_1
            Some(6) => 5.0,     // Q5_0
            Some(7) => 5.0,     // Q5_1
            Some(8) => 8.0,     // Q8_0
            Some(10) => 2.0,    // Q2_K
            Some(11) => 3.0,    // Q3_K_S
            Some(12) => 3.0,    // Q3_K_M
            Some(13) => 3.0,    // Q3_K_L
            Some(14) => 4.0,    // Q4_K_S
            Some(15) => 4.5,    // Q4_K_M  ← most common
            Some(16) => 5.0,    // Q5_K_S
            Some(17) => 5.5,    // Q5_K_M
            Some(18) => 6.5,    // Q6_K
            Some(19) => 8.0,    // Q8_K (full)
            Some(20) => 2.0,    // IQ2_XXS
            Some(21) => 2.5,    // IQ2_XS
            Some(24) => 3.0,    // IQ3_XXS
            Some(25) => 1.5,    // IQ1_S
            Some(26) => 4.0,    // IQ4_NL
            Some(27) => 3.0,    // IQ3_S
            Some(28) => 3.0,    // IQ3_M
            Some(29) => 2.5,    // IQ2_S
            Some(30) => 2.5,    // IQ2_M
            Some(31) => 6.5,    // IQ4_XS (close to Q6)
            Some(36) => 1.5,    // IQ1_M
            Some(1024) => 16.0, // MOSTLY_F16 (some layers F16, rest Q4)
            Some(2048) => 32.0, // ALL_F32
            _ => 4.5,           // safe default — Q4_K_M equivalent
        }
    }

    /// Human-readable quantization name, e.g. "Q4_K_M".
    pub fn quant_name(&self) -> &'static str {
        match self.file_type {
            Some(0) => "F32",
            Some(1) => "F16",
            Some(2) => "Q4_0",
            Some(3) => "Q4_1",
            Some(6) => "Q5_0",
            Some(7) => "Q5_1",
            Some(8) => "Q8_0",
            Some(10) => "Q2_K",
            Some(11) => "Q3_K_S",
            Some(12) => "Q3_K_M",
            Some(13) => "Q3_K_L",
            Some(14) => "Q4_K_S",
            Some(15) => "Q4_K_M",
            Some(16) => "Q5_K_S",
            Some(17) => "Q5_K_M",
            Some(18) => "Q6_K",
            Some(19) => "Q8_K",
            Some(20) => "IQ2_XXS",
            Some(21) => "IQ2_XS",
            Some(24) => "IQ3_XXS",
            Some(25) => "IQ1_S",
            Some(26) => "IQ4_NL",
            Some(27) => "IQ3_S",
            Some(28) => "IQ3_M",
            Some(29) => "IQ2_S",
            Some(30) => "IQ2_M",
            Some(31) => "IQ4_XS",
            Some(36) => "IQ1_M",
            Some(1024) => "MOSTLY_F16",
            Some(2048) => "ALL_F32",
            _ => "unknown",
        }
    }

    /// Whether this model likely supports Qwen3/3.5 thinking mode (`/think` prefix).
    ///
    /// Detected by architecture string starting with "qwen3" (case-insensitive),
    /// OR by `<think>` appearing in the chat template.
    pub fn supports_thinking_mode(&self) -> bool {
        if self.architecture.to_ascii_lowercase().starts_with("qwen3") {
            return true;
        }
        if let Some(tmpl) = &self.chat_template {
            return tmpl.contains("<think>") || tmpl.contains("thinking");
        }
        false
    }

    /// Whether this model uses Mixture-of-Experts.
    pub fn is_moe(&self) -> bool {
        self.expert_count.map(|n| n > 1).unwrap_or(false)
    }

    /// Whether this model uses Grouped Query Attention (KV heads < Q heads).
    pub fn is_gqa(&self) -> bool {
        match (self.attention_head_count, self.attention_head_count_kv) {
            (Some(q), Some(kv)) => kv < q,
            _ => false,
        }
    }

    /// GQA ratio (Q heads / KV heads). Returns 1.0 for MHA or when unknown.
    pub fn gqa_ratio(&self) -> f32 {
        match (self.attention_head_count, self.attention_head_count_kv) {
            (Some(q), Some(kv)) if kv > 0 => q as f32 / kv as f32,
            _ => 1.0,
        }
    }

    /// A short human-readable display name for this model.
    ///
    /// Prefers `general.name`, then constructs one from architecture + quant.
    pub fn display_name(&self) -> String {
        if let Some(name) = &self.general_name {
            if !name.is_empty() {
                return name.clone();
            }
        }
        let arch = if self.architecture.is_empty() {
            "unknown"
        } else {
            &self.architecture
        };
        format!("{}-{}", arch, self.quant_name())
    }
}

// ─── Public parse API ────────────────────────────────────────────────────────

/// Parse GGUF header metadata from a file at the given path.
///
/// Opens the file, reads only the header section (no tensors/weights),
/// and returns a `GgufMeta` populated with whatever fields were found.
///
/// # Errors
/// Returns `GgufError` if the file is not a valid GGUF v2/v3 file,
/// or if I/O fails.
pub fn parse_gguf_meta<P: AsRef<Path>>(path: P) -> Result<GgufMeta, GgufError> {
    let file = std::fs::File::open(path)?;
    let mut reader = BufReader::new(file);
    parse_from_reader(&mut reader)
}

/// Parse GGUF header metadata from any `Read + Seek` source.
///
/// Same as `parse_gguf_meta` but accepts an arbitrary reader,
/// useful for testing with in-memory buffers.
pub fn parse_from_reader<R: Read + Seek>(reader: &mut R) -> Result<GgufMeta, GgufError> {
    // ── Header ────────────────────────────────────────────────────────────
    let magic = read_u32_le(reader)?;
    if magic != GGUF_MAGIC {
        return Err(GgufError::BadMagic(magic));
    }

    let version = read_u32_le(reader)?;
    if version < 2 || version > 3 {
        return Err(GgufError::UnsupportedVersion(version));
    }

    let n_tensors = read_u64_le(reader)?;
    let n_kv = read_u64_le(reader)?;

    if n_kv > MAX_KV_PAIRS {
        return Err(GgufError::TooManyKv(n_kv));
    }

    let mut meta = GgufMeta {
        n_tensors,
        n_kv,
        ..Default::default()
    };

    // ── KV pairs ──────────────────────────────────────────────────────────
    for _ in 0..n_kv {
        let key = read_gguf_string(reader)?;
        let val_type = read_u32_le(reader)?;

        // Dispatch on key suffix to handle architecture-prefixed keys
        // (e.g. "llama.context_length", "qwen2.context_length" both map to context_length).
        let key_lower = key.to_ascii_lowercase();

        match val_type {
            GGUF_TYPE_STRING => {
                let val = read_gguf_string(reader)?;
                apply_string_kv(&mut meta, &key_lower, val);
            },
            GGUF_TYPE_UINT32 => {
                let val = read_u32_le(reader)?;
                apply_u32_kv(&mut meta, &key_lower, val);
            },
            GGUF_TYPE_UINT64 => {
                let val = read_u64_le(reader)?;
                apply_u64_kv(&mut meta, &key_lower, val);
            },
            GGUF_TYPE_INT32 => {
                let val = read_i32_le(reader)?;
                apply_i32_kv(&mut meta, &key_lower, val);
            },
            GGUF_TYPE_INT64 => {
                let val = read_i64_le(reader)?;
                // No i64 fields we care about — skip
                let _ = val;
            },
            GGUF_TYPE_FLOAT32 => {
                let val = read_f32_le(reader)?;
                // No f32 fields we care about — skip
                let _ = val;
            },
            GGUF_TYPE_FLOAT64 => {
                let val = read_f64_le(reader)?;
                let _ = val;
            },
            GGUF_TYPE_BOOL => {
                let val = read_u8(reader)?;
                let _ = val;
            },
            GGUF_TYPE_UINT8 => {
                let val = read_u8(reader)?;
                let _ = val;
            },
            GGUF_TYPE_INT8 => {
                let val = read_u8(reader)?;
                let _ = val;
            },
            GGUF_TYPE_UINT16 => {
                let val = read_u16_le(reader)?;
                let _ = val;
            },
            GGUF_TYPE_INT16 => {
                let val = read_u16_le(reader)?;
                let _ = val;
            },
            GGUF_TYPE_ARRAY => {
                skip_gguf_array(reader)?;
            },
            other => {
                return Err(GgufError::UnknownType(other));
            },
        }
    }

    Ok(meta)
}

// ─── KV dispatch helpers ──────────────────────────────────────────────────────

fn apply_string_kv(meta: &mut GgufMeta, key: &str, val: String) {
    if key == "general.architecture" {
        meta.architecture = val;
    } else if key == "general.name" {
        meta.general_name = Some(val);
    } else if key == "tokenizer.chat_template" {
        meta.chat_template = Some(val);
    } else if key == "tokenizer.ggml.model" {
        meta.tokenizer_model = Some(val);
    }
}

fn apply_u32_kv(meta: &mut GgufMeta, key: &str, val: u32) {
    if key == "general.file_type" {
        meta.file_type = Some(val);
    } else if key.ends_with(".context_length") {
        meta.context_length = Some(val);
    } else if key.ends_with(".embedding_length") {
        meta.embedding_length = Some(val);
    } else if key.ends_with(".block_count") {
        meta.block_count = Some(val);
    } else if key.ends_with(".feed_forward_length") {
        meta.feed_forward_length = Some(val);
    } else if key.ends_with(".attention.head_count") {
        meta.attention_head_count = Some(val);
    } else if key.ends_with(".attention.head_count_kv") {
        meta.attention_head_count_kv = Some(val);
    } else if key.ends_with(".expert_count") {
        meta.expert_count = Some(val);
    }
}

fn apply_u64_kv(meta: &mut GgufMeta, key: &str, val: u64) {
    // Some files use u64 for dimension fields
    if key.ends_with(".context_length") {
        meta.context_length = Some(val.min(u32::MAX as u64) as u32);
    } else if key.ends_with(".embedding_length") {
        meta.embedding_length = Some(val.min(u32::MAX as u64) as u32);
    } else if key.ends_with(".block_count") {
        meta.block_count = Some(val.min(u32::MAX as u64) as u32);
    } else if key.ends_with(".feed_forward_length") {
        meta.feed_forward_length = Some(val.min(u32::MAX as u64) as u32);
    } else if key.ends_with(".attention.head_count") {
        meta.attention_head_count = Some(val.min(u32::MAX as u64) as u32);
    } else if key.ends_with(".attention.head_count_kv") {
        meta.attention_head_count_kv = Some(val.min(u32::MAX as u64) as u32);
    }
}

fn apply_i32_kv(meta: &mut GgufMeta, key: &str, val: i32) {
    // Some quant tools write file_type as i32
    if key == "general.file_type" && val >= 0 {
        meta.file_type = Some(val as u32);
    }
}

// ─── Array skip ───────────────────────────────────────────────────────────────

/// Skip over a GGUF array value (array of scalars or strings).
///
/// We read element_type + count, then skip `count` elements.
/// This keeps the reader positioned at the next KV pair.
fn skip_gguf_array<R: Read + Seek>(reader: &mut R) -> Result<(), GgufError> {
    let elem_type = read_u32_le(reader)?;
    let count = read_u64_le(reader)?;

    if count > MAX_ARRAY_ELEMENTS {
        return Err(GgufError::ArrayTooLarge(count));
    }

    let elem_size: u64 = match elem_type {
        GGUF_TYPE_UINT8 | GGUF_TYPE_INT8 | GGUF_TYPE_BOOL => 1,
        GGUF_TYPE_UINT16 | GGUF_TYPE_INT16 => 2,
        GGUF_TYPE_UINT32 | GGUF_TYPE_INT32 | GGUF_TYPE_FLOAT32 => 4,
        GGUF_TYPE_UINT64 | GGUF_TYPE_INT64 | GGUF_TYPE_FLOAT64 => 8,
        GGUF_TYPE_STRING => {
            // Strings are variable-length: must iterate
            for _ in 0..count {
                let len = read_u64_le(reader)?;
                if len > MAX_STRING_BYTES {
                    return Err(GgufError::StringTooLong(len));
                }
                reader.seek(SeekFrom::Current(len as i64))?;
            }
            return Ok(());
        },
        GGUF_TYPE_ARRAY => {
            // Nested array: recurse for each element
            for _ in 0..count {
                skip_gguf_array(reader)?;
            }
            return Ok(());
        },
        other => return Err(GgufError::UnknownType(other)),
    };

    reader.seek(SeekFrom::Current((count * elem_size) as i64))?;
    Ok(())
}

// ─── Primitive readers ────────────────────────────────────────────────────────

fn read_u8<R: Read>(r: &mut R) -> Result<u8, GgufError> {
    let mut buf = [0u8; 1];
    r.read_exact(&mut buf)?;
    Ok(buf[0])
}

fn read_u16_le<R: Read>(r: &mut R) -> Result<u16, GgufError> {
    let mut buf = [0u8; 2];
    r.read_exact(&mut buf)?;
    Ok(u16::from_le_bytes(buf))
}

fn read_u32_le<R: Read>(r: &mut R) -> Result<u32, GgufError> {
    let mut buf = [0u8; 4];
    r.read_exact(&mut buf)?;
    Ok(u32::from_le_bytes(buf))
}

fn read_i32_le<R: Read>(r: &mut R) -> Result<i32, GgufError> {
    let mut buf = [0u8; 4];
    r.read_exact(&mut buf)?;
    Ok(i32::from_le_bytes(buf))
}

fn read_u64_le<R: Read>(r: &mut R) -> Result<u64, GgufError> {
    let mut buf = [0u8; 8];
    r.read_exact(&mut buf)?;
    Ok(u64::from_le_bytes(buf))
}

fn read_i64_le<R: Read>(r: &mut R) -> Result<i64, GgufError> {
    let mut buf = [0u8; 8];
    r.read_exact(&mut buf)?;
    Ok(i64::from_le_bytes(buf))
}

fn read_f32_le<R: Read>(r: &mut R) -> Result<f32, GgufError> {
    let mut buf = [0u8; 4];
    r.read_exact(&mut buf)?;
    Ok(f32::from_le_bytes(buf))
}

fn read_f64_le<R: Read>(r: &mut R) -> Result<f64, GgufError> {
    let mut buf = [0u8; 8];
    r.read_exact(&mut buf)?;
    Ok(f64::from_le_bytes(buf))
}

/// Read a GGUF string: u64 length prefix + raw UTF-8 bytes (no NUL terminator).
fn read_gguf_string<R: Read + Seek>(r: &mut R) -> Result<String, GgufError> {
    let len = read_u64_le(r)?;
    if len > MAX_STRING_BYTES {
        return Err(GgufError::StringTooLong(len));
    }
    let mut buf = vec![0u8; len as usize];
    r.read_exact(&mut buf)?;
    // Use lossy conversion — we only care about ASCII fields anyway
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;

    // ── Mini GGUF builder for tests ──────────────────────────────────────

    struct GgufBuilder {
        buf: Vec<u8>,
        kv_count: u64,
    }

    impl GgufBuilder {
        fn new(version: u32, n_tensors: u64) -> Self {
            let mut buf = Vec::new();
            buf.extend_from_slice(&GGUF_MAGIC.to_le_bytes());
            buf.extend_from_slice(&version.to_le_bytes());
            buf.extend_from_slice(&n_tensors.to_le_bytes());
            // Placeholder for n_kv (will be patched in finish())
            buf.extend_from_slice(&0u64.to_le_bytes());
            Self { buf, kv_count: 0 }
        }

        fn add_string(mut self, key: &str, val: &str) -> Self {
            self.write_gguf_string(key);
            self.buf.extend_from_slice(&GGUF_TYPE_STRING.to_le_bytes());
            self.write_gguf_string(val);
            self.kv_count += 1;
            self
        }

        fn add_u32(mut self, key: &str, val: u32) -> Self {
            self.write_gguf_string(key);
            self.buf.extend_from_slice(&GGUF_TYPE_UINT32.to_le_bytes());
            self.buf.extend_from_slice(&val.to_le_bytes());
            self.kv_count += 1;
            self
        }

        fn add_i32(mut self, key: &str, val: i32) -> Self {
            self.write_gguf_string(key);
            self.buf.extend_from_slice(&GGUF_TYPE_INT32.to_le_bytes());
            self.buf.extend_from_slice(&val.to_le_bytes());
            self.kv_count += 1;
            self
        }

        fn add_u64(mut self, key: &str, val: u64) -> Self {
            self.write_gguf_string(key);
            self.buf.extend_from_slice(&GGUF_TYPE_UINT64.to_le_bytes());
            self.buf.extend_from_slice(&val.to_le_bytes());
            self.kv_count += 1;
            self
        }

        fn add_u32_array(mut self, key: &str, vals: &[u32]) -> Self {
            self.write_gguf_string(key);
            self.buf.extend_from_slice(&GGUF_TYPE_ARRAY.to_le_bytes());
            self.buf.extend_from_slice(&GGUF_TYPE_UINT32.to_le_bytes());
            self.buf
                .extend_from_slice(&(vals.len() as u64).to_le_bytes());
            for v in vals {
                self.buf.extend_from_slice(&v.to_le_bytes());
            }
            self.kv_count += 1;
            self
        }

        fn add_string_array(mut self, key: &str, vals: &[&str]) -> Self {
            self.write_gguf_string(key);
            self.buf.extend_from_slice(&GGUF_TYPE_ARRAY.to_le_bytes());
            self.buf.extend_from_slice(&GGUF_TYPE_STRING.to_le_bytes());
            self.buf
                .extend_from_slice(&(vals.len() as u64).to_le_bytes());
            for v in vals {
                self.write_gguf_string(v);
            }
            self.kv_count += 1;
            self
        }

        fn write_gguf_string(&mut self, s: &str) {
            let bytes = s.as_bytes();
            self.buf
                .extend_from_slice(&(bytes.len() as u64).to_le_bytes());
            self.buf.extend_from_slice(bytes);
        }

        fn finish(mut self) -> Vec<u8> {
            // Patch n_kv at offset 16 (magic=4, version=4, n_tensors=8)
            let kv_bytes = self.kv_count.to_le_bytes();
            self.buf[16..24].copy_from_slice(&kv_bytes);
            self.buf
        }
    }

    fn parse_bytes(bytes: Vec<u8>) -> Result<GgufMeta, GgufError> {
        let mut cursor = Cursor::new(bytes);
        parse_from_reader(&mut cursor)
    }

    // ── Tests ────────────────────────────────────────────────────────────

    #[test]
    fn rejects_bad_magic() {
        let mut buf = vec![0u8; 32];
        buf[0..4].copy_from_slice(&0xDEADBEEFu32.to_le_bytes());
        let err = parse_bytes(buf).unwrap_err();
        assert!(matches!(err, GgufError::BadMagic(_)));
    }

    #[test]
    fn rejects_version_1() {
        let bytes = GgufBuilder::new(1, 0).finish();
        let err = parse_bytes(bytes).unwrap_err();
        assert!(matches!(err, GgufError::UnsupportedVersion(1)));
    }

    #[test]
    fn rejects_version_4() {
        let bytes = GgufBuilder::new(4, 0).finish();
        let err = parse_bytes(bytes).unwrap_err();
        assert!(matches!(err, GgufError::UnsupportedVersion(4)));
    }

    #[test]
    fn accepts_version_2() {
        let bytes = GgufBuilder::new(2, 0).finish();
        assert!(parse_bytes(bytes).is_ok());
    }

    #[test]
    fn accepts_version_3() {
        let bytes = GgufBuilder::new(3, 0).finish();
        assert!(parse_bytes(bytes).is_ok());
    }

    #[test]
    fn parses_general_architecture() {
        let bytes = GgufBuilder::new(3, 0)
            .add_string("general.architecture", "qwen3")
            .finish();
        let meta = parse_bytes(bytes).unwrap();
        assert_eq!(meta.architecture, "qwen3");
    }

    #[test]
    fn parses_context_length_via_suffix() {
        let bytes = GgufBuilder::new(3, 0)
            .add_string("general.architecture", "llama")
            .add_u32("llama.context_length", 8192)
            .finish();
        let meta = parse_bytes(bytes).unwrap();
        assert_eq!(meta.context_length, Some(8192));
        assert_eq!(meta.effective_context(), 8192);
    }

    #[test]
    fn parses_context_length_different_arch() {
        let bytes = GgufBuilder::new(3, 0)
            .add_string("general.architecture", "qwen2")
            .add_u32("qwen2.context_length", 32768)
            .finish();
        let meta = parse_bytes(bytes).unwrap();
        assert_eq!(meta.context_length, Some(32768));
    }

    #[test]
    fn context_clamped_to_128k() {
        let bytes = GgufBuilder::new(3, 0)
            .add_u32("qwen3.context_length", 1_000_000)
            .finish();
        let meta = parse_bytes(bytes).unwrap();
        assert_eq!(meta.effective_context(), MAX_MOBILE_CONTEXT);
    }

    #[test]
    fn context_defaults_to_4096_when_missing() {
        let bytes = GgufBuilder::new(3, 0).finish();
        let meta = parse_bytes(bytes).unwrap();
        assert_eq!(meta.effective_context(), 4096);
    }

    #[test]
    fn parses_embedding_length() {
        let bytes = GgufBuilder::new(3, 0)
            .add_string("general.architecture", "llama")
            .add_u32("llama.embedding_length", 4096)
            .finish();
        let meta = parse_bytes(bytes).unwrap();
        assert_eq!(meta.embedding_length, Some(4096));
    }

    #[test]
    fn parses_block_count() {
        let bytes = GgufBuilder::new(3, 0)
            .add_string("general.architecture", "llama")
            .add_u32("llama.block_count", 32)
            .finish();
        let meta = parse_bytes(bytes).unwrap();
        assert_eq!(meta.block_count, Some(32));
    }

    #[test]
    fn parses_file_type() {
        let bytes = GgufBuilder::new(3, 0)
            .add_u32("general.file_type", 15) // Q4_K_M
            .finish();
        let meta = parse_bytes(bytes).unwrap();
        assert_eq!(meta.file_type, Some(15));
        assert_eq!(meta.quant_name(), "Q4_K_M");
        assert!((meta.quant_bits_per_weight() - 4.5).abs() < 0.01);
    }

    #[test]
    fn file_type_as_i32() {
        let bytes = GgufBuilder::new(3, 0)
            .add_i32("general.file_type", 15)
            .finish();
        let meta = parse_bytes(bytes).unwrap();
        assert_eq!(meta.file_type, Some(15));
    }

    #[test]
    fn parses_chat_template() {
        let bytes = GgufBuilder::new(3, 0)
            .add_string(
                "tokenizer.chat_template",
                "{% if thinking %}<think>{% endif %}",
            )
            .finish();
        let meta = parse_bytes(bytes).unwrap();
        assert!(meta.chat_template.is_some());
    }

    #[test]
    fn thinking_mode_detected_via_arch() {
        let bytes = GgufBuilder::new(3, 0)
            .add_string("general.architecture", "qwen3")
            .finish();
        let meta = parse_bytes(bytes).unwrap();
        assert!(meta.supports_thinking_mode());
    }

    #[test]
    fn thinking_mode_detected_via_template() {
        let bytes = GgufBuilder::new(3, 0)
            .add_string("general.architecture", "mistral")
            .add_string(
                "tokenizer.chat_template",
                "use <think> blocks for reasoning",
            )
            .finish();
        let meta = parse_bytes(bytes).unwrap();
        assert!(meta.supports_thinking_mode());
    }

    #[test]
    fn no_thinking_mode_for_llama() {
        let bytes = GgufBuilder::new(3, 0)
            .add_string("general.architecture", "llama")
            .finish();
        let meta = parse_bytes(bytes).unwrap();
        assert!(!meta.supports_thinking_mode());
    }

    #[test]
    fn skips_u32_array() {
        let bytes = GgufBuilder::new(3, 0)
            .add_u32_array("tokenizer.ggml.token_type", &[1, 2, 3, 1, 1])
            .add_string("general.architecture", "llama")
            .finish();
        let meta = parse_bytes(bytes).unwrap();
        assert_eq!(meta.architecture, "llama");
    }

    #[test]
    fn skips_string_array() {
        let bytes = GgufBuilder::new(3, 0)
            .add_string_array("tokenizer.ggml.tokens", &["<unk>", "<s>", "</s>"])
            .add_string("general.architecture", "qwen2")
            .finish();
        let meta = parse_bytes(bytes).unwrap();
        assert_eq!(meta.architecture, "qwen2");
    }

    #[test]
    fn ram_estimate_for_qwen_1_5b() {
        // Qwen1.5B dims: emb=1536, layers=28, ffn=8960, heads=12, kv_heads=2
        let bytes = GgufBuilder::new(3, 0)
            .add_string("general.architecture", "qwen2")
            .add_u32("qwen2.embedding_length", 1536)
            .add_u32("qwen2.block_count", 28)
            .add_u32("qwen2.feed_forward_length", 8960)
            .add_u32("qwen2.attention.head_count", 12)
            .add_u32("qwen2.attention.head_count_kv", 2)
            .add_u32("qwen2.context_length", 32768)
            .add_u32("general.file_type", 15) // Q4_K_M
            .finish();
        let meta = parse_bytes(bytes).unwrap();
        let ram = meta.ram_estimate_mb();
        // Should be in the range 600–1400 MB for a 1.5B Q4_K_M
        assert!(ram >= 600, "RAM estimate too low: {ram} MB");
        assert!(ram <= 1400, "RAM estimate too high: {ram} MB");
    }

    #[test]
    fn ram_estimate_for_qwen_8b() {
        // Qwen3 8B dims: emb=4096, layers=36, ffn=22016, heads=32, kv_heads=8
        let bytes = GgufBuilder::new(3, 0)
            .add_string("general.architecture", "qwen3")
            .add_u32("qwen3.embedding_length", 4096)
            .add_u32("qwen3.block_count", 36)
            .add_u32("qwen3.feed_forward_length", 22016)
            .add_u32("qwen3.attention.head_count", 32)
            .add_u32("qwen3.attention.head_count_kv", 8)
            .add_u32("qwen3.context_length", 32768)
            .add_u32("general.file_type", 15) // Q4_K_M
            .finish();
        let meta = parse_bytes(bytes).unwrap();
        let ram = meta.ram_estimate_mb();
        // Should be in the range 3500–7000 MB for an 8B Q4_K_M
        assert!(ram >= 3500, "RAM estimate too low: {ram} MB");
        assert!(ram <= 7000, "RAM estimate too high: {ram} MB");
    }

    #[test]
    fn gqa_detection() {
        let bytes = GgufBuilder::new(3, 0)
            .add_u32("qwen3.attention.head_count", 32)
            .add_u32("qwen3.attention.head_count_kv", 8)
            .finish();
        let meta = parse_bytes(bytes).unwrap();
        assert!(meta.is_gqa());
        assert!((meta.gqa_ratio() - 4.0).abs() < 0.01);
    }

    #[test]
    fn moe_detection() {
        let bytes = GgufBuilder::new(3, 0)
            .add_u32("qwen2moe.expert_count", 64)
            .finish();
        let meta = parse_bytes(bytes).unwrap();
        assert!(meta.is_moe());
    }

    #[test]
    fn display_name_uses_general_name() {
        let bytes = GgufBuilder::new(3, 0)
            .add_string("general.architecture", "qwen3")
            .add_string("general.name", "Qwen3-8B-Instruct-Q4_K_M")
            .finish();
        let meta = parse_bytes(bytes).unwrap();
        assert_eq!(meta.display_name(), "Qwen3-8B-Instruct-Q4_K_M");
    }

    #[test]
    fn display_name_fallback_when_no_general_name() {
        let bytes = GgufBuilder::new(3, 0)
            .add_string("general.architecture", "llama")
            .add_u32("general.file_type", 15)
            .finish();
        let meta = parse_bytes(bytes).unwrap();
        assert_eq!(meta.display_name(), "llama-Q4_K_M");
    }

    #[test]
    fn display_name_fully_unknown() {
        let bytes = GgufBuilder::new(3, 0).finish();
        let meta = parse_bytes(bytes).unwrap();
        assert!(meta.display_name().contains("unknown"));
    }

    #[test]
    fn u64_context_length_parsed() {
        let bytes = GgufBuilder::new(3, 0)
            .add_u64("llama.context_length", 65536u64)
            .finish();
        let meta = parse_bytes(bytes).unwrap();
        assert_eq!(meta.context_length, Some(65536));
    }

    #[test]
    fn n_tensors_n_kv_stored() {
        let bytes = GgufBuilder::new(3, 42)
            .add_string("general.architecture", "llama")
            .finish();
        let meta = parse_bytes(bytes).unwrap();
        assert_eq!(meta.n_tensors, 42);
        assert_eq!(meta.n_kv, 1);
    }
}
