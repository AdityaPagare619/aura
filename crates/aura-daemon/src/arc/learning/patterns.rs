//! Pattern detection engine — temporal, sequential, and contextual pattern learning.
//!
//! # Architecture (SPEC-ARC §8.4 — Proactive Intelligence)
//!
//! Detects three types of recurring patterns from AURA's observation stream:
//!
//! 1. **Temporal patterns** — actions that recur at specific times of day / days of week.
//!    Uses sliding window analysis with Bayesian confidence updating.
//!
//! 2. **Sequence patterns** — ordered chains of actions that commonly follow each other.
//!    Detected via n-gram analysis over the recent action stream.
//!
//! 3. **Context patterns** — actions that correlate with environmental context
//!    (location, battery state, app foreground, connectivity, etc.).
//!
//! All patterns age over time and are pruned when their confidence drops below a
//! threshold.  This prevents stale patterns from polluting the suggestion engine.
//!
//! # Bayesian Confidence Updating
//!
//! ```text
//! P(pattern | data) ∝ P(data | pattern) × P(pattern)
//! confidence_new = (α × confidence_old + hit) / (α + 1)
//! ```
//!
//! Where `α` controls how quickly new evidence shifts the estimate.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use tracing::{debug, info, instrument, warn};

use super::super::ArcError;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum number of temporal patterns tracked.
pub const MAX_TEMPORAL_PATTERNS: usize = 128;

/// Maximum number of sequence patterns tracked.
pub const MAX_SEQUENCE_PATTERNS: usize = 256;

/// Maximum number of context patterns tracked.
pub const MAX_CONTEXT_PATTERNS: usize = 128;

/// Minimum confidence before a pattern is considered actionable.
pub const MIN_CONFIDENCE: f32 = 0.3;

/// Confidence below which a pattern is pruned during aging.
pub const PRUNE_CONFIDENCE: f32 = 0.05;

/// Bayesian smoothing factor (α). Higher = slower adaptation, more stable.
pub const BAYESIAN_ALPHA: f32 = 5.0;

/// Time tolerance for temporal pattern matching (minutes).
pub const TEMPORAL_TOLERANCE_MINUTES: u32 = 30;

/// Maximum sequence length for sequence patterns.
pub const MAX_SEQUENCE_LENGTH: usize = 5;

/// Aging decay factor per day of inactivity.
pub const AGING_DECAY_PER_DAY: f32 = 0.98;

/// Minimum observations before a pattern is surfaced.
pub const MIN_OBSERVATIONS: u32 = 3;

/// Maximum observations stored in the sliding window.
const MAX_OBSERVATION_WINDOW: usize = 1024;

/// Milliseconds in one day.
const MS_PER_DAY: u64 = 24 * 60 * 60 * 1000;

/// Milliseconds in one minute.
const MS_PER_MINUTE: u64 = 60 * 1000;

// ---------------------------------------------------------------------------
// TemporalPattern
// ---------------------------------------------------------------------------

/// A pattern that recurs at a specific time of day and/or day of week.
///
/// Example: "User opens Twitter every weekday at ~8:15 AM."
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TemporalPattern {
    /// Unique pattern identifier (hash of action + time bucket).
    pub id: u64,
    /// The action or concept associated with this pattern.
    pub action: String,
    /// Average minute-of-day when this pattern fires (0–1439).
    pub avg_minute_of_day: f32,
    /// Variance of the minute-of-day (for tolerance checking).
    pub minute_variance: f32,
    /// Bitmask of active days (bit 0 = Monday, bit 6 = Sunday).
    pub days_active: u8,
    /// Bayesian confidence in this pattern [0.0, 1.0].
    pub confidence: f32,
    /// Number of hits (observed occurrences).
    pub hit_count: u32,
    /// Number of misses (expected but not observed).
    pub miss_count: u32,
    /// Timestamp (ms) of last observation.
    pub last_observed_ms: u64,
    /// Timestamp (ms) of creation.
    pub created_ms: u64,
}

impl TemporalPattern {
    /// Create a new temporal pattern from an initial observation.
    #[must_use]
    pub fn new(action: &str, minute_of_day: f32, day_of_week: u8, now_ms: u64) -> Self {
        let id = super::hebbian::fnv1a_hash_public(
            format!("temporal:{}:{}", action, minute_of_day as u32 / 60).as_bytes(),
        );
        let mut days_active = 0u8;
        if day_of_week <= 6 {
            days_active = 1 << day_of_week;
        }
        Self {
            id,
            action: action.to_owned(),
            avg_minute_of_day: minute_of_day,
            minute_variance: 0.0,
            days_active,
            confidence: 1.0 / (BAYESIAN_ALPHA + 1.0),
            hit_count: 1,
            miss_count: 0,
            last_observed_ms: now_ms,
            created_ms: now_ms,
        }
    }

    /// Check if a given day is active for this pattern.
    #[must_use]
    pub fn is_active_on_day(&self, day_of_week: u8) -> bool {
        if day_of_week > 6 {
            return false;
        }
        (self.days_active >> day_of_week) & 1 == 1
    }

    /// Record a hit: the pattern was observed at the expected time.
    pub fn record_hit(&mut self, minute_of_day: f32, day_of_week: u8, now_ms: u64) {
        self.hit_count = self.hit_count.saturating_add(1);
        self.last_observed_ms = now_ms;

        // Bayesian confidence update: push toward 1.0
        self.confidence = (BAYESIAN_ALPHA * self.confidence + 1.0) / (BAYESIAN_ALPHA + 1.0);
        self.confidence = self.confidence.clamp(0.0, 1.0);

        // Update running average of minute_of_day using Welford's method
        let n = self.hit_count as f32;
        let delta = minute_of_day - self.avg_minute_of_day;
        self.avg_minute_of_day += delta / n;
        let delta2 = minute_of_day - self.avg_minute_of_day;
        // Online variance update
        if n > 1.0 {
            self.minute_variance += (delta * delta2 - self.minute_variance) / n;
        }

        // Mark day as active
        if day_of_week <= 6 {
            self.days_active |= 1 << day_of_week;
        }
    }

    /// Record a miss: the pattern was expected but not observed.
    pub fn record_miss(&mut self) {
        self.miss_count = self.miss_count.saturating_add(1);
        // Bayesian confidence update: push toward 0.0
        self.confidence = (BAYESIAN_ALPHA * self.confidence) / (BAYESIAN_ALPHA + 1.0);
        self.confidence = self.confidence.clamp(0.0, 1.0);
    }

    /// Whether this pattern matches a given time within tolerance.
    #[must_use]
    pub fn matches_time(&self, minute_of_day: f32, day_of_week: u8) -> bool {
        if !self.is_active_on_day(day_of_week) {
            return false;
        }
        let diff = (minute_of_day - self.avg_minute_of_day).abs();
        // Handle midnight wraparound
        let diff = diff.min(1440.0 - diff);
        diff <= TEMPORAL_TOLERANCE_MINUTES as f32
    }

    /// Total observations (hits + misses).
    #[must_use]
    pub fn total_observations(&self) -> u32 {
        self.hit_count.saturating_add(self.miss_count)
    }
}

// ---------------------------------------------------------------------------
// SequencePattern
// ---------------------------------------------------------------------------

/// An ordered chain of actions that commonly occur in succession.
///
/// Example: "User unlocks phone → opens Slack → opens Calendar" (sequence of 3).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SequencePattern {
    /// Unique pattern identifier (hash of the action sequence).
    pub id: u64,
    /// Ordered list of actions in this sequence.
    pub actions: Vec<String>,
    /// Bayesian confidence [0.0, 1.0].
    pub confidence: f32,
    /// Number of times this exact sequence was observed.
    pub occurrence_count: u32,
    /// Timestamp (ms) of last observation.
    pub last_observed_ms: u64,
    /// Timestamp (ms) of creation.
    pub created_ms: u64,
}

impl SequencePattern {
    /// Create a new sequence pattern from a list of actions.
    ///
    /// Returns `None` if the sequence is empty or exceeds [`MAX_SEQUENCE_LENGTH`].
    #[must_use]
    pub fn new(actions: &[&str], now_ms: u64) -> Option<Self> {
        if actions.is_empty() || actions.len() > MAX_SEQUENCE_LENGTH {
            return None;
        }
        let key = actions.join("→");
        let id = super::hebbian::fnv1a_hash_public(format!("seq:{key}").as_bytes());
        Some(Self {
            id,
            actions: actions.iter().map(|s| (*s).to_owned()).collect(),
            confidence: 1.0 / (BAYESIAN_ALPHA + 1.0),
            occurrence_count: 1,
            last_observed_ms: now_ms,
            created_ms: now_ms,
        })
    }

    /// Record another observation of this sequence.
    pub fn record_hit(&mut self, now_ms: u64) {
        self.occurrence_count = self.occurrence_count.saturating_add(1);
        self.last_observed_ms = now_ms;
        self.confidence = (BAYESIAN_ALPHA * self.confidence + 1.0) / (BAYESIAN_ALPHA + 1.0);
        self.confidence = self.confidence.clamp(0.0, 1.0);
    }

    /// Record a miss (start of sequence seen but not completed).
    pub fn record_miss(&mut self) {
        self.confidence = (BAYESIAN_ALPHA * self.confidence) / (BAYESIAN_ALPHA + 1.0);
        self.confidence = self.confidence.clamp(0.0, 1.0);
    }

    /// Length of this sequence.
    #[must_use]
    pub fn len(&self) -> usize {
        self.actions.len()
    }

    /// Whether this sequence is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.actions.is_empty()
    }
}

// ---------------------------------------------------------------------------
// ContextPattern
// ---------------------------------------------------------------------------

/// An environmental context descriptor.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ContextKey {
    /// Current foreground app.
    ForegroundApp(String),
    /// Battery level bucket (e.g. "low", "medium", "high").
    BatteryLevel(String),
    /// Network connectivity type (e.g. "wifi", "cellular", "offline").
    Connectivity(String),
    /// General location label (e.g. "home", "work", "gym").
    Location(String),
    /// Charging state.
    IsCharging(bool),
    /// Custom context key.
    Custom(String, String),
}

impl ContextKey {
    /// Produce a stable string key for hashing.
    #[must_use]
    pub fn to_key_string(&self) -> String {
        match self {
            ContextKey::ForegroundApp(app) => format!("app:{app}"),
            ContextKey::BatteryLevel(level) => format!("battery:{level}"),
            ContextKey::Connectivity(conn) => format!("conn:{conn}"),
            ContextKey::Location(loc) => format!("loc:{loc}"),
            ContextKey::IsCharging(c) => format!("charging:{c}"),
            ContextKey::Custom(k, v) => format!("custom:{k}:{v}"),
        }
    }
}

/// An action that correlates with an environmental context.
///
/// Example: "User opens Spotify when connected to car Bluetooth."
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextPattern {
    /// Unique pattern identifier.
    pub id: u64,
    /// The action/concept triggered by this context.
    pub action: String,
    /// The context key that triggers this pattern.
    pub context: ContextKey,
    /// Bayesian confidence [0.0, 1.0].
    pub confidence: f32,
    /// Number of times this context→action pair was observed.
    pub hit_count: u32,
    /// Number of times the context was seen WITHOUT the action.
    pub miss_count: u32,
    /// Timestamp (ms) of last observation.
    pub last_observed_ms: u64,
    /// Timestamp (ms) of creation.
    pub created_ms: u64,
}

impl ContextPattern {
    /// Create a new context pattern from an initial observation.
    #[must_use]
    pub fn new(action: &str, context: ContextKey, now_ms: u64) -> Self {
        let id = super::hebbian::fnv1a_hash_public(
            format!("ctx:{}:{}", action, context.to_key_string()).as_bytes(),
        );
        Self {
            id,
            action: action.to_owned(),
            context,
            confidence: 1.0 / (BAYESIAN_ALPHA + 1.0),
            hit_count: 1,
            miss_count: 0,
            last_observed_ms: now_ms,
            created_ms: now_ms,
        }
    }

    /// Record a hit.
    pub fn record_hit(&mut self, now_ms: u64) {
        self.hit_count = self.hit_count.saturating_add(1);
        self.last_observed_ms = now_ms;
        self.confidence = (BAYESIAN_ALPHA * self.confidence + 1.0) / (BAYESIAN_ALPHA + 1.0);
        self.confidence = self.confidence.clamp(0.0, 1.0);
    }

    /// Record a miss.
    pub fn record_miss(&mut self) {
        self.miss_count = self.miss_count.saturating_add(1);
        self.confidence = (BAYESIAN_ALPHA * self.confidence) / (BAYESIAN_ALPHA + 1.0);
        self.confidence = self.confidence.clamp(0.0, 1.0);
    }

    /// Total observations.
    #[must_use]
    pub fn total_observations(&self) -> u32 {
        self.hit_count.saturating_add(self.miss_count)
    }
}

// ---------------------------------------------------------------------------
// Observation (sliding window entry)
// ---------------------------------------------------------------------------

/// A single observation in the sliding window.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Observation {
    /// The action or concept observed.
    pub action: String,
    /// Timestamp (ms) of the observation.
    pub timestamp_ms: u64,
    /// Minute of day (0–1439) extracted from the timestamp.
    pub minute_of_day: f32,
    /// Day of week (0=Monday, 6=Sunday).
    pub day_of_week: u8,
    /// Active context keys at the time of observation.
    pub context: Vec<ContextKey>,
}

// ---------------------------------------------------------------------------
// PatternDetector
// ---------------------------------------------------------------------------

/// The main pattern detection engine.
///
/// Maintains a sliding window of recent observations and extracts
/// temporal, sequential, and contextual patterns via statistical analysis.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PatternDetector {
    /// Detected temporal patterns.
    temporal_patterns: HashMap<u64, TemporalPattern>,
    /// Detected sequence patterns.
    sequence_patterns: HashMap<u64, SequencePattern>,
    /// Detected context patterns.
    context_patterns: HashMap<u64, ContextPattern>,
    /// Sliding window of recent observations.
    observation_window: Vec<Observation>,
}

impl PatternDetector {
    /// Create a new empty pattern detector.
    #[must_use]
    pub fn new() -> Self {
        Self {
            temporal_patterns: HashMap::with_capacity(32),
            sequence_patterns: HashMap::with_capacity(64),
            context_patterns: HashMap::with_capacity(32),
            observation_window: Vec::with_capacity(128),
        }
    }

    // -- accessors ----------------------------------------------------------

    /// Number of temporal patterns tracked.
    #[must_use]
    pub fn temporal_count(&self) -> usize {
        self.temporal_patterns.len()
    }

    /// Number of sequence patterns tracked.
    #[must_use]
    pub fn sequence_count(&self) -> usize {
        self.sequence_patterns.len()
    }

    /// Number of context patterns tracked.
    #[must_use]
    pub fn context_count(&self) -> usize {
        self.context_patterns.len()
    }

    /// Total pattern count across all types.
    #[must_use]
    pub fn total_pattern_count(&self) -> usize {
        self.temporal_count() + self.sequence_count() + self.context_count()
    }

    /// Number of observations in the sliding window.
    #[must_use]
    pub fn observation_count(&self) -> usize {
        self.observation_window.len()
    }

    /// Get a temporal pattern by id.
    #[must_use]
    pub fn get_temporal(&self, id: u64) -> Option<&TemporalPattern> {
        self.temporal_patterns.get(&id)
    }

    /// Get a sequence pattern by id.
    #[must_use]
    pub fn get_sequence(&self, id: u64) -> Option<&SequencePattern> {
        self.sequence_patterns.get(&id)
    }

    /// Get a context pattern by id.
    #[must_use]
    pub fn get_context(&self, id: u64) -> Option<&ContextPattern> {
        self.context_patterns.get(&id)
    }

    // -- observation recording ----------------------------------------------

    /// Record a new observation and trigger pattern detection.
    ///
    /// This is the main entry point for the pattern detector.
    #[instrument(skip_all, fields(action = %obs.action))]
    pub fn observe(&mut self, obs: Observation) -> Result<PatternDetectionResult, ArcError> {
        let now_ms = obs.timestamp_ms;
        let mut result = PatternDetectionResult::default();

        // 1. Detect/update temporal patterns
        self.detect_temporal_pattern(&obs, &mut result)?;

        // 2. Detect/update context patterns
        self.detect_context_patterns(&obs, &mut result)?;

        // 3. Add to sliding window (bounded)
        self.observation_window.push(obs);
        if self.observation_window.len() > MAX_OBSERVATION_WINDOW {
            // Remove oldest
            self.observation_window.remove(0);
        }

        // 4. Detect/update sequence patterns from recent window
        self.detect_sequence_patterns(now_ms, &mut result)?;

        debug!(
            temporal = result.temporal_updates,
            sequence = result.sequence_updates,
            context = result.context_updates,
            "pattern detection pass complete"
        );

        Ok(result)
    }

    // -- temporal pattern detection -----------------------------------------

    fn detect_temporal_pattern(
        &mut self,
        obs: &Observation,
        result: &mut PatternDetectionResult,
    ) -> Result<(), ArcError> {
        // Find existing temporal pattern for this action at this time.
        // First try exact match (action + time + day), then relaxed match
        // (action + time only) so the pattern can learn new active days.
        let matching = self
            .temporal_patterns
            .values_mut()
            .find(|p| p.action == obs.action && p.matches_time(obs.minute_of_day, obs.day_of_week));

        if let Some(pattern) = matching {
            pattern.record_hit(obs.minute_of_day, obs.day_of_week, obs.timestamp_ms);
            result.temporal_updates += 1;
            debug!(
                pattern_id = pattern.id,
                confidence = pattern.confidence,
                "temporal pattern hit"
            );
        } else {
            // Try relaxed match: same action, similar time, but different day.
            // This lets a pattern accumulate hits across days so it can become
            // actionable instead of fragmenting into many single-day patterns.
            let relaxed = self.temporal_patterns.values_mut().find(|p| {
                if p.action != obs.action {
                    return false;
                }
                let diff = (obs.minute_of_day - p.avg_minute_of_day).abs();
                let diff = diff.min(1440.0 - diff);
                diff <= TEMPORAL_TOLERANCE_MINUTES as f32
            });

            if let Some(pattern) = relaxed {
                pattern.record_hit(obs.minute_of_day, obs.day_of_week, obs.timestamp_ms);
                result.temporal_updates += 1;
                debug!(
                    pattern_id = pattern.id,
                    confidence = pattern.confidence,
                    "temporal pattern hit (new day)"
                );
            } else {
                // New temporal pattern — check capacity
                if self.temporal_patterns.len() >= MAX_TEMPORAL_PATTERNS {
                    self.evict_weakest_temporal();
                }
                let pattern = TemporalPattern::new(
                    &obs.action,
                    obs.minute_of_day,
                    obs.day_of_week,
                    obs.timestamp_ms,
                );
                let id = pattern.id;
                self.temporal_patterns.insert(id, pattern);
                result.new_patterns += 1;
                debug!(pattern_id = id, "new temporal pattern created");
            }
        }

        Ok(())
    }

    // -- context pattern detection ------------------------------------------

    fn detect_context_patterns(
        &mut self,
        obs: &Observation,
        result: &mut PatternDetectionResult,
    ) -> Result<(), ArcError> {
        for ctx in &obs.context {
            let key_str = ctx.to_key_string();
            let probe_id = super::hebbian::fnv1a_hash_public(
                format!("ctx:{}:{}", obs.action, key_str).as_bytes(),
            );

            if let Some(pattern) = self.context_patterns.get_mut(&probe_id) {
                pattern.record_hit(obs.timestamp_ms);
                result.context_updates += 1;
            } else {
                if self.context_patterns.len() >= MAX_CONTEXT_PATTERNS {
                    self.evict_weakest_context();
                }
                let pattern = ContextPattern::new(&obs.action, ctx.clone(), obs.timestamp_ms);
                self.context_patterns.insert(pattern.id, pattern);
                result.new_patterns += 1;
            }
        }
        Ok(())
    }

    // -- sequence pattern detection -----------------------------------------

    fn detect_sequence_patterns(
        &mut self,
        _now_ms: u64,
        result: &mut PatternDetectionResult,
    ) -> Result<(), ArcError> {
        let window_len = self.observation_window.len();
        if window_len < 2 {
            return Ok(());
        }

        // Extract recent actions for n-gram analysis (last 10 observations).
        // Clone into owned strings to avoid holding a borrow on self.observation_window
        // while we mutate self.sequence_patterns below.
        let start = window_len.saturating_sub(10);
        let recent: Vec<String> = self.observation_window[start..]
            .iter()
            .map(|o| o.action.clone())
            .collect();

        // Generate 2-grams through MAX_SEQUENCE_LENGTH-grams
        for n in 2..=MAX_SEQUENCE_LENGTH.min(recent.len()) {
            for window in recent.windows(n) {
                let key = window.join("→");
                let id = super::hebbian::fnv1a_hash_public(format!("seq:{key}").as_bytes());

                if let Some(pattern) = self.sequence_patterns.get_mut(&id) {
                    let last_obs = self
                        .observation_window
                        .last()
                        .map(|o| o.timestamp_ms)
                        .unwrap_or(0);
                    pattern.record_hit(last_obs);
                    result.sequence_updates += 1;
                } else {
                    if self.sequence_patterns.len() >= MAX_SEQUENCE_PATTERNS {
                        self.evict_weakest_sequence();
                    }
                    let window_refs: Vec<&str> = window.iter().map(|s| s.as_str()).collect();
                    if let Some(pattern) = SequencePattern::new(&window_refs, 0) {
                        let last_obs = self
                            .observation_window
                            .last()
                            .map(|o| o.timestamp_ms)
                            .unwrap_or(0);
                        let mut pattern = pattern;
                        pattern.last_observed_ms = last_obs;
                        self.sequence_patterns.insert(pattern.id, pattern);
                        result.new_patterns += 1;
                    }
                }
            }
        }

        Ok(())
    }

    // -- querying -----------------------------------------------------------

    /// Get all actionable temporal patterns (above confidence + observation thresholds).
    #[must_use]
    pub fn actionable_temporal_patterns(&self) -> Vec<&TemporalPattern> {
        self.temporal_patterns
            .values()
            .filter(|p| p.confidence >= MIN_CONFIDENCE && p.hit_count >= MIN_OBSERVATIONS)
            .collect()
    }

    /// Get all actionable sequence patterns.
    #[must_use]
    pub fn actionable_sequence_patterns(&self) -> Vec<&SequencePattern> {
        self.sequence_patterns
            .values()
            .filter(|p| p.confidence >= MIN_CONFIDENCE && p.occurrence_count >= MIN_OBSERVATIONS)
            .collect()
    }

    /// Get all actionable context patterns.
    #[must_use]
    pub fn actionable_context_patterns(&self) -> Vec<&ContextPattern> {
        self.context_patterns
            .values()
            .filter(|p| p.confidence >= MIN_CONFIDENCE && p.hit_count >= MIN_OBSERVATIONS)
            .collect()
    }

    /// Predict what action might come next given a prefix of recent actions.
    ///
    /// Returns a list of (action, confidence) sorted by descending confidence.
    #[must_use]
    pub fn predict_next_action(&self, recent_actions: &[&str]) -> Vec<(String, f32)> {
        if recent_actions.is_empty() {
            return Vec::new();
        }

        let mut predictions: Vec<(String, f32)> = Vec::new();

        for pattern in self.sequence_patterns.values() {
            if pattern.confidence < MIN_CONFIDENCE || pattern.occurrence_count < MIN_OBSERVATIONS {
                continue;
            }

            // Check if the recent actions match a prefix of this pattern
            let pat_len = pattern.actions.len();
            let recent_len = recent_actions.len();
            if recent_len >= pat_len {
                continue; // Pattern is shorter than or equal to what we have
            }

            // Check if recent_actions matches the start of pattern.actions
            let matches = recent_actions
                .iter()
                .zip(pattern.actions.iter())
                .all(|(a, b)| *a == b.as_str());

            if matches {
                // The next predicted action is pattern.actions[recent_len]
                let next_action = pattern.actions[recent_len].clone();
                predictions.push((next_action, pattern.confidence));
            }
        }

        // Sort by descending confidence
        predictions.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        // Deduplicate by action, keeping highest confidence
        let mut seen = std::collections::HashSet::new();
        predictions.retain(|(action, _)| seen.insert(action.clone()));

        predictions
    }

    /// Get patterns matching a given context.
    ///
    /// Returns (action, confidence) pairs sorted by descending confidence.
    #[must_use]
    pub fn patterns_for_context(&self, context: &[ContextKey]) -> Vec<(String, f32)> {
        let mut results: Vec<(String, f32)> = Vec::new();

        for ctx_key in context {
            for pattern in self.context_patterns.values() {
                if pattern.context == *ctx_key
                    && pattern.confidence >= MIN_CONFIDENCE
                    && pattern.hit_count >= MIN_OBSERVATIONS
                {
                    results.push((pattern.action.clone(), pattern.confidence));
                }
            }
        }

        results.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        // Deduplicate
        let mut seen = std::collections::HashSet::new();
        results.retain(|(action, _)| seen.insert(action.clone()));

        results
    }

    // -- aging & pruning ----------------------------------------------------

    /// Age all patterns based on elapsed time since their last observation.
    ///
    /// Patterns that fall below [`PRUNE_CONFIDENCE`] are removed.
    /// Returns the number of patterns pruned.
    #[instrument(skip_all, fields(now_ms))]
    pub fn age_patterns(&mut self, now_ms: u64) -> usize {
        let mut pruned = 0usize;

        // Age temporal patterns
        let temporal_to_prune: Vec<u64> = self
            .temporal_patterns
            .iter()
            .filter_map(|(&id, p)| {
                let days_inactive =
                    now_ms.saturating_sub(p.last_observed_ms) as f64 / MS_PER_DAY as f64;
                let decayed = p.confidence * AGING_DECAY_PER_DAY.powf(days_inactive as f32);
                if decayed < PRUNE_CONFIDENCE {
                    Some(id)
                } else {
                    None
                }
            })
            .collect();
        for id in &temporal_to_prune {
            self.temporal_patterns.remove(id);
        }
        pruned += temporal_to_prune.len();

        // Age sequence patterns
        let seq_to_prune: Vec<u64> = self
            .sequence_patterns
            .iter()
            .filter_map(|(&id, p)| {
                let days_inactive =
                    now_ms.saturating_sub(p.last_observed_ms) as f64 / MS_PER_DAY as f64;
                let decayed = p.confidence * AGING_DECAY_PER_DAY.powf(days_inactive as f32);
                if decayed < PRUNE_CONFIDENCE {
                    Some(id)
                } else {
                    None
                }
            })
            .collect();
        for id in &seq_to_prune {
            self.sequence_patterns.remove(id);
        }
        pruned += seq_to_prune.len();

        // Age context patterns
        let ctx_to_prune: Vec<u64> = self
            .context_patterns
            .iter()
            .filter_map(|(&id, p)| {
                let days_inactive =
                    now_ms.saturating_sub(p.last_observed_ms) as f64 / MS_PER_DAY as f64;
                let decayed = p.confidence * AGING_DECAY_PER_DAY.powf(days_inactive as f32);
                if decayed < PRUNE_CONFIDENCE {
                    Some(id)
                } else {
                    None
                }
            })
            .collect();
        for id in &ctx_to_prune {
            self.context_patterns.remove(id);
        }
        pruned += ctx_to_prune.len();

        if pruned > 0 {
            info!(pruned, "pattern aging pass complete");
        }

        pruned
    }

    // -- eviction helpers ---------------------------------------------------

    fn evict_weakest_temporal(&mut self) {
        if let Some((&id, _)) = self.temporal_patterns.iter().min_by(|a, b| {
            a.1.confidence
                .partial_cmp(&b.1.confidence)
                .unwrap_or(std::cmp::Ordering::Equal)
        }) {
            self.temporal_patterns.remove(&id);
        }
    }

    fn evict_weakest_sequence(&mut self) {
        if let Some((&id, _)) = self.sequence_patterns.iter().min_by(|a, b| {
            a.1.confidence
                .partial_cmp(&b.1.confidence)
                .unwrap_or(std::cmp::Ordering::Equal)
        }) {
            self.sequence_patterns.remove(&id);
        }
    }

    fn evict_weakest_context(&mut self) {
        if let Some((&id, _)) = self.context_patterns.iter().min_by(|a, b| {
            a.1.confidence
                .partial_cmp(&b.1.confidence)
                .unwrap_or(std::cmp::Ordering::Equal)
        }) {
            self.context_patterns.remove(&id);
        }
    }
}

impl Default for PatternDetector {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// PatternDetectionResult
// ---------------------------------------------------------------------------

/// Summary of a single pattern detection pass.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PatternDetectionResult {
    /// Number of temporal pattern updates (hits).
    pub temporal_updates: usize,
    /// Number of sequence pattern updates (hits).
    pub sequence_updates: usize,
    /// Number of context pattern updates (hits).
    pub context_updates: usize,
    /// Number of new patterns created.
    pub new_patterns: usize,
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -- TemporalPattern tests ----------------------------------------------

    #[test]
    fn test_temporal_pattern_new() {
        let p = TemporalPattern::new("open_twitter", 495.0, 0, 1000);
        assert_eq!(p.action, "open_twitter");
        assert!((p.avg_minute_of_day - 495.0).abs() < f32::EPSILON);
        assert!(p.is_active_on_day(0)); // Monday
        assert!(!p.is_active_on_day(1));
        assert_eq!(p.hit_count, 1);
        assert!(p.confidence > 0.0);
    }

    #[test]
    fn test_temporal_pattern_hit_increases_confidence() {
        let mut p = TemporalPattern::new("open_email", 480.0, 0, 1000);
        let initial_conf = p.confidence;
        p.record_hit(485.0, 0, 2000);
        assert!(p.confidence > initial_conf);
        assert_eq!(p.hit_count, 2);
    }

    #[test]
    fn test_temporal_pattern_miss_decreases_confidence() {
        let mut p = TemporalPattern::new("open_email", 480.0, 0, 1000);
        // Build up confidence
        for i in 0..10 {
            p.record_hit(480.0 + i as f32, 0, 1000 + i * 1000);
        }
        let conf_before = p.confidence;
        p.record_miss();
        assert!(p.confidence < conf_before);
    }

    #[test]
    fn test_temporal_pattern_matches_time() {
        let mut p = TemporalPattern::new("gym", 360.0, 0, 1000);
        p.days_active = 0b0010101; // Mon, Wed, Fri
        assert!(p.matches_time(365.0, 0)); // Mon, 5 min off
        assert!(p.matches_time(340.0, 2)); // Wed, 20 min off
        assert!(!p.matches_time(420.0, 0)); // Mon, 60 min off — too far
        assert!(!p.matches_time(360.0, 1)); // Tue — not active
    }

    #[test]
    fn test_temporal_pattern_midnight_wraparound() {
        let p = TemporalPattern::new("bedtime", 1420.0, 0, 1000);
        // 1420 min = 23:40. Check that 0010 (00:10 = 10 min) matches within 30 min tolerance
        // diff = |10 - 1420| = 1410, min(1410, 1440-1410) = min(1410, 30) = 30. Exactly at boundary.
        assert!(p.matches_time(10.0, 0));
    }

    #[test]
    fn test_temporal_total_observations() {
        let mut p = TemporalPattern::new("test", 100.0, 0, 0);
        p.miss_count = 5;
        assert_eq!(p.total_observations(), 6); // 1 hit + 5 misses
    }

    // -- SequencePattern tests ----------------------------------------------

    #[test]
    fn test_sequence_pattern_new() {
        let p = SequencePattern::new(&["unlock", "slack", "calendar"], 1000);
        assert!(p.is_some());
        let p = p.expect("should create");
        assert_eq!(p.actions.len(), 3);
        assert_eq!(p.occurrence_count, 1);
    }

    #[test]
    fn test_sequence_pattern_empty_rejected() {
        let p = SequencePattern::new(&[], 1000);
        assert!(p.is_none());
    }

    #[test]
    fn test_sequence_pattern_too_long_rejected() {
        let actions: Vec<&str> = (0..MAX_SEQUENCE_LENGTH + 1).map(|_| "a").collect();
        let p = SequencePattern::new(&actions, 1000);
        assert!(p.is_none());
    }

    #[test]
    fn test_sequence_pattern_hit() {
        let mut p = SequencePattern::new(&["a", "b"], 1000).expect("ok");
        let initial_conf = p.confidence;
        p.record_hit(2000);
        assert!(p.confidence > initial_conf);
        assert_eq!(p.occurrence_count, 2);
    }

    #[test]
    fn test_sequence_pattern_miss() {
        let mut p = SequencePattern::new(&["a", "b"], 1000).expect("ok");
        for _ in 0..5 {
            p.record_hit(2000);
        }
        let conf = p.confidence;
        p.record_miss();
        assert!(p.confidence < conf);
    }

    // -- ContextPattern tests -----------------------------------------------

    #[test]
    fn test_context_pattern_new() {
        let p = ContextPattern::new(
            "open_spotify",
            ContextKey::Connectivity("bluetooth_car".into()),
            1000,
        );
        assert_eq!(p.action, "open_spotify");
        assert_eq!(p.hit_count, 1);
    }

    #[test]
    fn test_context_pattern_hit_and_miss() {
        let mut p = ContextPattern::new("open_maps", ContextKey::Location("car".into()), 1000);
        let c0 = p.confidence;
        p.record_hit(2000);
        assert!(p.confidence > c0);
        let c1 = p.confidence;
        p.record_miss();
        assert!(p.confidence < c1);
    }

    #[test]
    fn test_context_key_to_string() {
        assert_eq!(
            ContextKey::ForegroundApp("Chrome".into()).to_key_string(),
            "app:Chrome"
        );
        assert_eq!(
            ContextKey::IsCharging(true).to_key_string(),
            "charging:true"
        );
        assert_eq!(
            ContextKey::Custom("mode".into(), "focus".into()).to_key_string(),
            "custom:mode:focus"
        );
    }

    // -- PatternDetector tests ----------------------------------------------

    #[test]
    fn test_detector_new_empty() {
        let d = PatternDetector::new();
        assert_eq!(d.total_pattern_count(), 0);
        assert_eq!(d.observation_count(), 0);
    }

    #[test]
    fn test_detector_observe_creates_temporal() {
        let mut d = PatternDetector::new();
        let obs = Observation {
            action: "open_news".into(),
            timestamp_ms: 1000,
            minute_of_day: 480.0,
            day_of_week: 0,
            context: vec![],
        };
        let result = d.observe(obs).expect("ok");
        assert!(result.new_patterns >= 1);
        assert_eq!(d.temporal_count(), 1);
    }

    #[test]
    fn test_detector_observe_creates_context() {
        let mut d = PatternDetector::new();
        let obs = Observation {
            action: "open_spotify".into(),
            timestamp_ms: 1000,
            minute_of_day: 480.0,
            day_of_week: 0,
            context: vec![ContextKey::Location("car".into())],
        };
        let result = d.observe(obs).expect("ok");
        assert!(d.context_count() >= 1);
        assert!(result.new_patterns >= 1);
    }

    #[test]
    fn test_detector_repeated_observation_strengthens() {
        let mut d = PatternDetector::new();
        for i in 0..5 {
            let obs = Observation {
                action: "check_email".into(),
                timestamp_ms: 1000 + i * MS_PER_DAY,
                minute_of_day: 480.0,
                day_of_week: (i % 5) as u8,
                context: vec![],
            };
            d.observe(obs).expect("ok");
        }
        let actionable = d.actionable_temporal_patterns();
        assert!(
            !actionable.is_empty(),
            "should have at least one actionable temporal pattern"
        );
    }

    #[test]
    fn test_detector_sequence_detection() {
        let mut d = PatternDetector::new();
        // Feed a sequence twice
        for round in 0..4 {
            for (i, action) in ["unlock", "slack", "calendar"].iter().enumerate() {
                let obs = Observation {
                    action: action.to_string(),
                    timestamp_ms: round * 100_000 + i as u64 * 1000,
                    minute_of_day: 480.0,
                    day_of_week: 0,
                    context: vec![],
                };
                d.observe(obs).expect("ok");
            }
        }
        assert!(d.sequence_count() > 0, "should detect sequence patterns");
    }

    #[test]
    fn test_predict_next_action() {
        let mut d = PatternDetector::new();
        // Feed the same sequence multiple times to build confidence
        for round in 0..5 {
            for (i, action) in ["unlock", "slack", "calendar"].iter().enumerate() {
                let obs = Observation {
                    action: action.to_string(),
                    timestamp_ms: round * 100_000 + i as u64 * 1000,
                    minute_of_day: 480.0,
                    day_of_week: 0,
                    context: vec![],
                };
                d.observe(obs).expect("ok");
            }
        }
        let predictions = d.predict_next_action(&["unlock"]);
        // We should get "slack" as a predicted next action
        let has_slack = predictions.iter().any(|(a, _)| a == "slack");
        assert!(
            has_slack,
            "should predict 'slack' after 'unlock', got: {predictions:?}"
        );
    }

    #[test]
    fn test_patterns_for_context() {
        let mut d = PatternDetector::new();
        let ctx = ContextKey::Location("gym".into());
        for i in 0..5 {
            let obs = Observation {
                action: "open_fitness_app".into(),
                timestamp_ms: i * MS_PER_DAY,
                minute_of_day: 360.0,
                day_of_week: (i % 7) as u8,
                context: vec![ctx.clone()],
            };
            d.observe(obs).expect("ok");
        }
        let results = d.patterns_for_context(&[ctx]);
        let has_fitness = results.iter().any(|(a, _)| a == "open_fitness_app");
        assert!(
            has_fitness,
            "should find fitness app pattern for gym context"
        );
    }

    #[test]
    fn test_age_patterns_prunes_old() {
        let mut d = PatternDetector::new();
        let obs = Observation {
            action: "old_action".into(),
            timestamp_ms: 0,
            minute_of_day: 100.0,
            day_of_week: 0,
            context: vec![],
        };
        d.observe(obs).expect("ok");
        assert!(d.temporal_count() >= 1);

        // Age patterns by a huge amount of time (simulating years of inactivity)
        let far_future = 365 * 10 * MS_PER_DAY; // 10 years
        let pruned = d.age_patterns(far_future);
        assert!(pruned >= 1, "should prune old patterns");
    }

    #[test]
    fn test_observation_window_bounded() {
        let mut d = PatternDetector::new();
        for i in 0..(MAX_OBSERVATION_WINDOW + 100) {
            let obs = Observation {
                action: format!("action_{}", i % 10),
                timestamp_ms: i as u64 * 1000,
                minute_of_day: (i % 1440) as f32,
                day_of_week: (i % 7) as u8,
                context: vec![],
            };
            d.observe(obs).expect("ok");
        }
        assert!(
            d.observation_count() <= MAX_OBSERVATION_WINDOW,
            "window should be bounded"
        );
    }

    #[test]
    fn test_pattern_detection_result_default() {
        let r = PatternDetectionResult::default();
        assert_eq!(r.temporal_updates, 0);
        assert_eq!(r.sequence_updates, 0);
        assert_eq!(r.context_updates, 0);
        assert_eq!(r.new_patterns, 0);
    }

    #[test]
    fn test_serde_roundtrip_temporal() {
        let p = TemporalPattern::new("test_action", 600.0, 3, 5000);
        let json = serde_json::to_string(&p).expect("serialize");
        let back: TemporalPattern = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.action, "test_action");
        assert!((back.avg_minute_of_day - 600.0).abs() < f32::EPSILON);
    }

    #[test]
    fn test_serde_roundtrip_context_key() {
        let keys = vec![
            ContextKey::ForegroundApp("Chrome".into()),
            ContextKey::BatteryLevel("high".into()),
            ContextKey::Connectivity("wifi".into()),
            ContextKey::Location("home".into()),
            ContextKey::IsCharging(true),
            ContextKey::Custom("test".into(), "value".into()),
        ];
        for k in &keys {
            let json = serde_json::to_string(k).expect("serialize");
            let back: ContextKey = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(*k, back);
        }
    }

    #[test]
    fn test_serde_roundtrip_detector() {
        let mut d = PatternDetector::new();
        let obs = Observation {
            action: "test".into(),
            timestamp_ms: 1000,
            minute_of_day: 480.0,
            day_of_week: 0,
            context: vec![ContextKey::Location("home".into())],
        };
        d.observe(obs).expect("ok");

        let json = serde_json::to_string(&d).expect("serialize");
        let back: PatternDetector = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.temporal_count(), d.temporal_count());
        assert_eq!(back.context_count(), d.context_count());
    }
}
