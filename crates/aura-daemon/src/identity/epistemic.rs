//! Epistemic awareness — AURA knows what it doesn't know.
//!
//! # Architecture (Concept Design §5.2 — Epistemic Humility)
//!
//! A truly intelligent assistant must distinguish between:
//!
//! 1. **Things it knows** (high confidence, verified information)
//! 2. **Things it believes** (moderate confidence, patterns and inferences)
//! 3. **Things it doesn't know** (knowledge gaps, out-of-date info)
//! 4. **Things it knows it can find out** (accessible via device capabilities)
//!
//! This module tracks AURA's epistemic state across knowledge domains
//! and provides signals for honest communication:
//!
//! - "I'm quite sure about this" (confident assertion)
//! - "Based on what I've seen, I think..." (belief with hedging)
//! - "I don't know, but I could look it up" (knowledge gap + capability)
//! - "I'm not sure and can't easily check" (real uncertainty)
//!
//! # Cross-Domain Insight (Polymath: Epistemology → Software)
//!
//! In philosophy, the Dunning-Kruger effect shows that incompetence breeds
//! overconfidence.  For AI assistants, the analogue is pattern-matching on
//! insufficient data and presenting guesses as facts.  This module prevents
//! that by tracking *how* AURA knows things, not just *what* it knows.
//!
//! # Day-1 vs Year-1
//!
//! - **Day 1**: Nearly everything is `Unknown` or `CanDiscover`.  AURA is honest about being new
//!   and learning.
//! - **Month 1**: Many user-preference domains shift to `Believes` as patterns accumulate.
//! - **Year 1**: Core routines and preferences are `Knows`.  AURA only hedges on genuinely
//!   ambiguous or novel situations.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use tracing::{debug, info};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Number of observations required before a domain can reach `Knows`.
const KNOWS_THRESHOLD: u32 = 20;

/// Number of observations required before a domain can reach `Believes`.
const BELIEVES_THRESHOLD: u32 = 5;

/// Confidence must exceed this for a domain to be `Knows`.
const KNOWS_CONFIDENCE: f32 = 0.85;

/// Confidence must exceed this for a domain to be `Believes`.
const BELIEVES_CONFIDENCE: f32 = 0.50;

/// Maximum number of knowledge domains tracked.
const MAX_DOMAINS: usize = 256;

/// Decay factor per day of inactivity (prevents stale knowledge from
/// being presented as confident).
const KNOWLEDGE_DECAY_PER_DAY: f32 = 0.995;

/// Milliseconds in one day.
const MS_PER_DAY: u64 = 24 * 60 * 60 * 1000;

// ---------------------------------------------------------------------------
// EpistemicLevel
// ---------------------------------------------------------------------------

/// How confident AURA is about a specific knowledge domain.
///
/// Ordered from least to most confident. Each level has communication
/// implications for how AURA phrases its responses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum EpistemicLevel {
    /// AURA has no information and cannot easily obtain it.
    /// Communication: "I don't know and I'm not sure how to find out."
    Unknown,

    /// AURA doesn't have the info but knows it could get it via device
    /// capabilities (internet search, app data, notifications, etc.).
    /// Communication: "I don't know yet, but I could check for you."
    CanDiscover,

    /// AURA has some signal but not enough to be confident.
    /// Communication: "Based on what I've seen so far, I think..."
    Believes,

    /// AURA has strong evidence from multiple observations over time.
    /// Communication: "I'm fairly confident that..."
    Knows,
}

impl EpistemicLevel {
    /// Generate a communication hedge appropriate for this level.
    #[must_use]
    pub fn hedge_phrase(&self) -> &'static str {
        match self {
            EpistemicLevel::Unknown => "I don't have information about this",
            EpistemicLevel::CanDiscover => "I'm not sure yet, but I could look into it",
            EpistemicLevel::Believes => "Based on what I've observed, I think",
            EpistemicLevel::Knows => "I'm fairly confident that",
        }
    }
}

// ---------------------------------------------------------------------------
// KnowledgeDomain
// ---------------------------------------------------------------------------

/// Tracks AURA's epistemic state for a specific knowledge domain.
///
/// A domain is any category of information AURA might know about:
/// user preferences, routines, contacts, app usage patterns, etc.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KnowledgeDomain {
    /// Domain name (e.g. "morning_routine", "music_preference", "work_schedule").
    pub name: String,
    /// Current epistemic level.
    pub level: EpistemicLevel,
    /// Confidence in current knowledge [0.0, 1.0].
    pub confidence: f32,
    /// Number of supporting observations.
    pub observation_count: u32,
    /// Number of times AURA's knowledge in this domain was wrong
    /// (prediction error or user correction).
    pub contradiction_count: u32,
    /// Whether AURA has device capabilities to discover info in this domain.
    pub discoverable: bool,
    /// Timestamp of last observation or update.
    pub last_updated_ms: u64,
    /// Timestamp of creation.
    pub created_ms: u64,
}

impl KnowledgeDomain {
    /// Create a new knowledge domain with unknown state.
    #[must_use]
    pub fn new(name: &str, discoverable: bool, now_ms: u64) -> Self {
        let level = if discoverable {
            EpistemicLevel::CanDiscover
        } else {
            EpistemicLevel::Unknown
        };
        Self {
            name: name.to_owned(),
            level,
            confidence: 0.0,
            observation_count: 0,
            contradiction_count: 0,
            discoverable,
            last_updated_ms: now_ms,
            created_ms: now_ms,
        }
    }

    /// Record a supporting observation (evidence that aligns with current
    /// knowledge).
    pub fn record_observation(&mut self, now_ms: u64) {
        self.observation_count = self.observation_count.saturating_add(1);
        self.last_updated_ms = now_ms;

        // Bayesian-style confidence update: push toward 1.0
        let alpha = 5.0_f32;
        self.confidence = (alpha * self.confidence + 1.0) / (alpha + 1.0);
        self.confidence = self.confidence.clamp(0.0, 1.0);

        self.update_level();
    }

    /// Record a contradiction (evidence that conflicts with current knowledge).
    ///
    /// This is critical for epistemic honesty: if AURA was wrong, its
    /// confidence in this domain should decrease, potentially dropping
    /// the epistemic level.
    pub fn record_contradiction(&mut self, now_ms: u64) {
        self.contradiction_count = self.contradiction_count.saturating_add(1);
        self.last_updated_ms = now_ms;

        // Stronger decay for contradictions than simple observation miss
        let alpha = 3.0_f32;
        self.confidence = (alpha * self.confidence) / (alpha + 1.0);
        self.confidence = self.confidence.clamp(0.0, 1.0);

        self.update_level();

        debug!(
            domain = %self.name,
            confidence = self.confidence,
            level = ?self.level,
            "knowledge contradiction recorded"
        );
    }

    /// Apply time-based decay to confidence.
    ///
    /// Knowledge that hasn't been reinforced recently should gradually
    /// lose confidence.  This prevents AURA from being confidently wrong
    /// about things that may have changed (e.g., user changed jobs).
    pub fn apply_decay(&mut self, now_ms: u64) {
        let days_inactive = now_ms.saturating_sub(self.last_updated_ms) as f64 / MS_PER_DAY as f64;
        self.confidence *= KNOWLEDGE_DECAY_PER_DAY.powf(days_inactive as f32);
        self.confidence = self.confidence.clamp(0.0, 1.0);
        self.update_level();
    }

    /// Recalculate epistemic level from current confidence and observation count.
    fn update_level(&mut self) {
        self.level =
            if self.confidence >= KNOWS_CONFIDENCE && self.observation_count >= KNOWS_THRESHOLD {
                EpistemicLevel::Knows
            } else if self.confidence >= BELIEVES_CONFIDENCE
                && self.observation_count >= BELIEVES_THRESHOLD
            {
                EpistemicLevel::Believes
            } else if self.discoverable {
                EpistemicLevel::CanDiscover
            } else {
                EpistemicLevel::Unknown
            };
    }

    /// The reliability ratio: observations / (observations + contradictions).
    /// Returns 1.0 when there are no contradictions, 0.0 when all observations
    /// are contradictions.
    #[must_use]
    pub fn reliability(&self) -> f32 {
        let total = self.observation_count + self.contradiction_count;
        if total == 0 {
            return 0.0;
        }
        self.observation_count as f32 / total as f32
    }
}

// ---------------------------------------------------------------------------
// EpistemicAwareness
// ---------------------------------------------------------------------------

/// The epistemic awareness system.
///
/// Tracks AURA's knowledge boundaries across all domains and provides
/// signals for honest, appropriately-hedged communication.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EpistemicAwareness {
    /// Knowledge domains indexed by name.
    domains: HashMap<String, KnowledgeDomain>,
}

impl EpistemicAwareness {
    /// Create a new epistemic awareness system.
    #[must_use]
    pub fn new() -> Self {
        Self {
            domains: HashMap::with_capacity(32),
        }
    }

    /// Get or create a knowledge domain.
    pub fn get_or_create(
        &mut self,
        domain_name: &str,
        discoverable: bool,
        now_ms: u64,
    ) -> &mut KnowledgeDomain {
        if !self.domains.contains_key(domain_name) {
            if self.domains.len() >= MAX_DOMAINS {
                // Evict the least-confident domain
                self.evict_weakest();
            }
            self.domains.insert(
                domain_name.to_owned(),
                KnowledgeDomain::new(domain_name, discoverable, now_ms),
            );
        }
        self.domains.get_mut(domain_name).expect("just inserted")
    }

    /// Query the epistemic level for a domain.
    ///
    /// Returns `Unknown` if the domain has never been observed.
    #[must_use]
    pub fn level_for(&self, domain_name: &str) -> EpistemicLevel {
        self.domains
            .get(domain_name)
            .map(|d| d.level)
            .unwrap_or(EpistemicLevel::Unknown)
    }

    /// Get a domain by name (immutable).
    #[must_use]
    pub fn get_domain(&self, domain_name: &str) -> Option<&KnowledgeDomain> {
        self.domains.get(domain_name)
    }

    /// Number of tracked domains.
    #[must_use]
    pub fn domain_count(&self) -> usize {
        self.domains.len()
    }

    /// All domains at a specific epistemic level.
    #[must_use]
    pub fn domains_at_level(&self, level: EpistemicLevel) -> Vec<&KnowledgeDomain> {
        self.domains.values().filter(|d| d.level == level).collect()
    }

    /// Age all domains by applying time decay.
    ///
    /// Returns number of domains that dropped in epistemic level.
    pub fn age_all(&mut self, now_ms: u64) -> usize {
        let mut drops = 0;
        for domain in self.domains.values_mut() {
            let before = domain.level;
            domain.apply_decay(now_ms);
            if domain.level < before {
                drops += 1;
                debug!(
                    domain = %domain.name,
                    from = ?before,
                    to = ?domain.level,
                    "knowledge level dropped due to aging"
                );
            }
        }
        if drops > 0 {
            info!(drops, "epistemic aging pass complete");
        }
        drops
    }

    /// Generate a summary of AURA's epistemic state.
    #[must_use]
    pub fn summary(&self) -> EpistemicSummary {
        let mut knows = 0usize;
        let mut believes = 0usize;
        let mut can_discover = 0usize;
        let mut unknown = 0usize;

        for domain in self.domains.values() {
            match domain.level {
                EpistemicLevel::Knows => knows += 1,
                EpistemicLevel::Believes => believes += 1,
                EpistemicLevel::CanDiscover => can_discover += 1,
                EpistemicLevel::Unknown => unknown += 1,
            }
        }

        let mean_reliability = if self.domains.is_empty() {
            0.0
        } else {
            let sum: f32 = self.domains.values().map(|d| d.reliability()).sum();
            sum / self.domains.len() as f32
        };

        EpistemicSummary {
            total_domains: self.domains.len(),
            knows,
            believes,
            can_discover,
            unknown,
            mean_reliability,
        }
    }

    /// Evict the domain with lowest confidence to make room.
    fn evict_weakest(&mut self) {
        if let Some(weakest) = self
            .domains
            .iter()
            .min_by(|a, b| {
                a.1.confidence
                    .partial_cmp(&b.1.confidence)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .map(|(k, _)| k.clone())
        {
            self.domains.remove(&weakest);
        }
    }
}

impl Default for EpistemicAwareness {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// EpistemicSummary
// ---------------------------------------------------------------------------

/// A snapshot of AURA's epistemic state for reporting and introspection.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EpistemicSummary {
    pub total_domains: usize,
    pub knows: usize,
    pub believes: usize,
    pub can_discover: usize,
    pub unknown: usize,
    pub mean_reliability: f32,
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_domain_is_unknown() {
        let domain = KnowledgeDomain::new("test", false, 1000);
        assert_eq!(domain.level, EpistemicLevel::Unknown);
        assert_eq!(domain.confidence, 0.0);
    }

    #[test]
    fn test_discoverable_domain_starts_at_can_discover() {
        let domain = KnowledgeDomain::new("test", true, 1000);
        assert_eq!(domain.level, EpistemicLevel::CanDiscover);
    }

    #[test]
    fn test_observations_increase_confidence() {
        let mut domain = KnowledgeDomain::new("routine", true, 1000);
        for i in 0..10 {
            domain.record_observation(1000 + i);
        }
        assert!(
            domain.confidence > 0.5,
            "10 observations should build confidence, got {}",
            domain.confidence
        );
        assert_eq!(domain.level, EpistemicLevel::Believes);
    }

    #[test]
    fn test_many_observations_reach_knows() {
        let mut domain = KnowledgeDomain::new("morning_coffee", true, 1000);
        for i in 0..50 {
            domain.record_observation(1000 + i);
        }
        assert_eq!(domain.level, EpistemicLevel::Knows);
        assert!(domain.confidence > KNOWS_CONFIDENCE);
    }

    #[test]
    fn test_contradictions_reduce_confidence() {
        let mut domain = KnowledgeDomain::new("preference", true, 1000);
        // Build up
        for i in 0..30 {
            domain.record_observation(1000 + i);
        }
        let before = domain.confidence;

        // Contradict
        domain.record_contradiction(2000);
        assert!(
            domain.confidence < before,
            "contradiction should reduce confidence"
        );
    }

    #[test]
    fn test_sustained_contradictions_drop_level() {
        let mut domain = KnowledgeDomain::new("schedule", true, 1000);
        // Build up to Knows
        for i in 0..50 {
            domain.record_observation(1000 + i);
        }
        assert_eq!(domain.level, EpistemicLevel::Knows);

        // Sustained contradictions should drop level
        for i in 0..20 {
            domain.record_contradiction(2000 + i);
        }
        assert!(
            domain.level < EpistemicLevel::Knows,
            "sustained contradictions should drop from Knows"
        );
    }

    #[test]
    fn test_reliability_ratio() {
        let mut domain = KnowledgeDomain::new("test", false, 1000);
        for i in 0..8 {
            domain.record_observation(1000 + i);
        }
        for i in 0..2 {
            domain.record_contradiction(2000 + i);
        }
        let reliability = domain.reliability();
        assert!(
            (reliability - 0.8).abs() < 0.01,
            "8 obs + 2 contradictions → reliability 0.8, got {}",
            reliability
        );
    }

    #[test]
    fn test_hedge_phrases() {
        assert_eq!(
            EpistemicLevel::Unknown.hedge_phrase(),
            "I don't have information about this"
        );
        assert_eq!(
            EpistemicLevel::Knows.hedge_phrase(),
            "I'm fairly confident that"
        );
    }

    #[test]
    fn test_awareness_system() {
        let mut awareness = EpistemicAwareness::new();
        let domain = awareness.get_or_create("coffee_time", true, 1000);
        for i in 0..30 {
            domain.record_observation(1000 + i);
        }

        assert_eq!(awareness.level_for("coffee_time"), EpistemicLevel::Knows);
        assert_eq!(awareness.level_for("nonexistent"), EpistemicLevel::Unknown);
    }

    #[test]
    fn test_summary() {
        let mut awareness = EpistemicAwareness::new();

        // Create domains at different levels
        let d = awareness.get_or_create("known", true, 1000);
        for i in 0..50 {
            d.record_observation(1000 + i);
        }

        let d = awareness.get_or_create("believed", true, 1000);
        for i in 0..10 {
            d.record_observation(1000 + i);
        }

        awareness.get_or_create("discoverable", true, 1000);
        awareness.get_or_create("unknown", false, 1000);

        let summary = awareness.summary();
        assert_eq!(summary.total_domains, 4);
        assert_eq!(summary.knows, 1);
        assert_eq!(summary.believes, 1);
        assert_eq!(summary.can_discover, 1);
        assert_eq!(summary.unknown, 1);
    }

    #[test]
    fn test_serde_roundtrip() {
        let mut awareness = EpistemicAwareness::new();
        let d = awareness.get_or_create("test", true, 1000);
        d.record_observation(2000);

        let json = serde_json::to_string(&awareness).expect("serialize");
        let back: EpistemicAwareness = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.domain_count(), 1);
        assert_eq!(back.level_for("test"), EpistemicLevel::CanDiscover);
    }
}
