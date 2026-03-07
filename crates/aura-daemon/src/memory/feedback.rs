//! Error→learning feedback loop for AURA's memory system.
//!
//! Captures errors and their resolutions, turning failures into learning
//! opportunities. When AURA encounters a similar error in the future,
//! previously successful resolutions are suggested.

use std::collections::VecDeque;

use aura_types::errors::{AuraError, MemError};
use serde::{Deserialize, Serialize};
use tracing::instrument;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const MAX_PENDING: usize = 128;
const MAX_RESOLUTIONS: usize = 512;
/// Minimum success rate to suggest a resolution.
const MIN_SUGGEST_RATE: f32 = 0.2;

// ---------------------------------------------------------------------------
// Data structures
// ---------------------------------------------------------------------------

/// A captured error event.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorEvent {
    pub id: u64,
    pub error_type: String,
    pub error_message: String,
    pub context: String,
    pub timestamp_ms: u64,
    pub resolved: bool,
    pub resolution: Option<String>,
    pub resolution_ms: Option<u64>,
}

/// A learned error→resolution pair.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorResolution {
    pub error_type: String,
    pub error_pattern: String,
    pub resolution_strategy: String,
    pub success_rate: f32,
    pub times_applied: u32,
    pub times_succeeded: u32,
    pub last_applied_ms: u64,
}

// ---------------------------------------------------------------------------
// FeedbackLoop
// ---------------------------------------------------------------------------

/// The feedback loop engine.
pub struct FeedbackLoop {
    pending_errors: VecDeque<ErrorEvent>,
    resolutions: Vec<ErrorResolution>,
    next_id: u64,
    max_pending: usize,
    max_resolutions: usize,
    /// Track total errors ever recorded (for effectiveness).
    total_recorded: u64,
    total_resolved: u64,
}

impl Default for FeedbackLoop {
    fn default() -> Self {
        Self::new()
    }
}

impl FeedbackLoop {
    /// Create a new feedback loop engine.
    pub fn new() -> Self {
        Self {
            pending_errors: VecDeque::new(),
            resolutions: Vec::new(),
            next_id: 1,
            max_pending: MAX_PENDING,
            max_resolutions: MAX_RESOLUTIONS,
            total_recorded: 0,
            total_resolved: 0,
        }
    }

    /// Record a new error event. Returns its ID for later resolution.
    #[instrument(skip(self))]
    pub fn record_error(
        &mut self,
        error_type: &str,
        error_message: &str,
        context: &str,
        timestamp_ms: u64,
    ) -> u64 {
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        self.total_recorded = self.total_recorded.saturating_add(1);

        // Evict oldest if at capacity.
        if self.pending_errors.len() >= self.max_pending {
            self.pending_errors.pop_front();
        }

        self.pending_errors.push_back(ErrorEvent {
            id,
            error_type: error_type.to_string(),
            error_message: error_message.to_string(),
            context: context.to_string(),
            timestamp_ms,
            resolved: false,
            resolution: None,
            resolution_ms: None,
        });

        id
    }

    /// Mark an error as resolved and learn from it.
    #[instrument(skip(self))]
    pub fn resolve_error(
        &mut self,
        error_id: u64,
        resolution: &str,
        success: bool,
        timestamp_ms: u64,
    ) -> Result<(), AuraError> {
        // Find the pending error.
        let event = self
            .pending_errors
            .iter_mut()
            .find(|e| e.id == error_id)
            .ok_or_else(|| {
                AuraError::Memory(MemError::NotFound(format!("error event {error_id}")))
            })?;

        event.resolved = true;
        event.resolution = Some(resolution.to_string());
        event.resolution_ms = Some(timestamp_ms);
        self.total_resolved = self.total_resolved.saturating_add(1);

        let error_type = event.error_type.clone();
        let error_pattern = simplify_error_pattern(&event.error_message);

        // Look for existing resolution.
        let existing = self
            .resolutions
            .iter_mut()
            .find(|r| r.error_type == error_type && r.resolution_strategy == resolution);

        if let Some(res) = existing {
            res.times_applied = res.times_applied.saturating_add(1);
            if success {
                res.times_succeeded = res.times_succeeded.saturating_add(1);
            }
            res.success_rate = if res.times_applied > 0 {
                res.times_succeeded as f32 / res.times_applied as f32
            } else {
                0.0
            };
            res.last_applied_ms = timestamp_ms;
        } else {
            // Check capacity.
            if self.resolutions.len() >= self.max_resolutions {
                // Evict lowest success rate resolution.
                if let Some(worst_idx) = self
                    .resolutions
                    .iter()
                    .enumerate()
                    .min_by(|(_, a), (_, b)| {
                        a.success_rate
                            .partial_cmp(&b.success_rate)
                            .unwrap_or(std::cmp::Ordering::Equal)
                    })
                    .map(|(i, _)| i)
                {
                    self.resolutions.swap_remove(worst_idx);
                }
            }

            self.resolutions.push(ErrorResolution {
                error_type,
                error_pattern,
                resolution_strategy: resolution.to_string(),
                success_rate: if success { 1.0 } else { 0.0 },
                times_applied: 1,
                times_succeeded: if success { 1 } else { 0 },
                last_applied_ms: timestamp_ms,
            });
        }

        Ok(())
    }

    /// Look up known resolutions for a given error type/message.
    /// Returns resolutions sorted by success_rate descending.
    #[instrument(skip(self))]
    pub fn suggest_resolutions(
        &self,
        error_type: &str,
        error_message: &str,
    ) -> Vec<&ErrorResolution> {
        let pattern = simplify_error_pattern(error_message);

        let mut matches: Vec<&ErrorResolution> = self
            .resolutions
            .iter()
            .filter(|r| {
                r.error_type == error_type
                    && r.success_rate >= MIN_SUGGEST_RATE
                    && (r.error_pattern == pattern
                        || r.error_pattern.contains(&pattern)
                        || pattern.contains(&r.error_pattern))
            })
            .collect();

        matches.sort_by(|a, b| {
            b.success_rate
                .partial_cmp(&a.success_rate)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        matches
    }

    /// Get unresolved errors (pending).
    pub fn pending_errors(&self) -> &VecDeque<ErrorEvent> {
        &self.pending_errors
    }

    /// Prune old unresolved errors and low-success resolutions.
    #[instrument(skip(self))]
    pub fn prune(&mut self, now_ms: u64, max_age_ms: u64) {
        // Remove old unresolved pending errors.
        self.pending_errors.retain(|e| {
            let age = now_ms.saturating_sub(e.timestamp_ms);
            e.resolved || age <= max_age_ms
        });

        // Remove resolutions proven unreliable.
        self.resolutions.retain(|r| {
            // Keep if not enough data yet, or success rate is acceptable.
            r.times_applied <= 5 || r.success_rate >= 0.1
        });
    }

    /// Export for persistence.
    #[instrument(skip(self))]
    pub fn export(&self) -> Result<Vec<u8>, AuraError> {
        let data = FeedbackExport {
            resolutions: self.resolutions.clone(),
            next_id: self.next_id,
            total_recorded: self.total_recorded,
            total_resolved: self.total_resolved,
        };
        bincode::serde::encode_to_vec(&data, bincode::config::standard()).map_err(|e| {
            AuraError::Memory(MemError::SerializationFailed(format!(
                "feedback export: {e}"
            )))
        })
    }

    /// Import from persisted data.
    #[instrument(skip(data))]
    pub fn import(data: &[u8]) -> Result<Self, AuraError> {
        let (exported, _): (FeedbackExport, _) =
            bincode::serde::decode_from_slice(data, bincode::config::standard()).map_err(|e| {
                AuraError::Memory(MemError::SerializationFailed(format!(
                    "feedback import: {e}"
                )))
            })?;

        let mut loop_inst = Self::new();
        loop_inst.resolutions = exported.resolutions;
        loop_inst.next_id = exported.next_id;
        loop_inst.total_recorded = exported.total_recorded;
        loop_inst.total_resolved = exported.total_resolved;
        loop_inst.resolutions.truncate(MAX_RESOLUTIONS);
        Ok(loop_inst)
    }

    /// Number of learned resolutions.
    pub fn resolution_count(&self) -> usize {
        self.resolutions.len()
    }

    /// Number of pending (unresolved) errors.
    pub fn pending_count(&self) -> usize {
        self.pending_errors.iter().filter(|e| !e.resolved).count()
    }

    /// Overall learning effectiveness (fraction of errors that got resolved).
    pub fn effectiveness(&self) -> f32 {
        if self.total_recorded == 0 {
            return 0.0;
        }
        self.total_resolved as f32 / self.total_recorded as f32
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Simplify an error message into a generic pattern for matching.
/// Strips numbers, paths, and specific identifiers.
fn simplify_error_pattern(message: &str) -> String {
    let mut result = String::with_capacity(message.len());
    let mut chars = message.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch.is_ascii_digit() {
            // Replace runs of digits with '*'.
            result.push('*');
            while chars.peek().map_or(false, |c| c.is_ascii_digit()) {
                chars.next();
            }
        } else if ch == '\'' || ch == '"' {
            // Replace quoted strings with '*'.
            result.push('*');
            let quote = ch;
            while let Some(inner) = chars.next() {
                if inner == quote {
                    break;
                }
            }
        } else {
            result.push(ch);
        }
    }

    result
}

/// Serialization wrapper for persistence (we don't persist pending errors).
#[derive(Serialize, Deserialize)]
struct FeedbackExport {
    resolutions: Vec<ErrorResolution>,
    next_id: u64,
    total_recorded: u64,
    total_resolved: u64,
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_record_and_resolve_error() {
        let mut fb = FeedbackLoop::new();

        let id = fb.record_error(
            "TargetNotFound",
            "element not found: btn_ok",
            "tapping button",
            1000,
        );
        assert_eq!(fb.pending_count(), 1);

        fb.resolve_error(id, "retry with broader selector", true, 2000)
            .expect("resolve");

        assert_eq!(fb.resolution_count(), 1);
        assert!(fb.effectiveness() > 0.0);
    }

    #[test]
    fn test_suggest_resolutions() {
        let mut fb = FeedbackLoop::new();

        // Record and resolve a few similar errors.
        for i in 0..3 {
            let id = fb.record_error(
                "Timeout",
                &format!("step {i} timed out on action 'Tap'"),
                "executing plan",
                i * 1000,
            );
            fb.resolve_error(id, "increase timeout and retry", true, i * 1000 + 500)
                .expect("resolve");
        }

        let suggestions = fb.suggest_resolutions("Timeout", "step 5 timed out on action 'Tap'");
        assert!(!suggestions.is_empty());
        assert!(suggestions[0].success_rate > 0.5);
    }

    #[test]
    fn test_error_pattern_simplification() {
        let pattern =
            simplify_error_pattern("element not found: selector 'resource_id:btn_settings'");
        assert!(
            pattern.contains("*"),
            "should replace quoted string: {pattern}"
        );
        assert!(
            !pattern.contains("btn_settings"),
            "should strip specific IDs"
        );

        let pattern2 = simplify_error_pattern("step 3 timed out after 5000ms");
        assert!(pattern2.contains("*"), "should replace numbers: {pattern2}");
    }

    #[test]
    fn test_prune_old_errors() {
        let mut fb = FeedbackLoop::new();
        fb.record_error("OldError", "old problem", "context", 1000);
        fb.record_error("NewError", "new problem", "context", 100_000);

        // Prune with max_age of 50000ms from now=100000.
        fb.prune(100_000, 50_000);

        // Old unresolved error should be pruned.
        let pending: Vec<&ErrorEvent> =
            fb.pending_errors().iter().filter(|e| !e.resolved).collect();
        assert!(
            pending.iter().all(|e| e.error_type != "OldError"),
            "old error should be pruned"
        );
    }

    #[test]
    fn test_effectiveness_tracking() {
        let mut fb = FeedbackLoop::new();
        assert_eq!(fb.effectiveness(), 0.0);

        let id1 = fb.record_error("E1", "msg", "ctx", 1000);
        let _id2 = fb.record_error("E2", "msg", "ctx", 2000);
        fb.resolve_error(id1, "fix", true, 1500).expect("resolve");

        // 1 resolved out of 2 recorded.
        assert!((fb.effectiveness() - 0.5).abs() < f32::EPSILON);
    }

    #[test]
    fn test_bounded_capacity() {
        let mut fb = FeedbackLoop::new();
        fb.max_pending = 3;

        for i in 0..10 {
            fb.record_error("Err", "msg", "ctx", i * 1000);
        }

        assert!(
            fb.pending_errors().len() <= 3,
            "should not exceed capacity: {}",
            fb.pending_errors().len()
        );
    }

    #[test]
    fn test_export_import_roundtrip() {
        let mut fb = FeedbackLoop::new();
        let id = fb.record_error("TestErr", "message", "ctx", 1000);
        fb.resolve_error(id, "test fix", true, 2000)
            .expect("resolve");

        let bytes = fb.export().expect("export");
        let restored = FeedbackLoop::import(&bytes).expect("import");

        assert_eq!(restored.resolution_count(), fb.resolution_count());
        assert_eq!(restored.next_id, fb.next_id);
    }

    #[test]
    fn test_resolve_nonexistent_error() {
        let mut fb = FeedbackLoop::new();
        let result = fb.resolve_error(999, "fix", true, 1000);
        assert!(result.is_err());
    }
}
