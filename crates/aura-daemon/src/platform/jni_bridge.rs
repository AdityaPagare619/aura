//! Rust ↔ Kotlin JNI bridge for AURA v4.
//!
//! This module provides:
//!
//! 1. **`JNI_OnLoad`** — caches the `JavaVM` pointer for later use.
//! 2. **`jni_env()`** — returns a `JNIEnv` handle on any thread.
//! 3. **`Exported `Java_dev_aura_v4_AuraDaemonBridge_native*` functions** — called by Kotlin via
//!    `external fun` declarations.
//! 4. **Helper functions** (`jni_perform_tap`, `jni_get_screen_tree`, etc.) called by other Rust
//!    modules to invoke Kotlin-side methods.
//!
//! # Safety
//!
//! All JNI interactions are inherently unsafe. Every boundary crossing is
//! wrapped in error handling — the native layer **never panics** on JNI
//! errors; instead it returns `PlatformError::JniFailed`.
//!
//! # Cfg gating
//!
//! The entire module is `#[cfg(target_os = "android")]` in production.
//! For host builds, every public helper function has a desktop stub that
//! returns a sensible mock value.