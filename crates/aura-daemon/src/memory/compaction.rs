//! Context compaction for long sessions.
//!
//! When a user session runs long, working memory fills up.
//! Instead of simply evicting old memories (which loses immediate context)
//! or relying purely on ranking (which fragments the timeline),
//! the `ContextCompactor` identifies continuous chunks of older 
//! working memories and compresses them into dense summary slots.

use aura_types::events::EventSource;
use aura_types::memory::WorkingSlot;
use tracing::{debug, instrument};

/// Engine for compacting working memory context during long sessions.
#[derive(Debug, Clone)]
pub struct ContextCompactor {
    /// Target compression ratio.
    pub target_ratio: f32,
}

impl ContextCompactor {
    #[must_use]
    pub fn new() -> Self {
        Self { target_ratio: 0.3 }
    }

    /// Identifies slots eligible for compaction.
    /// Returns indices of slots that form a continuous "chunk" of older context.
    pub fn identify_compaction_candidates(
        &self,
        slots: &[(usize, &WorkingSlot)],
        max_recent_to_keep: usize,
    ) -> Vec<usize> {
        let mut temporal: Vec<_> = slots.to_vec();
        temporal.sort_by(|a, b| a.1.timestamp_ms.cmp(&b.1.timestamp_ms));

        let candidates_count = temporal.len().saturating_sub(max_recent_to_keep);
        if candidates_count < 3 {
            return Vec::new();
        }

        temporal.into_iter().take(candidates_count).map(|(idx, _)| idx).collect()
    }

    /// Compacts a list of slots into a single summarized slot.
    #[instrument(skip_all)]
    pub fn compact(&self, slots_to_compact: &[&WorkingSlot], _now_ms: u64) -> WorkingSlot {
        debug!("Compacting {} slots", slots_to_compact.len());

        let mut combined_content = String::new();
        let mut min_ts = u64::MAX;
        let mut max_importance = 0.0_f32;

        for slot in slots_to_compact.iter() {
            // Concatenate all content verbatim — no extractive summarization.
            // The LLM is responsible for understanding and summarizing this content.
            if !combined_content.is_empty() {
                combined_content.push_str("; ");
            }
            combined_content.push_str(&slot.content);
            min_ts = min_ts.min(slot.timestamp_ms);
            max_importance = max_importance.max(slot.importance);
        }

        WorkingSlot {
            // Raw concatenated content only — no labels, no injected strings.
            content: combined_content,
            source: EventSource::Internal,
            importance: max_importance.max(0.5),
            timestamp_ms: min_ts,
            ttl_ms: 10 * 60 * 60 * 1000, // 10 hours for a compacted slot
        }
    }
}

impl Default for ContextCompactor {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_compaction() {
        let compactor = ContextCompactor::new();
        let s1 = WorkingSlot { content: "A".to_string(), source: EventSource::UserCommand, importance: 0.8, timestamp_ms: 100, ttl_ms: 0 };
        let s2 = WorkingSlot { content: "B".to_string(), source: EventSource::UserCommand, importance: 0.2, timestamp_ms: 110, ttl_ms: 0 };
        let s3 = WorkingSlot { content: "C".to_string(), source: EventSource::UserCommand, importance: 0.3, timestamp_ms: 120, ttl_ms: 0 };

        let slots = [&s1, &s2, &s3];
        let result = compactor.compact(&slots, 200);
        // All content concatenated verbatim — no labels, no truncation, no heuristics.
        assert_eq!(result.content, "A; B; C");
        assert_eq!(result.importance, 0.8);
        assert_eq!(result.timestamp_ms, 100);
    }

    #[test]
    fn test_compaction_all_slots_preserved() {
        // Verifies no extractive heuristic truncates beyond the first/last N.
        let compactor = ContextCompactor::new();
        let make_slot = |s: &str| WorkingSlot {
            content: s.to_string(),
            source: EventSource::UserCommand,
            importance: 0.5,
            timestamp_ms: 100,
            ttl_ms: 0,
        };
        let slots_data: Vec<WorkingSlot> = (0..10).map(|i| make_slot(&format!("slot{i}"))).collect();
        let refs: Vec<&WorkingSlot> = slots_data.iter().collect();
        let result = compactor.compact(&refs, 200);
        // Every slot must be present.
        for i in 0..10 {
            assert!(result.content.contains(&format!("slot{i}")), "slot{i} missing from compacted content");
        }
    }
}
