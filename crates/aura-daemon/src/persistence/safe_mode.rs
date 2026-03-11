//! Safe Mode — degraded operation when state integrity cannot be guaranteed.
//!
//! When the integrity verifier detects critical issues (personality corruption,
//! journal corruption, or unrecoverable state), the daemon enters safe mode:
//!
//! - **No proactive actions** — only responds to direct user commands.
//! - **No learning** — personality evolution and ARC learning are frozen.
//! - **User notification** — sends a Telegram message explaining the situation.
//! - **Logging** — all safe-mode decisions are logged for post-mortem analysis.
//!
//! Safe mode is NOT a crash — the daemon continues running but in a
//! conservative, read-only-ish posture until the user acknowledges or
//! a restart with clean state resolves the issues.

use crate::persistence::integrity::VerificationReport;

// ---------------------------------------------------------------------------
// SafeModeState
// ---------------------------------------------------------------------------

/// Tracks whether the daemon is in safe mode and why.
#[derive(Debug, Clone)]
pub struct SafeModeState {
    /// Whether safe mode is currently active.
    pub active: bool,
    /// Human-readable reason(s) for safe mode activation.
    pub reasons: Vec<String>,
    /// Timestamp when safe mode was activated (ms since UNIX epoch, 0 if inactive).
    pub activated_at_ms: u64,
    /// Whether the user has been notified via Telegram.
    pub user_notified: bool,
}

impl SafeModeState {
    /// Create an inactive safe mode state (normal operation).
    pub fn inactive() -> Self {
        Self {
            active: false,
            reasons: Vec::new(),
            activated_at_ms: 0,
            user_notified: false,
        }
    }

    /// Activate safe mode based on a verification report.
    ///
    /// Only activates if the report has critical issues.  Returns `true`
    /// if safe mode was activated (or was already active).
    pub fn activate_from_report(&mut self, report: &VerificationReport) -> bool {
        if !report.safe_mode_required {
            return self.active;
        }

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;

        self.active = true;
        if self.activated_at_ms == 0 {
            self.activated_at_ms = now;
        }

        // Collect critical issue messages.
        for issue in &report.issues {
            if issue.severity == crate::persistence::integrity::VerificationSeverity::Critical {
                let reason = format!("[{}] {}", issue.subsystem, issue.message);
                if !self.reasons.contains(&reason) {
                    self.reasons.push(reason);
                }
            }
        }

        tracing::warn!(
            reasons = ?self.reasons,
            "SAFE MODE ACTIVATED — no proactive actions, no learning"
        );

        true
    }

    /// Deactivate safe mode (called after user acknowledgement or clean restart).
    pub fn deactivate(&mut self) {
        if self.active {
            tracing::info!("safe mode deactivated — resuming normal operation");
        }
        self.active = false;
        self.reasons.clear();
        self.activated_at_ms = 0;
        self.user_notified = false;
    }

    /// Build the Telegram notification message for safe mode.
    ///
    /// Returns `None` if safe mode is not active or the user was already notified.
    pub fn notification_message(&self) -> Option<String> {
        if !self.active || self.user_notified {
            return None;
        }

        let mut msg = String::from(
            "⚠️ AURA Safe Mode Active\n\n\
             I detected state inconsistencies on startup and am running \
             in safe mode to protect your data.\n\n\
             What this means:\n\
             • I won't take proactive actions\n\
             • I won't learn from interactions\n\
             • I'll only respond to your direct commands\n\n\
             Issues detected:\n",
        );

        for (i, reason) in self.reasons.iter().enumerate() {
            msg.push_str(&format!("{}. {}\n", i + 1, reason));
        }

        msg.push_str(
            "\nThis usually resolves after a clean restart. \
             If it persists, some data may need to be reset.\n\n\
             I remember, therefore I become — and I'll protect \
             those memories with everything I have. 🛡️",
        );

        Some(msg)
    }

    /// Mark the user as notified (call after Telegram send succeeds).
    pub fn mark_notified(&mut self) {
        self.user_notified = true;
    }

    /// Check whether an action should be blocked in safe mode.
    ///
    /// In safe mode, proactive and learning actions are blocked.
    /// Direct user commands are always allowed.
    pub fn should_block_action(&self, is_proactive: bool, is_learning: bool) -> bool {
        if !self.active {
            return false;
        }
        is_proactive || is_learning
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::persistence::integrity::{
        VerificationIssue, VerificationReport, VerificationSeverity,
    };

    fn critical_report() -> VerificationReport {
        VerificationReport {
            issues: vec![VerificationIssue {
                severity: VerificationSeverity::Critical,
                subsystem: "journal",
                message: "corruption detected".to_string(),
            }],
            safe_mode_required: true,
            repairs_applied: false,
            critical_count: 1,
            warning_count: 0,
        }
    }

    fn clean_report() -> VerificationReport {
        VerificationReport {
            issues: vec![],
            safe_mode_required: false,
            repairs_applied: false,
            critical_count: 0,
            warning_count: 0,
        }
    }

    #[test]
    fn test_inactive_by_default() {
        let sm = SafeModeState::inactive();
        assert!(!sm.active);
        assert!(sm.reasons.is_empty());
    }

    #[test]
    fn test_activate_from_critical_report() {
        let mut sm = SafeModeState::inactive();
        let activated = sm.activate_from_report(&critical_report());
        assert!(activated);
        assert!(sm.active);
        assert!(!sm.reasons.is_empty());
    }

    #[test]
    fn test_does_not_activate_on_clean_report() {
        let mut sm = SafeModeState::inactive();
        let activated = sm.activate_from_report(&clean_report());
        assert!(!activated);
        assert!(!sm.active);
    }

    #[test]
    fn test_blocks_proactive_in_safe_mode() {
        let mut sm = SafeModeState::inactive();
        sm.activate_from_report(&critical_report());

        assert!(sm.should_block_action(true, false)); // proactive blocked
        assert!(sm.should_block_action(false, true)); // learning blocked
        assert!(!sm.should_block_action(false, false)); // direct cmd allowed
    }

    #[test]
    fn test_notification_message() {
        let mut sm = SafeModeState::inactive();
        sm.activate_from_report(&critical_report());

        let msg = sm.notification_message();
        assert!(msg.is_some());
        let text = msg.expect("notification present");
        assert!(text.contains("Safe Mode"));
        assert!(text.contains("corruption"));
    }

    #[test]
    fn test_no_double_notification() {
        let mut sm = SafeModeState::inactive();
        sm.activate_from_report(&critical_report());
        sm.mark_notified();

        assert!(sm.notification_message().is_none());
    }

    #[test]
    fn test_deactivate() {
        let mut sm = SafeModeState::inactive();
        sm.activate_from_report(&critical_report());
        sm.deactivate();

        assert!(!sm.active);
        assert!(sm.reasons.is_empty());
        assert!(!sm.should_block_action(true, false));
    }
}
