@echo off
set PATH=C:\Android\ndk\android-ndk-r27d\toolchains\llvm\prebuilt\windows-x86_64\bin;%PATH%
set ANDROID_NDK_ROOT=C:\Android\ndk\android-ndk-r27d
set CC_aarch64_linux_android=C:\Android\ndk\android-ndk-r27d\toolchains\llvm\prebuilt\windows-x86_64\bin\aarch64-linux-android21-clang.cmd
set CXX_aarch64_linux_android=C:\Android\ndk\android-ndk-r27d\toolchains\llvm\prebuilt\windows-x86_64\bin\aarch64-linux-android21-clang++.cmd
set AR_aarch64_linux_android=C:\Android\ndk\android-ndk-r27d\toolchains\llvm\prebuilt\windows-x86_64\bin\llvm-ar.exe
cd /d C:\Users\Lenovo\aura-hotfix-link2
cargo check --target aarch64-linux-android -p aura-daemon --features curl-backend 2>&1 | findstr /C:"error" /C:"Finished" /C:"warning"
