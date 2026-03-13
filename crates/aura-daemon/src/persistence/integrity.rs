//! Startup integrity verification — validates personality, trust, memory, and
//! goal state are consistent after journal recovery.
//!
//! Run `IntegrityVerifier::full_verification()` after journal replay to detect
//! corruption, drift anomalies, or missing state before the daemon enters its
//! main event loop.

use crate::identity::personality::{ConsistencyReport, Personality};
use crate::identity::RelationshipTracker;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum number of verification issues retained in a single report.
/// Prevents unbounded Vec growth when many users or traits are checked.
const MAX_VERIFICATION_ISSUES: usize = 256;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Push a `VerificationIssue` only if the cap has not been reached.
#[inline]
fn push_issue(issues: &mut Vec<VerificationIssue>, issue: VerificationIssue) {
    if issues.len() < MAX_VERIFICATION_ISSUES {
        issues.push(issue);
    }
}

// ---------------------------------------------------------------------------
// VerificationSeverity
// ---------------------------------------------------------------------------

/// How bad an integrity issue is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum VerificationSeverity {
    /// Informational — no action needed.
    Info,
    /// Something unusual — log and continue.
    Warning,
    /// State may be compromised — enter safe mode.
    Critical,
}

// ---------------------------------------------------------------------------
// VerificationIssue
// ---------------------------------------------------------------------------

/// A single integrity issue found during verification.
#[derive(Debug, Clone)]
pub struct VerificationIssue {
    pub severity: VerificationSeverity,
    pub subsystem: &'static str,
    pub message: String,
}

// ---------------------------------------------------------------------------
// VerificationReport
// ---------------------------------------------------------------------------

/// Aggregate result of a full integrity check.
#[derive(Debug, Clone)]
pub struct VerificationReport {
    /// All issues found, sorted by severity (Critical first).
    pub issues: Vec<VerificationIssue>,
    /// Whether the daemon should enter safe mode.
    pub safe_mode_required: bool,
    /// Whether any data was automatically repaired.
    pub repairs_applied: bool,
    /// Number of critical issues.
    pub critical_count: usize,
    /// Number of warnings.
    pub warning_count: usize,
}

impl VerificationReport {
    /// Returns `true` if no issues were found.
    pub fn is_clean(&self) -> bool {
        self.issues.is_empty()
    }
}

// ---------------------------------------------------------------------------
// IntegrityVerifier
// ---------------------------------------------------------------------------

/// Verifies the consistency and integrity of all identity-critical state
/// after journal recovery and before the daemon enters normal operation.
pub struct IntegrityVerifier;

impl IntegrityVerifier {
    /// Run all verification checks and produce a consolidated report.
    ///
    /// This is the single entry point called from daemon init.  Individual
    /// verify methods are `pub` for unit testing but callers should prefer
    /// this method.
    pub fn full_verification(
        personality: &Personality,
        relationships: &RelationshipTracker,
        goal_count: usize,
        journal_recovered: bool,
        journal_corruption: bool,
    ) -> VerificationReport {
        let mut issues = Vec::new();

        // 1. Personality consistency
        Self::verify_personality(personality, &mut issues);

        // 2. Trust / relationship sanity
        Self::verify_trust(relationships, &mut issues);

        // 3. Goal state sanity
        Self::verify_goals(goal_count, &mut issues);

        // 4. Journal health
        Self::verify_journal_state(journal_recovered, journal_corruption, &mut issues);

        // Sort: Critical first, then Warning, then Info.
        issues.sort_by(|a, b| b.severity.cmp(&a.severity));

        let critical_count = issues
            .iter()
            .filter(|i| i.severity == VerificationSeverity::Critical)
            .count();
        let warning_count = issues
            .iter()
            .filter(|i| i.severity == VerificationSeverity::Warning)
            .count();

        let safe_mode_required = critical_count > 0;

        VerificationReport {
            issues,
            safe_mode_required,
            repairs_applied: false,
            critical_count,
            warning_count,
        }
    }

    /// Check that OCEAN traits are within valid bounds and consistent.
    pub fn verify_personality(
        personality: &Personality,
        issues: &mut Vec<VerificationIssue>,
    ) {
        let traits = &personality.traits;

        // Check bounds: all traits must be in [0.1, 0.9].
        let check_bound = |name: &str, val: f32, issues: &mut Vec<VerificationIssue>| {
            if val < 0.0 || val > 1.0 {
                push_issue(issues, VerificationIssue {
                    severity: VerificationSeverity::Critical,
                    subsystem: "personality",
                    message: format!("{name} out of valid range: {val:.4} (expected [0.0, 1.0])"),
                });
            } else if val < 0.1 || val > 0.9 {
                push_issue(issues, VerificationIssue {
                    severity: VerificationSeverity::Warning,
                    subsystem: "personality",
                    message: format!("{name} outside safe range: {val:.4} (expected [0.1, 0.9])"),
                });
            }
        };

        check_bound("openness", traits.openness, issues);
        check_bound("conscientiousness", traits.conscientiousness, issues);
        check_bound("extraversion", traits.extraversion, issues);
        check_bound("agreeableness", traits.agreeableness, issues);
        check_bound("neuroticism", traits.neuroticism, issues);

        // Check NaN/Inf — critical data corruption indicator.
        let check_finite = |name: &str, val: f32, issues: &mut Vec<VerificationIssue>| {
            if !val.is_finite() {
                push_issue(issues, VerificationIssue {
                    severity: VerificationSeverity::Critical,
                    subsystem: "personality",
                    message: format!("{name} is not finite: {val}"),
                });
            }
        };

        check_finite("openness", traits.openness, issues);
        check_finite("conscientiousness", traits.conscientiousness, issues);
        check_finite("extraversion", traits.extraversion, issues);
        check_finite("agreeableness", traits.agreeableness, issues);
        check_finite("neuroticism", traits.neuroticism, issues);

        // Use Personality's own consistency check.
        let report: ConsistencyReport = personality.consistency_check();
        if !report.is_consistent {
            let severity = if report.has_extreme_drift {
                VerificationSeverity::Critical
            } else {
                VerificationSeverity::Warning
            };

            for issue_msg in &report.issues {
                push_issue(issues, VerificationIssue {
                    severity,
                    subsystem: "personality",
                    message: issue_msg.clone(),
                });
            }
        }
    }

    /// Check that trust values are within valid bounds.
    pub fn verify_trust(
        relationships: &RelationshipTracker,
        issues: &mut Vec<VerificationIssue>,
    ) {
        let all_users = relationships.all_user_ids();

        for user_id in &all_users {
            if let Some(rel) = relationships.get_relationship(user_id) {
                // Trust must be in [0.0, 1.0].
                if !rel.trust.is_finite() {
                    push_issue(issues, VerificationIssue {
                        severity: VerificationSeverity::Critical,
                        subsystem: "trust",
                        message: format!("user '{user_id}' has non-finite trust: {}", rel.trust),
                    });
                } else if rel.trust < 0.0 || rel.trust > 1.0 {
                    push_issue(issues, VerificationIssue {
                        severity: VerificationSeverity::Warning,
                        subsystem: "trust",
                        message: format!(
                            "user '{user_id}' trust out of range: {:.4}",
                            rel.trust
                        ),
                    });
                }
            }
        }

        if all_users.len() > 500 {
            push_issue(issues, VerificationIssue {
                severity: VerificationSeverity::Warning,
                subsystem: "trust",
                message: format!(
                    "tracking {} users exceeds recommended max of 500",
                    all_users.len()
                ),
            });
        }
    }

    /// Check that goal count is within bounds.
    pub fn verify_goals(
        goal_count: usize,
        issues: &mut Vec<VerificationIssue>,
    ) {
        if goal_count > 64 {
            push_issue(issues, VerificationIssue {
                severity: VerificationSeverity::Warning,
                subsystem: "goals",
                message: format!(
                    "active goal count ({goal_count}) exceeds max 64 — possible state leak"
                ),
            });
        }
    }

    /// Check journal recovery health.
    fn verify_journal_state(
        journal_recovered: bool,
        journal_corruption: bool,
        issues: &mut Vec<VerificationIssue>,
    ) {
        if journal_corruption {
            push_issue(issues, VerificationIssue {
                severity: VerificationSeverity::Critical,
                subsystem: "journal",
                message: "journal corruption detected during recovery — \
                          some state may have been lost"
                    .to_string(),
            });
        }

        if !journal_recovered {
            push_issue(issues, VerificationIssue {
                severity: VerificationSeverity::Info,
                subsystem: "journal",
                message: "no journal found — starting with defaults".to_string(),
            });
        }
    }

    /// Attempt automatic repairs for known fixable issues.
    ///
    /// Returns `true` if any repairs were applied.  Currently supports:
    /// - Re-clamping personality traits to [0.1, 0.9] range.
    pub fn repair_if_needed(
        personality: &mut Personality,
        report: &VerificationReport,
    ) -> bool {
        let mut repaired = false;

        for issue in &report.issues {
            if issue.subsystem == "personality" && issue.severity >= VerificationSeverity::Warning {
                // If traits are out of range, clamp them.
                let traits = &personality.traits;
                if traits.openness < 0.1
                    || traits.openness > 0.9
                    || traits.conscientiousness < 0.1
                    || traits.conscientiousness > 0.9
                    || traits.extraversion < 0.1
                    || traits.extraversion > 0.9
                    || traits.agreeableness < 0.1
                    || traits.agreeableness > 0.9
                    || traits.neuroticism < 0.1
                    || traits.neuroticism > 0.9
                {
                    personality.traits.clamp_all();
                    tracing::warn!("auto-repaired: personality traits re-clamped to [0.1, 0.9]");
                    repaired = true;
                    break; // Only clamp once.
                }
            }
        }

        repaired
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use aura_types::identity::OceanTraits;

    fn default_personality() -> Personality {
        Personality::new()
    }

    fn default_relationships() -> RelationshipTracker {
        RelationshipTracker::new()
    }

    #[test]
    fn test_clean_verification() {
        let p = default_personality();
        let r = default_relationships();
        let report = IntegrityVerifier::full_verification(&p, &r, 0, true, false);
        assert!(report.is_clean());
        assert!(!report.safe_mode_required);
    }

    #[test]
    fn test_personality_nan_is_critical() {
        let mut p = default_personality();
        p.traits.openness = f32::NAN;

        let mut issues = Vec::new();
        IntegrityVerifier::verify_personality(&p, &mut issues);

        let critical = issues
            .iter()
            .any(|i| i.severity == VerificationSeverity::Critical);
        assert!(critical, "NaN trait should be critical");
    }

    #[test]
    fn test_excessive_goals_warning() {
        let mut issues = Vec::new();
        IntegrityVerifier::verify_goals(100, &mut issues);
        assert!(!issues.is_empty());
        assert_eq!(issues[0].severity, VerificationSeverity::Warning);
    }

    #[test]
    fn test_journal_corruption_is_critical() {
        let p = default_personality();
        let r = default_relationships();
        let report = IntegrityVerifier::full_verification(&p, &r, 0, true, true);
        assert!(report.safe_mode_required);
        assert!(report.critical_count > 0);
    }

    #[test]
    fn test_no_journal_is_info() {
        let p = default_personality();
        let r = default_relationships();
        let report = IntegrityVerifier::full_verification(&p, &r, 0, false, false);
        assert!(!report.safe_mode_required);
        // Should have an info-level issue about no journal.
        let info = report
            .issues
            .iter()
            .any(|i| i.severity == VerificationSeverity::Info);
        assert!(info);
    }

    #[test]
    fn test_repair_clamps_out_of_range() {
        let mut p = default_personality();
        p.traits.openness = 0.05; // below 0.1
        p.traits.neuroticism = 0.95; // above 0.9

        let report = IntegrityVerifier::full_verification(
            &p,
            &default_relationships(),
            0,
            true,
            false,
        );

        let repaired = IntegrityVerifier::repair_if_needed(&mut p, &report);
        assert!(repaired);
        assert!(p.traits.openness >= 0.1);
        assert!(p.traits.neuroticism <= 0.9);
    }
}
