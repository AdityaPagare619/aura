# ADR-005: Accessibility-First UI Interaction with L0-L7 Selector Cascade

**Status:** Accepted  
**Date:** 2026-03-01  
**Deciders:** AURA Core Team

## Context

AURA automates arbitrary Android apps without their cooperation. It has no APIs, SDKs, or source access to target apps. The only universal interface for UI automation on Android is the **AccessibilityService** API, which exposes a tree of UI nodes with attributes like resource IDs, text content, content descriptions, and XPaths.

The challenge: UI element identification is fragile. Apps update frequently, changing:
- Resource IDs (renamed or removed)
- Layout hierarchy (restructured views)
- Text content (localized, A/B tested)
- Screen coordinates (different devices, orientations)

A single targeting strategy will fail when any of these change. We need a **cascade** that tries the most precise method first and falls back through progressively weaker (but broader) strategies.

## Decision

Implement an **8-level selector cascade (L0-L7)** that attempts to resolve UI elements from most precise to most general, stopping at the first successful match.

### Selector Cascade

**Location:** `crates/aura-daemon/src/screen/selector.rs`

```
  Target Element Request
       │
       ▼
  ┌─────────┐  hit
  │ L0: Exact├────────► Element Found ✓
  │ XPath    │
  └────┬─────┘  miss
       ▼
  ┌─────────┐  hit
  │ L1:Struct├────────► Element Found ✓
  │ XPath    │
  └────┬─────┘  miss
       ▼
  ┌─────────┐  hit
  │ L2:ResID ├────────► Element Found ✓
  │ +context │
  └────┬─────┘  miss
       ▼
  ┌─────────┐  hit
  │ L3: Text ├────────► Element Found ✓
  │ +anchor  │
  └────┬─────┘  miss
       ▼
  ┌─────────┐  hit
  │ L4:Desc  ├────────► Element Found ✓
  │ +class   │
  └────┬─────┘  miss
       ▼
  ┌─────────┐  hit
  │ L5:Class ├────────► Element Found ✓
  │ +index   │
  └────┬─────┘  miss
       ▼
  ┌─────────┐  hit
  │ L6:Coord ├────────► Element Found ✓
  │ snap-near│
  └────┬─────┘  miss
       ▼
  ┌─────────┐  hit
  │ L7: LLM  ├────────► Element Found ✓
  │ semantic │
  └────┬─────┘  miss
       ▼
  Resolution Failed ✗
```

### Level Details

| Level | Strategy | Precision | Speed | Resilience | Notes |
|-------|----------|-----------|-------|------------|-------|
| L0 | Exact XPath | Highest | <1ms | Lowest | Full path with all attributes. Breaks on any layout change |
| L1 | Structural XPath | High | <1ms | Low | Volatile attributes stripped (indices, dynamic IDs) |
| L2 | ResourceID + context | High | 1-2ms | Medium | Matches ID within parent context. Survives layout reshuffles |
| L3 | Text + anchor | Medium | 1-3ms | Medium | Finds element by visible text near an anchor element |
| L4 | ContentDesc + class | Medium | 1-3ms | Medium | Accessibility label + widget type. Good for icon buttons |
| L5 | ClassName + index | Low | 1-2ms | Low-Med | Widget type + position among siblings. Fragile to reordering |
| L6 | Coordinates | Low | <1ms | Lowest | Snap-to-nearest-element from stored coordinates. Last resort at daemon level |
| L7 | LLM semantic | Varies | 1-30s | Highest | Escalates to neocortex via IPC. LLM interprets screen semantically |

### XPath Parser

The selector module includes an XPath parser (`selector.rs`) that handles:
- Bracket attribute predicates: `//Button[@resource-id='com.app/btn']`
- Positional indices: `//LinearLayout[2]/Button[1]`
- Short class name matching: `Button` matches `android.widget.Button`

### Parallelization Opportunity

L2, L3, and L4 are noted in the code as parallelizable (they query independent attributes) but are currently executed sequentially. This is a future optimization — the sequential overhead is 1-3ms total, acceptable for now.

### L7 Escalation

L7 is fundamentally different from L0-L6. It doesn't run in the daemon process:
1. Daemon sends a screen snapshot + target description to neocortex via IPC
2. Neocortex uses the LLM to identify the element semantically
3. Result returned as coordinates or an accessibility node path
4. Only invoked when all daemon-level selectors (L0-L6) fail

This keeps the daemon's hot path fast while providing a powerful fallback.

## Consequences

### Positive

- **Resilience to UI changes:** If an app update changes resource IDs (breaks L0-L2), text-based matching (L3-L4) may still work. If the UI is completely redesigned, LLM semantic matching (L7) can adapt
- **Graceful degradation:** Each level is a weaker but broader net. Most actions resolve at L0-L2 in <2ms. Only novel/changed UIs escalate deeper
- **ETG integration:** Successful selector resolutions are recorded in the ETG. Over time, AURA learns which selector level works best for each app, skipping slow levels
- **No app cooperation needed:** Works with any app using standard Android UI components via AccessibilityService

### Negative

- **Cascade latency:** Worst case (L0-L6 all miss, L7 resolves) takes 1-30s. Mitigated by caching successful levels in ETG
- **False positives at low levels:** L5 (ClassName + index) and L6 (coordinates) can match the wrong element if the screen layout changed. Mitigated by confidence scoring in the ReAct engine
- **L7 cost:** LLM-based resolution is expensive (battery, latency). Should be rare in steady state — most elements resolve at L0-L3

## Alternatives Considered

### 1. Coordinates Only (Screen Recording + Replay)
- **Rejected:** Breaks across device resolutions, orientation changes, and any UI update. The most fragile possible approach.

### 2. Resource ID Only
- **Rejected:** Many apps use auto-generated IDs (e.g., React Native, Flutter) that change between builds. Some elements have no resource ID at all.

### 3. Computer Vision (Template Matching)
- **Rejected:** Requires screenshot capture (permission issues), GPU/CPU intensive, resolution-dependent. AccessibilityService provides structured data — no need to parse pixels for element identification.

### 4. Fixed 3-Level Cascade (ID → Text → Coordinates)
- **Rejected:** Too coarse. The jump from text matching to coordinates is too large. Intermediate levels (structural XPath, content description, class+index) provide meaningful fallbacks that resolve many cases without resorting to fragile coordinates.

## References

- `crates/aura-daemon/src/screen/selector.rs` — Full L0-L7 cascade, XPath parser, all resolution functions
- `crates/aura-daemon/src/execution/etg.rs` — ETG records successful selector levels for future use
- `crates/aura-daemon/src/routing/system2.rs` — L7 IPC escalation to neocortex
