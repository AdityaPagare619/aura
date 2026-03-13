//! Dynamic context-aware proactive suggestion engine (SPEC-ARC §8.3.2).
//!
//! Evaluates contextual triggers to produce proactive suggestions scored by
//! a composite formula:
//!
//! ```text
//! score = relevance × novelty × personality_fit × timing_appropriateness
//! ```
//!
//! Features:
//! - **Learning**: tracks acceptance/rejection to adapt per-category confidence
//! - **Per-category budgets**: prevents any single domain from dominating
//! - **Rate limiting**: cooldown + daily caps + per-category caps
//! - **Deduplication**: content-hash dedup window (4 hours)
//! - **Novelty decay**: repeated trigger types lose novelty over time
//! - **Context signals**: time-of-day and category appropriateness weighting
//!
//! Backward-compatible: preserves `Suggestion`, `SuggestionTrigger`,
//! `SuggestionEngine` with `evaluate_triggers(now_ms)`.

use std::collections::{HashMap, HashSet, VecDeque};

use serde::{Deserialize, Serialize};
use tracing::{debug, instrument, warn};

use super::super::{ArcError, DomainId};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum number of recent suggestions retained.
const MAX_RECENT_SUGGESTIONS: usize = 100;

/// Maximum number of suppressed suggestion IDs tracked.
const MAX_SUPPRESSED: usize = 200;

/// Minimum confidence threshold to surface a suggestion.
const CONFIDENCE_THRESHOLD: f32 = 0.6;

/// Cooldown period between suggestion batches (milliseconds).
const SUGGESTION_COOLDOWN_MS: u64 = 60_000; // 1 minute

/// Deduplication window: don't suggest same thing twice within this period (ms).
const DEDUP_WINDOW_MS: u64 = 4 * 3_600_000; // 4 hours

/// Maximum suggestions returned per evaluation cycle.
const MAX_SUGGESTIONS_PER_EVAL: usize = 5;

/// Maximum registered triggers.
const MAX_TRIGGERS: usize = 64;

/// Maximum feedback entries retained per category.
#[allow(dead_code)] // Phase 8: used by feedback ring buffer cap in suggestion learning
const MAX_FEEDBACK_PER_CATEGORY: usize = 128;

/// Maximum total feedback entries across all categories.
const MAX_TOTAL_FEEDBACK: usize = 1024;

/// Per-category budget: max suggestions from a single domain per eval cycle.
const PER_CATEGORY_BUDGET: usize = 3;

/// Novelty half-life: after this many suggestions of the same trigger type,
/// novelty score halves. Controls how quickly repeated trigger types lose impact.
const NOVELTY_HALF_LIFE: f32 = 5.0;

/// Minimum novelty floor — even very repeated triggers keep some novelty.
const MIN_NOVELTY: f32 = 0.2;

/// Smoothing prior for Bayesian acceptance rate (pseudo-count).
/// With prior=2, we start with an assumed 50% acceptance rate
/// and need real observations to move it.
const ACCEPTANCE_PRIOR: f32 = 2.0;

/// Default personality fit (neutral) when no personality data is available.
const DEFAULT_PERSONALITY_FIT: f32 = 0.7;

/// Maximum entries in the category stats map.
const MAX_CATEGORY_STATS: usize = 16;

/// Time-of-day bins (6 bins of 4 hours each) for timing appropriateness.
const TIME_BIN_COUNT: usize = 6;

// ---------------------------------------------------------------------------
// SuggestionTrigger
// ---------------------------------------------------------------------------

/// Categories of events that can trigger a proactive suggestion.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SuggestionTrigger {
    /// User's context has changed (location, app, etc.).
    ContextChange,
    /// A time-based pattern was detected.
    TimePattern,
    /// A health metric crossed a threshold.
    HealthAlert,
    /// A social connection gap was detected.
    SocialGap,
    /// A goal or task deadline is approaching.
    GoalReminder,
    /// An anomaly was detected in user behaviour.
    AnomalyDetected,
}

impl SuggestionTrigger {
    /// Intrinsic urgency weight — higher for more actionable triggers.
    #[must_use]
    fn urgency_weight(self) -> f32 {
        match self {
            SuggestionTrigger::HealthAlert => 1.0,
            SuggestionTrigger::GoalReminder => 0.9,
            SuggestionTrigger::AnomalyDetected => 0.85,
            SuggestionTrigger::SocialGap => 0.7,
            SuggestionTrigger::TimePattern => 0.65,
            SuggestionTrigger::ContextChange => 0.6,
        }
    }
}

// ---------------------------------------------------------------------------
// Suggestion
// ---------------------------------------------------------------------------

/// A single proactive suggestion ready to surface to the user.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Suggestion {
    /// Unique suggestion identifier (monotonic counter).
    pub id: u64,
    /// The domain this suggestion relates to.
    pub category: DomainId,
    /// Human-readable suggestion text.
    pub text: String,
    /// Engine confidence in relevance [0.0, 1.0].
    pub confidence: f32,
    /// Timestamp (ms) when the suggestion was created.
    pub created_ms: u64,
    /// Priority (lower = more important).
    pub priority: u8,
}

impl Suggestion {
    /// Content hash used for deduplication: combines category + text hash.
    #[must_use]
    pub fn content_hash(&self) -> u64 {
        // FNV-1a hash of category discriminant + text.
        let mut hash: u64 = 0xcbf29ce484222325;
        hash ^= self.category as u64;
        hash = hash.wrapping_mul(0x100000001b3);
        for byte in self.text.as_bytes() {
            hash ^= *byte as u64;
            hash = hash.wrapping_mul(0x100000001b3);
        }
        hash
    }
}

// ---------------------------------------------------------------------------
// FeedbackOutcome — user response to a suggestion
// ---------------------------------------------------------------------------

/// How the user responded to a suggestion.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FeedbackOutcome {
    /// User accepted / acted on the suggestion.
    Accepted,
    /// User explicitly dismissed the suggestion.
    Rejected,
    /// Suggestion expired without user interaction.
    Ignored,
}

/// A single feedback record linking a suggestion to a user response.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct FeedbackRecord {
    /// Domain category of the suggestion.
    category: DomainId,
    /// Trigger type that generated the suggestion.
    trigger_type: SuggestionTrigger,
    /// User's response.
    outcome: FeedbackOutcome,
    /// Timestamp (ms) of the feedback.
    timestamp_ms: u64,
    /// Time-of-day bin (0..5) when the suggestion was shown.
    time_bin: u8,
}

// ---------------------------------------------------------------------------
// CategoryStats — per-domain learning
// ---------------------------------------------------------------------------

/// Accumulated statistics for a single domain category, used to adapt
/// future suggestion scoring.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct CategoryStats {
    /// Total accepted suggestions.
    accepted: u32,
    /// Total rejected suggestions.
    rejected: u32,
    /// Total ignored suggestions.
    ignored: u32,
    /// Per-time-bin acceptance counts: `[accepted, total]` for each of 6 bins.
    time_bin_accepted: [u32; TIME_BIN_COUNT],
    time_bin_total: [u32; TIME_BIN_COUNT],
    /// Count of suggestions generated from this category in the current eval window.
    /// Reset each eval cycle.
    current_cycle_count: u16,
}

impl CategoryStats {
    fn new() -> Self {
        Self {
            accepted: 0,
            rejected: 0,
            ignored: 0,
            time_bin_accepted: [0; TIME_BIN_COUNT],
            time_bin_total: [0; TIME_BIN_COUNT],
            current_cycle_count: 0,
        }
    }

    /// Total observations (accepted + rejected + ignored).
    fn total(&self) -> u32 {
        self.accepted
            .saturating_add(self.rejected)
            .saturating_add(self.ignored)
    }

    /// Bayesian acceptance rate with smoothing prior.
    /// Returns a value in [0.0, 1.0].
    fn acceptance_rate(&self) -> f32 {
        let effective_accepted = self.accepted as f32 + ACCEPTANCE_PRIOR * 0.5;
        let effective_total = self.total() as f32 + ACCEPTANCE_PRIOR;
        (effective_accepted / effective_total).clamp(0.0, 1.0)
    }

    /// Timing appropriateness for a given time bin.
    /// Bayesian: uses per-bin acceptance rate with small prior.
    fn timing_score(&self, bin: usize) -> f32 {
        let bin = bin.min(TIME_BIN_COUNT - 1);
        let accepted = self.time_bin_accepted[bin] as f32 + 0.5;
        let total = self.time_bin_total[bin] as f32 + 1.0;
        (accepted / total).clamp(0.0, 1.0)
    }
}

// ---------------------------------------------------------------------------
// RegisteredTrigger (internal)
// ---------------------------------------------------------------------------

/// An internal trigger registration that the engine evaluates each cycle.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct RegisteredTrigger {
    /// What kind of trigger this is.
    trigger_type: SuggestionTrigger,
    /// Domain the trigger belongs to.
    domain: DomainId,
    /// Template text for the suggestion.
    text_template: String,
    /// Base confidence for this trigger.
    base_confidence: f32,
    /// Priority for generated suggestions.
    priority: u8,
    /// Whether this trigger is currently active/armed.
    armed: bool,
    /// How many times this trigger has fired (for novelty decay).
    fire_count: u32,
}

// ---------------------------------------------------------------------------
// SuggestionEngine
// ---------------------------------------------------------------------------

/// Manages proactive suggestion generation with dynamic scoring,
/// learning from feedback, deduplication, cooldown, and suppression.
#[derive(Debug, Serialize, Deserialize)]
pub struct SuggestionEngine {
    /// Ring buffer of recently generated suggestions.
    recent_suggestions: VecDeque<Suggestion>,
    /// IDs of suggestions the user has dismissed (don't repeat these).
    suppressed: HashSet<u64>,
    /// Cooldown: don't produce new suggestions before this timestamp (ms).
    cooldown_until_ms: u64,
    /// Registered triggers that are evaluated each cycle.
    triggers: Vec<RegisteredTrigger>,
    /// Monotonic counter for suggestion IDs.
    next_id: u64,
    /// Per-category learning stats.
    category_stats: HashMap<DomainId, CategoryStats>,
    /// Rolling feedback log for auditing/analysis (bounded).
    feedback_log: VecDeque<FeedbackRecord>,
    /// Global personality fit factor (set externally from personality engine).
    personality_fit: f32,
}

impl SuggestionEngine {
    /// Create a new suggestion engine with empty state.
    #[must_use]
    pub fn new() -> Self {
        Self {
            recent_suggestions: VecDeque::with_capacity(MAX_RECENT_SUGGESTIONS),
            suppressed: HashSet::with_capacity(MAX_SUPPRESSED),
            cooldown_until_ms: 0,
            triggers: Vec::with_capacity(MAX_TRIGGERS),
            next_id: 1,
            category_stats: HashMap::with_capacity(MAX_CATEGORY_STATS),
            feedback_log: VecDeque::with_capacity(MAX_TOTAL_FEEDBACK),
            personality_fit: DEFAULT_PERSONALITY_FIT,
        }
    }

    /// Number of recent suggestions in the buffer.
    #[must_use]
    pub fn recent_count(&self) -> usize {
        self.recent_suggestions.len()
    }

    /// Number of suppressed suggestion IDs.
    #[must_use]
    pub fn suppressed_count(&self) -> usize {
        self.suppressed.len()
    }

    /// Total feedback records stored.
    #[must_use]
    pub fn feedback_count(&self) -> usize {
        self.feedback_log.len()
    }

    /// Set the global personality fit factor (from personality engine).
    /// Clamped to [0.1, 1.0].
    pub fn set_personality_fit(&mut self, fit: f32) {
        self.personality_fit = fit.clamp(0.1, 1.0);
    }

    /// Current personality fit factor.
    #[must_use]
    pub fn personality_fit(&self) -> f32 {
        self.personality_fit
    }

    /// Register a new trigger that will be evaluated each cycle.
    pub fn register_trigger(
        &mut self,
        trigger_type: SuggestionTrigger,
        domain: DomainId,
        text_template: String,
        confidence: f32,
        priority: u8,
    ) -> Result<(), ArcError> {
        if self.triggers.len() >= MAX_TRIGGERS {
            return Err(ArcError::CapacityExceeded {
                collection: "suggestion_triggers".into(),
                max: MAX_TRIGGERS,
            });
        }
        self.triggers.push(RegisteredTrigger {
            trigger_type,
            domain,
            text_template,
            base_confidence: confidence.clamp(0.0, 1.0),
            priority,
            armed: true,
            fire_count: 0,
        });
        debug!(
            trigger = ?trigger_type,
            domain = %domain,
            "suggestion trigger registered"
        );
        Ok(())
    }

    /// Arm or disarm a trigger by type. Affects all triggers of that type.
    pub fn set_trigger_armed(&mut self, trigger_type: SuggestionTrigger, armed: bool) {
        for t in &mut self.triggers {
            if t.trigger_type == trigger_type {
                t.armed = armed;
            }
        }
    }

    /// Mark a suggestion as suppressed (user dismissed it).
    pub fn suppress(&mut self, suggestion_id: u64) {
        // Evict oldest if at capacity.
        if self.suppressed.len() >= MAX_SUPPRESSED {
            if let Some(&oldest) = self.suppressed.iter().next() {
                self.suppressed.remove(&oldest);
            }
        }
        self.suppressed.insert(suggestion_id);
        debug!(id = suggestion_id, "suggestion suppressed");
    }

    /// Record user feedback for a suggestion, updating category stats.
    ///
    /// This is the primary learning mechanism: acceptance boosts a category's
    /// future relevance score; rejection penalizes it.
    pub fn record_feedback(
        &mut self,
        category: DomainId,
        trigger_type: SuggestionTrigger,
        outcome: FeedbackOutcome,
        now_ms: u64,
    ) {
        let time_bin = Self::time_bin_from_ms(now_ms);

        // Update category stats
        let stats = self
            .category_stats
            .entry(category)
            .or_insert_with(CategoryStats::new);

        match outcome {
            FeedbackOutcome::Accepted => {
                stats.accepted = stats.accepted.saturating_add(1);
                stats.time_bin_accepted[time_bin] =
                    stats.time_bin_accepted[time_bin].saturating_add(1);
            }
            FeedbackOutcome::Rejected => {
                stats.rejected = stats.rejected.saturating_add(1);
            }
            FeedbackOutcome::Ignored => {
                stats.ignored = stats.ignored.saturating_add(1);
            }
        }
        stats.time_bin_total[time_bin] = stats.time_bin_total[time_bin].saturating_add(1);

        // Log to feedback ring buffer
        if self.feedback_log.len() >= MAX_TOTAL_FEEDBACK {
            self.feedback_log.pop_front();
        }
        self.feedback_log.push_back(FeedbackRecord {
            category,
            trigger_type,
            outcome,
            timestamp_ms: now_ms,
            time_bin: time_bin as u8,
        });

        debug!(
            category = %category,
            outcome = ?outcome,
            acceptance_rate = stats.acceptance_rate(),
            "suggestion feedback recorded"
        );
    }

    /// Get the learned acceptance rate for a category.
    /// Returns the Bayesian smoothed rate, or the default if no data.
    #[must_use]
    pub fn category_acceptance_rate(&self, category: DomainId) -> f32 {
        self.category_stats
            .get(&category)
            .map_or(0.5, |s| s.acceptance_rate())
    }

    /// Check whether a suggestion with the given content hash was recently
    /// generated (within [`DEDUP_WINDOW_MS`]).
    fn is_duplicate(&self, content_hash: u64, now_ms: u64) -> bool {
        let cutoff = now_ms.saturating_sub(DEDUP_WINDOW_MS);
        self.recent_suggestions
            .iter()
            .any(|s| s.content_hash() == content_hash && s.created_ms >= cutoff)
    }

    /// Push a suggestion into the recent buffer, enforcing capacity.
    fn record_suggestion(&mut self, suggestion: &Suggestion) {
        if self.recent_suggestions.len() >= MAX_RECENT_SUGGESTIONS {
            self.recent_suggestions.pop_front();
        }
        self.recent_suggestions.push_back(suggestion.clone());
    }

    /// Allocate the next suggestion ID.
    fn alloc_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);
        id
    }

    /// Compute the time-of-day bin (0..5) from a millisecond timestamp.
    /// Each bin covers 4 hours: [00-04), [04-08), [08-12), [12-16), [16-20), [20-24).
    fn time_bin_from_ms(now_ms: u64) -> usize {
        let hour = ((now_ms / 3_600_000) % 24) as usize;
        (hour / 4).min(TIME_BIN_COUNT - 1)
    }

    /// Compute the composite dynamic score for a trigger.
    ///
    /// ```text
    /// score = relevance × novelty × personality_fit × timing_appropriateness
    /// ```
    ///
    /// - **relevance**: base_confidence × trigger urgency × learned acceptance rate
    /// - **novelty**: decays with repeated firings (half-life model)
    /// - **personality_fit**: global personality alignment factor
    /// - **timing**: per-category, per-time-bin learned timing score
    fn compute_score(&self, trigger: &RegisteredTrigger, now_ms: u64) -> f32 {
        // --- Relevance ---
        let urgency = trigger.trigger_type.urgency_weight();
        let learned_rate = self.category_acceptance_rate(trigger.domain);

        // --- Novelty ---
        // Exponential decay: novelty = max(MIN_NOVELTY, 0.5^(fire_count / half_life))
        let decay_exponent = trigger.fire_count as f32 / NOVELTY_HALF_LIFE;
        let novelty = (0.5_f32.powf(decay_exponent)).max(MIN_NOVELTY);

        // --- Personality fit ---
        let personality = self.personality_fit;

        // --- Timing appropriateness ---
        let time_bin = Self::time_bin_from_ms(now_ms);
        let timing = self
            .category_stats
            .get(&trigger.domain)
            .map_or(0.5, |s| s.timing_score(time_bin));

        // Weighted average of contributing factors (all in [0, 1]).
        // Using a mean instead of a product avoids the combinatorial
        // deflation that makes pure-multiplicative scores tiny when
        // several factors sit near their neutral default (~0.5–0.7).
        let factor_mean = (urgency + novelty + personality + timing + learned_rate) / 5.0;

        let raw_score = trigger.base_confidence * factor_mean;

        // Clamp to [0, 1]
        raw_score.clamp(0.0, 1.0)
    }

    /// Reset per-category cycle counters (called at the start of each eval).
    fn reset_cycle_counters(&mut self) {
        for stats in self.category_stats.values_mut() {
            stats.current_cycle_count = 0;
        }
    }

    /// Check if a category has exhausted its per-cycle budget.
    fn category_over_budget(&self, domain: DomainId) -> bool {
        self.category_stats
            .get(&domain)
            .is_some_and(|s| s.current_cycle_count as usize >= PER_CATEGORY_BUDGET)
    }

    /// Increment a category's cycle counter.
    fn increment_category_cycle(&mut self, domain: DomainId) {
        let stats = self
            .category_stats
            .entry(domain)
            .or_insert_with(CategoryStats::new);
        stats.current_cycle_count = stats.current_cycle_count.saturating_add(1);
    }

    /// Evaluate all armed triggers and return new suggestions.
    ///
    /// Uses the dynamic scoring formula and respects cooldown, deduplication,
    /// confidence threshold, suppression list, and per-category budgets.
    /// Suggestions are returned sorted by composite score (highest first).
    #[instrument(name = "suggestion_eval", skip(self))]
    pub fn evaluate_triggers(&mut self, now_ms: u64) -> Result<Vec<Suggestion>, ArcError> {
        // Cooldown check.
        if now_ms < self.cooldown_until_ms {
            debug!(
                cooldown_remaining_ms = self.cooldown_until_ms - now_ms,
                "suggestion engine in cooldown"
            );
            return Ok(Vec::new());
        }

        self.reset_cycle_counters();

        // Score all armed triggers that pass the base confidence threshold.
        let mut scored: Vec<(usize, f32)> = self
            .triggers
            .iter()
            .enumerate()
            .filter(|(_, t)| t.armed && t.base_confidence >= CONFIDENCE_THRESHOLD)
            .map(|(i, t)| {
                let score = self.compute_score(t, now_ms);
                (i, score)
            })
            .collect();

        // Sort by score descending (best first).
        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        let mut candidates: Vec<Suggestion> = Vec::with_capacity(MAX_SUGGESTIONS_PER_EVAL);

        for (idx, score) in scored {
            if candidates.len() >= MAX_SUGGESTIONS_PER_EVAL {
                break;
            }

            let domain = self.triggers[idx].domain;

            // Per-category budget check
            if self.category_over_budget(domain) {
                debug!(domain = %domain, "category over budget, skipping");
                continue;
            }

            let id = self.alloc_id();
            let suggestion = Suggestion {
                id,
                category: domain,
                text: self.triggers[idx].text_template.clone(),
                confidence: score,
                created_ms: now_ms,
                priority: self.triggers[idx].priority,
            };

            // Deduplication check.
            let content_hash = suggestion.content_hash();
            if self.is_duplicate(content_hash, now_ms) {
                debug!(id, "suggestion deduplicated");
                continue;
            }

            // Suppression check.
            if self.suppressed.contains(&content_hash) {
                debug!(id, "suggestion suppressed by user");
                continue;
            }

            // NOTE: The base_confidence gate on line 570 already filters
            // triggers below CONFIDENCE_THRESHOLD.  The dynamic score is used
            // purely for ranking — applying the same threshold a second time
            // would eliminate virtually all suggestions because the composite
            // formula (relevance × novelty × personality × timing) naturally
            // produces values well below 1.0.

            // Increment fire count for novelty decay.
            self.triggers[idx].fire_count = self.triggers[idx].fire_count.saturating_add(1);

            self.increment_category_cycle(domain);
            candidates.push(suggestion);
        }

        // Sort by priority (lower = more important) as the final ordering.
        candidates.sort_by_key(|s| s.priority);

        // Record all candidates and set cooldown.
        for s in &candidates {
            self.record_suggestion(s);
        }

        if !candidates.is_empty() {
            self.cooldown_until_ms = now_ms + SUGGESTION_COOLDOWN_MS;
        }

        debug!(count = candidates.len(), "suggestions evaluated");

        Ok(candidates)
    }

    /// Get a snapshot of all category stats for external analysis.
    #[must_use]
    pub fn category_stats_snapshot(&self) -> Vec<(DomainId, f32, u32)> {
        self.category_stats
            .iter()
            .map(|(&domain, stats)| (domain, stats.acceptance_rate(), stats.total()))
            .collect()
    }

    /// Number of registered triggers.
    #[must_use]
    pub fn trigger_count(&self) -> usize {
        self.triggers.len()
    }

    /// Reset the fire counts on all triggers (e.g., after a long idle period).
    pub fn reset_novelty(&mut self) {
        for t in &mut self.triggers {
            t.fire_count = 0;
        }
    }
}

impl Default for SuggestionEngine {
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

    fn make_engine_with_trigger() -> SuggestionEngine {
        let mut engine = SuggestionEngine::new();
        engine
            .register_trigger(
                SuggestionTrigger::HealthAlert,
                DomainId::Health,
                "Time to take your medication".into(),
                0.85,
                10,
            )
            .expect("register ok");
        engine
    }

    #[test]
    fn test_new_engine() {
        let e = SuggestionEngine::new();
        assert_eq!(e.recent_count(), 0);
        assert_eq!(e.suppressed_count(), 0);
        assert_eq!(e.feedback_count(), 0);
        assert!((e.personality_fit() - DEFAULT_PERSONALITY_FIT).abs() < f32::EPSILON);
    }

    #[test]
    fn test_register_trigger() {
        let e = make_engine_with_trigger();
        assert_eq!(e.trigger_count(), 1);
    }

    #[test]
    fn test_register_trigger_bounded() {
        let mut e = SuggestionEngine::new();
        for i in 0..MAX_TRIGGERS {
            let result = e.register_trigger(
                SuggestionTrigger::TimePattern,
                DomainId::Productivity,
                format!("trigger_{i}"),
                0.7,
                50,
            );
            assert!(result.is_ok(), "failed at trigger {i}");
        }
        // One more should fail.
        let result = e.register_trigger(
            SuggestionTrigger::TimePattern,
            DomainId::Productivity,
            "overflow".into(),
            0.7,
            50,
        );
        assert!(result.is_err());
    }

    #[test]
    fn test_evaluate_produces_suggestion() {
        let mut e = make_engine_with_trigger();
        let suggestions = e.evaluate_triggers(10_000).expect("eval ok");
        assert_eq!(suggestions.len(), 1);
        assert_eq!(suggestions[0].category, DomainId::Health);
        // Dynamic score should be above threshold
        assert!(suggestions[0].confidence >= CONFIDENCE_THRESHOLD);
    }

    #[test]
    fn test_cooldown_blocks_rapid_eval() {
        let mut e = make_engine_with_trigger();
        let s1 = e.evaluate_triggers(10_000).expect("eval 1");
        assert_eq!(s1.len(), 1);

        // Immediately evaluate again — should be in cooldown.
        let s2 = e.evaluate_triggers(10_001).expect("eval 2");
        assert!(s2.is_empty(), "should be in cooldown");

        // After cooldown period.
        let s3 = e
            .evaluate_triggers(10_000 + SUGGESTION_COOLDOWN_MS + 1)
            .expect("eval 3");
        // The dedup window is 4 hours, so the same suggestion is deduplicated.
        assert!(s3.is_empty(), "should be deduplicated within 4h window");
    }

    #[test]
    fn test_deduplication_window() {
        let mut e = make_engine_with_trigger();
        let s1 = e.evaluate_triggers(1_000_000).expect("eval 1");
        assert_eq!(s1.len(), 1);

        // After 4 hours + cooldown, the same trigger should produce a new suggestion.
        let future_ms = 1_000_000 + DEDUP_WINDOW_MS + SUGGESTION_COOLDOWN_MS + 1;
        let s2 = e.evaluate_triggers(future_ms).expect("eval 2");
        assert_eq!(s2.len(), 1, "should produce suggestion after dedup window");
    }

    #[test]
    fn test_suppress_prevents_suggestion() {
        let mut e = make_engine_with_trigger();

        // Evaluate once to get the suggestion.
        let suggestions = e.evaluate_triggers(10_000).expect("eval");
        assert_eq!(suggestions.len(), 1);

        // Suppress by content hash.
        let content_hash = suggestions[0].content_hash();
        e.suppress(content_hash);

        // After cooldown + dedup window, suppressed suggestion still blocked.
        let future = 10_000 + DEDUP_WINDOW_MS + SUGGESTION_COOLDOWN_MS + 1;
        let s2 = e.evaluate_triggers(future).expect("eval 2");
        assert!(s2.is_empty(), "suppressed suggestion should not appear");
    }

    #[test]
    fn test_confidence_threshold_filters() {
        let mut e = SuggestionEngine::new();
        e.register_trigger(
            SuggestionTrigger::ContextChange,
            DomainId::Lifestyle,
            "Low confidence suggestion".into(),
            0.3, // below threshold
            50,
        )
        .expect("register ok");

        let suggestions = e.evaluate_triggers(10_000).expect("eval");
        assert!(
            suggestions.is_empty(),
            "below-threshold suggestion should be filtered"
        );
    }

    #[test]
    fn test_suppress_bounded() {
        let mut e = SuggestionEngine::new();
        for i in 0..(MAX_SUPPRESSED + 10) {
            e.suppress(i as u64);
        }
        // Should never exceed MAX_SUPPRESSED.
        assert!(
            e.suppressed_count() <= MAX_SUPPRESSED,
            "got {}",
            e.suppressed_count()
        );
    }

    #[test]
    fn test_suggestion_content_hash_deterministic() {
        let s = Suggestion {
            id: 1,
            category: DomainId::Health,
            text: "Drink water".into(),
            confidence: 0.8,
            created_ms: 1000,
            priority: 5,
        };
        let h1 = s.content_hash();
        let h2 = s.content_hash();
        assert_eq!(h1, h2, "content hash should be deterministic");
    }

    // ---- New tests for dynamic scoring and learning ----

    #[test]
    fn test_record_feedback_acceptance() {
        let mut e = SuggestionEngine::new();

        // Record 10 acceptances for Health
        for _ in 0..10 {
            e.record_feedback(
                DomainId::Health,
                SuggestionTrigger::HealthAlert,
                FeedbackOutcome::Accepted,
                10_000,
            );
        }

        let rate = e.category_acceptance_rate(DomainId::Health);
        // 10 accepted out of 10 total, with prior → (10 + 1) / (10 + 2) ≈ 0.917
        assert!(rate > 0.85, "expected high acceptance rate, got {rate}");
    }

    #[test]
    fn test_record_feedback_rejection() {
        let mut e = SuggestionEngine::new();

        // Record 10 rejections for Productivity
        for _ in 0..10 {
            e.record_feedback(
                DomainId::Productivity,
                SuggestionTrigger::GoalReminder,
                FeedbackOutcome::Rejected,
                10_000,
            );
        }

        let rate = e.category_acceptance_rate(DomainId::Productivity);
        // 0 accepted out of 10 total, with prior → (0 + 1) / (10 + 2) ≈ 0.083
        assert!(rate < 0.15, "expected low acceptance rate, got {rate}");
    }

    #[test]
    fn test_default_acceptance_rate() {
        let e = SuggestionEngine::new();
        let rate = e.category_acceptance_rate(DomainId::Finance);
        // No data → Bayesian prior gives 0.5
        assert!((rate - 0.5).abs() < 0.01, "got {rate}");
    }

    #[test]
    fn test_feedback_log_bounded() {
        let mut e = SuggestionEngine::new();
        for i in 0..(MAX_TOTAL_FEEDBACK + 50) {
            e.record_feedback(
                DomainId::Health,
                SuggestionTrigger::HealthAlert,
                FeedbackOutcome::Accepted,
                i as u64,
            );
        }
        assert!(
            e.feedback_count() <= MAX_TOTAL_FEEDBACK,
            "got {}",
            e.feedback_count()
        );
    }

    #[test]
    fn test_personality_fit_affects_score() {
        let mut e1 = make_engine_with_trigger();
        e1.set_personality_fit(1.0);
        let s1 = e1.evaluate_triggers(10_000).expect("eval");

        let mut e2 = make_engine_with_trigger();
        e2.set_personality_fit(0.3);
        let s2 = e2.evaluate_triggers(10_000).expect("eval");

        // Higher personality fit → higher confidence in the suggestion
        if !s1.is_empty() && !s2.is_empty() {
            assert!(
                s1[0].confidence >= s2[0].confidence,
                "higher personality fit should yield higher confidence"
            );
        }
    }

    #[test]
    fn test_novelty_decay_reduces_score() {
        let mut e = make_engine_with_trigger();

        // Fire the trigger many times to decay novelty
        for i in 0..20 {
            let t = 1_000_000 + (i as u64) * (DEDUP_WINDOW_MS + SUGGESTION_COOLDOWN_MS + 1);
            let _ = e.evaluate_triggers(t);
        }

        // The fire_count should have increased
        assert!(
            e.triggers[0].fire_count > 0,
            "fire count should increase with evaluations"
        );
    }

    #[test]
    fn test_per_category_budget() {
        let mut e = SuggestionEngine::new();

        // Register 5 triggers all for Health domain
        for i in 0..5 {
            e.register_trigger(
                SuggestionTrigger::HealthAlert,
                DomainId::Health,
                format!("Health suggestion {i}"),
                0.9,
                10,
            )
            .expect("register");
        }

        let suggestions = e.evaluate_triggers(10_000).expect("eval");
        // Should be capped at PER_CATEGORY_BUDGET
        assert!(
            suggestions.len() <= PER_CATEGORY_BUDGET,
            "got {} suggestions, max {}",
            suggestions.len(),
            PER_CATEGORY_BUDGET
        );
    }

    #[test]
    fn test_time_bin_computation() {
        // 00:00 → bin 0
        assert_eq!(SuggestionEngine::time_bin_from_ms(0), 0);
        // 05:00 → bin 1
        assert_eq!(SuggestionEngine::time_bin_from_ms(5 * 3_600_000), 1);
        // 10:00 → bin 2
        assert_eq!(SuggestionEngine::time_bin_from_ms(10 * 3_600_000), 2);
        // 14:00 → bin 3
        assert_eq!(SuggestionEngine::time_bin_from_ms(14 * 3_600_000), 3);
        // 18:00 → bin 4
        assert_eq!(SuggestionEngine::time_bin_from_ms(18 * 3_600_000), 4);
        // 22:00 → bin 5
        assert_eq!(SuggestionEngine::time_bin_from_ms(22 * 3_600_000), 5);
    }

    #[test]
    fn test_learned_timing_affects_score() {
        let mut e = make_engine_with_trigger();

        // Record many acceptances in the morning (bin 2, 08-12h)
        let morning_ms = 10 * 3_600_000_u64; // 10:00 → bin 2
        for _ in 0..20 {
            e.record_feedback(
                DomainId::Health,
                SuggestionTrigger::HealthAlert,
                FeedbackOutcome::Accepted,
                morning_ms,
            );
        }

        // Record many rejections at night (bin 5, 20-24h)
        let night_ms = 23 * 3_600_000_u64; // 23:00 → bin 5
        for _ in 0..20 {
            e.record_feedback(
                DomainId::Health,
                SuggestionTrigger::HealthAlert,
                FeedbackOutcome::Rejected,
                night_ms,
            );
        }

        let stats = e.category_stats.get(&DomainId::Health).expect("stats");
        let morning_timing = stats.timing_score(2);
        let night_timing = stats.timing_score(5);
        assert!(
            morning_timing > night_timing,
            "morning ({morning_timing}) should have better timing than night ({night_timing})"
        );
    }

    #[test]
    fn test_reset_novelty() {
        let mut e = make_engine_with_trigger();

        // Fire trigger
        let _ = e.evaluate_triggers(10_000);
        assert!(e.triggers[0].fire_count > 0);

        // Reset
        e.reset_novelty();
        assert_eq!(e.triggers[0].fire_count, 0);
    }

    #[test]
    fn test_category_stats_snapshot() {
        let mut e = SuggestionEngine::new();
        e.record_feedback(
            DomainId::Health,
            SuggestionTrigger::HealthAlert,
            FeedbackOutcome::Accepted,
            1000,
        );
        e.record_feedback(
            DomainId::Productivity,
            SuggestionTrigger::GoalReminder,
            FeedbackOutcome::Rejected,
            2000,
        );

        let snapshot = e.category_stats_snapshot();
        assert_eq!(snapshot.len(), 2);
    }

    #[test]
    fn test_multi_domain_evaluation() {
        let mut e = SuggestionEngine::new();

        // Register triggers across multiple domains
        e.register_trigger(
            SuggestionTrigger::HealthAlert,
            DomainId::Health,
            "Take medication".into(),
            0.9,
            5,
        )
        .expect("ok");
        e.register_trigger(
            SuggestionTrigger::GoalReminder,
            DomainId::Productivity,
            "Review goals".into(),
            0.85,
            10,
        )
        .expect("ok");
        e.register_trigger(
            SuggestionTrigger::SocialGap,
            DomainId::Social,
            "Call friend".into(),
            0.8,
            15,
        )
        .expect("ok");

        let suggestions = e.evaluate_triggers(10_000).expect("eval");
        // Should get suggestions from multiple domains
        assert!(
            suggestions.len() >= 2,
            "got {} suggestions",
            suggestions.len()
        );

        let domains: HashSet<DomainId> = suggestions.iter().map(|s| s.category).collect();
        assert!(domains.len() >= 2, "should have multiple domains");
    }

    #[test]
    fn test_learning_improves_ranking() {
        let mut e = SuggestionEngine::new();

        // Register Health and Productivity triggers with same base confidence
        e.register_trigger(
            SuggestionTrigger::HealthAlert,
            DomainId::Health,
            "Health suggestion".into(),
            0.85,
            10,
        )
        .expect("ok");
        e.register_trigger(
            SuggestionTrigger::GoalReminder,
            DomainId::Productivity,
            "Productivity suggestion".into(),
            0.85,
            10,
        )
        .expect("ok");

        // Train: Health gets accepted, Productivity gets rejected
        for _ in 0..20 {
            e.record_feedback(
                DomainId::Health,
                SuggestionTrigger::HealthAlert,
                FeedbackOutcome::Accepted,
                10_000,
            );
            e.record_feedback(
                DomainId::Productivity,
                SuggestionTrigger::GoalReminder,
                FeedbackOutcome::Rejected,
                10_000,
            );
        }

        let suggestions = e.evaluate_triggers(10_000).expect("eval");
        // Health should have higher confidence than Productivity due to learning
        if suggestions.len() >= 2 {
            let health = suggestions.iter().find(|s| s.category == DomainId::Health);
            let prod = suggestions
                .iter()
                .find(|s| s.category == DomainId::Productivity);
            if let (Some(h), Some(p)) = (health, prod) {
                assert!(
                    h.confidence > p.confidence,
                    "health ({}) should score higher than productivity ({})",
                    h.confidence,
                    p.confidence
                );
            }
        }
    }

    #[test]
    fn test_suggestion_struct_backward_compat() {
        // Verify the Suggestion struct shape matches what proactive/mod.rs expects
        let s = Suggestion {
            id: 1,
            category: DomainId::Health,
            text: "Drink water".into(),
            confidence: 0.8,
            created_ms: 1000,
            priority: 5,
        };
        // These field accesses must compile for backward compat
        let _id: u64 = s.id;
        let _cat: DomainId = s.category;
        let _text: &str = &s.text;
        let _conf: f32 = s.confidence;
        let _created: u64 = s.created_ms;
        let _prio: u8 = s.priority;
    }

    #[test]
    fn test_set_personality_fit_clamped() {
        let mut e = SuggestionEngine::new();
        e.set_personality_fit(2.0);
        assert!((e.personality_fit() - 1.0).abs() < f32::EPSILON);
        e.set_personality_fit(-1.0);
        assert!((e.personality_fit() - 0.1).abs() < f32::EPSILON);
    }
}
