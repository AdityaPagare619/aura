# Agent 11 — Screen Semantic Analysis, Caching, and L7 LLM Fallback

**Status**: Complete  
**Date**: 2026-03-05  

## Scope

Implement the two missing screen analysis modules (`semantic.rs`, `cache.rs`), fix the L7 LLM fallback in `selector.rs`, and fix the misleading confidence values in `verifier.rs`'s `TextAppears` handler.

## Files Created

| File | Lines | Tests | Description |
|------|-------|-------|-------------|
| `screen/semantic.rs` | 2103 | 28 | Semantic graph: element classification, edge inference, pattern/landmark detection, state inference, LLM summary |
| `screen/cache.rs` | 1134 | 22 | LRU screen cache with TTL, memory bounds, QuickDiff, ETG prediction, injectable test clock |

## Files Modified

| File | Change | Tests Added |
|------|--------|-------------|
| `screen/mod.rs` | Added `pub mod semantic; pub mod cache;` and re-exports | — |
| `screen/selector.rs` | L7 fix: `resolve_by_description` now uses `score_nodes_recursive` for heuristic word-overlap matching | 7 |
| `screen/verifier.rs` | TextAppears fix: confidence 0.4→0.25 (wrong text changed), 0.3→0.2 (screen changed only) | 6 |

**Total new tests**: 63 (28 + 22 + 7 + 6)

## Design Decisions

### semantic.rs
- **Edge types**: Contains, Follows, Labels, Controls, GroupsWith — inferred from spatial relationships and heuristics (label proximity, toggle adjacency)
- **Patterns detected**: LoginForm, ListView, Dialog, Navigation, SettingsPage, SearchBar
- **Landmarks**: back button, nav bar, search, FAB, tab bar — identified by resource ID, content description, and class name patterns
- **State inference**: loading (spinners/progress), error (error text), empty (empty state indicators), input-focused
- **LLM summary**: generates structured prompt for downstream LLM consumption

### cache.rs
- **LRU eviction** with configurable max entries (default 50) and max memory (default 10 MB)
- **TTL**: 2-second default, entries expire and are lazily cleaned
- **QuickDiff**: compares hash of current vs. last screen, provides `changed: bool` and `diff_summary`
- **ETG predictions**: given current screen hash + ETG edges, prefetches likely next screens
- **Injectable clock**: `ClockFn` trait allows deterministic time in tests

### selector.rs L7 fix
- `resolve_by_description` was returning `None` unconditionally
- Now uses `score_nodes_recursive` which scores each node by counting matching words from the description (excluding stopwords)
- Min score: 2 for multi-word descriptions, 1 for single-word
- Ties broken by preferring clickable nodes

### verifier.rs TextAppears fix
- Old confidence when text changed but expected text missing: 0.4 (too high — suggested partial success)
- New: 0.25 — clearly below the word-match tier (0.60)
- Old confidence when screen changed but no text match: 0.3
- New: 0.2 — just above "nothing changed" (0.1)
- Full hierarchy: exact(0.95) > partial(0.80) > word(0.60) > wrong-text(0.25) > screen-only(0.2) > nothing(0.1)

## Known Issues

- `cargo test -p aura-daemon` cannot run due to 5 pre-existing compile errors in other modules (`daemon_core/main_loop.rs`, `arc/proactive/welcome.rs`, `pipeline/slots.rs`). Our screen module code compiles cleanly — verified via `cargo check` showing no errors in `screen/` files.
