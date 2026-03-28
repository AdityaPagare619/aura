// aura-llama-sys build script
// On Android: compiles llama.cpp with NEON flags
// On host: no-op (uses stub implementations via libloading-style backend)
//
// IMPORTANT: Use CARGO_CFG_TARGET_OS / CARGO_CFG_TARGET_ARCH instead of
// #[cfg(target_os)] / #[cfg(target_arch)] here. The #[cfg] attributes in
// build.rs reflect the BUILD HOST OS, not the compilation TARGET.
// During cross-compilation (Linux host → Android target), #[cfg(target_os = "android")]
// is always false even when we're building for Android. The CARGO_CFG_* env vars
// are set by Cargo to the actual target platform.

fn main() {
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let target_arch = std::env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();
    let stub_enabled = std::env::var_os("CARGO_FEATURE_STUB").is_some();
    let server_enabled = std::env::var_os("CARGO_FEATURE_SERVER").is_some();
    println!(
        "cargo:warning=aura-llama-sys build.rs: target_os={target_os} target_arch={target_arch} stub_enabled={stub_enabled} server_enabled={server_enabled}"
    );

    // Check for NDK in various environment variables
    let ndk_home = std::env::var("NDK_HOME")
        .ok()
        .or_else(|| std::env::var("ANDROID_NDK_HOME").ok())
        .or_else(|| std::env::var("ANDROID_NDK_ROOT").ok());

    // Only compile llama.cpp when targeting Android ARM64
    if target_os == "android" && target_arch == "aarch64" {
        // STUB MODE ENABLED (2026-03-28)
        // The OnceLock static initialization SIOF has been fixed in src/lib.rs.
        // However, NDK cross-compilation requires proper CI infrastructure.
        // For now, we use stub mode which returns hardcoded responses.
        //
        // TO ENABLE REAL LLAMA.CPP:
        // 1. Set up GitHub Actions with proper NDK toolchain
        // 2. Or build natively on the device using Termux
        //
        // The correct build flags (when ready):
        // -march=armv8.7a+fp16+dotprod (per official llama.cpp docs)
        // -DGGML_USE_NEON_FP16=ON
        // -DGGML_NATIVE=ON (runtime CPU detection)

        if ndk_home.is_some() {
            // NDK found - but still using stub for development stability
            println!("cargo:warning=aura-llama-sys build.rs: NDK found but using STUB mode for stability");
        } else {
            println!("cargo:warning=aura-llama-sys build.rs: NDK not found - using STUB mode");
        }

        println!("cargo:rustc-cfg=llama_stub");
        println!("cargo:stub=true");
        return;
    } else {
        // On host builds (non-Android), nothing to compile — stubs are pure Rust.
        // Emit a DEP_LLAMA_STUB marker so dependent crates can detect stub mode.
        println!("cargo:rustc-cfg=llama_stub");
        println!("cargo:stub=true");
    }
}
