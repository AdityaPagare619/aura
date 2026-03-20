//! AURA Failure Taxonomy System
//!
//! This module classifies every failure to its root domain, preventing
//! the same bug from being debugged twice. Inspired by medical taxonomy
//! and aviation incident reporting (LAHSO).
//!
//! The taxonomy has 4 top-level domains:
//! - NdkCompiler: NDK/toolchain failures (SIGSEGV, linker errors, etc.)
//! - Platform: Android/Termux environment issues (permissions, API level, etc.)
//! - Memory: Memory management failures (OOM, leaks, corruption)
//! - Logic: Code logic failures (wrong algorithm, edge cases, etc.)
//!
//! Each failure gets a unique ID (e.g., F001-SIGSEGV-AT-STARTUP) and includes:
//! - Trigger conditions
//! - Prevention mechanisms
//! - Regression test location
//! - Status (FIXED, NEEDS_TEST, UNKNOWN)

/// Top-level failure domains
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureDomain {
    /// NDK/compiler/toolchain failures
    NdkCompiler,
    /// Platform/environment failures  
    Platform,
    /// Memory management failures
    Memory,
    /// Code logic failures
    Logic,
    /// Inference/AI model failures
    Inference,
    /// Network connectivity failures
    Network,
}

/// Sub-category within a domain
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureCategory {
    // NdkCompiler subcategories
    SIGSEGV,
    LINKER_ERROR,
    COMPILER_CRASH,
    LTO_BUG,

    // Platform subcategories
    PERMISSION_DENIED,
    API_LEVEL,
    BIONIC_ALLOCATOR,

    // Memory subcategories
    OOM,
    MEMORY_LEAK,
    CORRUPTION,

    // Logic subcategories
    ETHICS_BYPASS,
    CONSENT_BYPASS,
    SCHEMA_MISMATCH,

    // Inference subcategories
    LLAMA_CPP_CRASH,
    MODEL_LOAD_FAILURE,
    INFERENCE_TIMEOUT,

    // Network subcategories
    CONNECTION_TIMEOUT,
    DNS_FAILURE,
}

/// Root cause classification
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RootCause {
    /// Root cause identified and fixed
    IdentifiedAndFixed,
    /// Root cause identified but no fix yet
    IdentifiedNoFix,
    /// Root cause unknown
    Unknown,
    /// Root cause being investigated
    UnderInvestigation,
    /// Regression test written
    RegressionTestWritten,
}

/// A single failure taxonomy entry
#[derive(Debug, Clone)]
pub struct FailureEntry {
    /// Unique ID like F001-SIGSEGV-AT-STARTUP
    pub id: &'static str,
    /// Human-readable name
    pub name: &'static str,
    /// Top-level domain
    pub domain: FailureDomain,
    /// Sub-category
    pub category: FailureCategory,
    /// Root cause status
    pub root_cause: RootCause,
    /// Trigger conditions
    pub trigger: &'static str,
    /// Prevention mechanism
    pub prevention: &'static str,
    /// Regression test file path
    pub regression_test: Option<&'static str>,
    /// Whether this is fixed
    pub is_fixed: bool,
    /// Additional notes
    pub notes: &'static str,
}

/// The full taxonomy registry
pub struct FailureTaxonomy;

impl FailureTaxonomy {
    /// Get all known failures
    pub fn all() -> Vec<FailureEntry> {
        vec![
            // === NDK COMPILER ===
            FailureEntry {
                id: "F001",
                name: "SIGSEGV at startup with NDK r26b",
                domain: FailureDomain::NdkCompiler,
                category: FailureCategory::SIGSEGV,
                root_cause: RootCause::IdentifiedAndFixed,
                trigger: "NDK r26b + lto=true + panic=abort causes SIGSEGV at startup",
                prevention: "CI checks Cargo.toml for lto=true + panic=abort combination; container tests with NDK r26b",
                regression_test: Some("infrastructure/tests/regression/test_ndk_lto.rs"),
                is_fixed: true,
                notes: "Fix: lto=thin + panic=unwind in Cargo.toml. Commit 128ed2e.",
            },
            FailureEntry {
                id: "F002",
                name: "Bionic allocator OOM on memory pressure",
                domain: FailureDomain::Memory,
                category: FailureCategory::OOM,
                root_cause: RootCause::IdentifiedNoFix,
                trigger: "bionic malloc returns NULL under memory pressure; Rust's GlobalAlloc interface doesn't handle this gracefully",
                prevention: "Container tests with --memory=512m simulate low-memory Android devices",
                regression_test: Some("infrastructure/tests/regression/test_memory_limits.rs"),
                is_fixed: false,
                notes: "Needs: custom allocator that handles bionic OOM, or increased memory limits in container",
            },
            FailureEntry {
                id: "F003",
                name: "Termux permission denied on startup",
                domain: FailureDomain::Platform,
                category: FailureCategory::PERMISSION_DENIED,
                root_cause: RootCause::Unknown,
                trigger: "AURA binary lacks execute permission or cannot read HOME directory",
                prevention: "Smoke test checks permissions; verify HOME is set",
                regression_test: Some("infrastructure/tests/regression/test_permissions.rs"),
                is_fixed: false,
                notes: "Needs: investigate Termux-specific permission model",
            },
            FailureEntry {
                id: "F004",
                name: "Reflection layer schema mismatch",
                domain: FailureDomain::Logic,
                category: FailureCategory::SCHEMA_MISMATCH,
                root_cause: RootCause::IdentifiedAndFixed,
                trigger: "prompts.rs outputs wrong schema, grammar.rs expects different format",
                prevention: "Schema validation tests in CI; grammar GBNF checked against prompt templates",
                regression_test: Some("crates/aura-daemon/tests/test_reflection_schema.rs"),
                is_fixed: true,
                notes: "Fix: prompts.rs now outputs {safe, correct, concerns, verdict}. Agent 2.",
            },
            FailureEntry {
                id: "F005",
                name: "Semantic similarity always returns 0.0",
                domain: FailureDomain::Logic,
                category: FailureCategory::SCHEMA_MISMATCH,
                root_cause: RootCause::IdentifiedAndFixed,
                trigger: "planner.rs compute_semantic_similarity was stub returning 0.0",
                prevention: "Unit tests verify similarity returns non-zero for similar strings",
                regression_test: Some("crates/aura-daemon/tests/test_semantic_similarity.rs"),
                is_fixed: true,
                notes: "Fix: LCS-based algorithm with Jaccard + subsequence similarity. Threshold 0.55. Agent 3.",
            },
            FailureEntry {
                id: "F006",
                name: "Ethics audit verdicts bypassable at high trust",
                domain: FailureDomain::Logic,
                category: FailureCategory::ETHICS_BYPASS,
                root_cause: RootCause::IdentifiedAndFixed,
                trigger: "ethics.rs downgraded Audit verdicts when trust_level > 0.6",
                prevention: "Unit test verifies Audit verdicts are never downgraded regardless of trust",
                regression_test: Some("crates/aura-daemon/tests/test_ethics_non_bypass.rs"),
                is_fixed: true,
                notes: "Fix: Removed downgrade logic. Audit verdicts are final per 7 Iron Laws. Agent 7.",
            },
            FailureEntry {
                id: "F007",
                name: "GDPR right to erasure incomplete",
                domain: FailureDomain::Logic,
                category: FailureCategory::SCHEMA_MISMATCH,
                root_cause: RootCause::IdentifiedAndFixed,
                trigger: "user_profile.rs only deleted from user_profile table, not memory tiers",
                prevention: "GDPR integration test verifies all 5 data tiers are cleared",
                regression_test: Some("crates/aura-daemon/tests/test_gdpr_complete.rs"),
                is_fixed: true,
                notes: "Fix: export_comprehensive(), delete_with_gdpr(), erase_all() on all memory tiers. Agent 5.",
            },
            FailureEntry {
                id: "F008",
                name: "LLama.cpp crash on model load",
                domain: FailureDomain::Inference,
                category: FailureCategory::LLAMA_CPP_CRASH,
                root_cause: RootCause::Unknown,
                trigger: "Model file incompatible with llama.cpp version or corrupt",
                prevention: "Smoke test loads model and verifies basic inference works",
                regression_test: Some("infrastructure/tests/regression/test_inference.rs"),
                is_fixed: false,
                notes: "Needs: Model version compatibility matrix + smoke test",
            },
        ]
    }

    /// Classify a new failure
    pub fn classify(domain: FailureDomain, category: FailureCategory) -> Option<FailureEntry> {
        Self::all()
            .into_iter()
            .find(|e| e.domain == domain && e.category == category)
    }

    /// Get all failures for a domain
    pub fn by_domain(domain: FailureDomain) -> Vec<FailureEntry> {
        Self::all()
            .into_iter()
            .filter(|e| e.domain == domain)
            .collect()
    }

    /// Get all fixed failures
    pub fn fixed() -> Vec<FailureEntry> {
        Self::all().into_iter().filter(|e| e.is_fixed).collect()
    }

    /// Get all unfixed failures
    pub fn unfixed() -> Vec<FailureEntry> {
        Self::all().into_iter().filter(|e| !e.is_fixed).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_all_failures_have_ids() {
        for entry in FailureTaxonomy::all() {
            assert!(!entry.id.is_empty(), "Entry {} missing ID", entry.name);
            assert!(
                entry.id.starts_with('F'),
                "ID {} should start with F",
                entry.id
            );
        }
    }

    #[test]
    fn test_f001_is_fixed() {
        let f001 = FailureTaxonomy::classify(FailureDomain::NdkCompiler, FailureCategory::SIGSEGV);
        assert!(f001.is_some());
        let f001 = f001.unwrap();
        assert!(f001.is_fixed);
        assert_eq!(f001.id, "F001");
    }

    #[test]
    fn test_domain_counts() {
        assert!(FailureTaxonomy::by_domain(FailureDomain::NdkCompiler).len() >= 1);
        assert!(FailureTaxonomy::by_domain(FailureDomain::Memory).len() >= 1);
        assert!(FailureTaxonomy::by_domain(FailureDomain::Logic).len() >= 4);
    }

    #[test]
    fn test_critical_failures_fixed() {
        // F001 (SIGSEGV) must be fixed before release
        let f001 = FailureTaxonomy::classify(FailureDomain::NdkCompiler, FailureCategory::SIGSEGV);
        assert!(
            f001.map(|e| e.is_fixed).unwrap_or(false),
            "F001 (SIGSEGV) must be fixed!"
        );
    }
}
