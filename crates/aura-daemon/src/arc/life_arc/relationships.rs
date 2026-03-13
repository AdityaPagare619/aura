//! Relationship life arc — tracks social interaction patterns.
//!
//! # Design principles
//!
//! - No assumptions about relationship type (family, friend, colleague, partner…)
//! - No assumptions about family structure or living situation
//! - Quality signals: interaction frequency relative to user's own baseline
//! - Drift detection: silence > 2× average interval = weak drift signal
//! - Max 200 tracked relationships (bounded memory)
//!
//! # What this tracks (FACTS, not judgements)
//!
//! - Interaction timestamps per person
//! - User-assigned importance level (1–10)
//! - Rolling 30-day interaction count
//! - Running average interaction interval
//!
//! # Scoring
//!
//! Score is based on the weighted-average "interaction health" of all
//! relationships, where importance level weights each relationship.
//! A relationship is "healthy" if interactions are occurring at or above
//! the user's own established baseline frequency.

use serde::{Deserialize, Serialize};

use super::primitives::{ArcHealth, ArcType, ProactiveTrigger, ONE_DAY_MS};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum number of tracked relationships.
const MAX_RELATIONSHIPS: usize = 200;

/// Maximum interaction timestamps stored per relationship (ring buffer).
const MAX_INTERACTION_TS_PER_PERSON: usize = 60;

/// Rolling window for interaction count (30 days).
const ROLLING_WINDOW_MS: u64 = 30 * ONE_DAY_MS;

/// Drift threshold multiplier: if elapsed > drift_factor × avg_interval → drifting.
const DRIFT_FACTOR: f32 = 2.0;

/// Minimum interactions with a person before drift detection is active.
const MIN_INTERACTIONS_FOR_DRIFT: u32 = 3;

/// Minimum total events before producing proactive triggers (day-zero guard).
const MIN_EVENTS_FOR_TRIGGER: u32 = 5;

// ---------------------------------------------------------------------------
// TrackedRelationship
// ---------------------------------------------------------------------------

/// A single tracked relationship with one person.
///
/// The `person_id` is an opaque user-defined string — AURA does not interpret it.
/// No assumptions about relationship type are made.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrackedRelationship {
    /// Opaque identifier. Can be a contact ID, phone hash, or user-chosen label.
    pub person_id: String,
    /// User-facing display name.
    pub display_name: String,
    /// Number of interactions in the rolling 30-day window.
    pub interaction_count_30d: u32,
    /// Unix millisecond timestamp of the most recent interaction.
    pub last_interaction_ms: u64,
    /// Running average interval between interactions, in hours.
    /// 0.0 until at least 2 interactions are recorded.
    pub avg_interaction_interval_hours: f32,
    /// User-set importance level (1 = low, 10 = critical). Default: 5.
    pub user_importance_level: u8,
    /// Lifetime interaction count.
    pub total_interactions: u32,
    // Internal: recent interaction timestamps (bounded ring buffer).
    interaction_timestamps: Vec<u64>,
}

impl TrackedRelationship {
    /// Create a new tracked relationship.
    #[must_use]
    pub fn new(person_id: String, display_name: String, importance: u8) -> Self {
        Self {
            person_id,
            display_name,
            interaction_count_30d: 0,
            last_interaction_ms: 0,
            avg_interaction_interval_hours: 0.0,
            user_importance_level: importance.clamp(1, 10),
            total_interactions: 0,
            interaction_timestamps: Vec::with_capacity(8),
        }
    }

    /// Record a new interaction at the given timestamp.
    pub fn record_interaction(&mut self, now_ms: u64) {
        // Bounded ring buffer.
        if self.interaction_timestamps.len() >= MAX_INTERACTION_TS_PER_PERSON {
            self.interaction_timestamps.remove(0);
        }
        self.interaction_timestamps.push(now_ms);
        self.last_interaction_ms = now_ms;
        self.total_interactions = self.total_interactions.saturating_add(1);

        // Recompute 30-day count.
        let window_start = now_ms.saturating_sub(ROLLING_WINDOW_MS);
        self.interaction_count_30d = self
            .interaction_timestamps
            .iter()
            .filter(|&&ts| ts >= window_start)
            .count() as u32;

        // Recompute running average interval (hours between interactions).
        self.refresh_avg_interval();
    }

    /// Recompute the running average interaction interval.
    fn refresh_avg_interval(&mut self) {
        let n = self.interaction_timestamps.len();
        if n < 2 {
            self.avg_interaction_interval_hours = 0.0;
            return;
        }
        // Compute average gap between consecutive timestamps.
        let mut gap_sum_ms: u64 = 0;
        let mut gap_count: u32 = 0;
        for i in 1..n {
            let gap = self.interaction_timestamps[i]
                .saturating_sub(self.interaction_timestamps[i - 1]);
            gap_sum_ms = gap_sum_ms.saturating_add(gap);
            gap_count += 1;
        }
        if gap_count > 0 {
            let avg_ms = gap_sum_ms as f64 / gap_count as f64;
            self.avg_interaction_interval_hours = (avg_ms / 3_600_000.0) as f32;
        }
    }

    /// Whether this relationship is drifting (interaction gap > drift_factor × avg interval).
    ///
    /// Returns `false` if insufficient data (day-zero safe).
    #[must_use]
    pub fn is_drifting(&self, now_ms: u64) -> bool {
        if self.total_interactions < MIN_INTERACTIONS_FOR_DRIFT {
            return false; // Not enough data
        }
        if self.avg_interaction_interval_hours <= 0.0 {
            return false;
        }
        if self.last_interaction_ms == 0 {
            return false;
        }
        let elapsed_hours =
            now_ms.saturating_sub(self.last_interaction_ms) as f32 / 3_600_000.0;
        elapsed_hours > DRIFT_FACTOR * self.avg_interaction_interval_hours
    }

    /// Interaction health score for this relationship in `[0.0, 1.0]`.
    ///
    /// Based on how recent the last interaction was relative to average interval.
    /// Day-zero (no interactions): returns 0.5 (neutral, not penalised).
    /// One interaction: uses a default 7-day reference interval so that a
    /// recent single interaction scores high and a stale one scores low.
    #[must_use]
    pub fn interaction_health_score(&self, now_ms: u64) -> f32 {
        if self.total_interactions == 0 {
            return 0.5; // Day-zero neutral
        }

        // Default reference interval: 7 days (168 hours) when we have only 1
        // interaction and no computed average yet.  This makes a recent single
        // interaction score high and a stale one score low, rather than always
        // returning the neutral 0.5.
        let reference_hours = if self.avg_interaction_interval_hours > 0.0 {
            self.avg_interaction_interval_hours
        } else {
            168.0 // 7-day default
        };

        let elapsed_hours =
            now_ms.saturating_sub(self.last_interaction_ms) as f32 / 3_600_000.0;
        let ratio = elapsed_hours / reference_hours;

        // ratio = 0 (just interacted) → score 1.0
        // ratio = 1 (right on schedule) → score ~0.75
        // ratio = 2 (2× overdue) → score ~0.5 (drift threshold)
        // ratio = 4 (4× overdue) → score ~0.25
        // ratio ≥ 5 → floor ~0.1
        let score = 1.0 / (1.0 + ratio * 0.4);
        score.clamp(0.0, 1.0)
    }
}

// ---------------------------------------------------------------------------
// RelationshipArc
// ---------------------------------------------------------------------------

/// Tracks the health of the user's relationships portfolio.
///
/// No assumptions about relationship structure — user defines who matters
/// and at what importance level.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelationshipArc {
    /// Current health level.
    pub health: ArcHealth,
    /// All tracked relationships (bounded at MAX_RELATIONSHIPS).
    pub relationships: Vec<TrackedRelationship>,
    /// Unix ms timestamp of the last proactive trigger (0 = never).
    pub last_trigger_ms: u64,
    /// Total events recorded (lifetime, for day-zero guard).
    pub total_events: u32,
}

impl RelationshipArc {
    /// Create a new relationship arc in the day-zero state.
    #[must_use]
    pub fn new() -> Self {
        Self {
            health: ArcHealth::Stable,
            relationships: Vec::with_capacity(16),
            last_trigger_ms: 0,
            total_events: 0,
        }
    }

    /// Record an interaction with a person identified by `person_id`.
    ///
    /// If the person is not yet tracked and capacity allows, adds them
    /// with default importance 5. Returns an error string if at capacity.
    pub fn record_interaction(
        &mut self,
        person_id: &str,
        display_name: Option<&str>,
        importance: Option<u8>,
        now_ms: u64,
    ) -> Result<(), &'static str> {
        self.total_events = self.total_events.saturating_add(1);

        if let Some(rel) = self.relationships.iter_mut().find(|r| r.person_id == person_id) {
            rel.record_interaction(now_ms);
            return Ok(());
        }

        // New person — check capacity.
        if self.relationships.len() >= MAX_RELATIONSHIPS {
            return Err("relationship capacity exceeded");
        }

        let name = display_name.unwrap_or(person_id).to_string();
        let imp = importance.unwrap_or(5);
        let mut rel = TrackedRelationship::new(person_id.to_string(), name, imp);
        rel.record_interaction(now_ms);
        self.relationships.push(rel);
        Ok(())
    }

    /// Update the importance level for a tracked relationship.
    pub fn set_importance(&mut self, person_id: &str, importance: u8) {
        if let Some(rel) = self.relationships.iter_mut().find(|r| r.person_id == person_id) {
            rel.user_importance_level = importance.clamp(1, 10);
        }
    }

    /// Compute the overall relationship arc score in `[0.0, 1.0]`.
    ///
    /// Weighted by importance level. Day-zero: returns 0.5.
    #[must_use]
    pub fn score(&self, now_ms: u64) -> f32 {
        if self.relationships.is_empty() {
            return 0.5; // Day-zero neutral
        }

        let mut weighted_sum = 0.0_f32;
        let mut weight_sum = 0.0_f32;

        for rel in &self.relationships {
            let w = rel.user_importance_level as f32;
            let s = rel.interaction_health_score(now_ms);
            weighted_sum += w * s;
            weight_sum += w;
        }

        if weight_sum > 0.0 {
            (weighted_sum / weight_sum).clamp(0.0, 1.0)
        } else {
            0.5
        }
    }

    /// Recompute and update the health level from the current score.
    pub fn update_health(&mut self, now_ms: u64) {
        let s = self.score(now_ms);
        self.health = ArcHealth::from_score(s);
    }

    /// Returns references to relationships that appear to be drifting.
    ///
    /// Drift = no interaction in >2× the person's average interval.
    #[must_use]
    pub fn drifting_relationships(&self, now_ms: u64) -> Vec<&TrackedRelationship> {
        self.relationships
            .iter()
            .filter(|r| r.is_drifting(now_ms))
            .collect()
    }

    /// Check whether a proactive trigger should fire.
    ///
    /// Triggers when:
    /// 1. Enough data exists (day-zero guard).
    /// 2. Health is AtRisk or NeedsAttention OR important relationships are drifting.
    /// 3. 24-hour cooldown has elapsed.
    #[must_use]
    pub fn check_proactive_trigger(&self, now_ms: u64) -> Option<ProactiveTrigger> {
        // Day-zero guard.
        if self.total_events < MIN_EVENTS_FOR_TRIGGER {
            return None;
        }

        let drifting = self.drifting_relationships(now_ms);
        let high_importance_drifting = drifting
            .iter()
            .any(|r| r.user_importance_level >= 7);

        let should_trigger = self.health.warrants_proactive() || high_importance_drifting;
        if !should_trigger {
            return None;
        }

        // Enforce 24-hour cooldown.
        if self.last_trigger_ms > 0
            && now_ms.saturating_sub(self.last_trigger_ms) < ONE_DAY_MS
        {
            return None;
        }

        Some(ProactiveTrigger {
            arc_type: ArcType::Relationship,
            health: self.health.clone(),
            triggered_at_ms: now_ms,
            context_for_llm: self.to_llm_context(now_ms),
        })
    }

    /// Acknowledge that a trigger was fired.
    pub fn mark_trigger_fired(&mut self, now_ms: u64) {
        self.last_trigger_ms = now_ms;
    }

    /// Build structured factual context string for LLM injection.
    ///
    /// Lists tracked relationships, their 30-day activity, and any drift signals.
    /// The LLM reasons about what (if anything) to say.
    #[must_use]
    pub fn to_llm_context(&self, now_ms: u64) -> String {
        let drifting = self.drifting_relationships(now_ms);
        let drifting_names: Vec<&str> = drifting
            .iter()
            .map(|r| r.display_name.as_str())
            .collect();

        let rel_summary: Vec<String> = self
            .relationships
            .iter()
            .take(10) // Cap at 10 to avoid LLM context overflow
            .map(|r| {
                format!(
                    "{}(importance={},interactions_30d={},last_interaction_days_ago={:.1})",
                    r.display_name,
                    r.user_importance_level,
                    r.interaction_count_30d,
                    if r.last_interaction_ms == 0 {
                        f32::INFINITY
                    } else {
                        now_ms.saturating_sub(r.last_interaction_ms) as f32
                            / ONE_DAY_MS as f32
                    }
                )
            })
            .collect();

        format!(
            "[relationship_arc] health={health} \
             tracked_relationships={total} \
             drifting=[{drifting}] \
             relationships=[{rels}] \
             total_events_lifetime={events}",
            health = self.health.label(),
            total = self.relationships.len(),
            drifting = drifting_names.join(","),
            rels = rel_summary.join("; "),
            events = self.total_events,
        )
    }
}

impl Default for RelationshipArc {
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

    const T0: u64 = 2_000_000_000_u64; // arbitrary baseline (ms)
    const ONE_HOUR: u64 = 3_600_000;
    const ONE_WEEK: u64 = 7 * ONE_DAY_MS;

    #[test]
    fn test_day_zero_stable() {
        let arc = RelationshipArc::new();
        assert_eq!(arc.health, ArcHealth::Stable);
        assert_eq!(arc.score(T0), 0.5);
    }

    #[test]
    fn test_no_trigger_day_zero() {
        let arc = RelationshipArc::new();
        assert!(arc.check_proactive_trigger(T0).is_none());
    }

    #[test]
    fn test_add_relationship_and_interact() {
        let mut arc = RelationshipArc::new();
        arc.record_interaction("alice", Some("Alice"), Some(8), T0)
            .expect("should add");
        assert_eq!(arc.relationships.len(), 1);
        assert_eq!(arc.relationships[0].interaction_count_30d, 1);
    }

    #[test]
    fn test_drift_detection() {
        let mut arc = RelationshipArc::new();
        // Simulate regular weekly interactions.
        for week in 0..4u64 {
            arc.record_interaction("bob", Some("Bob"), Some(7), T0 + week * ONE_WEEK)
                .expect("ok");
        }
        // Bob's avg interval is ~1 week. Now simulate 3 weeks of silence.
        let now = T0 + 4 * ONE_WEEK + 3 * ONE_WEEK;
        let drifting = arc.drifting_relationships(now);
        assert_eq!(drifting.len(), 1, "Bob should be drifting");
    }

    #[test]
    fn test_no_drift_when_recent() {
        let mut arc = RelationshipArc::new();
        for week in 0..4u64 {
            arc.record_interaction("carol", Some("Carol"), Some(5), T0 + week * ONE_WEEK)
                .expect("ok");
        }
        // Just interacted — should not be drifting.
        let now = T0 + 4 * ONE_WEEK + ONE_HOUR;
        let drifting = arc.drifting_relationships(now);
        assert!(drifting.is_empty(), "Carol just interacted, should not drift");
    }

    #[test]
    fn test_capacity_limit() {
        let mut arc = RelationshipArc::new();
        for i in 0..MAX_RELATIONSHIPS {
            arc.record_interaction(
                &format!("person_{i}"),
                None,
                None,
                T0 + i as u64 * ONE_HOUR,
            )
            .expect("within capacity");
        }
        // One more should fail.
        let result =
            arc.record_interaction("overflow_person", None, None, T0 + 9999 * ONE_HOUR);
        assert!(result.is_err(), "should hit capacity limit");
    }

    #[test]
    fn test_importance_weights_score() {
        let mut arc = RelationshipArc::new();
        // High-importance person with recent interaction.
        arc.record_interaction("vip", Some("VIP"), Some(10), T0)
            .expect("ok");
        // Low-importance person with stale interaction.
        arc.record_interaction("acquaintance", Some("Acquaintance"), Some(1), T0.saturating_sub(30 * ONE_DAY_MS))
            .expect("ok");

        // Score at T0 + 1 hour: VIP was just seen (high weight) → should pull score up.
        let s = arc.score(T0 + ONE_HOUR);
        assert!(s > 0.5, "high-importance recent interaction should improve score, got {s}");
    }

    #[test]
    fn test_trigger_cooldown() {
        let mut arc = RelationshipArc::new();
        // Add enough events.
        for i in 0..MIN_EVENTS_FOR_TRIGGER {
            arc.record_interaction("dave", Some("Dave"), Some(9), T0 + i as u64 * ONE_HOUR)
                .expect("ok");
        }
        arc.health = ArcHealth::NeedsAttention;

        let t1 = arc.check_proactive_trigger(T0 + 10 * ONE_HOUR);
        assert!(t1.is_some());
        arc.mark_trigger_fired(T0 + 10 * ONE_HOUR);

        // Within 24h → no trigger.
        let t2 = arc.check_proactive_trigger(T0 + 20 * ONE_HOUR);
        assert!(t2.is_none());

        // After 24h → trigger again.
        let t3 = arc.check_proactive_trigger(T0 + 10 * ONE_HOUR + ONE_DAY_MS);
        assert!(t3.is_some());
    }

    #[test]
    fn test_llm_context_structure() {
        let mut arc = RelationshipArc::new();
        arc.record_interaction("eve", Some("Eve"), Some(6), T0)
            .expect("ok");
        let ctx = arc.to_llm_context(T0 + ONE_HOUR);
        assert!(ctx.contains("[relationship_arc]"));
        assert!(ctx.contains("health="));
        assert!(ctx.contains("tracked_relationships="));
    }
}
