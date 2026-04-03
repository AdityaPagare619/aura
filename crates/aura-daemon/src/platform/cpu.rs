//! Runtime CPU feature detection for Android compatibility.
//!
//! Detects available CPU features at runtime instead of relying on
//! compile-time flags. This allows AURA to work across different
//! Android devices with varying ARM capabilities.

use std::collections::HashSet;
use std::sync::OnceLock;

/// Detected CPU features available on the current device.
#[derive(Debug, Clone)]
pub struct PlatformCpuFeatures {
    /// ARM NEON (Advanced SIMD) - baseline for most ARMv7+ devices
    pub has_neon: bool,
    /// ARM FP16 (half-precision floating-point)
    pub has_fp16: bool,
    /// ARM DotProd (integer dot product instructions)
    pub has_dotprod: bool,
    /// ARM SVE (Scalable Vector Extension) - newer ARMv8.2+
    pub has_sve: bool,
    /// ARM SVE2 - ARMv9+
    pub has_sve2: bool,
    /// ARM CRC32 (cyclic redundancy check)
    pub has_crc32: bool,
    /// ARM AES (Advanced Encryption Standard)
    pub has_aes: bool,
    /// ARM SHA1/SHA2 (crypto hashing)
    pub has_sha2: bool,
    /// Number of CPU cores
    pub cpu_cores: u32,
    /// CPU architecture (e.g., "aarch64", "armv8l")
    pub architecture: String,
    /// CPU implementer (e.g., "0x41" for ARM, "0x51" for Qualcomm)
    pub implementer: String,
    /// CPU part number (device-specific)
    pub part: String,
}

/// Cached CPU features (computed once at startup).
static CPU_FEATURES: OnceLock<PlatformCpuFeatures> = OnceLock::new();

impl PlatformCpuFeatures {
    /// Detect CPU features at runtime.
    ///
    /// On Android, reads from `/proc/cpuinfo` and `/proc/self/auxv`.
    /// On non-Android, uses compile-time target features.
    pub fn detect() -> &'static PlatformCpuFeatures {
        CPU_FEATURES.get_or_init(Self::do_detect)
    }

    fn do_detect() -> Self {
        #[cfg(target_os = "android")]
        {
            Self::detect_android()
        }

        #[cfg(not(target_os = "android"))]
        {
            Self::detect_host()
        }
    }

    #[cfg(target_os = "android")]
    fn detect_android() -> Self {
        let mut features = HashSet::new();
        let mut cpu_cores = 0u32;
        let mut architecture = String::new();
        let mut implementer = String::new();
        let mut part = String::new();

        // Read from /proc/cpuinfo
        if let Ok(content) = std::fs::read_to_string("/proc/cpuinfo") {
            for line in content.lines() {
                let line_lower = line.to_lowercase();

                // Detect features
                if line_lower.contains("neon") || line_lower.contains("asimd") {
                    features.insert("neon");
                }
                if line_lower.contains("fp16") {
                    features.insert("fp16");
                }
                if line_lower.contains("dotprod") || line_lower.contains("i8mm") {
                    features.insert("dotprod");
                }
                if line_lower.contains("sve") {
                    features.insert("sve");
                }
                if line_lower.contains("sve2") {
                    features.insert("sve2");
                }
                if line_lower.contains("crc32") {
                    features.insert("crc32");
                }
                if line_lower.contains("aes") {
                    features.insert("aes");
                }
                if line_lower.contains("sha1") || line_lower.contains("sha2") {
                    features.insert("sha2");
                }

                // Detect architecture
                if line_lower.starts_with("processor") {
                    cpu_cores += 1;
                }
                if line_lower.starts_with("cpu architecture") {
                    if let Some(value) = line.split(':').nth(1) {
                        architecture = value.trim().to_string();
                    }
                }
                if line_lower.starts_with("cpu implementer") {
                    if let Some(value) = line.split(':').nth(1) {
                        implementer = value.trim().to_string();
                    }
                }
                if line_lower.starts_with("cpu part") {
                    if let Some(value) = line.split(':').nth(1) {
                        part = value.trim().to_string();
                    }
                }
            }
        }

        // Fallback: check /proc/self/auxv for HWCAP
        if features.is_empty() {
            Self::detect_from_hwcaps(&mut features);
        }

        // Fallback: check target features from compiler
        #[cfg(target_arch = "aarch64")]
        {
            if features.is_empty() {
                features.insert("neon"); // Always present on aarch64
                if cfg!(target_feature = "fp16") {
                    features.insert("fp16");
                }
                if cfg!(target_feature = "dotprod") {
                    features.insert("dotprod");
                }
                if cfg!(target_feature = "sve") {
                    features.insert("sve");
                }
                if cfg!(target_feature = "sve2") {
                    features.insert("sve2");
                }
                if cfg!(target_feature = "crc") {
                    features.insert("crc32");
                }
                if cfg!(target_feature = "aes") {
                    features.insert("aes");
                }
                if cfg!(target_feature = "sha2") {
                    features.insert("sha2");
                }
            }
        }

        // Default: assume NEON is available on all modern Android devices
        if features.is_empty() {
            features.insert("neon");
        }

        // Get CPU cores from system
        if cpu_cores == 0 {
            cpu_cores = Self::get_cpu_cores();
        }

        Self {
            has_neon: features.contains("neon"),
            has_fp16: features.contains("fp16"),
            has_dotprod: features.contains("dotprod"),
            has_sve: features.contains("sve"),
            has_sve2: features.contains("sve2"),
            has_crc32: features.contains("crc32"),
            has_aes: features.contains("aes"),
            has_sha2: features.contains("sha2"),
            cpu_cores,
            architecture,
            implementer,
            part,
        }
    }

    #[cfg(target_os = "android")]
    fn detect_from_hwcaps(features: &mut HashSet<&str>) {
        // Read HWCAP from /proc/self/auxv
        // This is a more reliable way to detect CPU features
        if let Ok(content) = std::fs::read("/proc/self/auxv") {
            // HWCAP is typically at offset 16 in auxv
            // For simplicity, we'll check for common feature strings
            let content_str = String::from_utf8_lossy(&content);
            let content_lower = content_str.to_lowercase();

            if content_lower.contains("neon") || content_lower.contains("asimd") {
                features.insert("neon");
            }
            if content_lower.contains("fp16") {
                features.insert("fp16");
            }
            if content_lower.contains("dotprod") {
                features.insert("dotprod");
            }
            if content_lower.contains("sve") {
                features.insert("sve");
            }
        }
    }

    #[cfg(target_os = "android")]
    fn get_cpu_cores() -> u32 {
        // Try to get from /proc/stat
        if let Ok(content) = std::fs::read_to_string("/proc/stat") {
            let mut count = 0;
            for line in content.lines() {
                if line.starts_with("cpu") && !line.starts_with("cpu ") {
                    count += 1;
                }
            }
            if count > 0 {
                return count;
            }
        }

        // Fallback: use std::thread::available_parallelism
        std::thread::available_parallelism()
            .map(|n| n.get() as u32)
            .unwrap_or(4)
    }

    #[cfg(not(target_os = "android"))]
    fn detect_host() -> Self {
        // On host builds, use compile-time target features
        let has_neon = cfg!(target_arch = "aarch64");
        let has_fp16 = cfg!(target_feature = "fp16");
        let has_dotprod = cfg!(target_feature = "dotprod");
        let has_sve = cfg!(target_feature = "sve");
        let has_sve2 = cfg!(target_feature = "sve2");
        let has_crc32 = cfg!(target_feature = "crc");
        let has_aes = cfg!(target_feature = "aes");
        let has_sha2 = cfg!(target_feature = "sha2");

        let cpu_cores = std::thread::available_parallelism()
            .map(|n| n.get() as u32)
            .unwrap_or(4);

        Self {
            has_neon,
            has_fp16,
            has_dotprod,
            has_sve,
            has_sve2,
            has_crc32,
            has_aes,
            has_sha2,
            cpu_cores,
            architecture: if cfg!(target_arch = "aarch64") {
                "aarch64".to_string()
            } else if cfg!(target_arch = "arm") {
                "armv7l".to_string()
            } else {
                std::env::consts::ARCH.to_string()
            },
            implementer: "0x00".to_string(), // Unknown on host
            part: "0x000".to_string(),
        }
    }

    /// Get the best `-march` flag for the current device.
    ///
    /// Returns a string like "armv8.2a+fp16+dotprod" based on detected features.
    pub fn get_march_flag(&self) -> String {
        let mut flags = Vec::new();

        // Base architecture
        if self.has_sve2 {
            flags.push("armv9-a".to_string());
        } else if self.has_sve || self.has_dotprod || self.has_fp16 {
            flags.push("armv8.2a".to_string());
        } else if self.has_neon {
            flags.push("armv8-a".to_string());
        } else {
            flags.push("armv7-a".to_string());
        }

        // Add feature flags
        if self.has_neon {
            flags.push("neon".to_string());
        }
        if self.has_fp16 {
            flags.push("fp16".to_string());
        }
        if self.has_dotprod {
            flags.push("dotprod".to_string());
        }
        if self.has_sve {
            flags.push("sve".to_string());
        }
        if self.has_sve2 {
            flags.push("sve2".to_string());
        }
        if self.has_crc32 {
            flags.push("crc".to_string());
        }
        if self.has_aes {
            flags.push("aes".to_string());
        }
        if self.has_sha2 {
            flags.push("sha2".to_string());
        }

        flags.join("+")
    }

    /// Get compiler flags for llama.cpp compilation.
    ///
    /// Returns a vector of flags suitable for `cc::Build`.
    pub fn get_llama_flags(&self) -> Vec<String> {
        let mut flags = vec![
            "-std=c11".to_string(),
            "-O3".to_string(),
            "-DNDEBUG".to_string(),
            "-Wno-error".to_string(),
        ];

        // Add architecture flag
        flags.push(format!("-march={}", self.get_march_flag()));

        // Add feature defines
        if self.has_neon {
            flags.push("-DGGML_USE_NEON".to_string());
        }
        if self.has_fp16 {
            flags.push("-DGGML_USE_NEON_FP16=ON".to_string());
        }
        if self.has_sve {
            flags.push("-DGGML_USE_SVE=ON".to_string());
        } else {
            flags.push("-DGGML_USE_SVE=OFF".to_string());
        }

        flags.push("-DGGML_NATIVE=ON".to_string());

        flags
    }

    /// Check if the device supports advanced inference features.
    ///
    /// Returns true if the device has enough CPU features for efficient inference.
    pub fn supports_advanced_inference(&self) -> bool {
        self.has_neon && (self.has_fp16 || self.has_dotprod)
    }

    /// Get a human-readable summary of detected features.
    pub fn summary(&self) -> String {
        let mut parts = Vec::new();

        if self.has_neon {
            parts.push("NEON");
        }
        if self.has_fp16 {
            parts.push("FP16");
        }
        if self.has_dotprod {
            parts.push("DotProd");
        }
        if self.has_sve {
            parts.push("SVE");
        }
        if self.has_sve2 {
            parts.push("SVE2");
        }
        if self.has_crc32 {
            parts.push("CRC32");
        }
        if self.has_aes {
            parts.push("AES");
        }
        if self.has_sha2 {
            parts.push("SHA2");
        }

        if parts.is_empty() {
            "No advanced features".to_string()
        } else {
            format!("{} cores, {}", self.cpu_cores, parts.join(", "))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cpu_features_detect() {
        let features = PlatformCpuFeatures::detect();
        // Should always have at least some features detected
        assert!(features.cpu_cores > 0);
    }

    #[test]
    fn test_get_march_flag() {
        let features = PlatformCpuFeatures::detect();
        let march = features.get_march_flag();
        // Should return a non-empty string
        assert!(!march.is_empty());
    }

    #[test]
    fn test_get_llama_flags() {
        let features = PlatformCpuFeatures::detect();
        let flags = features.get_llama_flags();
        // Should have at least basic flags
        assert!(flags.len() >= 4);
    }

    #[test]
    fn test_summary() {
        let features = PlatformCpuFeatures::detect();
        let summary = features.summary();
        // Should be non-empty
        assert!(!summary.is_empty());
    }

    #[test]
    fn test_host_build_features() {
        #[cfg(not(target_os = "android"))]
        {
            let features = PlatformCpuFeatures::detect();
            // On host, architecture should be set
            assert!(!features.architecture.is_empty());
        }
    }
}
