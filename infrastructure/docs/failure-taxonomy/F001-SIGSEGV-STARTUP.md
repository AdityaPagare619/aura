# F001: SIGSEGV at Startup

## Classification

| Field | Value |
|-------|-------|
| **ID** | F001 |
| **Domain** | NDK Compiler |
| **Category** | SigsegvStartup |
| **Type** | lto_panic_ndk_interaction |
| **Severity** | P0-CRITICAL |
| **Status** | VERIFIED |

## Title

SIGSEGV at startup on Android/Termux with NDK r26b

## Description

AURA v4 crashes with SIGSEGV (segmentation fault) immediately at startup when running on Termux/Android with NDK r26b. The crash occurs at address 0x5ad0b4 with a NULL dereference pattern.

## Timing Clarification

**Critical:** this failure class occurs during compiler/runtime initialization,
before meaningful AURA application logic and before llama backend initialization.

- Crash window: PREINIT/early runtime startup
- Not an inference-path panic inside `aura-neocortex` business logic
- Not a post-start allocator pressure event (see F002 for that class)

In practice, if this failure triggers, process startup can abort before normal
runtime stage checks execute.

## Root Cause

**NDK Issue #2073**: Known incompatibility between:
- `lto=true` (link-time optimization enabled)
- `panic="abort"` (Rust panic strategy)
- Android NDK r26b toolchain

When these three conditions combine, the LLVM linker produces corrupted unwind tables that cause a NULL pointer dereference at runtime startup.

**Technical Details:**
- Rust Issue #94564: LTO + panic=abort on ARM causes SIGSEGV
- Rust Issue #121033: LLVM LTO generates invalid stack unwind info
- Rust Issue #123733: Related to LLVM's interaction with bionic libc

## Triggers

All conditions must be met:
- [ ] Running on Android/Termux with NDK r26b
- [ ] Cargo.toml has `lto=true` (full LTO, not thin)
- [ ] Cargo.toml has `panic="abort"`
- [ ] Compiled release build (debug builds are unaffected)

## Prevention

- [x] **CI checks:** `infrastructure/scripts/smoke/container_smoke.sh` validates LTO and panic settings in Cargo.toml before build
- [x] **Docker validation:** Termux-like container catches this in CI
- [x] **Regression test:** `infrastructure/tests/regression/test_ndk_lto.rs`

## Fix Applied

**File:** `Cargo.toml`

**Changes:**
```toml
[profile.release]
lto = "thin"  # Changed from "true"
panic = "unwind"  # Changed from "abort"
```

**Commit:** `128ed2e` on branch `fix/f001-panic-ndk-rootfix`

## Workaround

If NDK r26b must be used with full LTO:
```toml
[profile.release]
lto = "thin"  # Not "true"
panic = "unwind"  # Not "abort"
```

## Verification

```bash
# Check current settings
grep -A3 '\[profile.release\]' Cargo.toml

# Run smoke test
bash infrastructure/scripts/smoke/container_smoke.sh

# Expected: All 8 tests pass
```

## Related Failures

- F002: BIONIC_ALLOCATOR_OOM (same NDK r26b environment)
- NDK Issue #2073 (upstream tracking)

## Regression Test

**Location:** `infrastructure/tests/regression/test_ndk_lto.rs`

The test verifies:
1. Cargo.toml does not have `lto = "true"` (must be thin, not true)
2. Cargo.toml does not have `panic = "abort"` (must be unwind)
3. Compiled binary starts without SIGSEGV on termux-like environment
