# COMPLETE UNDERSTANDING - AURA ON ANDROID

## Architecture: Termux on Android

```
Android OS
    ↓
Termux (Linux environment)
    ↓
AURA Binary (aarch64-linux-android)
    ├── aura-daemon (shared library .so)
    └── aura-neocortex (binary)
```

## NOT a native Android app
- No JNI bindings in the traditional sense
- No APK packaging
- Runs inside Termux like any Linux binary

## The Real Problem

Our binary (`aura-neocortex` or `aura-daemon.so`) crashes at **bionic initialization** (`GetPropAreaForName`).

This is happening BEFORE any inference - it's the Rust runtime / system libraries initialization.

## Root Causes to Consider

### 1. Rust Runtime + Bionic Issues
- The crash is at bionic level, not at our code
- Could be Rust's std library interacting with MediaTek's bionic
- This is DIFFERENT from llama.cpp issues

### 2. Termux-Specific Issues
- Termux has its own environment quirks
- Different libc than stock Android
- LD_PRELOAD issues (mentioned in docs)

### 3. Static Linking Issues
- We tried `-crt-static` - made it worse (no dependencies)
- The binary might need dynamic linking

### 4. The Build Approach
- We're using cc crate to build llama.cpp
- But the crash happens in Rust runtime itself
- Maybe our entire build approach is wrong

## What Actually Works

The successful approach from Reddit:
- Use official CMake to build llama.cpp
- Build ONCE locally, then use the binary
- Don't cross-compile from CI

## The Question

Given the crash is at bionic init (not at llama.cpp):
- Is the problem even related to llama.cpp?
- Or is it the Rust binary itself on MediaTek?
- Should we test a SIMPLE Rust binary first?

## Next Steps

1. Test if a minimal Rust binary works on the device
2. If minimal binary crashes, problem is Rust + MediaTek bionic
3. If minimal works, problem is our specific code
4. Then address llama.cpp separately
