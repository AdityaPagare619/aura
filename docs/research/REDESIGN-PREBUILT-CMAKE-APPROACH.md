# AURA ANDROID INFERENCE - COMPLETE REDESIGN
## Using Official llama.cpp CMake + Pre-built Binaries Approach

**Based on real developer experience:** "use the llama.cpp cmake directly and just wire it into your gradle build"

---

## THE PROBLEM (Why we crash)

Our current approach:
- Uses `cc` crate + `build.rs` to compile llama.cpp
- This is WRONG according to real Android developers
- The successful approach: use official llama.cpp CMake + Gradle

---

## THE SOLUTION: Two Options

### Option A: Pre-built .so Files (RECOMMENDED)
Use pre-compiled llama.cpp binaries instead of building from source.

**How it works:**
1. Download pre-built `libllama.so` from llama.cpp releases or build with official CMake once
2. Package the .so in your APK
3. Load via `libloading` at runtime (we already do this!)

**Benefits:**
- Zero build complexity in our CI
- Tested/working binaries
- Easy updates (just replace .so)
- Matches how successful apps do it

### Option B: Official CMake Integration
Build llama.cpp using official CMake, integrated into Gradle.

**How it works:**
1. Add llama.cpp as Gradle submodule
2. Use official CMake build (not cc crate)
3. Link resulting .so

---

## ARCHITECTURE CHANGE REQUIRED

### Current (Broken):
```
build.rs (cc crate) → compile llama.cpp → link into aura-neocortex
```

### New Option A (Pre-built):
```
libllama.so (pre-built) → packaged in APK → loaded via libloading at runtime
```

### New Option B (CMake):
```
llama.cpp (official CMake) → .so → link dynamically → loaded at runtime
```

---

## IMPLEMENTATION STEPS

### Step 1: Get Pre-built libllama.so

Options:
1. Build once locally with official CMake, commit the .so
2. Download from llama.cpp CI artifacts
3. Use android-llama.cpp-example approach

### Step 2: Update build.rs

Remove all cc crate compilation. Just:
- Tell Cargo about the .so location
- Set up runtime loading via libloading

### Step 3: Update APK Packaging

Add libllama.so to APK under `jniLibs`:
```
android/app/src/main/jniLibs/
├── arm64-v8a/
│   └── libllama.so
└── armeabi-v7a/
    └── libllama.so
```

### Step 4: Update Gradle

```groovy
android {
    sourceSets {
        main {
            jniLibs.srcDirs = ['src/main/jniLibs']
        }
    }
}
```

---

## WHAT TO REMOVE FROM OUR CODE

### build.rs changes:
- Remove all `cc::Build` compilation
- Remove NDK toolchain paths
- Remove `-march=armv8-a` flags
- Remove all the complex linking code
- Just emit stub cfg for Android

### Cargo.toml changes:
- Keep `libloading = "0.8"` (for runtime loading)
- Remove `cc = "1"` from build-dependencies

---

## WHAT TO KEEP

1. **libloading** - Our runtime .so loading works fine
2. **GGUF metadata parsing** - Pure Rust, no issues
3. **IPC architecture** - Works correctly
4. **neocortex process** - Separate process is good design

---

## RISK ANALYSIS

| Risk | Mitigation |
|------|------------|
| .so version mismatch | Build .so from same llama.cpp version we reference |
| ABI compatibility | Use same NDK version for building .so |
| Update complexity | Simple file replacement |
| Testing | Test .so separately before packaging |

---

## IMMEDIATE ACTION ITEMS

1. [ ] Build libllama.so locally using official CMake
2. [ ] Test loading the .so on Android (not in binary, standalone)
3. [ ] Update build.rs to skip compilation on Android
4. [ ] Add .so to APK jniLibs
5. [ ] Test full stack

---

## THE KEY INSIGHT

The developer who spent months on this said:
> "dont try to build llama.cpp with the default NDK cmake setup. use the llama.cpp cmake directly"

We were trying to BUILD from source using wrong tools. The solution is to:
1. Build once correctly (or download)
2. Just USE the pre-built .so

This is how ALL successful Android LLM apps work.

---

**Document Status**: Ready for implementation  
**Next**: Build .so using official CMake approach
