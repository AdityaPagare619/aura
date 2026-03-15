use std::collections::HashMap;

use aura_types::{
    etg::{ActionPlan, EtgEdge, EtgNode},
    events::{Intent, ParsedEvent},
};
use serde::{Deserialize, Serialize};
use tracing::instrument;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum entries in the ETG lookup cache. Bounded to prevent OOM.
const MAX_CACHE_ENTRIES: usize = 256;

/// Minimum confidence required to return a cached plan.
const CONFIDENCE_THRESHOLD: f32 = 0.70;

/// Half-life for freshness decay in milliseconds (14 days).
const FRESHNESS_HALF_LIFE_MS: u64 = 14 * 24 * 60 * 60 * 1000;

/// Current ETG schema version — plans from older versions get a discount.
const CURRENT_ETG_VERSION: u32 = 1;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// A cached action plan with metadata for freshness and reliability tracking.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedPlan {
    /// The action plan steps and metadata.
    pub plan: ActionPlan,
    /// Aggregated path reliability from ETG edges (0.0–1.0).
    pub path_reliability: f32,
    /// Timestamp (ms) when this plan was last successfully executed.
    pub last_success_ms: u64,
    /// ETG schema version this plan was created under.
    pub etg_version: u32,
    /// Number of times this plan was successfully used.
    pub hit_count: u32,
}

/// Result of a System1 (fast-path, daemon-only) execution attempt.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct System1Result {
    pub success: bool,
    pub action_plan: Option<ActionPlan>,
    pub response_text: Option<String>,
    pub execution_time_ms: u64,
}

// System1 is a cache-hit fast path for ETG lookups — it does not generate responses

/// Fast path executor — handles events without invoking the Neocortex LLM.
///
/// Capabilities:
/// - ETG (Experience-Trace Graph) lookup for known action sequences.
/// - Routine event suppression.
///
/// System1 does NOT generate any response text. All user input that does not
/// hit a cached ETG must flow to the LLM (System2/Neocortex).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct System1 {
    /// Cached ETG nodes (populated by the execution engine at runtime).
    etg_nodes: Vec<EtgNode>,
    /// Cached ETG edges.
    etg_edges: Vec<EtgEdge>,
    /// Bounded cache: normalized key terms → cached action plan.
    /// Capacity hard-limited to `MAX_CACHE_ENTRIES`.
    plan_cache: HashMap<String, CachedPlan>,
}

// ---------------------------------------------------------------------------
// Implementation
// ---------------------------------------------------------------------------

impl System1 {
    pub fn new() -> Self {
        Self {
            etg_nodes: Vec::new(),
            etg_edges: Vec::new(),
            plan_cache: HashMap::new(),
        }
    }

    /// Attempt to resolve an event via the fast path.
    #[instrument(skip(self, event), fields(intent = ?event.intent))]
    pub fn execute(&self, event: &ParsedEvent, now_ms: u64) -> System1Result {
        match event.intent {
            Intent::RoutineEvent => {
                tracing::debug!(content = %event.content, "routine event suppressed by System1");
                return System1Result {
                    success: true,
                    action_plan: None,
                    response_text: None,
                    execution_time_ms: 0,
                };
            },
            Intent::ActionRequest => {
                // Try ETG lookup.
                if let Some(plan) = self.try_etg_lookup(&event.content, now_ms) {
                    return System1Result {
                        success: true,
                        action_plan: Some(plan),
                        response_text: None,
                        execution_time_ms: 0,
                    };
                }
            },
            _ => {},
        }

        // Fast path couldn't handle it.
        System1Result {
            success: false,
            action_plan: None,
            response_text: None,
            execution_time_ms: 0,
        }
    }

    /// Try to find a known action sequence in the ETG plan cache.
    ///
    /// Performs key-term extraction on the content, looks up the normalized
    /// key in the bounded cache, and applies freshness decay before returning.
    ///
    /// Confidence formula:
    /// ```text
    /// confidence = path_reliability × freshness(half_life=14d) × version_factor
    /// ```
    #[instrument(skip(self))]
    pub fn try_etg_lookup(&self, content: &str, now_ms: u64) -> Option<ActionPlan> {
        let cache_key = Self::extract_cache_key(content);

        let cached = self.plan_cache.get(&cache_key)?;

        // Compute freshness decay: exponential with 14-day half-life.
        let age_ms = now_ms.saturating_sub(cached.last_success_ms);
        let freshness = Self::freshness_decay(age_ms);

        // Version factor: discount plans from older ETG schema versions.
        let version_factor = if cached.etg_version == CURRENT_ETG_VERSION {
            1.0
        } else {
            0.8_f32.powi((CURRENT_ETG_VERSION.saturating_sub(cached.etg_version)) as i32)
        };

        let effective_confidence = cached.path_reliability * freshness * version_factor;

        tracing::debug!(
            cache_key = %cache_key,
            path_reliability = cached.path_reliability,
            freshness,
            version_factor,
            effective_confidence,
            threshold = CONFIDENCE_THRESHOLD,
            "ETG cache lookup"
        );

        if effective_confidence < CONFIDENCE_THRESHOLD {
            return None;
        }

        // Return a copy with the decayed confidence.
        let mut plan = cached.plan.clone();
        plan.confidence = effective_confidence;
        Some(plan)
    }

    /// Register a successful action plan in the cache for future fast-path use.
    ///
    /// If the cache is at capacity, evicts the least-recently-used entry
    /// (by `last_success_ms`).
    #[instrument(skip(self, plan))]
    pub fn cache_plan(
        &mut self,
        content: &str,
        plan: ActionPlan,
        path_reliability: f32,
        now_ms: u64,
    ) {
        let key = Self::extract_cache_key(content);

        // If already cached, update in-place.
        if let Some(existing) = self.plan_cache.get_mut(&key) {
            existing.plan = plan;
            existing.path_reliability = path_reliability;
            existing.last_success_ms = now_ms;
            existing.hit_count = existing.hit_count.saturating_add(1);
            return;
        }

        // Evict if at capacity — remove the entry with the oldest last_success_ms.
        if self.plan_cache.len() >= MAX_CACHE_ENTRIES {
            if let Some(oldest_key) = self
                .plan_cache
                .iter()
                .min_by_key(|(_, v)| v.last_success_ms)
                .map(|(k, _)| k.clone())
            {
                self.plan_cache.remove(&oldest_key);
                tracing::debug!(evicted = %oldest_key, "ETG cache eviction (LRU)");
            }
        }

        self.plan_cache.insert(
            key,
            CachedPlan {
                plan,
                path_reliability,
                last_success_ms: now_ms,
                etg_version: CURRENT_ETG_VERSION,
                hit_count: 1,
            },
        );
    }

    /// Number of plans currently in the cache.
    #[must_use]
    pub fn cache_size(&self) -> usize {
        self.plan_cache.len()
    }

    // -- Helpers --------------------------------------------------------------

    /// Extract a normalized cache key from content by lowercasing and keeping
    /// only semantically meaningful words, sorted for order-independence.
    ///
    /// Short words that carry semantic weight (e.g. "on", "off", "no") are
    /// preserved. Only true filler words ("a", "an", "the", etc.) are dropped.
    #[must_use]
    fn extract_cache_key(content: &str) -> String {
        /// Filler/stop words that never affect action semantics.
        const STOP_WORDS: &[&str] = &[
            "a", "an", "the", "and", "or", "but", "for", "nor", "yet", "of", "at", "by", "as",
        ];

        let lowered = content.to_ascii_lowercase();
        let mut words: Vec<&str> = lowered
            .split_whitespace()
            .filter(|w| !STOP_WORDS.contains(w))
            .collect();
        words.sort_unstable();
        words.dedup();
        words.join(" ")
    }

    /// Exponential freshness decay with configurable half-life.
    ///
    /// Returns a value in (0.0, 1.0] where 1.0 = just used, 0.5 = one half-life ago.
    #[must_use]
    fn freshness_decay(age_ms: u64) -> f32 {
        if age_ms == 0 {
            return 1.0;
        }
        // decay = 0.5^(age / half_life) = e^(-ln2 * age / half_life)
        let exponent = -std::f64::consts::LN_2 * (age_ms as f64 / FRESHNESS_HALF_LIFE_MS as f64);
        (exponent.exp() as f32).clamp(0.0, 1.0)
    }

    /// Register ETG nodes and edges (called by execution engine).
    #[instrument(skip(self, nodes, edges))]
    pub fn load_etg(&mut self, nodes: Vec<EtgNode>, edges: Vec<EtgEdge>) {
        self.etg_nodes = nodes;
        self.etg_edges = edges;
        tracing::info!(
            nodes = self.etg_nodes.len(),
            edges = self.etg_edges.len(),
            "System1 ETG cache loaded"
        );
    }
}

impl Default for System1 {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use aura_types::events::EventSource;

    use super::*;

    fn make_event(intent: Intent, content: &str) -> ParsedEvent {
        ParsedEvent {
            source: EventSource::UserCommand,
            intent,
            content: content.to_string(),
            entities: vec![],
            timestamp_ms: 1_000_000,
            raw_event_type: 0,
        }
    }

    #[test]
    fn test_etg_miss_returns_failure() {
        let s1 = System1::new();
        let event = make_event(Intent::ActionRequest, "open whatsapp");
        let result = s1.execute(&event, 1_000_000);
        assert!(!result.success, "ETG cache is empty, should fail");
    }

    #[test]
    fn test_unknown_conversation_not_handled() {
        // System1 only handles RoutineEvent and cached ActionRequest.
        // ConversationContinue must always fall through to the LLM (System2).
        let s1 = System1::new();
        let event = make_event(Intent::ConversationContinue, "I disagree with that");
        let result = s1.execute(&event, 1_000_000);
        assert!(
            !result.success,
            "ConversationContinue must not be handled by System1 — LLM decides"
        );
    }
}
