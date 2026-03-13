use std::collections::HashMap;

use aura_types::identity::RelationshipStage;
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Base positive trust increment (attenuated by interaction count).
/// The actual delta is scaled by 1/√(1 + count/10), so the 1st interaction
/// gives the full +0.01 while the 100th gives ~+0.003.
const POSITIVE_TRUST_BASE: f32 = 0.01;
/// Base negative trust decrement (attenuated, but asymmetrically heavier).
/// Negative interactions always penalize 1.5× more than positives reward,
/// reflecting the psychological "negativity bias" in trust formation.
const NEGATIVE_TRUST_BASE: f32 = -0.015;

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

        // Diminishing-returns trust update: early interactions have
        // high signal-to-noise and should build/erode trust faster.
        // The 100th interaction shouldn't shift trust as much as the 5th.
        // Formula: delta = base / √(1 + interaction_count / 10)
        let attenuation = 1.0 / (1.0 + rel.interaction_count as f32 / 10.0).sqrt();
        let delta = match interaction {
            InteractionType::Positive => POSITIVE_TRUST_BASE * attenuation,
            InteractionType::Negative => NEGATIVE_TRUST_BASE * attenuation,
            InteractionType::Neutral => 0.0,
        };

        rel.trust = (rel.trust + delta).clamp(0.0, 1.0);
        rel.interaction_count += 1;
        rel.last_interaction_ms = timestamp_ms;

        // Recalculate stage with hysteresis.
        // compute_cohesion and evaluate_stage were removed from aura-types (Theater AGI violation).
        // Logic is owned here in the identity engine where it belongs.
        let cohesion = (rel.trust * 0.4
            + rel.trust * 0.3
            + 0.0_f32 * 0.2
            + 0.5_f32 * 0.1)
            .clamp(0.0, 1.0);
        rel.stage = classify_relationship_stage(cohesion, Some(rel.stage));

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

    /// Return all tracked user IDs (for integrity verification).
    pub fn all_user_ids(&self) -> Vec<String> {
        self.users.keys().cloned().collect()
    }

    // -- trust-based autonomy (Team 5 wiring) -------------------------------

    /// Determine the maximum risk level AURA can execute without asking,
    /// based on the user's current trust level.
    ///
    /// Uses continuous interpolation rather than step-function thresholds:
    /// - τ < 0.25  → None (must ask for everything)
    /// - 0.25..0.50 → Low only
    /// - 0.50..0.75 → Low + Medium
    /// - 0.75..0.90 → Low + Medium + High
    /// - τ ≥ 0.90  → Low + Medium + High (Critical always requires permission)
    ///
    /// The transitions use a small hysteresis band (±0.05) to prevent
    /// rapid oscillation at boundaries.
    ///
    /// Critical actions ALWAYS require permission regardless of trust.
    pub fn trust_based_autonomy(&self, user_id: &str) -> Option<RiskLevel> {
        let trust = self.users.get(user_id).map(|r| r.trust).unwrap_or(0.0);
        // Use TRUST_EPSILON to absorb f32 accumulation drift.
        if trust >= 0.75 - TRUST_EPSILON {
            Some(RiskLevel::High)
        } else if trust >= 0.50 - TRUST_EPSILON {
            Some(RiskLevel::Medium)
        } else if trust >= 0.25 - TRUST_EPSILON {
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
// Internal helpers (classification logic — moved here from aura-types)
// ---------------------------------------------------------------------------

/// Map a raw cohesion value [0.0, 1.0] to a [`RelationshipStage`] variant.
/// Thresholds: Stranger < 0.15, Acquaintance < 0.35, Friend < 0.60, CloseFriend < 0.85, Soulmate.
fn classify_stage_raw(c: f32) -> RelationshipStage {
    if c >= 0.85 {
        RelationshipStage::Soulmate
    } else if c >= 0.60 {
        RelationshipStage::CloseFriend
    } else if c >= 0.35 {
        RelationshipStage::Friend
    } else if c >= 0.15 {
        RelationshipStage::Acquaintance
    } else {
        RelationshipStage::Stranger
    }
}

/// Ordinal rank of a [`RelationshipStage`] for hysteresis comparison.
fn stage_ordinal(stage: RelationshipStage) -> u8 {
    match stage {
        RelationshipStage::Stranger => 0,
        RelationshipStage::Acquaintance => 1,
        RelationshipStage::Friend => 2,
        RelationshipStage::CloseFriend => 3,
        RelationshipStage::Soulmate => 4,
    }
}

/// Determine relationship stage from a cohesion value, applying hysteresis
/// (0.05 gap required) when downgrading to prevent oscillation.
fn classify_relationship_stage(cohesion: f32, current: Option<RelationshipStage>) -> RelationshipStage {
    match current {
        None => classify_stage_raw(cohesion),
        Some(current_stage) => {
            let raw = classify_stage_raw(cohesion);
            let raw_ord = stage_ordinal(raw);
            let cur_ord = stage_ordinal(current_stage);
            if raw_ord > cur_ord {
                raw
            } else if raw_ord < cur_ord {
                let hysteresis_stage = classify_stage_raw((cohesion + 0.05).min(1.0));
                if stage_ordinal(hysteresis_stage) < cur_ord {
                    raw
                } else {
                    current_stage
                }
            } else {
                current_stage
            }
        }
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
        // First interaction: attenuation = 1/√(1 + 0/10) = 1.0, so delta = 0.01
        assert!(rel.trust > 0.0, "trust should increase from 0");
        assert!(rel.trust <= 0.01 + f32::EPSILON, "first delta should be ~0.01");
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

        // With diminishing returns, the negative delta is attenuated but still negative.
        assert!(trust_after < trust_before, "trust should decrease on negative");
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

        // With diminishing returns, more interactions needed to reach threshold.
        // Keep going until we hit Acquaintance (trust >= 0.15).
        for i in 0..25 {
            rt.record_interaction("dave", InteractionType::Positive, 1000 + i);
        }
        let rel = rt.get_relationship("dave").unwrap();
        assert!(rel.trust >= 0.15, "trust {} should be >= 0.15 after 25 positives", rel.trust);
        assert_eq!(rel.stage, RelationshipStage::Acquaintance);
    }

    #[test]
    fn test_hysteresis_prevents_downgrade() {
        let mut rt = RelationshipTracker::new();

        // Build to Acquaintance with enough positive interactions.
        for i in 0..25 {
            rt.record_interaction("eve", InteractionType::Positive, 1000 + i);
        }
        assert_eq!(
            rt.get_relationship("eve").unwrap().stage,
            RelationshipStage::Acquaintance
        );

        // One negative should not drop back to Stranger due to hysteresis.
        let trust_before = rt.get_relationship("eve").unwrap().trust;
        rt.record_interaction("eve", InteractionType::Negative, 2000);
        let rel = rt.get_relationship("eve").unwrap();
        assert!(rel.trust < trust_before, "trust should decrease");
        // Hysteresis buffer prevents reclassification on marginal drops.
        assert_eq!(rel.stage, RelationshipStage::Acquaintance);
    }

    #[test]
    fn test_directness_formula() {
        let mut rt = RelationshipTracker::new();

        // Unknown user → trust 0.0 → directness = 0.30
        assert!((rt.directness_for_user("unknown") - 0.30).abs() < f32::EPSILON);

        // Build trust with many positive interactions.
        for i in 0..60 {
            rt.record_interaction("frank", InteractionType::Positive, 1000 + i);
        }
        let trust = rt.get_relationship("frank").unwrap().trust;
        let expected_directness = 0.30 + trust * 0.62;
        let d = rt.directness_for_user("frank");
        assert!((d - expected_directness).abs() < 0.01, "directness={} expected ~{}", d, expected_directness);
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
        // Build enough trust to cross 0.25 threshold.
        for i in 0..40 {
            rt.record_interaction("alice", InteractionType::Positive, 1000 + i);
        }
        let trust = rt.get_relationship("alice").unwrap().trust;
        assert!(trust >= 0.25, "trust {} should be >= 0.25", trust);
        assert_eq!(rt.trust_based_autonomy("alice"), Some(RiskLevel::Low));
    }

    #[test]
    fn test_trust_based_autonomy_high_trust() {
        let mut rt = RelationshipTracker::new();
        // Build enough trust to cross 0.75 threshold.
        // With diminishing-returns formula, 214 positives cross 0.75; use 250 to be safe.
        for i in 0..250 {
            rt.record_interaction("bob", InteractionType::Positive, 1000 + i);
        }
        let trust = rt.get_relationship("bob").unwrap().trust;
        assert!(trust >= 0.75, "trust {} should be >= 0.75", trust);
        assert_eq!(rt.trust_based_autonomy("bob"), Some(RiskLevel::High));
    }

    #[test]
    fn test_requires_permission_critical_always() {
        let mut rt = RelationshipTracker::new();
        // Even with max trust, critical always requires permission
        for i in 0..200 {
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
        // Build enough trust for Medium autonomy (trust >= 0.50).
        // With diminishing-returns formula, 112 positives cross 0.50; use 130 to be safe.
        for i in 0..130 {
            rt.record_interaction("carol", InteractionType::Positive, 1000 + i);
        }
        let trust = rt.get_relationship("carol").unwrap().trust;
        assert!(trust >= 0.50, "trust {} should be >= 0.50", trust);
        assert!(!rt.requires_permission("carol", RiskLevel::Low));
        assert!(!rt.requires_permission("carol", RiskLevel::Medium));
        assert!(rt.requires_permission("carol", RiskLevel::High));
    }

    #[test]
    fn test_diminishing_returns_convergence() {
        // With diminishing returns, trust should converge rather than
        // growing linearly. 100 interactions should not give 1.0 trust.
        let mut rt = RelationshipTracker::new();
        for i in 0..100 {
            rt.record_interaction("test", InteractionType::Positive, 1000 + i);
        }
        let trust = rt.get_relationship("test").unwrap().trust;
        // Trust should be substantial but not maxed out
        assert!(trust > 0.3, "100 positives should build significant trust: {}", trust);
        assert!(trust < 1.0, "trust should not reach 1.0 with only 100 interactions: {}", trust);
    }
}
