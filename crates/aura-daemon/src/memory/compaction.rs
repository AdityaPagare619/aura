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
    pub fn compact(&self, slots_to_compact: &[&WorkingSlot], now_ms: u64) -> WorkingSlot {
        debug!("Compacting {} slots", slots_to_compact.len());
        
        let mut combined_content = String::new();
        let mut min_ts = u64::MAX;
        let mut max_importance = 0.0_f32;

        for (i, slot) in slots_to_compact.iter().enumerate() {
            // Extractive summarization heuristic: keep first and last few events.
            if i < 3 || i >= slots_to_compact.len().saturating_sub(2) {
                combined_content.push_str(&slot.content);
                combined_content.push_str("; ");
            } else if i == 3 {
                combined_content.push_str("... ");
            }
            min_ts = min_ts.min(slot.timestamp_ms);
            max_importance = max_importance.max(slot.importance);
        }

        let summary = format!("[Compacted Session ({} events)] {}", slots_to_compact.len(), combined_content);

        WorkingSlot {
            content: summary,
            source: EventSource::Internal,
            importance: max_importance.max(0.5),
            timestamp_ms: min_ts, 
            ttl_ms: 10 * 60 * 60 * 1000, // 10 hours for a summary
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
        let summary = compactor.compact(&slots, 200);
        assert!(summary.content.contains("Compacted Session"));
        assert!(summary.content.contains("A; B; C;"));
        assert_eq!(summary.importance, 0.8);
        assert_eq!(summary.timestamp_ms, 100);
    }
}
