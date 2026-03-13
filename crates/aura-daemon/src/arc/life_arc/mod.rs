//! Life Arc subsystem — state machines for the four primary life arcs.
//!
//! This module is SEPARATE from the daemon-level sensor arcs (`arc::health`,
//! `arc::social`, etc.). Those arcs track raw sensor data (meds, vitals,
//! contacts). This subsystem tracks USER-DEFINED life trajectory facts and
//! produces structured context for the LLM to reason about.
//!
//! # Design principles
//! - Arc subsystem TRACKS and SCORES — the LLM reasons.
//! - Day-zero safe: all arcs return `Stable` health with no data.
//! - No format strings as intelligence — output is raw context, not wisdom.
//! - User dimensions are infinite — `ArcType::Custom(String)` exists for this.
//! - Trigger dedup: max 1 trigger per arc per 24 hours.

pub mod financial;
pub mod growth;
pub mod health_arc;
pub mod primitives;
pub mod relationships;

pub use financial::{FinancialArc, FinancialEvent};
pub use growth::{GrowthArc, GrowthEvent, GrowthGoal};
pub use health_arc::{HealthArc, HealthEvent};
pub use primitives::{ArcHealth, ArcType, ProactiveTrigger};
pub use relationships::{RelationshipArc, TrackedRelationship};

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// LifeArcManager — aggregates all four arcs
// ---------------------------------------------------------------------------

/// Owns all four life arcs and provides aggregate access for the LLM.
///
/// `LifeArcManager` is a field of [`crate::arc::ArcManager`] and is driven
/// by the main daemon event loop. It never reasons — it only aggregates
/// state and surfaces context.
#[derive(Debug, Default)]
pub struct LifeArcManager {
    pub financial: FinancialArc,
    pub relationships: RelationshipArc,
    pub health: HealthArc,
    pub growth: GrowthArc,
}

impl LifeArcManager {
    /// Create a new manager with all arcs at day-zero state.
    #[must_use]
    pub fn new() -> Self {
        Self {
            financial: FinancialArc::new(),
            relationships: RelationshipArc::new(),
            health: HealthArc::new(),
            growth: GrowthArc::new(),
        }
    }

    /// Refresh the cached health level for every arc.
    ///
    /// Must be called before [`collect_triggers`] so that each arc's
    /// `health` field reflects the latest scores. Safe to call on every
    /// cron tick — each arc computes O(n) over its own event log.
    pub fn update_health_all(&mut self, now_ms: u64) {
        self.financial.update_health(now_ms);
        self.relationships.update_health(now_ms);
        self.health.update_health(now_ms);
        self.growth.update_health(now_ms);
    }

    /// Collect all pending proactive triggers across every arc.
    ///
    /// Each arc enforces its own 24-hour dedup. Returns only arcs that
    /// have crossed a threshold and have not fired recently.
    #[must_use]
    pub fn collect_triggers(&self, now_ms: u64) -> Vec<ProactiveTrigger> {
        let mut triggers = Vec::new();

        if let Some(t) = self.financial.check_proactive_trigger(now_ms) {
            triggers.push(t);
        }
        if let Some(t) = self.relationships.check_proactive_trigger(now_ms) {
            triggers.push(t);
        }
        if let Some(t) = self.health.check_proactive_trigger(now_ms) {
            triggers.push(t);
        }
        if let Some(t) = self.growth.check_proactive_trigger(now_ms) {
            triggers.push(t);
        }

        triggers
    }

    /// Build a full LLM context payload for all arcs at the given timestamp.
    ///
    /// Returns a serialisable struct that the LLM context builder can attach
    /// to any prompt that needs life-arc awareness.
    #[must_use]
    pub fn full_llm_context(&self, now_ms: u64) -> LifeArcContext {
        LifeArcContext {
            financial: self.financial.to_llm_context(),
            relationships: self.relationships.to_llm_context(now_ms),
            health: self.health.to_llm_context_at(now_ms),
            growth: self.growth.to_llm_context(now_ms),
        }
    }
}

// ---------------------------------------------------------------------------
// LifeArcContext — serialisable snapshot for LLM injection
// ---------------------------------------------------------------------------

/// Serialisable aggregate snapshot of all four life arcs.
///
/// Attach this to an LLM prompt as structured context. The LLM can then
/// reason about the user's current life trajectory without needing to call
/// back into the daemon.
///
/// Each field is a pre-formatted context string produced by the arc's
/// `to_llm_context()` method — raw facts, no reasoning.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LifeArcContext {
    pub financial: String,
    pub relationships: String,
    pub health: String,
    pub growth: String,
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_manager_day_zero() {
        let mgr = LifeArcManager::new();
        // All arcs should be Stable at day zero.
        assert_eq!(mgr.financial.health, ArcHealth::Stable);
        assert_eq!(mgr.relationships.health, ArcHealth::Stable);
        assert_eq!(mgr.health.health, ArcHealth::Stable);
        assert_eq!(mgr.growth.health, ArcHealth::Stable);
    }

    #[test]
    fn test_collect_triggers_empty_at_day_zero() {
        let mgr = LifeArcManager::new();
        let triggers = mgr.collect_triggers(1_000_000);
        // No events recorded, no triggers should fire.
        assert!(triggers.is_empty(), "expected no triggers at day zero");
    }

    #[test]
    fn test_full_llm_context_serialisable() {
        let mgr = LifeArcManager::new();
        let ctx = mgr.full_llm_context(1_000_000);
        // Must round-trip through JSON without panicking.
        let json = serde_json::to_string(&ctx).expect("should serialise");
        let _back: LifeArcContext = serde_json::from_str(&json).expect("should deserialise");
    }

    #[test]
    fn test_full_llm_context_has_all_arcs() {
        let mgr = LifeArcManager::new();
        let ctx = mgr.full_llm_context(1_000_000);
        // Each arc key must be non-empty.
        assert!(!ctx.financial.is_empty(), "financial context missing");
        assert!(!ctx.relationships.is_empty(), "relationships context missing");
        assert!(!ctx.health.is_empty(), "health context missing");
        assert!(!ctx.growth.is_empty(), "growth context missing");
    }

    #[test]
    fn test_manager_default_is_new() {
        let a = LifeArcManager::new();
        let b = LifeArcManager::default();
        // Both should be at day-zero — same health state.
        assert_eq!(a.financial.health, b.financial.health);
        assert_eq!(a.growth.health, b.growth.health);
    }
}
