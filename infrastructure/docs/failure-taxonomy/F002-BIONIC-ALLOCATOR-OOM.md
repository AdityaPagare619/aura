# F002: Bionic Allocator OOM

## Classification

| Field | Value |
|-------|-------|
| **ID** | F002 |
| **Domain** | Memory |
| **Category** | OomBionic |
| **Type** | bionic_malloc_null |
| **Severity** | P1-HIGH |
| **Status** | NEEDS_TEST |

## Title

Out-of-memory failure in bionic libc allocator on Android/Termux

## Description

bionic's malloc() returns NULL under memory pressure instead of terminating the process (unlike glibc). AURA may not handle NULL returns correctly, causing crashes or undefined behavior.

## Root Cause

- bionic's memory allocator behaves differently from glibc
- AURA's memory allocation paths may not check for NULL returns
- 512MiB memory limit on mid-range Android devices
- glibc calls `std::terminate()` on failed allocation; bionic returns NULL

## Triggers

- [ ] Running on Android with limited memory (≤1GB)
- [ ] Multiple AURA subsystems allocating simultaneously
- [ ] Memory pressure from other Android apps
- [ ] Large model loading (neocortex)

## Prevention

- [x] **Docker validation:** `--memory=512m` limit in Termux-like container
- [ ] Regression test: `infrastructure/tests/regression/test_memory_limits.rs`

## Status

**NOT YET TESTED** — needs regression test to be built

## Workaround

Ensure all malloc/calloc/realloc returns are checked for NULL:
```c
void* ptr = malloc(size);
if (ptr == NULL) {
    // Handle OOM gracefully
}
```

## Verification

```bash
# Build and run with memory limit
docker run --memory=512m aura:latest

# Expected: Graceful OOM handling, not crash
```

## Related Failures

- F001: SIGSEGV_STARTUP (same NDK r26b environment)

## Regression Test

**Location:** `infrastructure/tests/regression/test_memory_limits.rs`

The test should verify:
1. All allocation paths check for NULL returns
2. OOM is handled gracefully with informative error message
3. No SIGSEGV or SIGABRT under memory pressure
