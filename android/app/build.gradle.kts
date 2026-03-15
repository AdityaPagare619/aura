plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
}

android {
    namespace = "dev.aura.v4"
    compileSdk = 34

    defaultConfig {
        applicationId = "dev.aura.v4"
        minSdk = 26          // Android 8.0 — GestureDescription requires API 24+, we target 26 for NotificationChannel
        targetSdk = 34
        versionCode = 1
        versionName = "4.0.0"

        ndk {
            // AND-HIGH-5: Restrict to arm64-v8a only — the sole ABI built by
            // Cargo. Including armeabi-v7a and x86_64 causes install-time crashes
            // on those architectures when the .so is missing.
            abiFilters += listOf("arm64-v8a")
        }
    }

    buildTypes {
        release {
            isMinifyEnabled = false   // native .so — no point in R8 on Kotlin glue
            proguardFiles(
                getDefaultProguardFile("proguard-android-optimize.txt"),
                "proguard-rules.pro"
            )
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    kotlinOptions {
        jvmTarget = "17"
    }

    // Pre-built .so from Rust cross-compilation goes here
    sourceSets["main"].jniLibs.srcDirs("src/main/jniLibs")

    buildFeatures {
        buildConfig = true
    }
}

dependencies {
    implementation("androidx.core:core-ktx:1.12.0")
}
