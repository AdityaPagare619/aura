//! Model geometry capabilities — single source of truth for all dimension values.
//!
//! `ModelCapabilities` is derived from GGUF metadata with a strict priority chain:
//!
//!   **GGUF metadata → user config override → device-probed defaults → compiled fallback**
//!
//! No step can be skipped. The embedding_dim, context_length, and other geometry
//! values are never hardcoded in inference paths — this struct is the only place
//! those values live at runtime.
//!
//! # Neuroscience analogy
//! Model geometry (embedding_dim, context_length) is equivalent to cortical column
//! dimensions. Wrong dimensions = catastrophic information collapse: the same as
//! giving a brain the wrong number of neurons per layer. The GGUF header is the
//! authoritative specification of the model's neural architecture — read it once
//! at startup, cache here, never re-read.
//!
//! # AI/ML invariant
//! `embedding_dim` MUST match what llama.cpp returns at inference time.
//! Mismatched dimensions produce garbage embeddings with no error signal.

use aura_llama_sys::GgufMeta;
use serde::{Deserialize, Serialize};
use tracing::warn;

// ─── Source provenance ───────────────────────────────────────────────────────

/// Records which priority tier provided a given capability value.
///
/// Exposed for observability/debugging — helps diagnose why a particular
/// embedding_dim was chosen (e.g., "why is this producing garbage embeddings?").
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ModelCapabilitySource {
    /// Value read directly from GGUF header metadata. Most authoritative.
    GgufMetadata,
    /// Value explicitly overridden by the user in `aura.config.toml`.
    /// Use only when GGUF metadata is absent or known-wrong.
    UserConfigOverride,
    /// Value derived from device probing (e.g., available RAM, detected arch).
    /// Not yet implemented — reserved for future device-adaptive tier selection.
    DeviceProbed,
    /// Compiled-in fallback values. Used only when GGUF parse fails completely.
    /// These are conservative modern-model defaults and may not match actual geometry.
    CompiledFallback,
}

// ─── ModelCapabilities ───────────────────────────────────────────────────────

/// Single source of truth for all model geometry at runtime.
///
/// Derived from GGUF metadata with user config overrides applied.
/// Every component that needs a model dimension (embedding_dim, context_length,
/// etc.) reads from this struct — never from hardcoded constants.
///
/// # Priority chain (strictly enforced in `from_gguf`)
/// 1. GGUF metadata  (`GgufMeta` fields)
/// 2. User config override (`user_override_embedding_dim` parameter)
/// 3. Device-probed defaults (not yet implemented, reserved)
/// 4. Compiled fallback (`fallback_defaults()`)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelCapabilities {
    /// Embedding dimension — the hidden state / residual stream dimension.
    ///
    /// CRITICAL: must match what llama.cpp actually produces at inference time.
    /// A mismatch silently produces garbage embeddings with no error signal.
    /// Source: GGUF `{arch}.embedding_length`.
    pub embedding_dim: u32,

    /// Maximum context window in tokens (clamped to 128K on mobile).
    /// Source: GGUF `{arch}.context_length` via `GgufMeta::effective_context()`.
    pub context_length: u32,

    /// Number of transformer layers (attention + FFN blocks).
    /// Source: GGUF `{arch}.block_count`.
    pub block_count: u32,

    /// Feed-forward network intermediate hidden dimension.
    /// Source: GGUF `{arch}.feed_forward_length`.
    pub feed_forward_length: u32,

    /// Model architecture family string (e.g. "llama", "qwen2", "qwen3", "mistral").
    /// Source: GGUF `general.architecture`.
    pub architecture: String,

    /// Which priority tier provided the `embedding_dim` value.
    /// Used for observability — log this at startup for diagnosability.
    pub embedding_dim_source: ModelCapabilitySource,
}

impl ModelCapabilities {
    // ── Constructors ──────────────────────────────────────────────────────

    /// Build `ModelCapabilities` from parsed GGUF metadata.
    ///
    /// Applies the full priority chain:
    /// 1. GGUF `embedding_length` field (most authoritative).
    /// 2. `user_override_embedding_dim` if the caller supplied one.
    /// 3. Compiled fallback (4096) with a `warn!` log if neither is available.
    ///
    /// All other geometry fields fall back to `fallback_defaults()` values
    /// when absent from GGUF — those fallbacks are logged individually.
    ///
    /// # Arguments
    /// - `meta` — parsed GGUF header metadata (source of truth).
    /// - `user_override_embedding_dim` — optional power-user override from `aura.config.toml`.
    ///   Applied only when GGUF metadata is absent or explicitly wrong. When present AND GGUF is
    ///   present, GGUF wins.
    pub fn from_gguf(meta: &GgufMeta, user_override_embedding_dim: Option<u32>) -> Self {
        let fallback = Self::fallback_defaults();

        // ── embedding_dim: GGUF > user override > compiled fallback ──────
        let (embedding_dim, embedding_dim_source) = match meta.embedding_length {
            Some(v) => {
                if let Some(override_val) = user_override_embedding_dim {
                    if override_val != v {
                        warn!(
                            gguf_value = v,
                            user_override = override_val,
                            "user_override_embedding_dim ignored — GGUF metadata takes priority; \
                             set override only when GGUF metadata is known-wrong"
                        );
                    }
                }
                (v, ModelCapabilitySource::GgufMetadata)
            },
            None => match user_override_embedding_dim {
                Some(v) => {
                    warn!(
                        override_value = v,
                        "GGUF embedding_length absent — using user config override; \
                         verify model file integrity"
                    );
                    (v, ModelCapabilitySource::UserConfigOverride)
                },
                None => {
                    warn!(
                        fallback_value = fallback.embedding_dim,
                        "GGUF embedding_length absent and no user override — \
                         using compiled fallback; embeddings may be wrong for this model"
                    );
                    (
                        fallback.embedding_dim,
                        ModelCapabilitySource::CompiledFallback,
                    )
                },
            },
        };

        // ── context_length: GGUF effective_context() with mobile clamp ───
        // GgufMeta::effective_context() already handles None → 4096 and
        // clamps to 128K. We use it directly.
        let context_length = meta.effective_context();
        if meta.context_length.is_none() {
            warn!(
                fallback_value = context_length,
                "GGUF context_length absent — using effective_context() fallback"
            );
        }

        // ── block_count ───────────────────────────────────────────────────
        let block_count = match meta.block_count {
            Some(v) => v,
            None => {
                warn!(
                    fallback_value = fallback.block_count,
                    "GGUF block_count absent — using compiled fallback"
                );
                fallback.block_count
            },
        };

        // ── feed_forward_length ───────────────────────────────────────────
        let feed_forward_length = match meta.feed_forward_length {
            Some(v) => v,
            None => {
                warn!(
                    fallback_value = fallback.feed_forward_length,
                    "GGUF feed_forward_length absent — using compiled fallback"
                );
                fallback.feed_forward_length
            },
        };

        // ── architecture ──────────────────────────────────────────────────
        let architecture = if meta.architecture.is_empty() {
            warn!("GGUF architecture absent — using 'unknown'");
            fallback.architecture
        } else {
            meta.architecture.clone()
        };

        Self {
            embedding_dim,
            context_length,
            block_count,
            feed_forward_length,
            architecture,
            embedding_dim_source,
        }
    }

    /// Compiled fallback values — used only when GGUF parse fails completely.
    ///
    /// These are calibrated to modern Qwen2/Qwen3 7B-class and Llama3 8B-class
    /// models — the most common on-device AURA targets as of 2025.
    ///
    /// **Do not use these for inference decisions.** They are last-resort
    /// defaults; every real model should produce proper GGUF metadata.
    pub fn fallback_defaults() -> Self {
        Self {
            // 4096: Qwen2-7B / Llama3-8B baseline.
            // Deliberately NOT 768 (BERT-era) — on-device models are 4B-8B class.
            embedding_dim: 4096,
            // 4096 tokens: conservative mobile context.
            context_length: 4096,
            // 32: standard for 7B/8B models (Llama3-8B has 32).
            block_count: 32,
            // 14336: Llama3-8B FFN dim. Qwen2-7B uses 11008.
            // We pick Llama3-8B as it's more common in GGUF ecosystem.
            feed_forward_length: 14336,
            architecture: "unknown".to_string(),
            embedding_dim_source: ModelCapabilitySource::CompiledFallback,
        }
    }

    // ── Derived helpers ───────────────────────────────────────────────────

    /// Whether this model's capabilities came entirely from GGUF (best case).
    pub fn is_fully_from_gguf(&self) -> bool {
        self.embedding_dim_source == ModelCapabilitySource::GgufMetadata
    }

    /// Human-readable summary for log output.
    pub fn summary(&self) -> String {
        format!(
            "arch={} emb_dim={} ctx={} layers={} ffn={} emb_src={:?}",
            self.architecture,
            self.embedding_dim,
            self.context_length,
            self.block_count,
            self.feed_forward_length,
            self.embedding_dim_source,
        )
    }
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn meta_with_all_dims() -> GgufMeta {
        GgufMeta {
            architecture: "qwen2".to_string(),
            embedding_length: Some(1536),
            context_length: Some(32768),
            block_count: Some(28),
            feed_forward_length: Some(8960),
            ..GgufMeta::default()
        }
    }

    fn meta_empty() -> GgufMeta {
        GgufMeta::default()
    }

    #[test]
    fn from_gguf_uses_metadata_fields() {
        let meta = meta_with_all_dims();
        let caps = ModelCapabilities::from_gguf(&meta, None);

        assert_eq!(caps.embedding_dim, 1536);
        assert_eq!(caps.context_length, 32768);
        assert_eq!(caps.block_count, 28);
        assert_eq!(caps.feed_forward_length, 8960);
        assert_eq!(caps.architecture, "qwen2");
        assert_eq!(
            caps.embedding_dim_source,
            ModelCapabilitySource::GgufMetadata
        );
    }

    #[test]
    fn from_gguf_gguf_wins_over_user_override() {
        // GGUF has embedding_length=1536; user tries to override with 768.
        // GGUF must win.
        let meta = meta_with_all_dims();
        let caps = ModelCapabilities::from_gguf(&meta, Some(768));

        assert_eq!(caps.embedding_dim, 1536);
        assert_eq!(
            caps.embedding_dim_source,
            ModelCapabilitySource::GgufMetadata
        );
    }

    #[test]
    fn from_gguf_user_override_when_gguf_absent() {
        let meta = meta_empty(); // no embedding_length
        let caps = ModelCapabilities::from_gguf(&meta, Some(2048));

        assert_eq!(caps.embedding_dim, 2048);
        assert_eq!(
            caps.embedding_dim_source,
            ModelCapabilitySource::UserConfigOverride
        );
    }

    #[test]
    fn from_gguf_compiled_fallback_when_nothing_available() {
        let meta = meta_empty();
        let caps = ModelCapabilities::from_gguf(&meta, None);

        // Must use compiled fallback, not 768 (BERT-era)
        assert_eq!(caps.embedding_dim, 4096);
        assert_eq!(
            caps.embedding_dim_source,
            ModelCapabilitySource::CompiledFallback
        );
    }

    #[test]
    fn fallback_defaults_not_768() {
        // Iron law: the compiled fallback must never be 768.
        // 768 is BERT-era; all modern on-device models use 1536-4096+.
        let caps = ModelCapabilities::fallback_defaults();
        assert_ne!(caps.embedding_dim, 768, "fallback must not be BERT-era 768");
        assert!(
            caps.embedding_dim >= 1024,
            "fallback must be modern model size"
        );
    }

    #[test]
    fn from_gguf_qwen_large_dims() {
        // Qwen3-8B: emb=4096, layers=36, ffn=22016
        let meta = GgufMeta {
            architecture: "qwen3".to_string(),
            embedding_length: Some(4096),
            context_length: Some(32768),
            block_count: Some(36),
            feed_forward_length: Some(22016),
            ..GgufMeta::default()
        };
        let caps = ModelCapabilities::from_gguf(&meta, None);

        assert_eq!(caps.embedding_dim, 4096);
        assert_eq!(caps.block_count, 36);
        assert_eq!(caps.feed_forward_length, 22016);
        assert_eq!(caps.architecture, "qwen3");
        assert!(caps.is_fully_from_gguf());
    }

    #[test]
    fn context_clamped_via_effective_context() {
        // GgufMeta::effective_context() clamps to 128K; verify we use it
        let meta = GgufMeta {
            context_length: Some(1_000_000),
            ..GgufMeta::default()
        };
        let caps = ModelCapabilities::from_gguf(&meta, None);
        assert!(
            caps.context_length <= 131_072,
            "context must be mobile-clamped"
        );
    }

    #[test]
    fn summary_contains_key_fields() {
        let caps = ModelCapabilities::fallback_defaults();
        let s = caps.summary();
        assert!(s.contains("emb_dim="));
        assert!(s.contains("ctx="));
        assert!(s.contains("layers="));
    }
}
