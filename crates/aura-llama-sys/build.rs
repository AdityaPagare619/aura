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
    let ci_compile = std::env::var("AURA_COMPILE_LLAMA").unwrap_or_default();

    println!(
        "cargo:warning=aura-llama-sys build.rs: target_os={target_os} target_arch={target_arch} stub_enabled={stub_enabled} server_enabled={server_enabled} ci_compile={ci_compile}"
    );

    // Check for NDK in various environment variables
    let ndk_home = std::env::var("NDK_HOME")
        .ok()
        .or_else(|| std::env::var("ANDROID_NDK_HOME").ok())
        .or_else(|| std::env::var("ANDROID_NDK_ROOT").ok());

    // Only compile llama.cpp when targeting Android ARM64 AND either:
    // 1. CI_COMPILE_LLAMA=true is set, OR
    // 2. Not Android (host build)
    let do_compile = ci_compile == "true" || target_os != "android";

    // Also check if NDK is available for Android builds
    let has_ndk = ndk_home.is_some();

    // Only compile real llama.cpp if explicitly requested (CI) or on host
    if target_os == "android" && target_arch == "aarch64" {
        if do_compile && has_ndk {
            println!("cargo:warning=aura-llama-sys build.rs: COMPILING REAL LLAMA.CPP");
            compile_llama_cpp();
        } else {
            // STUB MODE for local development without proper NDK
            println!("cargo:rustc-cfg=llama_stub");
            println!("cargo:stub=true");
            println!("cargo:warning=aura-llama-sys build.rs: Using STUB mode (set AURA_COMPILE_LLAMA=true to compile real llama.cpp)");
        }
    } else {
        // On host builds (non-Android), use stub
        println!("cargo:rustc-cfg=llama_stub");
        println!("cargo:stub=true");
    }
}

fn compile_llama_cpp() {
    // Check for NDK
    let ndk_home = std::env::var("NDK_HOME")
        .ok()
        .or_else(|| std::env::var("ANDROID_NDK_HOME").ok())
        .or_else(|| std::env::var("ANDROID_NDK_ROOT").ok());

    let Some(ndk_home) = ndk_home else {
        panic!("NDK not found - cannot compile llama.cpp");
    };

    println!("cargo:warning=aura-llama-sys build.rs: Using NDK at {ndk_home}");

    // Ensure Rust linker can resolve Android libc++ static archive.
    emit_android_cpp_runtime_linking();

    // Guard: fail with a clear message if the submodule isn't initialized.
    if !std::path::Path::new("llama.cpp/llama.cpp").exists() {
        panic!(
            "llama.cpp submodule not initialized. \
             Run: git submodule update --init --recursive\n\
             Or build with --features aura-llama-sys/stub to skip native compilation."
        );
    }

    // Compile C files with -std=c11
    // Use conservative defaults for maximum device compatibility
    // Runtime detection will be handled by PlatformCpuFeatures in the Rust code
    let mut c_build = cc::Build::new();
    c_build
        .cpp(false)
        .flag("-std=c11")
        // Use armv8-a as baseline (supported by all 64-bit Android devices)
        .flag("-march=armv8-a")
        .flag("-DGGML_USE_NEON")
        // Don't enable FP16/DotProd at compile time - use runtime detection
        .flag("-DGGML_USE_NEON_FP16=OFF")
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

    // Compile C++ files with -std=c++17
    let mut cpp_build = cc::Build::new();
    cpp_build
        .cpp(true)
        .cpp_link_stdlib(None)
        .flag("-std=c++17")
        // Use armv8-a as baseline (supported by all 64-bit Android devices)
        .flag("-march=armv8-a")
        .flag("-DGGML_USE_NEON")
        // Don't enable FP16/DotProd at compile time - use runtime detection
        .flag("-DGGML_USE_NEON_FP16=OFF")
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

fn emit_android_cpp_runtime_linking() {
    use std::{env, path::PathBuf};

    let ndk_home = env::var("NDK_HOME")
        .ok()
        .or_else(|| env::var("ANDROID_NDK_HOME").ok())
        .or_else(|| env::var("ANDROID_NDK_ROOT").ok());

    let Some(ndk_home) = ndk_home else {
        println!("cargo:warning=Android target detected but NDK not found");
        return;
    };

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

    for path in roots {
        if path.exists() {
            println!("cargo:rustc-link-search=native={}", path.display());
        }
    }

    println!("cargo:rustc-link-lib=static=c++_static");
    println!("cargo:rustc-link-lib=static=c++abi");
}
