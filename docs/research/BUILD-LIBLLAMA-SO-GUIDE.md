# COMPLETE IMPLEMENTATION GUIDE
## Building libllama.so for Android using Official CMake

---

## STEP 1: Clone llama.cpp

```bash
git clone https://github.com/ggerganov/llama.cpp.git
cd llama.cpp
```

---

## STEP 2: Build for Android ARM64

```bash
# Set NDK path
export NDK_HOME=/path/to/android-ndk

# Create build directory
mkdir -p build
cd build

# Configure for Android ARM64
cmake .. \
    -DCMAKE_TOOLCHAIN_FILE=../cmake/android-toolchain.cmake \
    -DANDROID_ABI=arm64-v8a \
    -DANDROID_PLATFORM=android-24 \
    -DBUILD_SHARED_LIBS=ON \
    -DLLAMA_BUILD_SERVER=OFF \
    -DLLAMA_BUILD_EXAMPLES=OFF

# Build libllama.so
cmake --build . --target llama -j$(nproc)
```

---

## STEP 3: Find the .so file

After build, the shared library will be at:
```
build/src/libllama.so
```

---

## STEP 4: Copy to AURA project

```bash
# Copy to your AURA project
cp build/src/libllama.so /path/to/aura-hotfix-link2/android/app/src/main/jniLibs/arm64-v8a/
```

---

## STEP 5: Update build.rs

Simplify your build.rs to NOT compile on Android:

```rust
fn main() {
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let target_arch = std::env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();
    
    println!("cargo:warning=target_os={} target_arch={}", target_os, target_arch);
    
    // For Android, we use pre-built .so
    if target_os == "android" && target_arch == "aarch64" {
        // Just emit stub - .so is loaded at runtime
        println!("cargo:rustc-cfg=llama_stub");
        println!("cargo:stub=true");
        return;
    }
    
    // For non-Android, also stub (using libloading on host)
    println!("cargo:rustc-cfg=llama_stub");
    println!("cargo:stub=true");
}
```

---

## STEP 6: Ensure .so is packaged in APK

Make sure your `build.gradle` includes:

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

## STEP 7: Runtime Loading

Your existing code using `libloading` will work:
- The .so will be loaded from APK's lib directory
- Use `libloading` to load functions at runtime

---

## VERIFICATION CHECKLIST

- [ ] libllama.so builds successfully
- [ ] .so is in correct APK directory (jniLibs/arm64-v8a/)
- [ ] APK includes .so (check with `unzip -l`)
- [ ] Device can load the .so (test separately first)
- [ ] Full inference works end-to-end

---

## ALTERNATIVE: Use Pre-built from CI

You can also download from llama.cpp GitHub Actions artifacts:
1. Go to llama.cpp Actions tab
2. Find a build that includes Android
3. Download the artifact
4. Extract libllama.so

---

## NOTES

- Use same NDK version consistently
- Match Android API level (24 = Android 7.0 minimum)
- The .so must be built for the same ABI as target device (arm64-v8a)
