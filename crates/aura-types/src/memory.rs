use serde::{Deserialize, Serialize};

use crate::events::EventSource;
use crate::ipc::MemoryTier;

/// A slot in working memory — short-lived, high-priority data.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkingSlot {
    pub content: String,
    pub source: EventSource,
    pub importance: f32,
    pub timestamp_ms: u64,
    /// Time-to-live in milliseconds from timestamp.
    pub ttl_ms: u64,
}

impl WorkingSlot {
    /// Check if this slot has expired relative to the given current time.
    #[must_use]
    pub fn is_expired(&self, now_ms: u64) -> bool {
        now_ms > self.timestamp_ms + self.ttl_ms
    }
}

/// An episodic memory — a specific event/experience with emotional context.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Episode {
    pub id: u64,
    pub content: String,
    pub emotional_valence: f32,
    pub importance: f32,
    /// Bounded: max MAX_EPISODE_CONTEXT_TAGS items enforced at collection site.
    pub context_tags: Vec<String>,
    pub timestamp_ms: u64,
    pub access_count: u32,
    pub last_access_ms: u64,
    /// Bounded at runtime to the model's fixed embedding dimension — enforced by the embedding engine.
    pub embedding: Option<Vec<f32>>,
}

/// Max context tags on a single [`Episode`].
pub const MAX_EPISODE_CONTEXT_TAGS: usize = 16;

/// A semantic memory entry — distilled knowledge or concept.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SemanticEntry {
    pub id: u64,
    pub concept: String,
    pub knowledge: String,
    pub confidence: f32,
    /// Bounded: max MAX_SEMANTIC_SOURCE_EPISODES items enforced at collection site.
    pub source_episodes: Vec<u64>,
    pub created_ms: u64,
    pub last_reinforced_ms: u64,
    pub access_count: u32,
}

/// Max source episode IDs on a single [`SemanticEntry`].
pub const MAX_SEMANTIC_SOURCE_EPISODES: usize = 32;

/// Compressed archive blob for long-term cold storage.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArchiveBlob {
    pub id: u64,
    /// Bounded at runtime to MAX_ARCHIVE_BLOB_BYTES — enforced by the archive writer.
    pub compressed_data: Vec<u8>,
    pub original_size: u32,
    pub importance: f32,
    pub period_start_ms: u64,
    pub period_end_ms: u64,
}

/// Query parameters for memory retrieval.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryQuery {
    pub query_text: String,
    pub max_results: usize,
    pub min_relevance: f32,
    /// Bounded: max MAX_MEMORY_QUERY_TIERS items (fixed — only 4 tiers exist).
    pub tiers: Vec<MemoryTier>,
    pub time_range: Option<(u64, u64)>,
}

/// Max tiers in a [`MemoryQuery`] (fixed — exactly 4 tiers exist in [`MemoryTier`]).
pub const MAX_MEMORY_QUERY_TIERS: usize = 4;

impl Default for MemoryQuery {
    fn default() -> Self {
        Self {
            query_text: String::new(),
            max_results: 10,
            min_relevance: 0.3,
            tiers: vec![
                MemoryTier::Working,
                MemoryTier::Episodic,
                MemoryTier::Semantic,
                MemoryTier::Archive,
            ],
            time_range: None,
        }
    }
}

/// A single memory retrieval result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryResult {
    pub content: String,
    pub tier: MemoryTier,
    pub relevance: f32,
    pub importance: f32,
    pub timestamp_ms: u64,
    pub source_id: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_working_slot_expiry() {
        let slot = WorkingSlot {
            content: "test data".to_string(),
            source: EventSource::Notification,
            importance: 0.8,
            timestamp_ms: 1000,
            ttl_ms: 5000,
        };
        assert!(!slot.is_expired(3000)); // 3000 < 1000 + 5000
        assert!(!slot.is_expired(6000)); // 6000 == 1000 + 5000
        assert!(slot.is_expired(6001)); // 6001 > 1000 + 5000
    }

    #[test]
    fn test_memory_query_defaults() {
        let query = MemoryQuery::default();
        assert_eq!(query.max_results, 10);
        assert!((query.min_relevance - 0.3).abs() < f32::EPSILON);
        assert_eq!(query.tiers.len(), 4);
        assert!(query.time_range.is_none());
    }

    #[test]
    fn test_episode_creation() {
        let episode = Episode {
            id: 42,
            content: "User asked about weather".to_string(),
            emotional_valence: 0.3,
            importance: 0.5,
            context_tags: vec!["weather".to_string(), "morning".to_string()],
            timestamp_ms: 1_700_000_000_000,
            access_count: 0,
            last_access_ms: 1_700_000_000_000,
            embedding: None,
        };
        assert_eq!(episode.id, 42);
        assert_eq!(episode.context_tags.len(), 2);
        assert!(episode.embedding.is_none());
    }

    #[test]
    fn test_archive_blob_creation() {
        let blob = ArchiveBlob {
            id: 1,
            compressed_data: vec![0x1F, 0x8B, 0x08],
            original_size: 4096,
            importance: 0.2,
            period_start_ms: 1_600_000_000_000,
            period_end_ms: 1_700_000_000_000,
        };
        assert_eq!(blob.compressed_data.len(), 3);
        assert!(blob.period_end_ms > blob.period_start_ms);
    }
}
