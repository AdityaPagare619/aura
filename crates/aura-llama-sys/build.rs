// aura-llama-sys build script
// On Android: compiles llama.cpp with NEON flags
// On host: no-op (uses stub implementations via libloading-style backend)

fn main() {
    // Only compile llama.cpp when targeting Android ARM64
    #[cfg(target_os = "android")]
    {
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

    // On host builds, nothing to compile — stubs are pure Rust.
    // We still need to satisfy the `links = "llama"` directive.
    #[cfg(not(target_os = "android"))]
    {
        // Emit a DEP_LLAMA_STUB marker so dependent crates can detect stub mode
        println!("cargo:rustc-cfg=llama_stub");
        println!("cargo:stub=true");
    }
}
