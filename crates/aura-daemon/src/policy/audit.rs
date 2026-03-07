//! Security Audit System — immutable, hash-chained action logging.
//!
//! Every action AURA takes is recorded in an append-only audit log with:
//! - Monotonic sequence numbers
//! - Hash chain integrity (each entry includes hash of previous)
//! - Risk-level classification
//! - Time-range and level-based query API
//! - JSON export for user review
//!
//! # Architecture
//!
//! ```text
//! PolicyGate::evaluate()──┐
//! Executor::run_action()──┼──► AuditLog::log_action() ──► AuditEntry
//! Sandbox::execute()──────┘           │
//!                              ┌──────┴──────┐
//!                              │ Ring Buffer  │
//!                              │  (10K entries│
//!                              │   in memory) │
//!                              └──────────────┘
//! ```
//!
//! # Hash Chain
//!
//! Each entry stores `prev_hash` — the hash of the preceding entry.
//! This forms a tamper-evident chain: modifying any past entry invalidates
//! all subsequent hashes.  Verification is O(n) over the in-memory buffer.
//!
//! # Note on Hash Function
//!
//! Uses `std::hash::DefaultHasher` (SipHash) for portability.
//! In a production deployment with forensic requirements, replace with
//! SHA-256 from the `sha2` crate.

use std::collections::VecDeque;
use std::hash::{Hash, Hasher};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use aura_types::errors::SecurityError;

use super::rules::RuleEffect;

// ---------------------------------------------------------------------------
// AuditLevel
// ---------------------------------------------------------------------------

/// Risk classification for audit entries.
///
/// Ordered from lowest to highest severity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum AuditLevel {
    /// Routine operations (navigation, reading).
    Info = 0,
    /// Operations that access non-trivial data.
    Elevated = 1,
    /// Operations touching personal data (contacts, messages).
    Sensitive = 2,
    /// Destructive or high-impact operations (uninstall, settings change).
    Critical = 3,
    /// Operations that must NEVER execute (factory reset, data wipe).
    Forbidden = 4,
}

impl AuditLevel {
    /// Parse from a numeric value.
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(Self::Info),
            1 => Some(Self::Elevated),
            2 => Some(Self::Sensitive),
            3 => Some(Self::Critical),
            4 => Some(Self::Forbidden),
            _ => None,
        }
    }
}

impl std::fmt::Display for AuditLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Info => write!(f, "INFO"),
            Self::Elevated => write!(f, "ELEVATED"),
            Self::Sensitive => write!(f, "SENSITIVE"),
            Self::Critical => write!(f, "CRITICAL"),
            Self::Forbidden => write!(f, "FORBIDDEN"),
        }
    }
}

// ---------------------------------------------------------------------------
// AuditEntry
// ---------------------------------------------------------------------------

/// A single, immutable audit log entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEntry {
    /// Monotonically increasing sequence number.
    pub seq: u64,
    /// Unix timestamp in milliseconds.
    pub timestamp_ms: u64,
    /// Human-readable action description (e.g. "tap(540, 1200)").
    pub action: String,
    /// Target of the action (e.g. package name, contact name, UI element).
    pub target: String,
    /// The policy decision result for this action.
    pub result: AuditResult,
    /// Risk classification.
    pub level: AuditLevel,
    /// Hash of the contextual state (screen hash, app foreground, etc.).
    pub context_hash: u64,
    /// Hash of the previous entry — forms the integrity chain.
    pub prev_hash: u64,
    /// Hash of THIS entry (computed after all other fields are set).
    pub entry_hash: u64,
}

/// The outcome recorded in an audit entry.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum AuditResult {
    /// Action was allowed and executed successfully.
    Allowed,
    /// Action was allowed but execution failed.
    Failed(String),
    /// Action was denied by policy.
    Denied(String),
    /// Action required confirmation.
    PendingConfirmation,
    /// Action was confirmed by user.
    Confirmed,
    /// Action was blocked by sandbox.
    Sandboxed(String),
    /// Action was halted by emergency stop.
    EmergencyStopped,
}

impl std::fmt::Display for AuditResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Allowed => write!(f, "allowed"),
            Self::Failed(r) => write!(f, "failed: {r}"),
            Self::Denied(r) => write!(f, "denied: {r}"),
            Self::PendingConfirmation => write!(f, "pending_confirmation"),
            Self::Confirmed => write!(f, "confirmed"),
            Self::Sandboxed(r) => write!(f, "sandboxed: {r}"),
            Self::EmergencyStopped => write!(f, "emergency_stopped"),
        }
    }
}

impl AuditEntry {
    /// Compute the hash of this entry (excluding `entry_hash` itself).
    fn compute_hash(&self) -> u64 {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        self.seq.hash(&mut hasher);
        self.timestamp_ms.hash(&mut hasher);
        self.action.hash(&mut hasher);
        self.target.hash(&mut hasher);
        format!("{}", self.result).hash(&mut hasher);
        (self.level as u8).hash(&mut hasher);
        self.context_hash.hash(&mut hasher);
        self.prev_hash.hash(&mut hasher);
        hasher.finish()
    }

    /// Verify that this entry's hash is correct.
    pub fn verify_integrity(&self) -> bool {
        self.entry_hash == self.compute_hash()
    }
}

// ---------------------------------------------------------------------------
// AuditLog
// ---------------------------------------------------------------------------

/// Default in-memory ring buffer capacity.
pub const DEFAULT_CAPACITY: usize = 10_000;

/// Append-only audit log with hash-chain integrity.
///
/// Maintains the last `capacity` entries in memory as a ring buffer.
/// When the buffer is full, the oldest entries are evicted (in production,
/// they should be flushed to SQLite before eviction).
pub struct AuditLog {
    /// In-memory ring buffer of entries.
    entries: VecDeque<AuditEntry>,
    /// Maximum entries to keep in memory.
    capacity: usize,
    /// Next sequence number to assign.
    next_seq: u64,
    /// Hash of the most recent entry (for chain continuity).
    last_hash: u64,
    /// Total entries ever recorded (including evicted).
    total_logged: u64,
    /// Total entries evicted from the ring buffer.
    total_evicted: u64,
    /// Whether the log is frozen (emergency stop can freeze auditing).
    frozen: bool,
}

impl AuditLog {
    /// Create a new audit log with the given capacity.
    pub fn new(capacity: usize) -> Self {
        Self {
            entries: VecDeque::with_capacity(capacity.min(DEFAULT_CAPACITY)),
            capacity,
            next_seq: 0,
            last_hash: 0,
            total_logged: 0,
            total_evicted: 0,
            frozen: false,
        }
    }

    /// Create a new audit log with default capacity (10,000 entries).
    pub fn with_default_capacity() -> Self {
        Self::new(DEFAULT_CAPACITY)
    }

    /// Log an action. This is the primary API called by PolicyGate, Executor, and Sandbox.
    ///
    /// Returns the sequence number of the new entry, or an error if the log is frozen.
    pub fn log_action(
        &mut self,
        action: &str,
        target: &str,
        result: AuditResult,
        level: AuditLevel,
        context_hash: u64,
    ) -> Result<u64, SecurityError> {
        if self.frozen {
            return Err(SecurityError::AuditCorrupted(
                "audit log is frozen".to_string(),
            ));
        }

        let seq = self.next_seq;
        let timestamp_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;

        let mut entry = AuditEntry {
            seq,
            timestamp_ms,
            action: action.to_string(),
            target: target.to_string(),
            result,
            level,
            context_hash,
            prev_hash: self.last_hash,
            entry_hash: 0, // computed below
        };

        entry.entry_hash = entry.compute_hash();

        tracing::info!(
            target: "SECURITY",
            seq = seq,
            action = action,
            level = %level,
            result = %entry.result,
            "audit: action logged"
        );

        // Evict oldest if at capacity.
        if self.entries.len() >= self.capacity {
            self.entries.pop_front();
            self.total_evicted += 1;
        }

        self.last_hash = entry.entry_hash;
        self.next_seq += 1;
        self.total_logged += 1;
        self.entries.push_back(entry);

        Ok(seq)
    }

    /// Log a policy decision.
    pub fn log_policy_decision(
        &mut self,
        action: &str,
        effect: &RuleEffect,
        reason: &str,
        context_hash: u64,
    ) -> Result<u64, SecurityError> {
        let (result, level) = match effect {
            RuleEffect::Allow => (AuditResult::Allowed, AuditLevel::Info),
            RuleEffect::Audit => (AuditResult::Allowed, AuditLevel::Elevated),
            RuleEffect::Confirm => (AuditResult::PendingConfirmation, AuditLevel::Sensitive),
            RuleEffect::Deny => (
                AuditResult::Denied(reason.to_string()),
                AuditLevel::Critical,
            ),
        };
        self.log_action(action, "policy_gate", result, level, context_hash)
    }

    /// Freeze the log — no new entries can be written.
    /// Used during emergency stop to preserve state.
    pub fn freeze(&mut self) {
        self.frozen = true;
        tracing::warn!(target: "SECURITY", "audit log FROZEN");
    }

    /// Unfreeze the log — resume accepting entries.
    pub fn unfreeze(&mut self) {
        self.frozen = false;
        tracing::info!(target: "SECURITY", "audit log UNFROZEN");
    }

    /// Whether the log is currently frozen.
    pub fn is_frozen(&self) -> bool {
        self.frozen
    }

    // -----------------------------------------------------------------------
    // Query API
    // -----------------------------------------------------------------------

    /// Return all entries in the in-memory buffer.
    pub fn entries(&self) -> &VecDeque<AuditEntry> {
        &self.entries
    }

    /// Number of entries currently in memory.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the in-memory buffer is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Total entries ever logged (including evicted).
    pub fn total_logged(&self) -> u64 {
        self.total_logged
    }

    /// Total entries evicted from the ring buffer.
    pub fn total_evicted(&self) -> u64 {
        self.total_evicted
    }

    /// Query entries within a time range (inclusive, unix ms).
    pub fn query_by_time_range(&self, from_ms: u64, to_ms: u64) -> Vec<&AuditEntry> {
        self.entries
            .iter()
            .filter(|e| e.timestamp_ms >= from_ms && e.timestamp_ms <= to_ms)
            .collect()
    }

    /// Query entries at or above a given audit level.
    pub fn query_by_min_level(&self, min_level: AuditLevel) -> Vec<&AuditEntry> {
        self.entries
            .iter()
            .filter(|e| e.level >= min_level)
            .collect()
    }

    /// Query entries matching an action substring (case-insensitive).
    pub fn query_by_action(&self, pattern: &str) -> Vec<&AuditEntry> {
        let lower = pattern.to_ascii_lowercase();
        self.entries
            .iter()
            .filter(|e| e.action.to_ascii_lowercase().contains(&lower))
            .collect()
    }

    /// Get the last N entries (most recent first).
    pub fn last_n(&self, n: usize) -> Vec<&AuditEntry> {
        self.entries.iter().rev().take(n).collect()
    }

    /// Get a specific entry by sequence number.
    pub fn get_by_seq(&self, seq: u64) -> Option<&AuditEntry> {
        self.entries.iter().find(|e| e.seq == seq)
    }

    /// Count entries by level in the current buffer.
    pub fn count_by_level(&self) -> AuditLevelCounts {
        let mut counts = AuditLevelCounts::default();
        for entry in &self.entries {
            match entry.level {
                AuditLevel::Info => counts.info += 1,
                AuditLevel::Elevated => counts.elevated += 1,
                AuditLevel::Sensitive => counts.sensitive += 1,
                AuditLevel::Critical => counts.critical += 1,
                AuditLevel::Forbidden => counts.forbidden += 1,
            }
        }
        counts
    }

    // -----------------------------------------------------------------------
    // Integrity verification
    // -----------------------------------------------------------------------

    /// Verify the hash chain integrity of all in-memory entries.
    ///
    /// Returns `Ok(())` if the chain is valid, or the index of the first
    /// tampered entry.
    pub fn verify_chain(&self) -> Result<(), SecurityError> {
        let mut expected_prev_hash: u64 = 0;

        // Find the prev_hash of the first in-memory entry.
        // If we have evicted entries, we cannot verify the link from
        // evicted → first-in-memory, so we start from the first entry's
        // own prev_hash.
        if let Some(first) = self.entries.front() {
            expected_prev_hash = first.prev_hash;
        }

        for entry in &self.entries {
            // Check chain link.
            if entry.prev_hash != expected_prev_hash {
                return Err(SecurityError::HashChainTampered { index: entry.seq });
            }

            // Check entry self-integrity.
            if !entry.verify_integrity() {
                return Err(SecurityError::HashChainTampered { index: entry.seq });
            }

            expected_prev_hash = entry.entry_hash;
        }

        Ok(())
    }

    // -----------------------------------------------------------------------
    // Export
    // -----------------------------------------------------------------------

    /// Export the entire in-memory log to JSON.
    pub fn export_json(&self) -> Result<String, SecurityError> {
        serde_json::to_string_pretty(&Vec::from_iter(self.entries.iter().cloned()))
            .map_err(|e| SecurityError::AuditCorrupted(format!("JSON export failed: {e}")))
    }

    /// Export a compact summary (for Telegram / notification).
    pub fn export_summary(&self) -> AuditSummary {
        let counts = self.count_by_level();
        let last_entry_seq = self.entries.back().map(|e| e.seq);
        let first_entry_seq = self.entries.front().map(|e| e.seq);

        AuditSummary {
            entries_in_memory: self.entries.len() as u64,
            total_logged: self.total_logged,
            total_evicted: self.total_evicted,
            first_seq: first_entry_seq,
            last_seq: last_entry_seq,
            counts,
            chain_valid: self.verify_chain().is_ok(),
        }
    }
}

// ---------------------------------------------------------------------------
// Supporting types
// ---------------------------------------------------------------------------

/// Counts of entries by audit level.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AuditLevelCounts {
    pub info: u64,
    pub elevated: u64,
    pub sensitive: u64,
    pub critical: u64,
    pub forbidden: u64,
}

/// Compact summary of the audit log state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditSummary {
    pub entries_in_memory: u64,
    pub total_logged: u64,
    pub total_evicted: u64,
    pub first_seq: Option<u64>,
    pub last_seq: Option<u64>,
    pub counts: AuditLevelCounts,
    pub chain_valid: bool,
}

// ---------------------------------------------------------------------------
// Utility: classify action risk level
// ---------------------------------------------------------------------------

/// Classify the risk level of an action string.
///
/// This is a heuristic based on keyword matching. PolicyGate rules take
/// precedence — this function provides a fallback classification for
/// actions that bypass the gate.
pub fn classify_action_risk(action: &str) -> AuditLevel {
    let lower = action.to_ascii_lowercase();

    // Forbidden patterns
    if lower.contains("factory reset")
        || lower.contains("wipe data")
        || lower.contains("format")
        || lower.contains("brick")
    {
        return AuditLevel::Forbidden;
    }

    // Critical patterns
    if lower.contains("uninstall")
        || lower.contains("delete")
        || lower.contains("root")
        || lower.contains("su ")
        || lower.contains("settings change")
        || lower.contains("permission grant")
    {
        return AuditLevel::Critical;
    }

    // Sensitive patterns
    if lower.contains("contact")
        || lower.contains("message")
        || lower.contains("sms")
        || lower.contains("call")
        || lower.contains("photo")
        || lower.contains("credential")
        || lower.contains("password")
        || lower.contains("payment")
        || lower.contains("bank")
    {
        return AuditLevel::Sensitive;
    }

    // Elevated patterns
    if lower.contains("install")
        || lower.contains("download")
        || lower.contains("open app")
        || lower.contains("notification")
    {
        return AuditLevel::Elevated;
    }

    AuditLevel::Info
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn make_log(cap: usize) -> AuditLog {
        AuditLog::new(cap)
    }

    #[test]
    fn test_create_default_capacity() {
        let log = AuditLog::with_default_capacity();
        assert_eq!(log.capacity, DEFAULT_CAPACITY);
        assert!(log.is_empty());
        assert_eq!(log.total_logged(), 0);
    }

    #[test]
    fn test_log_single_action() {
        let mut log = make_log(100);
        let seq = log
            .log_action(
                "tap(100, 200)",
                "screen",
                AuditResult::Allowed,
                AuditLevel::Info,
                0,
            )
            .unwrap();
        assert_eq!(seq, 0);
        assert_eq!(log.len(), 1);
        assert_eq!(log.total_logged(), 1);
    }

    #[test]
    fn test_log_multiple_actions() {
        let mut log = make_log(100);
        for i in 0..10 {
            let seq = log
                .log_action(
                    &format!("action_{i}"),
                    "test",
                    AuditResult::Allowed,
                    AuditLevel::Info,
                    i as u64,
                )
                .unwrap();
            assert_eq!(seq, i as u64);
        }
        assert_eq!(log.len(), 10);
        assert_eq!(log.total_logged(), 10);
    }

    #[test]
    fn test_ring_buffer_eviction() {
        let mut log = make_log(5);
        for i in 0..10 {
            log.log_action(
                &format!("action_{i}"),
                "test",
                AuditResult::Allowed,
                AuditLevel::Info,
                0,
            )
            .unwrap();
        }
        assert_eq!(log.len(), 5);
        assert_eq!(log.total_logged(), 10);
        assert_eq!(log.total_evicted(), 5);
        // Oldest remaining should be seq 5.
        assert_eq!(log.entries().front().unwrap().seq, 5);
    }

    #[test]
    fn test_hash_chain_integrity() {
        let mut log = make_log(100);
        for i in 0..5 {
            log.log_action(
                &format!("action_{i}"),
                "test",
                AuditResult::Allowed,
                AuditLevel::Info,
                0,
            )
            .unwrap();
        }
        assert!(log.verify_chain().is_ok());
    }

    #[test]
    fn test_hash_chain_detects_tampering() {
        let mut log = make_log(100);
        for i in 0..5 {
            log.log_action(
                &format!("action_{i}"),
                "test",
                AuditResult::Allowed,
                AuditLevel::Info,
                0,
            )
            .unwrap();
        }

        // Tamper with the middle entry.
        if let Some(entry) = log.entries.get_mut(2) {
            entry.action = "TAMPERED".to_string();
        }

        assert!(log.verify_chain().is_err());
    }

    #[test]
    fn test_entry_self_integrity() {
        let mut log = make_log(100);
        log.log_action("test", "target", AuditResult::Allowed, AuditLevel::Info, 42)
            .unwrap();
        let entry = log.entries().front().unwrap();
        assert!(entry.verify_integrity());
    }

    #[test]
    fn test_query_by_min_level() {
        let mut log = make_log(100);
        log.log_action("nav", "screen", AuditResult::Allowed, AuditLevel::Info, 0)
            .unwrap();
        log.log_action(
            "read contacts",
            "contacts",
            AuditResult::Allowed,
            AuditLevel::Sensitive,
            0,
        )
        .unwrap();
        log.log_action(
            "delete app",
            "settings",
            AuditResult::Denied("blocked".into()),
            AuditLevel::Critical,
            0,
        )
        .unwrap();

        let sensitive_plus = log.query_by_min_level(AuditLevel::Sensitive);
        assert_eq!(sensitive_plus.len(), 2);

        let critical_plus = log.query_by_min_level(AuditLevel::Critical);
        assert_eq!(critical_plus.len(), 1);
    }

    #[test]
    fn test_query_by_action() {
        let mut log = make_log(100);
        log.log_action(
            "tap(100, 200)",
            "screen",
            AuditResult::Allowed,
            AuditLevel::Info,
            0,
        )
        .unwrap();
        log.log_action(
            "swipe up",
            "screen",
            AuditResult::Allowed,
            AuditLevel::Info,
            0,
        )
        .unwrap();
        log.log_action(
            "tap(300, 400)",
            "screen",
            AuditResult::Allowed,
            AuditLevel::Info,
            0,
        )
        .unwrap();

        let taps = log.query_by_action("tap");
        assert_eq!(taps.len(), 2);
    }

    #[test]
    fn test_last_n() {
        let mut log = make_log(100);
        for i in 0..5 {
            log.log_action(
                &format!("action_{i}"),
                "test",
                AuditResult::Allowed,
                AuditLevel::Info,
                0,
            )
            .unwrap();
        }
        let last3 = log.last_n(3);
        assert_eq!(last3.len(), 3);
        assert_eq!(last3[0].seq, 4); // Most recent first.
        assert_eq!(last3[1].seq, 3);
        assert_eq!(last3[2].seq, 2);
    }

    #[test]
    fn test_get_by_seq() {
        let mut log = make_log(100);
        for i in 0..5 {
            log.log_action(
                &format!("action_{i}"),
                "test",
                AuditResult::Allowed,
                AuditLevel::Info,
                0,
            )
            .unwrap();
        }
        let entry = log.get_by_seq(3).unwrap();
        assert_eq!(entry.action, "action_3");

        assert!(log.get_by_seq(999).is_none());
    }

    #[test]
    fn test_freeze_blocks_logging() {
        let mut log = make_log(100);
        log.log_action("ok", "test", AuditResult::Allowed, AuditLevel::Info, 0)
            .unwrap();

        log.freeze();
        assert!(log.is_frozen());

        let result = log.log_action("blocked", "test", AuditResult::Allowed, AuditLevel::Info, 0);
        assert!(result.is_err());
        assert_eq!(log.len(), 1); // No new entry.
    }

    #[test]
    fn test_unfreeze_resumes_logging() {
        let mut log = make_log(100);
        log.freeze();
        assert!(log
            .log_action("x", "t", AuditResult::Allowed, AuditLevel::Info, 0)
            .is_err());

        log.unfreeze();
        assert!(!log.is_frozen());
        assert!(log
            .log_action("y", "t", AuditResult::Allowed, AuditLevel::Info, 0)
            .is_ok());
    }

    #[test]
    fn test_log_policy_decision_allow() {
        let mut log = make_log(100);
        log.log_policy_decision("navigate_home", &RuleEffect::Allow, "safe", 0)
            .unwrap();
        let entry = log.entries().front().unwrap();
        assert_eq!(entry.level, AuditLevel::Info);
        assert_eq!(entry.result, AuditResult::Allowed);
    }

    #[test]
    fn test_log_policy_decision_deny() {
        let mut log = make_log(100);
        log.log_policy_decision("factory reset", &RuleEffect::Deny, "destructive", 0)
            .unwrap();
        let entry = log.entries().front().unwrap();
        assert_eq!(entry.level, AuditLevel::Critical);
        assert!(matches!(entry.result, AuditResult::Denied(_)));
    }

    #[test]
    fn test_log_policy_decision_confirm() {
        let mut log = make_log(100);
        log.log_policy_decision("install app", &RuleEffect::Confirm, "needs confirmation", 0)
            .unwrap();
        let entry = log.entries().front().unwrap();
        assert_eq!(entry.level, AuditLevel::Sensitive);
        assert_eq!(entry.result, AuditResult::PendingConfirmation);
    }

    #[test]
    fn test_export_json() {
        let mut log = make_log(100);
        log.log_action("tap(0,0)", "s", AuditResult::Allowed, AuditLevel::Info, 0)
            .unwrap();
        let json = log.export_json().unwrap();
        assert!(json.contains("tap(0,0)"));
        assert!(json.contains("\"seq\": 0"));
    }

    #[test]
    fn test_export_summary() {
        let mut log = make_log(100);
        log.log_action("a", "t", AuditResult::Allowed, AuditLevel::Info, 0)
            .unwrap();
        log.log_action(
            "b",
            "t",
            AuditResult::Denied("x".into()),
            AuditLevel::Critical,
            0,
        )
        .unwrap();

        let summary = log.export_summary();
        assert_eq!(summary.entries_in_memory, 2);
        assert_eq!(summary.total_logged, 2);
        assert!(summary.chain_valid);
        assert_eq!(summary.counts.info, 1);
        assert_eq!(summary.counts.critical, 1);
    }

    #[test]
    fn test_count_by_level() {
        let mut log = make_log(100);
        log.log_action("a", "t", AuditResult::Allowed, AuditLevel::Info, 0)
            .unwrap();
        log.log_action("b", "t", AuditResult::Allowed, AuditLevel::Info, 0)
            .unwrap();
        log.log_action("c", "t", AuditResult::Allowed, AuditLevel::Sensitive, 0)
            .unwrap();
        log.log_action("d", "t", AuditResult::Allowed, AuditLevel::Forbidden, 0)
            .unwrap();

        let counts = log.count_by_level();
        assert_eq!(counts.info, 2);
        assert_eq!(counts.sensitive, 1);
        assert_eq!(counts.forbidden, 1);
        assert_eq!(counts.elevated, 0);
    }

    #[test]
    fn test_classify_action_risk_forbidden() {
        assert_eq!(
            classify_action_risk("perform factory reset"),
            AuditLevel::Forbidden
        );
        assert_eq!(classify_action_risk("wipe data now"), AuditLevel::Forbidden);
    }

    #[test]
    fn test_classify_action_risk_critical() {
        assert_eq!(
            classify_action_risk("uninstall com.example"),
            AuditLevel::Critical
        );
        assert_eq!(
            classify_action_risk("delete all files"),
            AuditLevel::Critical
        );
    }

    #[test]
    fn test_classify_action_risk_sensitive() {
        assert_eq!(classify_action_risk("read contacts"), AuditLevel::Sensitive);
        assert_eq!(
            classify_action_risk("send sms to 555-1234"),
            AuditLevel::Sensitive
        );
        assert_eq!(
            classify_action_risk("access password store"),
            AuditLevel::Sensitive
        );
    }

    #[test]
    fn test_classify_action_risk_elevated() {
        assert_eq!(
            classify_action_risk("install app com.game"),
            AuditLevel::Elevated
        );
        assert_eq!(
            classify_action_risk("open app settings"),
            AuditLevel::Elevated
        );
    }

    #[test]
    fn test_classify_action_risk_info() {
        assert_eq!(classify_action_risk("tap(100, 200)"), AuditLevel::Info);
        assert_eq!(classify_action_risk("swipe up"), AuditLevel::Info);
        assert_eq!(classify_action_risk("navigate home"), AuditLevel::Info);
    }

    #[test]
    fn test_audit_level_ordering() {
        assert!(AuditLevel::Info < AuditLevel::Elevated);
        assert!(AuditLevel::Elevated < AuditLevel::Sensitive);
        assert!(AuditLevel::Sensitive < AuditLevel::Critical);
        assert!(AuditLevel::Critical < AuditLevel::Forbidden);
    }

    #[test]
    fn test_audit_level_display() {
        assert_eq!(AuditLevel::Info.to_string(), "INFO");
        assert_eq!(AuditLevel::Critical.to_string(), "CRITICAL");
        assert_eq!(AuditLevel::Forbidden.to_string(), "FORBIDDEN");
    }

    #[test]
    fn test_audit_level_from_u8() {
        assert_eq!(AuditLevel::from_u8(0), Some(AuditLevel::Info));
        assert_eq!(AuditLevel::from_u8(4), Some(AuditLevel::Forbidden));
        assert_eq!(AuditLevel::from_u8(5), None);
    }

    #[test]
    fn test_audit_result_display() {
        assert_eq!(AuditResult::Allowed.to_string(), "allowed");
        assert_eq!(AuditResult::Denied("bad".into()).to_string(), "denied: bad");
        assert_eq!(
            AuditResult::EmergencyStopped.to_string(),
            "emergency_stopped"
        );
    }

    #[test]
    fn test_chain_valid_after_eviction() {
        let mut log = make_log(3);
        for i in 0..6 {
            log.log_action(
                &format!("a{i}"),
                "t",
                AuditResult::Allowed,
                AuditLevel::Info,
                0,
            )
            .unwrap();
        }
        // After eviction, chain within remaining entries should still hold.
        assert!(log.verify_chain().is_ok());
    }

    #[test]
    fn test_empty_log_chain_valid() {
        let log = make_log(100);
        assert!(log.verify_chain().is_ok());
    }

    #[test]
    fn test_sequential_hashes_differ() {
        let mut log = make_log(100);
        log.log_action("a", "t", AuditResult::Allowed, AuditLevel::Info, 0)
            .unwrap();
        log.log_action("b", "t", AuditResult::Allowed, AuditLevel::Info, 0)
            .unwrap();
        let h1 = log.entries()[0].entry_hash;
        let h2 = log.entries()[1].entry_hash;
        assert_ne!(h1, h2);
    }

    #[test]
    fn test_prev_hash_chain_links() {
        let mut log = make_log(100);
        log.log_action("a", "t", AuditResult::Allowed, AuditLevel::Info, 0)
            .unwrap();
        log.log_action("b", "t", AuditResult::Allowed, AuditLevel::Info, 0)
            .unwrap();
        log.log_action("c", "t", AuditResult::Allowed, AuditLevel::Info, 0)
            .unwrap();

        assert_eq!(log.entries()[0].prev_hash, 0); // Genesis entry.
        assert_eq!(log.entries()[1].prev_hash, log.entries()[0].entry_hash);
        assert_eq!(log.entries()[2].prev_hash, log.entries()[1].entry_hash);
    }

    #[test]
    fn test_query_by_time_range() {
        let mut log = make_log(100);
        // Log two entries — they'll have similar timestamps.
        log.log_action("a", "t", AuditResult::Allowed, AuditLevel::Info, 0)
            .unwrap();
        log.log_action("b", "t", AuditResult::Allowed, AuditLevel::Info, 0)
            .unwrap();

        // Query a wide range — should include both.
        let results = log.query_by_time_range(0, u64::MAX);
        assert_eq!(results.len(), 2);

        // Query a range that excludes everything.
        let results = log.query_by_time_range(0, 0);
        assert!(results.is_empty());
    }
}
