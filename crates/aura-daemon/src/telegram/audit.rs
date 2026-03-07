//! Audit logging for Telegram bot commands.
//!
//! Layer 5 of the security system. Every command attempt — successful or
//! denied — is recorded with a timestamp, chat ID, command summary, and
//! outcome. The log is backed by an in-memory ring buffer with optional
//! SQLite persistence.

use std::collections::VecDeque;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tracing::instrument;

// ─── Types ──────────────────────────────────────────────────────────────────

/// Outcome of a security check / command execution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AuditOutcome {
    /// Command was allowed and executed successfully.
    Allowed,
    /// Command was denied by security gate.
    Denied(String),
    /// Command was allowed but execution failed.
    Failed(String),
}

impl std::fmt::Display for AuditOutcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Allowed => write!(f, "ALLOWED"),
            Self::Denied(reason) => write!(f, "DENIED: {reason}"),
            Self::Failed(reason) => write!(f, "FAILED: {reason}"),
        }
    }
}

/// A single audit log entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEntry {
    /// Monotonically increasing sequence number.
    pub seq: u64,
    /// Unix timestamp (seconds).
    pub timestamp: u64,
    /// Telegram chat ID of the requester.
    pub chat_id: i64,
    /// Summary of the command (e.g., "/status", "/do open whatsapp").
    /// Sensitive arguments (like PINs) are redacted.
    pub command_summary: String,
    /// Result of the security check and execution.
    pub outcome: AuditOutcome,
}

// ─── AuditLog ───────────────────────────────────────────────────────────────

/// In-memory audit log with a fixed-size ring buffer.
///
/// Oldest entries are evicted when the capacity is exceeded.
pub struct AuditLog {
    entries: VecDeque<AuditEntry>,
    capacity: usize,
    next_seq: u64,
}

impl AuditLog {
    /// Create a new audit log with the given capacity.
    pub fn new(capacity: usize) -> Self {
        Self {
            entries: VecDeque::with_capacity(capacity.min(8192)),
            capacity,
            next_seq: 0,
        }
    }

    /// Record a new audit entry.
    ///
    /// Sensitive data (PINs, tokens) must be redacted by the caller.
    #[instrument(skip(self), fields(seq = self.next_seq, chat_id, outcome = %outcome))]
    pub fn record(&mut self, chat_id: i64, command_summary: &str, outcome: AuditOutcome) -> u64 {
        let seq = self.next_seq;
        self.next_seq += 1;

        let entry = AuditEntry {
            seq,
            timestamp: unix_timestamp(),
            chat_id,
            command_summary: command_summary.to_string(),
            outcome,
        };

        if self.entries.len() >= self.capacity {
            self.entries.pop_front();
        }
        self.entries.push_back(entry);

        seq
    }

    /// Get the last `n` entries, most recent first.
    pub fn last_n(&self, n: usize) -> Vec<&AuditEntry> {
        self.entries.iter().rev().take(n).collect()
    }

    /// Get all entries for a given chat ID, most recent first.
    pub fn by_chat_id(&self, chat_id: i64) -> Vec<&AuditEntry> {
        self.entries
            .iter()
            .rev()
            .filter(|e| e.chat_id == chat_id)
            .collect()
    }

    /// Get all denied entries, most recent first.
    pub fn denied(&self) -> Vec<&AuditEntry> {
        self.entries
            .iter()
            .rev()
            .filter(|e| matches!(e.outcome, AuditOutcome::Denied(_)))
            .collect()
    }

    /// Total entries currently held.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the log is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Clear all entries.
    pub fn clear(&mut self) {
        self.entries.clear();
    }

    /// Format the last `n` entries as a human-readable string.
    pub fn format_last_n(&self, n: usize) -> String {
        let entries = self.last_n(n);
        if entries.is_empty() {
            return "No audit entries.".to_string();
        }

        let mut out = String::with_capacity(entries.len() * 80);
        for entry in &entries {
            out.push_str(&format!(
                "#{} [{}] chat:{} {} -> {}\n",
                entry.seq,
                format_timestamp(entry.timestamp),
                entry.chat_id,
                entry.command_summary,
                entry.outcome,
            ));
        }
        out
    }
}

impl std::fmt::Debug for AuditLog {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AuditLog")
            .field("entries", &self.entries.len())
            .field("capacity", &self.capacity)
            .field("next_seq", &self.next_seq)
            .finish()
    }
}

// ─── Helpers ────────────────────────────────────────────────────────────────

fn unix_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn format_timestamp(ts: u64) -> String {
    // Simple HH:MM:SS from unix timestamp (UTC).
    let secs = ts % 86400;
    let h = secs / 3600;
    let m = (secs % 3600) / 60;
    let s = secs % 60;
    format!("{h:02}:{m:02}:{s:02}")
}

/// Redact sensitive arguments from a command string.
///
/// Replaces PIN values and tokens with `***`.
pub fn redact_sensitive(command: &str) -> String {
    // Redact /pin set <value> → "/pin set ***" (3-token command, PIN is 3rd token).
    if command.starts_with("/pin set ") {
        let parts: Vec<&str> = command.splitn(3, ' ').collect();
        if parts.len() >= 3 {
            return format!("{} {} ***", parts[0], parts[1]);
        }
    }
    // Redact /unlock <value> → "/unlock ***" (2-token command, PIN is 2nd token).
    if command.starts_with("/unlock ") {
        let parts: Vec<&str> = command.splitn(2, ' ').collect();
        if parts.len() >= 2 {
            return format!("{} ***", parts[0]);
        }
    }
    command.to_string()
}

// ─── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_record_and_retrieve() {
        let mut log = AuditLog::new(100);
        let seq = log.record(42, "/status", AuditOutcome::Allowed);
        assert_eq!(seq, 0);
        assert_eq!(log.len(), 1);

        let entries = log.last_n(10);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].chat_id, 42);
        assert_eq!(entries[0].command_summary, "/status");
    }

    #[test]
    fn test_capacity_eviction() {
        let mut log = AuditLog::new(3);
        log.record(1, "/a", AuditOutcome::Allowed);
        log.record(2, "/b", AuditOutcome::Allowed);
        log.record(3, "/c", AuditOutcome::Allowed);
        assert_eq!(log.len(), 3);

        // This should evict the oldest (/a).
        log.record(4, "/d", AuditOutcome::Allowed);
        assert_eq!(log.len(), 3);

        let entries = log.last_n(10);
        assert_eq!(entries[0].command_summary, "/d");
        assert_eq!(entries[2].command_summary, "/b");
    }

    #[test]
    fn test_by_chat_id() {
        let mut log = AuditLog::new(100);
        log.record(1, "/status", AuditOutcome::Allowed);
        log.record(2, "/ask foo", AuditOutcome::Allowed);
        log.record(1, "/health", AuditOutcome::Allowed);

        let entries = log.by_chat_id(1);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].command_summary, "/health");
    }

    #[test]
    fn test_denied_filter() {
        let mut log = AuditLog::new(100);
        log.record(1, "/status", AuditOutcome::Allowed);
        log.record(2, "/restart", AuditOutcome::Denied("unauthorized".into()));

        let denied = log.denied();
        assert_eq!(denied.len(), 1);
        assert_eq!(denied[0].chat_id, 2);
    }

    #[test]
    fn test_redact_sensitive() {
        assert_eq!(redact_sensitive("/pin set 1234"), "/pin set ***");
        assert_eq!(redact_sensitive("/unlock 5678"), "/unlock ***");
        assert_eq!(redact_sensitive("/status"), "/status");
    }

    #[test]
    fn test_format_last_n() {
        let mut log = AuditLog::new(100);
        log.record(42, "/status", AuditOutcome::Allowed);
        let formatted = log.format_last_n(10);
        assert!(formatted.contains("#0"));
        assert!(formatted.contains("chat:42"));
        assert!(formatted.contains("/status"));
        assert!(formatted.contains("ALLOWED"));
    }
}
