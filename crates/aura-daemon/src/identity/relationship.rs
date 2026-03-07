use std::collections::HashMap;

use aura_types::identity::RelationshipStage;
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const POSITIVE_TRUST_DELTA: f32 = 0.01;
const NEGATIVE_TRUST_DELTA: f32 = -0.015;

/// Small epsilon for floating-point trust threshold comparisons.
/// Required because `POSITIVE_TRUST_DELTA` (0.01) cannot be exactly
/// represented in f32 (it is 0.009999999776...), so repeated addition
/// of N * 0.01 accumulates a tiny shortfall that would fail strict `>=`
/// comparisons against round thresholds like 0.30, 0.60, 0.80.
const TRUST_EPSILON: f32 = 1e-6;

/// Maximum number of users tracked concurrently.  When the limit is reached
/// the least-recently-interacted user with the lowest trust is evicted.
const MAX_TRACKED_USERS: usize = 500;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Type of interaction that affects trust.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InteractionType {
    Positive,
    Negative,
    Neutral,
}

/// Risk level of an action — used by trust-based autonomy control.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum RiskLevel {
    /// Trivial actions (read-only, display information).
    Low,
    /// Non-trivial but reversible actions (send message, install app).
    Medium,
    /// Important actions with consequences (delete files, spend money).
    High,
    /// Irreversible or dangerous actions (factory reset, root device).
    Critical,
}

/// Per-user relationship state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserRelationship {
    pub trust: f32,
    pub stage: RelationshipStage,
    pub interaction_count: u64,
    pub last_interaction_ms: u64,
}

impl UserRelationship {
    fn new() -> Self {
        Self {
            trust: 0.0,
            stage: RelationshipStage::Stranger,
            interaction_count: 0,
            last_interaction_ms: 0,
        }
    }
}

/// Tracks trust and relationship progression for every known user.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelationshipTracker {
    users: HashMap<String, UserRelationship>,
}

// ---------------------------------------------------------------------------
// Implementation
// ---------------------------------------------------------------------------

impl RelationshipTracker {
    pub fn new() -> Self {
        Self {
            users: HashMap::new(),
        }
    }

    /// Record an interaction and update trust + stage for the given user.
    ///
    /// When the tracker is at capacity and the `user_id` is new, the
    /// least-recently-interacted user with the lowest trust is evicted.
    pub fn record_interaction(
        &mut self,
        user_id: &str,
        interaction: InteractionType,
        timestamp_ms: u64,
    ) {
        // Evict if at capacity and this is a brand-new user.
        if self.users.len() >= MAX_TRACKED_USERS && !self.users.contains_key(user_id) {
            self.evict_one();
        }

        let rel = self
            .users
            .entry(user_id.to_owned())
            .or_insert_with(UserRelationship::new);

        let delta = match interaction {
            InteractionType::Positive => POSITIVE_TRUST_DELTA,
            InteractionType::Negative => NEGATIVE_TRUST_DELTA,
            InteractionType::Neutral => 0.0,
        };

        rel.trust = (rel.trust + delta).clamp(0.0, 1.0);
        rel.interaction_count += 1;
        rel.last_interaction_ms = timestamp_ms;

        // Recalculate stage with hysteresis (uses aura-types implementation).
        rel.stage = RelationshipStage::from_trust(rel.trust, Some(rel.stage));

        tracing::debug!(
            user = user_id,
            trust = rel.trust,
            stage = ?rel.stage,
            "relationship updated"
        );
    }

    /// Get the current relationship for a user (if known).
    pub fn get_relationship(&self, user_id: &str) -> Option<&UserRelationship> {
        self.users.get(user_id)
    }

    /// Compute the directness level for a specific user.
    ///
    /// Formula: `directness(τ) = 0.30 + τ × 0.62`, clamped to \[0, 1\].
    pub fn directness_for_user(&self, user_id: &str) -> f32 {
        let trust = self.users.get(user_id).map(|r| r.trust).unwrap_or(0.0);
        (0.30 + trust * 0.62).clamp(0.0, 1.0)
    }

    /// Number of tracked users.
    pub fn user_count(&self) -> usize {
        self.users.len()
    }

    // -- trust-based autonomy (Team 5 wiring) -------------------------------

    /// Determine the maximum risk level AURA can execute without asking,
    /// based on the user's current trust level.
    ///
    /// | Trust τ         | Auto-execute up to  |
    /// |-----------------|---------------------|
    /// | τ < 0.30        | (nothing — ask all) |
    /// | 0.30 ≤ τ < 0.60 | Low only            |
    /// | 0.60 ≤ τ < 0.80 | Low + Medium        |
    /// | τ ≥ 0.80        | Low + Medium + High |
    ///
    /// Critical actions ALWAYS require permission regardless of trust.
    pub fn trust_based_autonomy(&self, user_id: &str) -> Option<RiskLevel> {
        let trust = self.users.get(user_id).map(|r| r.trust).unwrap_or(0.0);
        // Use TRUST_EPSILON to absorb f32 accumulation drift from repeated
        // addition of POSITIVE_TRUST_DELTA (0.01), which is not exactly
        // representable in IEEE 754 single precision.
        if trust >= 0.80 - TRUST_EPSILON {
            Some(RiskLevel::High)
        } else if trust >= 0.60 - TRUST_EPSILON {
            Some(RiskLevel::Medium)
        } else if trust >= 0.30 - TRUST_EPSILON {
            Some(RiskLevel::Low)
        } else {
            None // Must ask for everything
        }
    }

    /// Check whether a specific action at a given risk level requires
    /// explicit user permission.
    ///
    /// Returns `true` if AURA must ask before executing.
    pub fn requires_permission(&self, user_id: &str, risk: RiskLevel) -> bool {
        // Critical always requires permission.
        if risk == RiskLevel::Critical {
            return true;
        }
        match self.trust_based_autonomy(user_id) {
            None => true, // Low trust → ask for everything
            Some(max_auto) => risk > max_auto,
        }
    }

    /// Get the trust level for a user (0.0 if unknown).
    pub fn trust_level(&self, user_id: &str) -> f32 {
        self.users.get(user_id).map(|r| r.trust).unwrap_or(0.0)
    }

    /// Evict the single "least valuable" user: lowest trust first, then
    /// oldest `last_interaction_ms` as tiebreaker.  This keeps high-trust
    /// users safe from eviction even if they haven't interacted recently.
    fn evict_one(&mut self) {
        let victim = self
            .users
            .iter()
            .min_by(|a, b| {
                a.1.trust
                    .partial_cmp(&b.1.trust)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then_with(|| a.1.last_interaction_ms.cmp(&b.1.last_interaction_ms))
            })
            .map(|(k, _)| k.clone());

        if let Some(key) = victim {
            tracing::debug!(
                evicted_user = %key,
                "relationship tracker at capacity — evicting lowest-trust user"
            );
            self.users.remove(&key);
        }
    }
}

impl Default for RelationshipTracker {
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

    #[test]
    fn test_trust_increases_on_positive() {
        let mut rt = RelationshipTracker::new();
        rt.record_interaction("alice", InteractionType::Positive, 1000);
        let rel = rt.get_relationship("alice").unwrap();
        assert!((rel.trust - 0.01).abs() < f32::EPSILON);
        assert_eq!(rel.stage, RelationshipStage::Stranger);
    }

    #[test]
    fn test_trust_decreases_on_negative() {
        let mut rt = RelationshipTracker::new();

        // First build some trust
        for i in 0..30 {
            rt.record_interaction("bob", InteractionType::Positive, 1000 + i);
        }
        let trust_before = rt.get_relationship("bob").unwrap().trust;

        rt.record_interaction("bob", InteractionType::Negative, 2000);
        let trust_after = rt.get_relationship("bob").unwrap().trust;

        assert!(trust_after < trust_before);
        assert!((trust_before - trust_after - 0.015).abs() < f32::EPSILON);
    }

    #[test]
    fn test_trust_clamped_to_bounds() {
        let mut rt = RelationshipTracker::new();

        // Cannot go below 0.0
        rt.record_interaction("carol", InteractionType::Negative, 1000);
        let rel = rt.get_relationship("carol").unwrap();
        assert!((rel.trust - 0.0).abs() < f32::EPSILON);
    }

    #[test]
    fn test_stage_progression() {
        let mut rt = RelationshipTracker::new();

        // 16 positive interactions → trust = 0.16 → Acquaintance (threshold 0.15)
        // Using 16 instead of 15 to avoid floating-point boundary issues.
        for i in 0..16 {
            rt.record_interaction("dave", InteractionType::Positive, 1000 + i);
        }
        let rel = rt.get_relationship("dave").unwrap();
        assert!(rel.trust >= 0.15, "trust {} should be >= 0.15", rel.trust);
        assert_eq!(rel.stage, RelationshipStage::Acquaintance);
    }

    #[test]
    fn test_hysteresis_prevents_downgrade() {
        let mut rt = RelationshipTracker::new();

        // Build to Acquaintance: 16 positives → trust = 0.16
        for i in 0..16 {
            rt.record_interaction("eve", InteractionType::Positive, 1000 + i);
        }
        assert_eq!(
            rt.get_relationship("eve").unwrap().stage,
            RelationshipStage::Acquaintance
        );

        // One negative → trust = 0.16 - 0.015 = 0.145
        // Raw would say Stranger (< 0.15), but hysteresis keeps Acquaintance
        // because 0.145 + 0.05 = 0.195 ≥ 0.15
        rt.record_interaction("eve", InteractionType::Negative, 2000);
        let rel = rt.get_relationship("eve").unwrap();
        assert!((rel.trust - 0.145).abs() < 0.001);
        assert_eq!(rel.stage, RelationshipStage::Acquaintance);
    }

    #[test]
    fn test_directness_formula() {
        let mut rt = RelationshipTracker::new();

        // Unknown user → trust 0.0 → directness = 0.30
        assert!((rt.directness_for_user("unknown") - 0.30).abs() < f32::EPSILON);

        // Build trust to 0.50 → directness = 0.30 + 0.50*0.62 = 0.61
        for i in 0..50 {
            rt.record_interaction("frank", InteractionType::Positive, 1000 + i);
        }
        let d = rt.directness_for_user("frank");
        assert!((d - 0.61).abs() < 0.01, "directness={}", d);
    }

    #[test]
    fn test_trust_based_autonomy_stranger() {
        let rt = RelationshipTracker::new();
        // Unknown user → trust 0.0 → None (ask everything)
        assert_eq!(rt.trust_based_autonomy("unknown"), None);
    }

    #[test]
    fn test_trust_based_autonomy_low_trust() {
        let mut rt = RelationshipTracker::new();
        // 30 positives → trust = 0.30 → Low
        for i in 0..30 {
            rt.record_interaction("alice", InteractionType::Positive, 1000 + i);
        }
        assert_eq!(rt.trust_based_autonomy("alice"), Some(RiskLevel::Low));
    }

    #[test]
    fn test_trust_based_autonomy_high_trust() {
        let mut rt = RelationshipTracker::new();
        // 80 positives → trust = 0.80 → High
        for i in 0..80 {
            rt.record_interaction("bob", InteractionType::Positive, 1000 + i);
        }
        assert_eq!(rt.trust_based_autonomy("bob"), Some(RiskLevel::High));
    }

    #[test]
    fn test_requires_permission_critical_always() {
        let mut rt = RelationshipTracker::new();
        // Even with max trust, critical always requires permission
        for i in 0..100 {
            rt.record_interaction("trusted", InteractionType::Positive, 1000 + i);
        }
        assert!(
            rt.requires_permission("trusted", RiskLevel::Critical),
            "critical should always require permission"
        );
    }

    #[test]
    fn test_requires_permission_low_trust() {
        let rt = RelationshipTracker::new();
        assert!(rt.requires_permission("unknown", RiskLevel::Low));
        assert!(rt.requires_permission("unknown", RiskLevel::Medium));
        assert!(rt.requires_permission("unknown", RiskLevel::High));
    }

    #[test]
    fn test_requires_permission_medium_trust() {
        let mut rt = RelationshipTracker::new();
        // Build to ~0.60 trust → auto for Low+Medium
        for i in 0..60 {
            rt.record_interaction("carol", InteractionType::Positive, 1000 + i);
        }
        assert!(!rt.requires_permission("carol", RiskLevel::Low));
        assert!(!rt.requires_permission("carol", RiskLevel::Medium));
        assert!(rt.requires_permission("carol", RiskLevel::High));
    }
}
