//! Pattern discovery engine for AURA's memory system.
//!
//! Tracks action→outcome relationships with Hebbian learning
//! (success: +0.1, failure: -0.15) and discovers temporal co-occurrence
//! patterns from event streams.

use std::collections::VecDeque;

use aura_types::errors::{AuraError, MemError};
use serde::{Deserialize, Serialize};
use tracing::instrument;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const MAX_ACTION_PATTERNS: usize = 2048;
const MAX_TEMPORAL_PATTERNS: usize = 512;
const MAX_RECENT_EVENTS: usize = 256;

/// Hebbian strengthening increment on success.
const HEBBIAN_SUCCESS: f32 = 0.1;
/// Hebbian weakening decrement on failure (asymmetric — failures matter more).
const HEBBIAN_FAILURE: f32 = 0.15;

/// Prune patterns weaker than this after the age threshold.
const PRUNE_STRENGTH_THRESHOLD: f32 = 0.05;
/// 7 days in milliseconds.
const PRUNE_AGE_MS: u64 = 604_800_000;

// ---------------------------------------------------------------------------
// Data structures
// ---------------------------------------------------------------------------

/// An action→outcome relationship tracked over time.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionPattern {
    pub action: String,
    pub context: String,
    pub outcome: String,
    pub strength: f32,
    pub occurrences: u32,
    pub last_seen_ms: u64,
    pub created_ms: u64,
    pub success_count: u32,
    pub failure_count: u32,
}

/// Temporal co-occurrence pattern.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TemporalPattern {
    pub events: Vec<String>,
    pub avg_interval_ms: u64,
    pub confidence: f32,
    pub occurrences: u32,
}

// ---------------------------------------------------------------------------
// PatternEngine
// ---------------------------------------------------------------------------

/// The pattern discovery engine.
pub struct PatternEngine {
    action_patterns: Vec<ActionPattern>,
    temporal_patterns: Vec<TemporalPattern>,
    recent_events: VecDeque<(String, u64)>,
    max_action_patterns: usize,
    max_temporal_patterns: usize,
    max_recent_events: usize,
}

impl Default for PatternEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl PatternEngine {
    /// Create a new pattern engine with default capacity bounds.
    pub fn new() -> Self {
        Self {
            action_patterns: Vec::new(),
            temporal_patterns: Vec::new(),
            recent_events: VecDeque::new(),
            max_action_patterns: MAX_ACTION_PATTERNS,
            max_temporal_patterns: MAX_TEMPORAL_PATTERNS,
            max_recent_events: MAX_RECENT_EVENTS,
        }
    }

    /// Record an action and its outcome. Strengthens or weakens existing patterns.
    #[instrument(skip(self))]
    pub fn record_outcome(
        &mut self,
        action: &str,
        context: &str,
        outcome: &str,
        success: bool,
        timestamp_ms: u64,
    ) -> Result<(), AuraError> {
        // Look for an existing pattern with matching action + context.
        let existing = self.action_patterns.iter_mut().find(|p| {
            p.action.eq_ignore_ascii_case(action) && p.context.eq_ignore_ascii_case(context)
        });

        if let Some(pattern) = existing {
            // Update existing pattern.
            pattern.occurrences = pattern.occurrences.saturating_add(1);
            pattern.last_seen_ms = timestamp_ms;
            pattern.outcome = outcome.to_string();
            if success {
                pattern.success_count = pattern.success_count.saturating_add(1);
                pattern.strength = (pattern.strength + HEBBIAN_SUCCESS).clamp(-1.0, 1.0);
            } else {
                pattern.failure_count = pattern.failure_count.saturating_add(1);
                pattern.strength = (pattern.strength - HEBBIAN_FAILURE).clamp(-1.0, 1.0);
            }
        } else {
            // Check capacity.
            if self.action_patterns.len() >= self.max_action_patterns {
                // Evict the weakest pattern.
                if let Some(weakest_idx) = self
                    .action_patterns
                    .iter()
                    .enumerate()
                    .min_by(|(_, a), (_, b)| {
                        a.strength
                            .abs()
                            .partial_cmp(&b.strength.abs())
                            .unwrap_or(std::cmp::Ordering::Equal)
                    })
                    .map(|(i, _)| i)
                {
                    self.action_patterns.swap_remove(weakest_idx);
                }
            }

            let initial_strength = if success {
                HEBBIAN_SUCCESS
            } else {
                -HEBBIAN_FAILURE
            };

            self.action_patterns.push(ActionPattern {
                action: action.to_string(),
                context: context.to_string(),
                outcome: outcome.to_string(),
                strength: initial_strength,
                occurrences: 1,
                last_seen_ms: timestamp_ms,
                created_ms: timestamp_ms,
                success_count: if success { 1 } else { 0 },
                failure_count: if success { 0 } else { 1 },
            });
        }

        Ok(())
    }

    /// Record an event for temporal pattern discovery.
    #[instrument(skip(self))]
    pub fn record_event(&mut self, event: &str, timestamp_ms: u64) {
        if self.recent_events.len() >= self.max_recent_events {
            self.recent_events.pop_front();
        }
        self.recent_events
            .push_back((event.to_string(), timestamp_ms));
    }

    /// Apply Hebbian learning to a specific action+context pattern.
    #[instrument(skip(self))]
    pub fn hebbian_update(&mut self, action: &str, context: &str, success: bool) {
        if let Some(pattern) = self.action_patterns.iter_mut().find(|p| {
            p.action.eq_ignore_ascii_case(action) && p.context.eq_ignore_ascii_case(context)
        }) {
            if success {
                pattern.strength = (pattern.strength + HEBBIAN_SUCCESS).clamp(-1.0, 1.0);
                pattern.success_count = pattern.success_count.saturating_add(1);
            } else {
                pattern.strength = (pattern.strength - HEBBIAN_FAILURE).clamp(-1.0, 1.0);
                pattern.failure_count = pattern.failure_count.saturating_add(1);
            }
            pattern.occurrences = pattern.occurrences.saturating_add(1);
        }
    }

    /// Find patterns matching a given action/context query.
    /// Returns patterns sorted by strength descending.
    #[instrument(skip(self))]
    pub fn find_patterns(&self, action: &str, context: &str) -> Vec<&ActionPattern> {
        let action_lower = action.to_lowercase();
        let context_lower = context.to_lowercase();

        let mut matched: Vec<&ActionPattern> = self
            .action_patterns
            .iter()
            .filter(|p| {
                p.action.to_lowercase().contains(&action_lower)
                    && p.context.to_lowercase().contains(&context_lower)
            })
            .collect();

        matched.sort_by(|a, b| {
            b.strength
                .partial_cmp(&a.strength)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        matched
    }

    /// Predict outcome for an action in a given context.
    /// Returns (predicted_outcome, confidence) or None.
    #[instrument(skip(self))]
    pub fn predict_outcome(&self, action: &str, context: &str) -> Option<(String, f32)> {
        let patterns = self.find_patterns(action, context);
        let best = patterns.first()?;

        // Confidence is based on strength and number of occurrences.
        let occ_factor = (best.occurrences as f32).ln().max(0.0) / 5.0;
        let confidence = (best.strength.abs() * 0.7 + occ_factor * 0.3).clamp(0.0, 1.0);

        Some((best.outcome.clone(), confidence))
    }

    /// Discover temporal patterns from recent events.
    /// Looks for repeated pairs in the event stream.
    #[instrument(skip(self))]
    pub fn discover_temporal_patterns(&mut self) -> Vec<TemporalPattern> {
        use std::collections::HashMap;

        let events: Vec<&(String, u64)> = self.recent_events.iter().collect();
        if events.len() < 2 {
            return Vec::new();
        }

        // Count adjacent pairs and their intervals.
        let mut pair_counts: HashMap<(String, String), Vec<u64>> = HashMap::new();
        for window in events.windows(2) {
            let key = (window[0].0.clone(), window[1].0.clone());
            let interval = window[1].1.saturating_sub(window[0].1);
            pair_counts.entry(key).or_default().push(interval);
        }

        let mut discovered = Vec::new();
        for ((e1, e2), intervals) in &pair_counts {
            if intervals.len() < 2 {
                continue; // Need at least 2 occurrences.
            }
            let avg_interval = intervals.iter().sum::<u64>() / intervals.len() as u64;
            let confidence =
                intervals.len() as f32 / (events.len().saturating_sub(1).max(1)) as f32;

            discovered.push(TemporalPattern {
                events: vec![e1.clone(), e2.clone()],
                avg_interval_ms: avg_interval,
                confidence,
                occurrences: intervals.len() as u32,
            });
        }

        // Merge with existing temporal patterns and cap.
        for new_pat in &discovered {
            let existing = self
                .temporal_patterns
                .iter_mut()
                .find(|p| p.events == new_pat.events);
            if let Some(existing) = existing {
                existing.occurrences = existing.occurrences.saturating_add(new_pat.occurrences);
                existing.confidence = (existing.confidence + new_pat.confidence) / 2.0;
                existing.avg_interval_ms = (existing.avg_interval_ms + new_pat.avg_interval_ms) / 2;
            } else if self.temporal_patterns.len() < self.max_temporal_patterns {
                self.temporal_patterns.push(new_pat.clone());
            }
        }

        discovered
    }

    /// Get the strongest patterns (top N by absolute strength).
    #[instrument(skip(self))]
    pub fn strongest_patterns(&self, n: usize) -> Vec<&ActionPattern> {
        let mut sorted: Vec<&ActionPattern> = self.action_patterns.iter().collect();
        sorted.sort_by(|a, b| {
            b.strength
                .abs()
                .partial_cmp(&a.strength.abs())
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        sorted.truncate(n);
        sorted
    }

    /// Prune weak/old patterns to stay within bounds.
    #[instrument(skip(self))]
    pub fn prune(&mut self, now_ms: u64) {
        // Remove patterns with near-zero strength that are old.
        self.action_patterns.retain(|p| {
            let is_weak = p.strength.abs() < PRUNE_STRENGTH_THRESHOLD;
            let is_old = now_ms.saturating_sub(p.last_seen_ms) > PRUNE_AGE_MS;
            !(is_weak && is_old)
        });

        // If still over capacity, remove weakest.
        while self.action_patterns.len() > self.max_action_patterns {
            if let Some(weakest_idx) = self
                .action_patterns
                .iter()
                .enumerate()
                .min_by(|(_, a), (_, b)| {
                    a.strength
                        .abs()
                        .partial_cmp(&b.strength.abs())
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
                .map(|(i, _)| i)
            {
                self.action_patterns.swap_remove(weakest_idx);
            } else {
                break;
            }
        }

        // Prune temporal patterns with very low confidence.
        self.temporal_patterns
            .retain(|p| p.confidence > 0.01 || p.occurrences > 1);
        self.temporal_patterns.truncate(self.max_temporal_patterns);
    }

    /// Export patterns as serializable data for persistence.
    #[instrument(skip(self))]
    pub fn export(&self) -> Result<Vec<u8>, AuraError> {
        let data = ExportData {
            action_patterns: self.action_patterns.clone(),
            temporal_patterns: self.temporal_patterns.clone(),
        };
        bincode::serde::encode_to_vec(&data, bincode::config::standard()).map_err(|e| {
            AuraError::Memory(MemError::SerializationFailed(format!(
                "pattern export: {e}"
            )))
        })
    }

    /// Import patterns from serialized data.
    #[instrument(skip(data))]
    pub fn import(data: &[u8]) -> Result<Self, AuraError> {
        let (exported, _): (ExportData, _) =
            bincode::serde::decode_from_slice(data, bincode::config::standard()).map_err(|e| {
                AuraError::Memory(MemError::SerializationFailed(format!(
                    "pattern import: {e}"
                )))
            })?;

        let mut engine = Self::new();
        engine.action_patterns = exported.action_patterns;
        engine.temporal_patterns = exported.temporal_patterns;
        // Enforce bounds.
        engine.action_patterns.truncate(MAX_ACTION_PATTERNS);
        engine.temporal_patterns.truncate(MAX_TEMPORAL_PATTERNS);
        Ok(engine)
    }

    /// Number of action patterns tracked.
    pub fn action_pattern_count(&self) -> usize {
        self.action_patterns.len()
    }

    /// Number of temporal patterns tracked.
    pub fn temporal_pattern_count(&self) -> usize {
        self.temporal_patterns.len()
    }
}

/// Internal serialization wrapper.
#[derive(Serialize, Deserialize)]
struct ExportData {
    action_patterns: Vec<ActionPattern>,
    temporal_patterns: Vec<TemporalPattern>,
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_record_and_find_pattern() {
        let mut engine = PatternEngine::new();
        engine
            .record_outcome("open_app", "home_screen", "app opened", true, 1000)
            .expect("record");

        let found = engine.find_patterns("open_app", "home_screen");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].action, "open_app");
        assert_eq!(found[0].outcome, "app opened");
        assert_eq!(found[0].success_count, 1);
    }

    #[test]
    fn test_hebbian_strengthening() {
        let mut engine = PatternEngine::new();
        engine
            .record_outcome("tap", "settings", "opened", true, 1000)
            .expect("record");

        let initial = engine.find_patterns("tap", "settings")[0].strength;

        // Apply 5 more successes.
        for _ in 0..5 {
            engine.hebbian_update("tap", "settings", true);
        }

        let after = engine.find_patterns("tap", "settings")[0].strength;
        assert!(
            after > initial,
            "strength should increase: {initial} -> {after}"
        );
        assert!(after <= 1.0, "strength must stay <= 1.0");
    }

    #[test]
    fn test_hebbian_weakening() {
        let mut engine = PatternEngine::new();
        engine
            .record_outcome("scroll", "feed", "loaded", true, 1000)
            .expect("record");

        let initial = engine.find_patterns("scroll", "feed")[0].strength;

        // Apply failures — should weaken faster than success strengthens.
        for _ in 0..3 {
            engine.hebbian_update("scroll", "feed", false);
        }

        let after = engine.find_patterns("scroll", "feed")[0].strength;
        assert!(
            after < initial,
            "strength should decrease: {initial} -> {after}"
        );
    }

    #[test]
    fn test_predict_outcome() {
        let mut engine = PatternEngine::new();
        // Record several successful outcomes.
        for t in 0..5 {
            engine
                .record_outcome("launch", "browser", "page loaded", true, t * 1000)
                .expect("record");
        }

        let prediction = engine.predict_outcome("launch", "browser");
        assert!(prediction.is_some());
        let (outcome, confidence) = prediction.expect("prediction");
        assert_eq!(outcome, "page loaded");
        assert!(confidence > 0.0);
    }

    #[test]
    fn test_prune_weak_patterns() {
        let mut engine = PatternEngine::new();
        // Create a weak pattern that's old.
        engine
            .record_outcome("weak_action", "ctx", "result", true, 1000)
            .expect("record");
        // Weaken it.
        engine.hebbian_update("weak_action", "ctx", false);
        engine.hebbian_update("weak_action", "ctx", false); // strength now ≈ -0.15

        // Set it to be very weak.
        if let Some(p) = engine
            .action_patterns
            .iter_mut()
            .find(|p| p.action == "weak_action")
        {
            p.strength = 0.01; // Below threshold.
        }

        let now = 1000 + PRUNE_AGE_MS + 1;
        engine.prune(now);
        assert!(engine.find_patterns("weak_action", "ctx").is_empty());
    }

    #[test]
    fn test_bounded_capacity() {
        let mut engine = PatternEngine::new();
        engine.max_action_patterns = 5; // Small cap for testing.

        for i in 0..10 {
            engine
                .record_outcome(
                    &format!("action_{i}"),
                    "ctx",
                    "result",
                    true,
                    i as u64 * 1000,
                )
                .expect("record");
        }

        assert!(
            engine.action_pattern_count() <= 5,
            "should not exceed capacity: {}",
            engine.action_pattern_count()
        );
    }

    #[test]
    fn test_temporal_pattern_discovery() {
        let mut engine = PatternEngine::new();

        // Simulate a repeated pattern: unlock → open_app, multiple times.
        for i in 0..6 {
            let base = i * 10_000;
            engine.record_event("unlock", base);
            engine.record_event("open_app", base + 2000);
        }

        let discovered = engine.discover_temporal_patterns();
        let unlock_app = discovered
            .iter()
            .find(|p| p.events == vec!["unlock", "open_app"]);
        assert!(
            unlock_app.is_some(),
            "should discover unlock→open_app pattern"
        );
    }

    #[test]
    fn test_export_import_roundtrip() {
        let mut engine = PatternEngine::new();
        engine
            .record_outcome("test", "ctx", "ok", true, 1000)
            .expect("record");
        engine.record_event("event_a", 2000);
        engine.record_event("event_b", 3000);
        let _ = engine.discover_temporal_patterns();

        let bytes = engine.export().expect("export");
        let restored = PatternEngine::import(&bytes).expect("import");

        assert_eq!(
            restored.action_pattern_count(),
            engine.action_pattern_count()
        );
        assert_eq!(
            restored.temporal_pattern_count(),
            engine.temporal_pattern_count()
        );
    }
}
