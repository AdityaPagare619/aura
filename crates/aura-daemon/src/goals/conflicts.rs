//! Goal conflict detection and resolution — prevents contradictory goals from
//! degrading AURA's performance.
//!
//! # Conflict Types
//!
//! - **Resource**: Two goals need the same app/resource simultaneously
//!   (e.g., two goals both need the camera).
//! - **Temporal**: Goals have overlapping deadline windows that cannot both be met.
//! - **Logical**: Goals are mutually exclusive by intent (e.g., "enable dark mode"
//!   vs. "enable light mode").
//!
//! # Resolution Strategies
//!
//! - **PriorityBased**: Higher-priority goal wins, lower is suspended.
//! - **TemporalScheduling**: Schedule goals sequentially within their deadline windows.
//! - **Negotiation**: Attempt to find a compatible ordering or partial execution.
//! - **UserDecision**: Flag the conflict for user resolution.

use serde::{Deserialize, Serialize};
use tracing::instrument;

use super::{BoundedVec, CircularBuffer};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum number of active conflicts tracked at once.
const MAX_ACTIVE_CONFLICTS: usize = 64;

/// Maximum conflict history entries retained.
const MAX_CONFLICT_HISTORY: usize = 256;

/// Maximum resource tags per goal.
const MAX_RESOURCE_TAGS: usize = 8;

/// Maximum number of goal entries that can be checked for conflicts.
const MAX_GOAL_ENTRIES: usize = 256;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Classification of a conflict between two goals.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConflictType {
    /// Both goals need the same resource simultaneously.
    Resource,
    /// Goals have overlapping deadline windows that cannot both be met.
    Temporal,
    /// Goals are logically contradictory (mutually exclusive intents).
    Logical,
}

/// How a conflict was resolved.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ResolutionStrategy {
    /// Higher-priority goal proceeds; lower is suspended.
    PriorityBased,
    /// Goals are scheduled sequentially to avoid overlap.
    TemporalScheduling,
    /// Partial execution or re-ordering resolves the issue.
    Negotiation,
    /// The user must decide.
    UserDecision,
}

/// Outcome of a conflict resolution attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ResolutionOutcome {
    /// The conflict was resolved automatically.
    Resolved,
    /// The conflict requires user input.
    NeedsUser,
    /// Resolution deferred (will re-check later).
    Deferred,
}

/// A detected conflict between two goals.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GoalConflict {
    /// Unique conflict ID.
    pub id: u64,
    /// First goal involved.
    pub goal_a_id: u64,
    /// Second goal involved.
    pub goal_b_id: u64,
    /// Type of conflict.
    pub conflict_type: ConflictType,
    /// The resource or reason string (e.g., "camera", "overlapping deadlines").
    pub reason: String,
    /// Priority score of goal A (used for priority-based resolution).
    pub score_a: f32,
    /// Priority score of goal B.
    pub score_b: f32,
    /// When this conflict was detected (epoch ms).
    pub detected_at_ms: u64,
    /// Whether this conflict has been resolved.
    pub resolved: bool,
    /// Strategy used to resolve (if resolved).
    pub resolution: Option<ResolutionStrategy>,
}

/// A goal's resource and timing metadata, used for conflict detection.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GoalConflictEntry {
    /// Goal ID.
    pub goal_id: u64,
    /// Priority score (composite from scheduler).
    pub score: f32,
    /// Resources this goal needs (e.g., "camera", "com.whatsapp", "microphone").
    pub resources: Vec<String>,
    /// Earliest start time (epoch ms).
    pub earliest_start_ms: Option<u64>,
    /// Deadline (epoch ms).
    pub deadline_ms: Option<u64>,
    /// Intent keywords for logical conflict detection.
    pub intent_keywords: Vec<String>,
}

/// A historical record of a resolved conflict (for learning).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConflictRecord {
    /// Type of conflict.
    pub conflict_type: ConflictType,
    /// How it was resolved.
    pub strategy: ResolutionStrategy,
    /// Outcome.
    pub outcome: ResolutionOutcome,
    /// Which goal "won" (was prioritized).
    pub winner_goal_id: u64,
    /// Which goal was deferred/suspended.
    pub deferred_goal_id: u64,
    /// When this was resolved (epoch ms).
    pub resolved_at_ms: u64,
}

/// The conflict resolver — detects and resolves conflicts among active goals.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConflictResolver {
    /// Currently unresolved conflicts.
    active_conflicts: BoundedVec<GoalConflict, MAX_ACTIVE_CONFLICTS>,
    /// History of resolved conflicts for learning.
    history: CircularBuffer<ConflictRecord, MAX_CONFLICT_HISTORY>,
    /// Next conflict ID.
    next_conflict_id: u64,
}

// ---------------------------------------------------------------------------
// Known logical opposites for automatic logical conflict detection.
// ---------------------------------------------------------------------------

/// Pairs of keywords that indicate logically contradictory intents.
const LOGICAL_OPPOSITES: &[(&[&str], &[&str])] = &[
    (
        &["enable", "turn on", "activate"],
        &["disable", "turn off", "deactivate"],
    ),
    (&["dark mode", "dark theme"], &["light mode", "light theme"]),
    (&["mute", "silence"], &["unmute", "ring"]),
    (&["lock"], &["unlock"]),
    (&["connect wifi"], &["disconnect wifi"]),
    (&["bluetooth on"], &["bluetooth off"]),
];

// ---------------------------------------------------------------------------
// Implementation
// ---------------------------------------------------------------------------

impl ConflictResolver {
    /// Create a new empty conflict resolver.
    pub fn new() -> Self {
        Self {
            active_conflicts: BoundedVec::new(),
            history: CircularBuffer::new(),
            next_conflict_id: 1,
        }
    }

    /// Detect all conflicts among a set of active goal entries.
    ///
    /// Compares every pair of goals for resource, temporal, and logical conflicts.
    /// Returns the number of new conflicts detected.
    #[instrument(skip(self, entries))]
    pub fn detect_conflicts(
        &mut self,
        entries: &[GoalConflictEntry],
        now_ms: u64,
    ) -> Vec<GoalConflict> {
        let mut new_conflicts = Vec::new();
        let n = entries.len().min(MAX_GOAL_ENTRIES);

        for i in 0..n {
            for j in (i + 1)..n {
                let a = &entries[i];
                let b = &entries[j];

                // Skip if a conflict between these two already exists.
                if self.has_active_conflict(a.goal_id, b.goal_id) {
                    continue;
                }

                // Check resource conflict.
                if let Some(resource) = Self::check_resource_conflict(a, b) {
                    if let Some(conflict) = self.record_conflict(
                        a.goal_id,
                        b.goal_id,
                        ConflictType::Resource,
                        resource,
                        a.score,
                        b.score,
                        now_ms,
                    ) {
                        new_conflicts.push(conflict);
                    }
                }

                // Check temporal conflict.
                if Self::check_temporal_conflict(a, b) {
                    if let Some(conflict) = self.record_conflict(
                        a.goal_id,
                        b.goal_id,
                        ConflictType::Temporal,
                        "overlapping deadline windows".to_string(),
                        a.score,
                        b.score,
                        now_ms,
                    ) {
                        new_conflicts.push(conflict);
                    }
                }

                // Check logical conflict.
                if let Some(reason) = Self::check_logical_conflict(a, b) {
                    if let Some(conflict) = self.record_conflict(
                        a.goal_id,
                        b.goal_id,
                        ConflictType::Logical,
                        reason,
                        a.score,
                        b.score,
                        now_ms,
                    ) {
                        new_conflicts.push(conflict);
                    }
                }
            }
        }

        if !new_conflicts.is_empty() {
            tracing::info!(count = new_conflicts.len(), "new goal conflicts detected");
        }

        new_conflicts
    }

    /// Attempt to resolve a conflict using the most appropriate strategy.
    ///
    /// Returns the resolution outcome and the strategy used.
    #[instrument(skip(self))]
    pub fn resolve(
        &mut self,
        conflict_id: u64,
        now_ms: u64,
    ) -> Option<(ResolutionStrategy, ResolutionOutcome)> {
        let conflict = self
            .active_conflicts
            .iter_mut()
            .find(|c| c.id == conflict_id && !c.resolved)?;

        let (strategy, outcome, winner, deferred) = match conflict.conflict_type {
            ConflictType::Resource => {
                // Priority-based: higher score wins.
                let (winner, deferred) = if conflict.score_a >= conflict.score_b {
                    (conflict.goal_a_id, conflict.goal_b_id)
                } else {
                    (conflict.goal_b_id, conflict.goal_a_id)
                };
                (
                    ResolutionStrategy::PriorityBased,
                    ResolutionOutcome::Resolved,
                    winner,
                    deferred,
                )
            }
            ConflictType::Temporal => {
                // Try temporal scheduling — if both have deadlines, schedule sequentially.
                let (winner, deferred) = if conflict.score_a >= conflict.score_b {
                    (conflict.goal_a_id, conflict.goal_b_id)
                } else {
                    (conflict.goal_b_id, conflict.goal_a_id)
                };
                (
                    ResolutionStrategy::TemporalScheduling,
                    ResolutionOutcome::Resolved,
                    winner,
                    deferred,
                )
            }
            ConflictType::Logical => {
                // Logical conflicts are harder — if scores are close, ask user.
                let score_diff = (conflict.score_a - conflict.score_b).abs();
                if score_diff < 0.1 {
                    // Scores too close — need user decision.
                    let (winner, deferred) = (conflict.goal_a_id, conflict.goal_b_id);
                    (
                        ResolutionStrategy::UserDecision,
                        ResolutionOutcome::NeedsUser,
                        winner,
                        deferred,
                    )
                } else {
                    let (winner, deferred) = if conflict.score_a >= conflict.score_b {
                        (conflict.goal_a_id, conflict.goal_b_id)
                    } else {
                        (conflict.goal_b_id, conflict.goal_a_id)
                    };
                    (
                        ResolutionStrategy::PriorityBased,
                        ResolutionOutcome::Resolved,
                        winner,
                        deferred,
                    )
                }
            }
        };

        conflict.resolved = true;
        conflict.resolution = Some(strategy);

        // Record in history.
        self.history.push(ConflictRecord {
            conflict_type: conflict.conflict_type,
            strategy,
            outcome,
            winner_goal_id: winner,
            deferred_goal_id: deferred,
            resolved_at_ms: now_ms,
        });

        tracing::info!(
            conflict_id,
            strategy = ?strategy,
            outcome = ?outcome,
            winner,
            deferred,
            "conflict resolved"
        );

        Some((strategy, outcome))
    }

    /// Resolve a conflict with a specific user-chosen strategy.
    #[instrument(skip(self))]
    pub fn resolve_with_strategy(
        &mut self,
        conflict_id: u64,
        strategy: ResolutionStrategy,
        winner_goal_id: u64,
        now_ms: u64,
    ) -> Option<ResolutionOutcome> {
        let conflict = self
            .active_conflicts
            .iter_mut()
            .find(|c| c.id == conflict_id && !c.resolved)?;

        let deferred = if winner_goal_id == conflict.goal_a_id {
            conflict.goal_b_id
        } else {
            conflict.goal_a_id
        };

        conflict.resolved = true;
        conflict.resolution = Some(strategy);

        self.history.push(ConflictRecord {
            conflict_type: conflict.conflict_type,
            strategy,
            outcome: ResolutionOutcome::Resolved,
            winner_goal_id,
            deferred_goal_id: deferred,
            resolved_at_ms: now_ms,
        });

        Some(ResolutionOutcome::Resolved)
    }

    /// Get all unresolved conflicts.
    pub fn unresolved_conflicts(&self) -> Vec<&GoalConflict> {
        self.active_conflicts
            .iter()
            .filter(|c| !c.resolved)
            .collect()
    }

    /// Get all conflicts involving a specific goal.
    pub fn conflicts_for_goal(&self, goal_id: u64) -> Vec<&GoalConflict> {
        self.active_conflicts
            .iter()
            .filter(|c| c.goal_a_id == goal_id || c.goal_b_id == goal_id)
            .collect()
    }

    /// Remove resolved conflicts from the active list.
    pub fn gc_resolved(&mut self) -> usize {
        let before = self.active_conflicts.len();
        self.active_conflicts.retain(|c| !c.resolved);
        let removed = before - self.active_conflicts.len();
        if removed > 0 {
            tracing::debug!(removed, "garbage collected resolved conflicts");
        }
        removed
    }

    /// Number of currently active (unresolved) conflicts.
    #[must_use]
    pub fn active_count(&self) -> usize {
        self.active_conflicts.iter().filter(|c| !c.resolved).count()
    }

    /// Number of total tracked conflicts (resolved + unresolved).
    #[must_use]
    pub fn total_count(&self) -> usize {
        self.active_conflicts.len()
    }

    /// Number of conflicts in history.
    #[must_use]
    pub fn history_count(&self) -> usize {
        self.history.len()
    }

    /// Compute the historical success rate for a given resolution strategy.
    #[must_use]
    pub fn strategy_success_rate(&self, strategy: ResolutionStrategy) -> f32 {
        let matching: Vec<_> = self
            .history
            .iter()
            .filter(|r| r.strategy == strategy)
            .collect();

        if matching.is_empty() {
            return 0.0;
        }

        let resolved = matching
            .iter()
            .filter(|r| r.outcome == ResolutionOutcome::Resolved)
            .count();

        resolved as f32 / matching.len() as f32
    }

    // -- Private helpers ----------------------------------------------------

    /// Check if an active (unresolved) conflict already exists between two goals.
    fn has_active_conflict(&self, goal_a: u64, goal_b: u64) -> bool {
        self.active_conflicts.iter().any(|c| {
            !c.resolved
                && ((c.goal_a_id == goal_a && c.goal_b_id == goal_b)
                    || (c.goal_a_id == goal_b && c.goal_b_id == goal_a))
        })
    }

    /// Record a new conflict. Returns `None` if at capacity.
    fn record_conflict(
        &mut self,
        goal_a: u64,
        goal_b: u64,
        conflict_type: ConflictType,
        reason: String,
        score_a: f32,
        score_b: f32,
        now_ms: u64,
    ) -> Option<GoalConflict> {
        let id = self.next_conflict_id;
        self.next_conflict_id += 1;

        let conflict = GoalConflict {
            id,
            goal_a_id: goal_a,
            goal_b_id: goal_b,
            conflict_type,
            reason,
            score_a,
            score_b,
            detected_at_ms: now_ms,
            resolved: false,
            resolution: None,
        };

        match self.active_conflicts.try_push(conflict.clone()) {
            Ok(()) => Some(conflict),
            Err(_) => {
                tracing::warn!("conflict capacity exceeded, dropping new conflict");
                None
            }
        }
    }

    /// Check if two goals have a resource conflict (shared resource).
    fn check_resource_conflict(a: &GoalConflictEntry, b: &GoalConflictEntry) -> Option<String> {
        for ra in &a.resources {
            for rb in &b.resources {
                if ra == rb {
                    return Some(ra.clone());
                }
            }
        }
        None
    }

    /// Check if two goals have a temporal conflict (overlapping non-meetable windows).
    ///
    /// A temporal conflict exists when both goals have deadlines and the
    /// time windows overlap such that they cannot be run sequentially.
    fn check_temporal_conflict(a: &GoalConflictEntry, b: &GoalConflictEntry) -> bool {
        // Both must have deadlines for a temporal conflict.
        let (deadline_a, deadline_b) = match (a.deadline_ms, b.deadline_ms) {
            (Some(da), Some(db)) => (da, db),
            _ => return false,
        };

        let start_a = a.earliest_start_ms.unwrap_or(0);
        let start_b = b.earliest_start_ms.unwrap_or(0);

        // Check if the windows overlap: A's start < B's deadline AND B's start < A's deadline.
        start_a < deadline_b && start_b < deadline_a
    }

    /// Check if two goals have a logical conflict (contradictory intents).
    fn check_logical_conflict(a: &GoalConflictEntry, b: &GoalConflictEntry) -> Option<String> {
        let a_text: String = a.intent_keywords.join(" ").to_ascii_lowercase();
        let b_text: String = b.intent_keywords.join(" ").to_ascii_lowercase();

        for (group_pos, group_neg) in LOGICAL_OPPOSITES {
            let a_pos = group_pos.iter().any(|kw| a_text.contains(kw));
            let a_neg = group_neg.iter().any(|kw| a_text.contains(kw));
            let b_pos = group_pos.iter().any(|kw| b_text.contains(kw));
            let b_neg = group_neg.iter().any(|kw| b_text.contains(kw));

            // Conflict if one goal matches positive and the other matches negative.
            if (a_pos && b_neg) || (a_neg && b_pos) {
                return Some(format!(
                    "logical opposition: {:?} vs {:?}",
                    group_pos, group_neg
                ));
            }
        }

        None
    }
}

impl Default for ConflictResolver {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn make_entry(
        goal_id: u64,
        score: f32,
        resources: Vec<&str>,
        deadline_ms: Option<u64>,
        intent_keywords: Vec<&str>,
    ) -> GoalConflictEntry {
        GoalConflictEntry {
            goal_id,
            score,
            resources: resources.into_iter().map(String::from).collect(),
            earliest_start_ms: Some(1_000),
            deadline_ms,
            intent_keywords: intent_keywords.into_iter().map(String::from).collect(),
        }
    }

    #[test]
    fn test_resource_conflict_detection() {
        let mut resolver = ConflictResolver::new();
        let entries = vec![
            make_entry(1, 0.8, vec!["camera"], None, vec![]),
            make_entry(2, 0.6, vec!["camera"], None, vec![]),
        ];

        let conflicts = resolver.detect_conflicts(&entries, 1000);
        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflicts[0].conflict_type, ConflictType::Resource);
        assert_eq!(conflicts[0].reason, "camera");
    }

    #[test]
    fn test_temporal_conflict_detection() {
        let mut resolver = ConflictResolver::new();
        let entries = vec![
            make_entry(1, 0.8, vec![], Some(5_000), vec![]),
            make_entry(2, 0.6, vec![], Some(4_000), vec![]),
        ];

        let conflicts = resolver.detect_conflicts(&entries, 1000);
        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflicts[0].conflict_type, ConflictType::Temporal);
    }

    #[test]
    fn test_logical_conflict_detection() {
        let mut resolver = ConflictResolver::new();
        let entries = vec![
            make_entry(1, 0.8, vec![], None, vec!["enable", "dark mode"]),
            make_entry(2, 0.6, vec![], None, vec!["disable", "dark mode"]),
        ];

        let conflicts = resolver.detect_conflicts(&entries, 1000);
        // Should detect logical conflict: enable vs disable
        assert!(
            conflicts
                .iter()
                .any(|c| c.conflict_type == ConflictType::Logical),
            "expected logical conflict, got {:?}",
            conflicts
        );
    }

    #[test]
    fn test_no_conflict_different_resources() {
        let mut resolver = ConflictResolver::new();
        let entries = vec![
            make_entry(1, 0.8, vec!["camera"], None, vec![]),
            make_entry(2, 0.6, vec!["microphone"], None, vec![]),
        ];

        let conflicts = resolver.detect_conflicts(&entries, 1000);
        assert!(conflicts.is_empty());
    }

    #[test]
    fn test_resolve_resource_conflict_priority() {
        let mut resolver = ConflictResolver::new();
        let entries = vec![
            make_entry(1, 0.9, vec!["camera"], None, vec![]),
            make_entry(2, 0.3, vec!["camera"], None, vec![]),
        ];

        let conflicts = resolver.detect_conflicts(&entries, 1000);
        assert_eq!(conflicts.len(), 1);

        let (strategy, outcome) = resolver.resolve(conflicts[0].id, 2000).unwrap();
        assert_eq!(strategy, ResolutionStrategy::PriorityBased);
        assert_eq!(outcome, ResolutionOutcome::Resolved);
        assert_eq!(resolver.active_count(), 0);
    }

    #[test]
    fn test_resolve_logical_conflict_close_scores_needs_user() {
        let mut resolver = ConflictResolver::new();
        let entries = vec![
            make_entry(1, 0.50, vec![], None, vec!["enable", "dark mode"]),
            make_entry(2, 0.51, vec![], None, vec!["disable", "dark mode"]),
        ];

        let conflicts = resolver.detect_conflicts(&entries, 1000);
        let logical = conflicts
            .iter()
            .find(|c| c.conflict_type == ConflictType::Logical);
        assert!(logical.is_some(), "expected logical conflict");

        let (strategy, outcome) = resolver.resolve(logical.unwrap().id, 2000).unwrap();
        assert_eq!(strategy, ResolutionStrategy::UserDecision);
        assert_eq!(outcome, ResolutionOutcome::NeedsUser);
    }

    #[test]
    fn test_resolve_with_user_strategy() {
        let mut resolver = ConflictResolver::new();
        let entries = vec![
            make_entry(1, 0.8, vec!["screen"], None, vec![]),
            make_entry(2, 0.7, vec!["screen"], None, vec![]),
        ];

        let conflicts = resolver.detect_conflicts(&entries, 1000);
        let cid = conflicts[0].id;

        let outcome = resolver.resolve_with_strategy(
            cid,
            ResolutionStrategy::Negotiation,
            2, // user picks goal 2
            2000,
        );
        assert_eq!(outcome, Some(ResolutionOutcome::Resolved));
        assert_eq!(resolver.history_count(), 1);
    }

    #[test]
    fn test_gc_resolved_conflicts() {
        let mut resolver = ConflictResolver::new();
        let entries = vec![
            make_entry(1, 0.8, vec!["camera"], None, vec![]),
            make_entry(2, 0.6, vec!["camera"], None, vec![]),
        ];

        resolver.detect_conflicts(&entries, 1000);
        assert_eq!(resolver.total_count(), 1);

        // Resolve the conflict.
        resolver.resolve(1, 2000);
        assert_eq!(resolver.active_count(), 0);
        assert_eq!(resolver.total_count(), 1); // still in list

        // GC should remove it.
        let removed = resolver.gc_resolved();
        assert_eq!(removed, 1);
        assert_eq!(resolver.total_count(), 0);
    }

    #[test]
    fn test_duplicate_conflict_not_redetected() {
        let mut resolver = ConflictResolver::new();
        let entries = vec![
            make_entry(1, 0.8, vec!["camera"], None, vec![]),
            make_entry(2, 0.6, vec!["camera"], None, vec![]),
        ];

        let first = resolver.detect_conflicts(&entries, 1000);
        assert_eq!(first.len(), 1);

        // Run detection again — should not create a duplicate.
        let second = resolver.detect_conflicts(&entries, 2000);
        assert_eq!(second.len(), 0);
    }

    #[test]
    fn test_conflicts_for_goal() {
        let mut resolver = ConflictResolver::new();
        let entries = vec![
            make_entry(1, 0.8, vec!["camera", "screen"], None, vec![]),
            make_entry(2, 0.6, vec!["camera"], None, vec![]),
            make_entry(3, 0.5, vec!["screen"], None, vec![]),
        ];

        resolver.detect_conflicts(&entries, 1000);

        let goal_1_conflicts = resolver.conflicts_for_goal(1);
        assert_eq!(goal_1_conflicts.len(), 2); // conflicts with both 2 and 3
    }

    #[test]
    fn test_strategy_success_rate() {
        let mut resolver = ConflictResolver::new();

        // Create and resolve two conflicts.
        let entries = vec![
            make_entry(1, 0.9, vec!["camera"], None, vec![]),
            make_entry(2, 0.3, vec!["camera"], None, vec![]),
        ];
        let conflicts = resolver.detect_conflicts(&entries, 1000);
        resolver.resolve(conflicts[0].id, 2000);

        let rate = resolver.strategy_success_rate(ResolutionStrategy::PriorityBased);
        assert!((rate - 1.0).abs() < f32::EPSILON); // 1/1 = 100%
    }

    #[test]
    fn test_no_temporal_conflict_without_deadlines() {
        let mut resolver = ConflictResolver::new();
        let entries = vec![
            make_entry(1, 0.8, vec![], None, vec![]),
            make_entry(2, 0.6, vec![], None, vec![]),
        ];

        let conflicts = resolver.detect_conflicts(&entries, 1000);
        assert!(conflicts.is_empty());
    }

    #[test]
    fn test_multiple_conflict_types_between_same_goals() {
        let mut resolver = ConflictResolver::new();
        let entries = vec![
            make_entry(
                1,
                0.8,
                vec!["camera"],
                Some(5_000),
                vec!["enable", "dark mode"],
            ),
            make_entry(
                2,
                0.6,
                vec!["camera"],
                Some(4_000),
                vec!["disable", "dark mode"],
            ),
        ];

        let conflicts = resolver.detect_conflicts(&entries, 1000);
        // Should find resource, temporal, AND logical conflicts.
        assert!(
            conflicts.len() >= 2,
            "expected multiple conflicts, got {}",
            conflicts.len()
        );
    }
}
