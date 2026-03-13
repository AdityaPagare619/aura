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

    // Only compile llama.cpp when targeting Android ARM64
    if target_os == "android" && target_arch == "aarch64" {
        // Guard: fail with a clear message if the submodule isn't initialized.
        // Wrap in stub-feature check so CI can compile without the submodule.
        #[cfg(not(feature = "stub"))]
        {
            if !std::path::Path::new("llama.cpp/llama.cpp").exists() {
                panic!(
                    "llama.cpp submodule not initialized. \
                     Run: git submodule update --init --recursive\n\
                     Or build with --features aura-llama-sys/stub to skip native compilation."
                );
            }

            let mut build = cc::Build::new();
            build
                .cpp(true)
                .flag("-std=c++17")
                .flag("-march=armv8-a+fp+simd")
                .flag("-DGGML_USE_NEON")
                .flag("-O3")
                .flag("-DNDEBUG")
                .file("llama.cpp/llama.cpp")
                .file("llama.cpp/ggml.c")
                .file("llama.cpp/ggml-alloc.c")
                .file("llama.cpp/ggml-backend.c")
                .file("llama.cpp/ggml-quants.c")
                .include("llama.cpp");
            build.compile("llama");
            println!("cargo:rustc-link-lib=static=llama");
        }

        // In stub mode on Android: emit the stub marker instead of compiling native code.
        #[cfg(feature = "stub")]
        {
            println!("cargo:rustc-cfg=llama_stub");
            println!("cargo:stub=true");
        }
    } else {
        // On host builds (non-Android), nothing to compile — stubs are pure Rust.
        // Emit a DEP_LLAMA_STUB marker so dependent crates can detect stub mode.
        println!("cargo:rustc-cfg=llama_stub");
        println!("cargo:stub=true");
    }
}
