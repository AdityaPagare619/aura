# TEAM 4: Memory Fixes Research Plan

## Executive Summary

After analyzing the codebase, I found:

1. **Episodic Memory**: Already has HNSW implemented for main query path! BUT `find_similar_sync()` at line 773-814 does O(n) linear scan - this is the bug to fix.

2. **Archive Memory**: Already has real LZ4/ZSTD compression implemented! The TODO comment about RLE (line 61-62) is outdated - the actual code uses real compression.

## Analysis Details

### Episodic Memory (`episodic.rs`)

**Already Fixed:**
- Main `query()` function (lines 288-307) uses HNSW index
- `query_episodes_hnsw()` (lines 574-683) uses HNSW for fast candidate retrieval
- HNSW is properly built on startup via `build_hnsw_index()`

**O(n) Bug to Fix:**
- `find_similar_sync()` (lines 773-814) scans ALL episodes with embeddings
- This function is called by `find_similar()` API
- Need to add HNSW index usage here

**Secondary O(n) Issue:**
- `query_episodes_sync()` (lines 686-756) - legacy O(n) function
- Not used in main query path, but should be removed or converted

### Archive Memory (`archive.rs`)

**Already Implemented:**
- Real LZ4 compression via `lz4::block::compress()` (line 88)
- Real ZSTD compression via `zstd::encode_all()` (line 105)
- Wire format with magic header (line 65)
- Proper decompression (lines 117-169)

**Outdated TODO:**
- Line 61-62 mentions "Replace RLE body" but this was already done!
- The TODO comment should be removed

## Implementation Plan

### Task 1: Fix O(n) in `find_similar_sync()`

**Current Code (O(n)):**
```rust
fn find_similar_sync(conn: &Connection, content: &str, ...) {
    // Scans ALL episodes - O(n)
    let mut stmt = conn.prepare(
        "SELECT ... FROM episodes WHERE embedding IS NOT NULL"
    );
    // Then iterates all rows computing cosine similarity
}
```

**Fix:** Use HNSW index like `query_episodes_hnsw()` does:
1. Get query embedding
2. Search HNSW for candidates
3. Filter by min_similarity
4. Return top-k results

### Task 2: Verify Archive Compression

The compression is already implemented correctly. Just need to:
1. Remove outdated TODO comment
2. Add 20+ comprehensive tests

### Task 3: Performance Benchmarking

Target: 10,000 memories → <50ms query
- Current HNSW query should achieve this
- Need to add benchmark tests to verify

## Test Plan

Write 20+ tests covering:
1. Episodic HNSW query performance at scale
2. Archive compression ratio metrics
3. Archive decompression correctness
4. Edge cases (empty, large data, compression boundaries)

## Success Criteria

- [ ] `cargo check` → 0 errors
- [ ] `cargo test` → 0 new failures
- [ ] 10K memories query < 50ms (verified via benchmark)
- [ ] All 20+ new tests pass
