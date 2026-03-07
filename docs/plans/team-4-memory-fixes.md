# TEAM 4: Memory Fixes - Research & Implementation Plan

## Problem Analysis

### 1. Episodic O(n) Scan Bug
**Location**: `crates/aura-daemon/src/memory/episodic.rs:451-521`

The `query_episodes_sync` function performs a full table scan:
```rust
let mut stmt = conn.prepare("SELECT ... FROM episodes");  // No WHERE clause
let rows = stmt.query_map([], row_to_episode)?;          // Loads ALL episodes
for row in rows {
    // Computes similarity for EVERY episode - O(n)
}
```

**Impact**: With 10,000 memories, query time becomes O(n) = slow.

**Solution**: Add HNSW index (already proven in semantic.rs).

### 2. Fake RLE Compression
**Location**: `crates/aura-daemon/src/memory/archive.rs:56-65`

Current implementation uses simple RLE that doesn't compress well. The code explicitly has:
```rust
// TODO: Replace RLE body with zstd::encode_all / zstd::decode_all once the
//       `zstd` crate is added to Cargo.toml.
```

**Solution**: Implement real LZ4 (fast) and ZSTD (high ratio) compression.

---

## Implementation Plan

### Phase 1: Add Dependencies
- Add `lz4` and `zstd` to workspace Cargo.toml
- Add to aura-daemon Cargo.toml

### Phase 2: Episodic HNSW Fix
1. Create `HnswState` struct (mirror semantic.rs:63-119)
2. Add `hnsw` field to `EpisodicMemory`
3. Create `build_hnsw_from_db` function
4. Modify `store_episode_sync` to index embeddings
5. Modify `query_episodes_sync` to use HNSW first, then full match
6. Query strategy:
   - First: time window filter (SQL WHERE timestamp_ms BETWEEN)
   - Second: HNSW search (get candidates)
   - Third: full semantic match on candidates

### Phase 3: Real Archive Compression
1. Add LZ4 and ZSTD compression functions
2. Wire format: magic + original_len + compressed_data
3. Implement `compress_lz4`, `compress_zstd`, `decompress`
4. Add compression ratio metrics tracking
5. Support both algorithms with choice based on data type

### Phase 4: Testing (20+ tests)
1. **Query Performance Tests** (10K+ items → <50ms)
2. **Compression Ratio Tests** (>3x for repetitive data)
3. **Decompression Correctness** (roundtrip verification)
4. **HNSW Integration Tests**
5. **Archive Retrieval Tests**

---

## Key Code Patterns to Follow

From semantic.rs (working implementation):
- `HnswState` with bidirectional node↔sqlite ID mapping
- `Arc<Mutex<HnswState>>` for thread-safe access
- `build_hnsw_from_db` at startup
- `query_semantic_rrf` as template for episodic query

---

## Success Criteria
- [ ] `cargo check` → 0 errors
- [ ] `cargo test -p aura-daemon --lib` → no new failures
- [ ] 10K memories query < 50ms
- [ ] Compression ratio > 3x on repetitive text
- [ ] All compression roundtrips pass
