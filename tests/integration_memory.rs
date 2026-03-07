//! Integration tests for the AURA v4 Memory subsystem.
//!
//! Tests the 4-tier memory hierarchy: working, episodic, semantic, archive.
//! All tests use in-memory databases (no disk I/O).

use aura_daemon::memory::AuraMemory;
use aura_types::events::EventSource;
use aura_types::ipc::MemoryTier;
use aura_types::memory::MemoryQuery;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn now_ms() -> u64 {
    1_700_000_000_000
}

fn make_memory() -> AuraMemory {
    AuraMemory::new_in_memory().expect("in-memory AuraMemory should initialize")
}

// ---------------------------------------------------------------------------
// Working memory tests
// ---------------------------------------------------------------------------

#[test]
fn test_working_memory_store_and_basic_state() {
    let mut mem = make_memory();
    mem.store_working(
        "User opened WhatsApp".to_string(),
        EventSource::Accessibility,
        0.6,
        now_ms(),
    );
    // No panic = success. Working memory is fire-and-forget.
}

#[test]
fn test_working_memory_multiple_stores() {
    let mut mem = make_memory();
    for i in 0..50 {
        mem.store_working(
            format!("Event number {}", i),
            EventSource::Notification,
            0.5,
            now_ms() + i * 1000,
        );
    }
    // 50 items stored without panic.
}

#[tokio::test]
async fn test_working_memory_query() {
    let mut mem = make_memory();
    mem.store_working(
        "Alice sent a message about dinner".to_string(),
        EventSource::Notification,
        0.8,
        now_ms(),
    );
    mem.store_working(
        "System update available".to_string(),
        EventSource::Internal,
        0.3,
        now_ms() + 1000,
    );

    let query = MemoryQuery {
        query_text: "dinner".to_string(),
        max_results: 10,
        min_relevance: 0.0,
        tiers: vec![MemoryTier::Working],
        time_range: None,
    };

    let results = mem.query(&query, now_ms() + 2000).await.unwrap();
    // At least one result should mention dinner.
    assert!(!results.is_empty());
}

// ---------------------------------------------------------------------------
// Episodic memory tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_episodic_store_and_retrieve() {
    let mem = make_memory();

    let id = mem
        .store_episodic(
            "Had a great conversation with Alice about travel plans".to_string(),
            0.8,  // positive emotional valence
            0.7,  // high importance
            vec!["social".to_string(), "planning".to_string()],
            now_ms(),
        )
        .await
        .unwrap();

    assert!(id > 0, "episodic store should return a positive ID");
}

#[tokio::test]
async fn test_episodic_multiple_entries() {
    let mem = make_memory();

    for i in 0..10 {
        let id = mem
            .store_episodic(
                format!("Episode {} — daily standup", i),
                0.5,
                0.4,
                vec!["work".to_string()],
                now_ms() + i * 60_000,
            )
            .await
            .unwrap();
        assert!(id > 0);
    }
}

#[tokio::test]
async fn test_episodic_query() {
    let mem = make_memory();

    mem.store_episodic(
        "Booked flight tickets to Tokyo".to_string(),
        0.9,
        0.8,
        vec!["travel".to_string()],
        now_ms(),
    )
    .await
    .unwrap();

    mem.store_episodic(
        "Grocery shopping at the supermarket".to_string(),
        0.2,
        0.3,
        vec!["errands".to_string()],
        now_ms() + 1000,
    )
    .await
    .unwrap();

    let query = MemoryQuery {
        query_text: "Tokyo flight".to_string(),
        max_results: 5,
        min_relevance: 0.0,
        tiers: vec![MemoryTier::Episodic],
        time_range: None,
    };

    let results = mem.query(&query, now_ms() + 5000).await.unwrap();
    // Should return at least the Tokyo entry.
    assert!(!results.is_empty());
}

// ---------------------------------------------------------------------------
// Semantic memory tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_semantic_store_and_retrieve() {
    let mem = make_memory();

    let id = mem
        .store_semantic(
            "Rust ownership".to_string(),
            "Rust uses an ownership system with borrowing and lifetimes.".to_string(),
            0.95,
            vec![],
            now_ms(),
        )
        .await
        .unwrap();

    assert!(id > 0, "semantic store should return a positive ID");
}

#[tokio::test]
async fn test_semantic_query() {
    let mem = make_memory();

    mem.store_semantic(
        "Tokyo timezone".to_string(),
        "Tokyo is in JST (UTC+9).".to_string(),
        0.9,
        vec![],
        now_ms(),
    )
    .await
    .unwrap();

    mem.store_semantic(
        "Rust traits".to_string(),
        "Traits define shared behavior in Rust.".to_string(),
        0.85,
        vec![],
        now_ms() + 1000,
    )
    .await
    .unwrap();

    let query = MemoryQuery {
        query_text: "timezone".to_string(),
        max_results: 5,
        min_relevance: 0.0,
        tiers: vec![MemoryTier::Semantic],
        time_range: None,
    };

    let results = mem.query(&query, now_ms() + 5000).await.unwrap();
    assert!(!results.is_empty());
}

// ---------------------------------------------------------------------------
// Cross-tier query tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_cross_tier_query() {
    let mut mem = make_memory();

    // Store across tiers
    mem.store_working(
        "Just saw Alice at the cafe".to_string(),
        EventSource::Notification,
        0.6,
        now_ms(),
    );

    mem.store_episodic(
        "Alice and I discussed the project roadmap".to_string(),
        0.7,
        0.8,
        vec!["work".to_string()],
        now_ms() - 86_400_000, // yesterday
    )
    .await
    .unwrap();

    mem.store_semantic(
        "Alice".to_string(),
        "Alice is a close friend and colleague.".to_string(),
        0.9,
        vec![],
        now_ms() - 172_800_000, // two days ago
    )
    .await
    .unwrap();

    let query = MemoryQuery {
        query_text: "Alice".to_string(),
        max_results: 10,
        min_relevance: 0.0,
        tiers: vec![
            MemoryTier::Working,
            MemoryTier::Episodic,
            MemoryTier::Semantic,
        ],
        time_range: None,
    };

    let results = mem.query(&query, now_ms() + 1000).await.unwrap();
    // Should find results from multiple tiers.
    assert!(!results.is_empty());
}

// ---------------------------------------------------------------------------
// Consolidation tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_consolidation_does_not_crash() {
    let mut mem = make_memory();

    // Populate working memory with enough entries to trigger consolidation.
    for i in 0..100 {
        mem.store_working(
            format!("Working memory entry {}", i),
            EventSource::Internal,
            0.5,
            now_ms() + i * 500,
        );
    }

    // Consolidation should not panic or error on in-memory databases.
    let report = mem
        .consolidate(aura_daemon::memory::ConsolidationLevel::Micro, now_ms() + 100_000)
        .await;
    assert!(report.is_ok(), "micro consolidation should succeed");
}

// ---------------------------------------------------------------------------
// Memory usage report tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_memory_usage_report() {
    let mem = make_memory();

    let report = mem.memory_usage().await;
    assert!(report.is_ok(), "memory usage report should succeed");

    let report = report.unwrap();
    assert_eq!(report.working_slot_count, 0);
}

#[tokio::test]
async fn test_memory_usage_after_stores() {
    let mut mem = make_memory();

    mem.store_working(
        "test content".to_string(),
        EventSource::UserCommand,
        0.5,
        now_ms(),
    );

    mem.store_episodic(
        "episode content".to_string(),
        0.5,
        0.5,
        vec![],
        now_ms(),
    )
    .await
    .unwrap();

    let report = mem.memory_usage().await.unwrap();
    assert!(report.working_slot_count >= 1);
    assert!(report.episodic_count >= 1);
}
