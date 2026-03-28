# 05 — Android Case Study (Code + CI Evidence)

## 1) Deployment modes on Android

AURA has two Android-adjacent operational modes:

1. **APK/JNI path**: daemon loaded as shared library through Android app layer.
2. **Termux operational path**: daemon/neocortex binaries installed and run via shell/service model.

Reference:
- daemon binary notes: `crates/aura-daemon/src/bin/main.rs:4-10`
- installer Termux paths: `install.sh:93-118`

## 2) JNI and startup architecture boundaries

Startup sequence explicitly includes JNI load phase before runtime/subsystems.

Reference: `crates/aura-daemon/src/daemon_core/startup.rs:271-279`, `:443`.

This enforces a pre-runtime boundary where JNI/environment failures are distinct from later model/inference failures.

## 3) Native llama build path (current code)

`aura-llama-sys/build.rs` currently:

- compiles native llama sources for Android aarch64 when not in stub feature mode.
- emits stub cfg markers for host builds and for Android stub-feature mode.

Reference: `crates/aura-llama-sys/build.rs:16-25`, `:38-73`, `:76-87`.

So current code does **not** show an unconditional "always skip native compile" override. It is feature and target dependent.

## 4) Backend initialization reality

Neocortex model load path checks backend initialization and explicitly errors if Android FFI backend has not been initialized.

Reference: `crates/aura-neocortex/src/model.rs:1082-1097`.

Global backend accessor itself requires prior init and will panic if used without initialization.

Reference: `crates/aura-llama-sys/src/lib.rs:1749-1754`.

## 5) Android build and CI evidence

### 5.1 Green reference

Mainline green run `23677762816` demonstrates Android build success under current CI strategy.

### 5.2 Failure class in separate workflow

`Device Validate` run `23677813385` failed due to missing artifact in download stage, not due to runtime logic crash.

This indicates infrastructure coupling issues can mask runtime confidence progression.

## 6) Memory context risk framing

`aura-types` explicitly models KV-cache-driven memory scaling via `ModelMemoryEstimate::kv_cache_bytes(context_len)`.

Reference: `crates/aura-types/src/power.rs:578-589`.

This provides a code-level mechanism for context-window risk analysis even when runtime crashes are OEM/allocator-specific.

## 7) Android case-study conclusions

1. Android reliability requires jointly solving:
   - runtime initialization contracts,
   - native build/link correctness,
   - memory/context sizing policies,
   - CI artifact handoff reliability.
2. The architecture and observed failures indicate the problem domain is broader than llama-only debugging.
3. Operational confidence should be driven by integrated evidence: installer path + runtime path + CI artifact path + on-device validation.
