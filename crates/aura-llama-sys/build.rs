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
        // Ensure Rust linker can resolve Android libc++ static archive.
        // cc-rs emits `cargo:rustc-link-lib=static=c++_static`, but rustc
        // needs an explicit native search path to locate libc++_static.a.
        emit_android_cpp_runtime_linking();

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

            // Compile C files separately with -std=c11.
            // Using .cpp(true) on .c files forces C++ mode and breaks NDK r26b clang
            // which rejects C99 compound literals and void* implicit casts in C++ mode.
            let mut c_build = cc::Build::new();
            c_build
                .cpp(false)
                .flag("-std=c11")
                // CONSERVATIVE: Use generic armv8-a to avoid MediaTek Dimensity 6300 SIGSEGV
                // Research: MediaTek devices crash with +fp16+dotprod flags due to
                // FP16_VECTOR_ARITHMETIC issues (llama.cpp #13708, #18766)
                .flag("-march=armv8-a")
                // NEON is standard on all ARMv8-A cores, but disable FP16 vectorization
                .flag("-DGGML_USE_NEON")
                .flag("-DGGML_USE_NEON_FP16=OFF")  // Disable FP16 vectorization for stability
                // Disable GGML_NATIVE to prevent SVE/other CPU feature auto-detection issues
                // Research: GGML_NATIVE can cause SIGSEGV on some Android devices (#8109)
                .flag("-DGGML_NATIVE=OFF")
                .flag("-DGGML_USE_SVE=OFF")
                .flag("-O3")
                .flag("-DNDEBUG")
                .flag("-Wno-error")
                .file("llama.cpp/ggml.c")
                .file("llama.cpp/ggml-alloc.c")
                .file("llama.cpp/ggml-backend.c")
                .file("llama.cpp/ggml-quants.c")
                .include("llama.cpp");
            c_build.compile("llama_c");

            // Compile C++ files with -std=c++17.
            let mut cpp_build = cc::Build::new();
            cpp_build
                .cpp(true)
                // Disable cc-rs automatic C++ stdlib linkage. We emit explicit
                // link-search + link-lib directives in `emit_android_cpp_runtime_linking`
                // so rustc can reliably resolve static libc++ in CI cross builds.
                .cpp_link_stdlib(None)
                .flag("-std=c++17")
                // CONSERVATIVE: Use generic armv8-a to avoid MediaTek Dimensity 6300 SIGSEGV
                // Research: MediaTek devices crash with +fp16+dotprod flags due to
                // FP16_VECTOR_ARITHMETIC issues (llama.cpp #13708, #18766)
                .flag("-march=armv8-a")
                // NEON is standard on all ARMv8-A cores, but disable FP16 vectorization
                .flag("-DGGML_USE_NEON")
                .flag("-DGGML_USE_NEON_FP16=OFF")  // Disable FP16 vectorization for stability
                // Disable GGML_NATIVE to prevent SVE/other CPU feature auto-detection issues
                // Research: GGML_NATIVE can cause SIGSEGV on some Android devices (#8109)
                .flag("-DGGML_NATIVE=OFF")
                .flag("-DGGML_USE_SVE=OFF")
                .flag("-O3")
                .flag("-DNDEBUG")
                .flag("-Wno-error")
                .file("llama.cpp/llama.cpp")
                .include("llama.cpp");
            cpp_build.compile("llama_cpp");

            println!("cargo:rustc-link-lib=static=llama_c");
            println!("cargo:rustc-link-lib=static=llama_cpp");
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

fn emit_android_cpp_runtime_linking() {
    use std::{env, path::PathBuf};

    // Android NDK root can be exposed under multiple conventional names.
    let ndk_home = env::var("NDK_HOME")
        .ok()
        .or_else(|| env::var("ANDROID_NDK_HOME").ok())
        .or_else(|| env::var("ANDROID_NDK_ROOT").ok());

    let Some(ndk_home) = ndk_home else {
        println!(
            "cargo:warning=Android target detected but NDK_HOME/ANDROID_NDK_HOME/ANDROID_NDK_ROOT is not set; c++_static link may fail"
        );
        println!("cargo:rustc-link-lib=static=c++_static");
        return;
    };

    // Prefer explicit host tag if provided by setup; otherwise use Linux default
    // (our CI runs on ubuntu-latest).
    let host_tag = env::var("ANDROID_NDK_HOST_TAG").unwrap_or_else(|_| "linux-x86_64".to_string());

    let mut roots = vec![PathBuf::from(&ndk_home)
        .join("toolchains")
        .join("llvm")
        .join("prebuilt")
        .join(&host_tag)
        .join("sysroot")
        .join("usr")
        .join("lib")
        .join("aarch64-linux-android")];

    // Also include the generic sysroot lib dir. Some NDK layouts place
    // libc++ artifacts there instead of (or in addition to) triple-specific dirs.
    roots.push(
        PathBuf::from(&ndk_home)
            .join("toolchains")
            .join("llvm")
            .join("prebuilt")
            .join(&host_tag)
            .join("sysroot")
            .join("usr")
            .join("lib"),
    );

    // Some NDK layouts also place API-specific variants under .../<triple>/<api>.
    // Emit both if present to make rustc static-lib resolution robust.
    let api_level = env::var("API_LEVEL")
        .ok()
        .or_else(|| env::var("CARGO_NDK_ANDROID_PLATFORM").ok())
        .or_else(|| env::var("ANDROID_PLATFORM").ok());
    if let Some(api_level) = api_level {
        if !api_level.is_empty() {
            roots.push(
                PathBuf::from(&ndk_home)
                    .join("toolchains")
                    .join("llvm")
                    .join("prebuilt")
                    .join(&host_tag)
                    .join("sysroot")
                    .join("usr")
                    .join("lib")
                    .join("aarch64-linux-android")
                    .join(api_level),
            );
        }
    }

    let mut found_static_archive = false;
    for path in roots {
        if path.exists() {
            let archive = path.join("libc++_static.a");
            if archive.exists() {
                found_static_archive = true;
                println!(
                    "cargo:warning=Found Android static libc++ archive at {}",
                    archive.display()
                );
            }
            println!("cargo:rustc-link-search=native={}", path.display());
        }
    }

    if !found_static_archive {
        println!(
            "cargo:warning=Did not find libc++_static.a in emitted Android link-search paths; rustc may fail if NDK layout differs"
        );
    }

    // Enforce static C++ runtime for self-contained Android release binaries.
    // NDK splits some exception ABI symbols into libc++abi.a, so link it
    // explicitly to avoid unresolved __cxa* / __gxx_personality_* symbols.
    println!("cargo:rustc-link-lib=static=c++_static");
    println!("cargo:rustc-link-lib=static=c++abi");
}
