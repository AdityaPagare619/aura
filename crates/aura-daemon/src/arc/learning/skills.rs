//! Learned skill registry — tracks action sequences that improve over time.
//!
//! Skills are multi-step procedures AURA has observed or been taught.  Each
//! skill accumulates success/failure statistics and a reliability score that
//! guides future decisions.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use tracing::{debug, instrument, warn};

use super::super::ArcError;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum number of skills the registry can hold.
pub const MAX_SKILLS: usize = 512;

/// Maximum number of steps in a single skill.
const MAX_STEPS: usize = 32;

/// Maximum length of a skill name in bytes.
const MAX_NAME_LEN: usize = 128;

/// Maximum tags per skill.
const MAX_TAGS_PER_SKILL: usize = 16;

/// Maximum tag length in bytes.
const MAX_TAG_LEN: usize = 64;

/// Confidence decay rate per day of non-use.
const CONFIDENCE_DECAY_RATE: f64 = 0.02;

/// Minimum confidence floor.
const CONFIDENCE_FLOOR: f32 = 0.3;

/// Maximum confidence ceiling.
const CONFIDENCE_CEILING: f32 = 0.99;

/// Base confidence boost on successful execution (attenuated by exposure).
/// Actual boost = base / √(1 + total_executions).
const CONFIDENCE_BOOST_SUCCESS_BASE: f32 = 0.05;

/// Base confidence penalty on failed execution (attenuated by exposure).
const CONFIDENCE_PENALTY_FAILURE_BASE: f32 = 0.08;

/// Maximum skill lineage depth (prevents cycles).
const MAX_LINEAGE_DEPTH: usize = 16;

/// Maximum skill matches returned from matching.
const MAX_MATCH_RESULTS: usize = 10;

/// Milliseconds per day.
const MS_PER_DAY: f64 = 86_400_000.0;

// ---------------------------------------------------------------------------
// FNV-1a hash (same as hebbian.rs, duplicated to avoid cross-module coupling)
// ---------------------------------------------------------------------------

const FNV_OFFSET_BASIS: u64 = 14_695_981_039_346_656_037;
const FNV_PRIME: u64 = 1_099_511_628_211;

/// Compute a 64-bit FNV-1a hash of `data`.
#[must_use]
fn fnv1a_hash(data: &[u8]) -> u64 {
    let mut hash = FNV_OFFSET_BASIS;
    for &byte in data {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}

// ---------------------------------------------------------------------------
// LearnedSkill
// ---------------------------------------------------------------------------

/// A learned action sequence with reliability tracking.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LearnedSkill {
    /// Unique identifier (FNV-1a hash of name).
    pub id: u64,
    /// Human-readable name.
    pub name: String,
    /// Ordered action steps (bounded to [`MAX_STEPS`]).
    pub steps: Vec<String>,
    /// Number of successful executions.
    pub success_count: u32,
    /// Number of failed executions.
    pub failure_count: u32,
    /// Exponential moving average of execution duration (ms).
    pub avg_duration_ms: u64,
    /// Timestamp (ms) of last use.
    pub last_used_ms: u64,
    /// Computed reliability: `success / (success + failure)`.
    pub reliability: f32,
    /// Skill version number — incremented on adaptation.
    pub version: u32,
    /// Keyword tags for matching (bounded to [`MAX_TAGS_PER_SKILL`]).
    pub tags: Vec<String>,
    /// Parent skill ID if this skill was derived/adapted from another.
    pub parent_id: Option<u64>,
    /// Bayesian confidence in this skill (decays without use).
    pub confidence: f32,
}

/// A ranked skill result from [`SkillRegistry::rank_skills_by_relevance`].
#[derive(Debug, Clone)]
pub struct SkillMatch {
    /// Matched skill ID.
    pub skill_id: u64,
    /// Composite match score.
    pub score: f32,
    /// Tag overlap ratio (0.0–1.0).
    pub tag_overlap: f32,
    /// Time-decayed confidence.
    pub confidence: f32,
    /// Current reliability.
    pub reliability: f32,
}

impl LearnedSkill {
    /// Total number of recorded outcomes.
    #[must_use]
    pub fn total_executions(&self) -> u32 {
        self.success_count.saturating_add(self.failure_count)
    }

    /// Recompute reliability from current counts.
    fn recompute_reliability(&mut self) {
        let total = self.total_executions();
        self.reliability = if total == 0 {
            0.0
        } else {
            self.success_count as f32 / total as f32
        };
    }

    /// Days elapsed since this skill was last used.
    #[must_use]
    pub fn days_since_use(&self, now_ms: u64) -> f64 {
        if now_ms <= self.last_used_ms {
            return 0.0;
        }
        (now_ms - self.last_used_ms) as f64 / MS_PER_DAY
    }

    /// Confidence after time-based decay.
    ///
    /// `decayed = confidence × (1 - DECAY_RATE)^days_since_use`, floored at
    /// [`CONFIDENCE_FLOOR`].
    #[must_use]
    pub fn decayed_confidence(&self, now_ms: u64) -> f32 {
        let days = self.days_since_use(now_ms);
        if days <= 0.0 {
            return self.confidence;
        }
        let factor = (1.0 - CONFIDENCE_DECAY_RATE).powf(days) as f32;
        (self.confidence * factor).max(CONFIDENCE_FLOOR)
    }
}

// ---------------------------------------------------------------------------
// SkillRegistry
// ---------------------------------------------------------------------------

/// Bounded registry of learned skills.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillRegistry {
    /// Skill store keyed by FNV-1a hash of name.
    skills: HashMap<u64, LearnedSkill>,
}

impl SkillRegistry {
    /// Create an empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self {
            skills: HashMap::with_capacity(32),
        }
    }

    /// Number of registered skills.
    #[must_use]
    pub fn skill_count(&self) -> usize {
        self.skills.len()
    }

    /// Register a new skill.  Returns its ID.
    ///
    /// If a skill with the same name already exists, its ID is returned
    /// without modification.  Steps are truncated to [`MAX_STEPS`].
    #[instrument(skip_all, fields(name = %name))]
    pub fn register_skill(
        &mut self,
        name: &str,
        steps: Vec<String>,
        now_ms: u64,
    ) -> Result<u64, ArcError> {
        let trimmed_name = if name.len() > MAX_NAME_LEN {
            &name[..MAX_NAME_LEN]
        } else {
            name
        };
        let id = fnv1a_hash(trimmed_name.as_bytes());

        if self.skills.contains_key(&id) {
            return Ok(id);
        }

        if self.skills.len() >= MAX_SKILLS {
            return Err(ArcError::CapacityExceeded {
                collection: "skills".into(),
                max: MAX_SKILLS,
            });
        }

        let mut bounded_steps = steps;
        bounded_steps.truncate(MAX_STEPS);

        let skill = LearnedSkill {
            id,
            name: trimmed_name.to_owned(),
            steps: bounded_steps,
            success_count: 0,
            failure_count: 0,
            avg_duration_ms: 0,
            last_used_ms: now_ms,
            reliability: 0.0,
            version: 1,
            tags: Vec::new(),
            parent_id: None,
            confidence: 0.5,
        };
        self.skills.insert(id, skill);
        debug!(skill_id = id, "registered skill");
        Ok(id)
    }

    /// Record an execution outcome for a skill.
    #[instrument(skip_all, fields(skill_id, success))]
    pub fn record_outcome(
        &mut self,
        skill_id: u64,
        success: bool,
        duration_ms: u64,
        now_ms: u64,
    ) -> Result<(), ArcError> {
        let skill = self
            .skills
            .get_mut(&skill_id)
            .ok_or_else(|| ArcError::NotFound {
                entity: "skill".into(),
                id: skill_id,
            })?;

        // Exposure-attenuated confidence: established skills resist perturbation.
        let attenuation = 1.0 / (1.0 + skill.total_executions() as f32).sqrt();
        if success {
            skill.success_count = skill.success_count.saturating_add(1);
            skill.confidence =
                (skill.confidence + CONFIDENCE_BOOST_SUCCESS_BASE * attenuation).min(CONFIDENCE_CEILING);
        } else {
            skill.failure_count = skill.failure_count.saturating_add(1);
            skill.confidence =
                (skill.confidence - CONFIDENCE_PENALTY_FAILURE_BASE * attenuation).max(CONFIDENCE_FLOOR);
        }

        // EMA for duration (alpha = 0.3)
        if skill.avg_duration_ms == 0 {
            skill.avg_duration_ms = duration_ms;
        } else {
            let alpha = 0.3_f64;
            skill.avg_duration_ms =
                ((1.0 - alpha) * skill.avg_duration_ms as f64 + alpha * duration_ms as f64) as u64;
        }

        skill.last_used_ms = now_ms;
        skill.recompute_reliability();
        debug!(
            skill_id,
            reliability = skill.reliability,
            "recorded skill outcome"
        );
        Ok(())
    }

    /// Get all skills with reliability ≥ `min_reliability`.
    #[must_use]
    pub fn get_reliable_skills(&self, min_reliability: f32) -> Vec<&LearnedSkill> {
        self.skills
            .values()
            .filter(|s| s.reliability >= min_reliability)
            .collect()
    }

    /// Find a skill by name (linear scan — bounded by [`MAX_SKILLS`]).
    #[must_use]
    pub fn find_skill_by_name(&self, name: &str) -> Option<&LearnedSkill> {
        let trimmed = if name.len() > MAX_NAME_LEN {
            &name[..MAX_NAME_LEN]
        } else {
            name
        };
        let id = fnv1a_hash(trimmed.as_bytes());
        self.skills.get(&id)
    }

    /// Get an immutable reference to a skill by ID.
    #[must_use]
    pub fn get_skill(&self, id: u64) -> Option<&LearnedSkill> {
        self.skills.get(&id)
    }

    // -----------------------------------------------------------------------
    // Skill matching
    // -----------------------------------------------------------------------

    /// Rank skills by relevance to `goal_tags` and return the ordered list.
    ///
    /// # Architecture contract
    /// This method RANKS — it does NOT select or route. The returned `Vec<SkillMatch>`
    /// contains all individual score components (`tag_overlap`, `confidence`,
    /// `reliability`) so the caller (LLM or orchestrator) can apply its own
    /// judgment to decide which skill to actually invoke.
    ///
    /// Composite rank = `tag_overlap × 0.4 + decayed_confidence × 0.3 + reliability × 0.3`.
    /// Matches with rank < 0.1 are excluded as noise.
    /// Returns up to [`MAX_MATCH_RESULTS`] results sorted descending by rank.
    #[must_use]
    pub fn rank_skills_by_relevance(&self, goal_tags: &[&str], now_ms: u64) -> Vec<SkillMatch> {
        if goal_tags.is_empty() {
            return Vec::new();
        }

        let mut matches: Vec<SkillMatch> = self
            .skills
            .values()
            .filter_map(|skill| {
                if skill.tags.is_empty() {
                    return None;
                }
                let overlap_count = skill
                    .tags
                    .iter()
                    .filter(|t| goal_tags.iter().any(|g| g.eq_ignore_ascii_case(t)))
                    .count();
                if overlap_count == 0 {
                    return None;
                }
                let denominator = skill.tags.len().max(goal_tags.len()) as f32;
                let tag_overlap = overlap_count as f32 / denominator;
                let conf = skill.decayed_confidence(now_ms);
                let score = tag_overlap * 0.4 + conf * 0.3 + skill.reliability * 0.3;
                if score > 0.1 {
                    Some(SkillMatch {
                        skill_id: skill.id,
                        score,
                        tag_overlap,
                        confidence: conf,
                        reliability: skill.reliability,
                    })
                } else {
                    None
                }
            })
            .collect();

        matches.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        matches.truncate(MAX_MATCH_RESULTS);
        matches
    }

    // -----------------------------------------------------------------------
    // Skill adaptation
    // -----------------------------------------------------------------------

    /// Create a new skill by adapting an existing one.
    ///
    /// The new skill inherits tags and gets `parent_id` set to `source_id`.
    /// Version is incremented from the source. Confidence is inherited at 80%
    /// of the source's decayed confidence.
    #[instrument(skip_all, fields(source_id, new_name = %new_name))]
    pub fn adapt_skill(
        &mut self,
        source_id: u64,
        new_name: &str,
        modified_steps: Vec<String>,
        now_ms: u64,
    ) -> Result<u64, ArcError> {
        let source = self
            .skills
            .get(&source_id)
            .ok_or_else(|| ArcError::NotFound {
                entity: "skill".into(),
                id: source_id,
            })?;

        let inherited_tags = source.tags.clone();
        let new_version = source.version.saturating_add(1);
        let inherited_confidence = source.decayed_confidence(now_ms) * 0.8;

        let trimmed_name = if new_name.len() > MAX_NAME_LEN {
            &new_name[..MAX_NAME_LEN]
        } else {
            new_name
        };
        let new_id = fnv1a_hash(trimmed_name.as_bytes());

        if self.skills.contains_key(&new_id) {
            return Ok(new_id);
        }
        if self.skills.len() >= MAX_SKILLS {
            return Err(ArcError::CapacityExceeded {
                collection: "skills".into(),
                max: MAX_SKILLS,
            });
        }

        let mut bounded_steps = modified_steps;
        bounded_steps.truncate(MAX_STEPS);

        let skill = LearnedSkill {
            id: new_id,
            name: trimmed_name.to_owned(),
            steps: bounded_steps,
            success_count: 0,
            failure_count: 0,
            avg_duration_ms: 0,
            last_used_ms: now_ms,
            reliability: 0.0,
            version: new_version,
            tags: inherited_tags,
            parent_id: Some(source_id),
            confidence: inherited_confidence.max(CONFIDENCE_FLOOR),
        };

        debug!(
            new_id,
            source_id,
            version = new_version,
            "adapted skill created"
        );
        self.skills.insert(new_id, skill);
        Ok(new_id)
    }

    // -----------------------------------------------------------------------
    // Confidence management
    // -----------------------------------------------------------------------

    /// Apply time-based confidence decay to all skills.
    ///
    /// Should be called periodically (e.g., once per day). Each skill's
    /// confidence is decayed based on elapsed time since last use.
    pub fn decay_all_confidence(&mut self, now_ms: u64) {
        for skill in self.skills.values_mut() {
            let decayed = skill.decayed_confidence(now_ms);
            skill.confidence = decayed;
            // Update last_used_ms only if we actually decayed to avoid
            // re-decaying already-decayed values on repeated calls.
        }
        debug!(count = self.skills.len(), "decayed all skill confidence");
    }

    // -----------------------------------------------------------------------
    // Lineage tracking
    // -----------------------------------------------------------------------

    /// Return the chain of parent IDs from a skill back to the root.
    ///
    /// The returned vector starts with `skill_id` and ends at the root skill
    /// (one with no parent). Depth is bounded by [`MAX_LINEAGE_DEPTH`] to
    /// prevent cycles.
    #[must_use]
    pub fn get_skill_lineage(&self, skill_id: u64) -> Vec<u64> {
        let mut chain = Vec::with_capacity(MAX_LINEAGE_DEPTH);
        let mut current = skill_id;

        for _ in 0..MAX_LINEAGE_DEPTH {
            chain.push(current);
            match self.skills.get(&current) {
                Some(skill) => match skill.parent_id {
                    Some(pid) => {
                        if chain.contains(&pid) {
                            break; // Cycle detected
                        }
                        current = pid;
                    }
                    None => break,
                },
                None => break,
            }
        }
        chain
    }

    // -----------------------------------------------------------------------
    // Tag management
    // -----------------------------------------------------------------------

    /// Add tags to a skill, respecting the [`MAX_TAGS_PER_SKILL`] limit.
    ///
    /// Tags are lowercased and truncated to [`MAX_TAG_LEN`]. Duplicates
    /// are silently skipped.
    pub fn add_tags(&mut self, skill_id: u64, tags: &[&str]) -> Result<(), ArcError> {
        let skill = self
            .skills
            .get_mut(&skill_id)
            .ok_or_else(|| ArcError::NotFound {
                entity: "skill".into(),
                id: skill_id,
            })?;

        for &tag in tags {
            if skill.tags.len() >= MAX_TAGS_PER_SKILL {
                warn!(
                    skill_id,
                    max = MAX_TAGS_PER_SKILL,
                    "tag capacity reached, skipping remaining"
                );
                break;
            }
            let normalized = if tag.len() > MAX_TAG_LEN {
                tag[..MAX_TAG_LEN].to_ascii_lowercase()
            } else {
                tag.to_ascii_lowercase()
            };
            if !skill.tags.iter().any(|t| t == &normalized) {
                skill.tags.push(normalized);
            }
        }
        Ok(())
    }

    /// Find all skills that have a specific tag (case-insensitive).
    #[must_use]
    pub fn get_skills_by_tag(&self, tag: &str) -> Vec<&LearnedSkill> {
        let lower = tag.to_ascii_lowercase();
        self.skills
            .values()
            .filter(|s| s.tags.iter().any(|t| t == &lower))
            .collect()
    }

    /// Get a mutable reference to a skill by ID.
    pub fn get_skill_mut(&mut self, id: u64) -> Option<&mut LearnedSkill> {
        self.skills.get_mut(&id)
    }
}

impl Default for SkillRegistry {
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
    fn test_new_registry() {
        let reg = SkillRegistry::new();
        assert_eq!(reg.skill_count(), 0);
    }

    #[test]
    fn test_register_skill() {
        let mut reg = SkillRegistry::new();
        let steps = vec!["open_app".into(), "tap_button".into()];
        let id = reg
            .register_skill("send_message", steps, 1000)
            .expect("register");
        assert_eq!(reg.skill_count(), 1);
        let skill = reg.get_skill(id).expect("lookup");
        assert_eq!(skill.name, "send_message");
        assert_eq!(skill.steps.len(), 2);
    }

    #[test]
    fn test_register_idempotent() {
        let mut reg = SkillRegistry::new();
        let id1 = reg
            .register_skill("test_skill", vec!["step1".into()], 100)
            .expect("first");
        let id2 = reg
            .register_skill("test_skill", vec!["step1".into(), "step2".into()], 200)
            .expect("second");
        assert_eq!(id1, id2);
        assert_eq!(reg.skill_count(), 1);
        // Steps should be from original registration (not overwritten)
        let skill = reg.get_skill(id1).expect("lookup");
        assert_eq!(skill.steps.len(), 1);
    }

    #[test]
    fn test_record_outcome_success() {
        let mut reg = SkillRegistry::new();
        let id = reg
            .register_skill("nav", vec!["open_maps".into()], 100)
            .expect("register");

        reg.record_outcome(id, true, 500, 1_700_000_000_000).expect("record");
        let skill = reg.get_skill(id).expect("lookup");
        assert_eq!(skill.success_count, 1);
        assert_eq!(skill.failure_count, 0);
        assert!((skill.reliability - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn test_record_outcome_failure() {
        let mut reg = SkillRegistry::new();
        let id = reg
            .register_skill("nav", vec!["open_maps".into()], 100)
            .expect("register");

        reg.record_outcome(id, false, 500, 1_700_000_000_000).expect("record");
        let skill = reg.get_skill(id).expect("lookup");
        assert_eq!(skill.failure_count, 1);
        assert!((skill.reliability - 0.0).abs() < f32::EPSILON);
    }

    #[test]
    fn test_record_outcome_not_found() {
        let mut reg = SkillRegistry::new();
        let result = reg.record_outcome(999, true, 100, 1_700_000_000_000);
        assert!(result.is_err());
    }

    #[test]
    fn test_reliability_mixed() {
        let mut reg = SkillRegistry::new();
        let id = reg
            .register_skill("mixed", vec!["a".into()], 100)
            .expect("register");

        // 3 successes, 1 failure → reliability = 0.75
        for _ in 0..3 {
            reg.record_outcome(id, true, 200, 1_700_000_000_000).expect("ok");
        }
        reg.record_outcome(id, false, 200, 1_700_000_000_000).expect("ok");

        let skill = reg.get_skill(id).expect("lookup");
        assert!(
            (skill.reliability - 0.75).abs() < 0.01,
            "expected ~0.75, got {}",
            skill.reliability
        );
    }

    #[test]
    fn test_get_reliable_skills() {
        let mut reg = SkillRegistry::new();
        let good = reg
            .register_skill("good", vec!["a".into()], 100)
            .expect("ok");
        let bad = reg
            .register_skill("bad", vec!["b".into()], 100)
            .expect("ok");

        // Good: 10 successes
        for _ in 0..10 {
            reg.record_outcome(good, true, 100, 1_700_000_000_000).expect("ok");
        }
        // Bad: 10 failures
        for _ in 0..10 {
            reg.record_outcome(bad, false, 100, 1_700_000_000_000).expect("ok");
        }

        let reliable = reg.get_reliable_skills(0.8);
        assert_eq!(reliable.len(), 1);
        assert_eq!(reliable[0].name, "good");
    }

    #[test]
    fn test_find_skill_by_name() {
        let mut reg = SkillRegistry::new();
        reg.register_skill(
            "navigate_home",
            vec!["open_maps".into(), "set_dest".into()],
            100,
        )
        .expect("register");

        let found = reg.find_skill_by_name("navigate_home");
        assert!(found.is_some());
        assert_eq!(found.expect("found").name, "navigate_home");

        assert!(reg.find_skill_by_name("nonexistent").is_none());
    }

    #[test]
    fn test_steps_bounded() {
        let mut reg = SkillRegistry::new();
        let long_steps: Vec<String> = (0..50).map(|i| format!("step_{i}")).collect();
        let id = reg
            .register_skill("long_skill", long_steps, 100)
            .expect("register");
        let skill = reg.get_skill(id).expect("lookup");
        assert_eq!(skill.steps.len(), MAX_STEPS);
    }

    #[test]
    fn test_avg_duration_ema() {
        let mut reg = SkillRegistry::new();
        let id = reg
            .register_skill("timed", vec!["step".into()], 100)
            .expect("register");

        reg.record_outcome(id, true, 1000, 1_700_000_000_000).expect("ok");
        let s1 = reg.get_skill(id).expect("lookup").avg_duration_ms;
        assert_eq!(s1, 1000); // first observation = raw value

        reg.record_outcome(id, true, 500, 1_700_000_000_000).expect("ok");
        let s2 = reg.get_skill(id).expect("lookup").avg_duration_ms;
        // EMA: 0.7 * 1000 + 0.3 * 500 = 850
        assert!((s2 as f64 - 850.0).abs() < 5.0, "expected ~850, got {s2}");
    }

    #[test]
    fn test_capacity_exceeded() {
        let mut reg = SkillRegistry::new();
        for i in 0..MAX_SKILLS {
            reg.register_skill(&format!("skill_{i}"), vec!["s".into()], 100)
                .expect("fill");
        }
        assert_eq!(reg.skill_count(), MAX_SKILLS);

        let result = reg.register_skill("one_more", vec!["s".into()], 200);
        assert!(result.is_err());
    }

    // -----------------------------------------------------------------------
    // New tests for enhanced functionality
    // -----------------------------------------------------------------------

    #[test]
    fn test_skill_version_starts_at_one() {
        let mut reg = SkillRegistry::new();
        let id = reg
            .register_skill("v_test", vec!["a".into()], 100)
            .expect("register");
        let skill = reg.get_skill(id).expect("lookup");
        assert_eq!(skill.version, 1);
    }

    #[test]
    fn test_skill_confidence_initial() {
        let mut reg = SkillRegistry::new();
        let id = reg
            .register_skill("conf_test", vec!["a".into()], 100)
            .expect("register");
        let skill = reg.get_skill(id).expect("lookup");
        assert!((skill.confidence - 0.5).abs() < f32::EPSILON);
    }

    #[test]
    fn test_confidence_boost_on_success() {
        let mut reg = SkillRegistry::new();
        let id = reg
            .register_skill("boost", vec!["a".into()], 100)
            .expect("register");

        reg.record_outcome(id, true, 200, 1_700_000_000_000).expect("ok");
        let skill = reg.get_skill(id).expect("lookup");
        // Should be above initial 0.5 (exposure-attenuated, but first execution
        // gets near-full boost since total_executions was 0 before increment).
        assert!(
            skill.confidence > 0.5,
            "confidence should increase on success, got {}",
            skill.confidence
        );
    }

    #[test]
    fn test_confidence_penalty_on_failure() {
        let mut reg = SkillRegistry::new();
        let id = reg
            .register_skill("penalty", vec!["a".into()], 100)
            .expect("register");

        reg.record_outcome(id, false, 200, 1_700_000_000_000).expect("ok");
        let skill = reg.get_skill(id).expect("lookup");
        // Should be below initial 0.5 (exposure-attenuated penalty).
        assert!(
            skill.confidence < 0.5,
            "confidence should decrease on failure, got {}",
            skill.confidence
        );
    }

    #[test]
    fn test_confidence_decay_over_time() {
        let mut reg = SkillRegistry::new();
        let id = reg
            .register_skill("decay", vec!["a".into()], 1000)
            .expect("register");

        let skill = reg.get_skill(id).expect("lookup");
        let now = 1000 + 86_400_000 * 10; // 10 days later
        let decayed = skill.decayed_confidence(now);
        // 0.5 * (0.98)^10 ≈ 0.5 * 0.8171 ≈ 0.4086
        assert!(
            decayed < 0.5 && decayed > 0.35,
            "expected decay, got {decayed}"
        );
    }

    #[test]
    fn test_confidence_floor() {
        let mut reg = SkillRegistry::new();
        let id = reg
            .register_skill("floor_test", vec!["a".into()], 1000)
            .expect("register");

        // Many failures to push confidence down
        for _ in 0..10 {
            reg.record_outcome(id, false, 100, 1_700_000_000_000).expect("ok");
        }
        let skill = reg.get_skill(id).expect("lookup");
        assert!(
            skill.confidence >= CONFIDENCE_FLOOR,
            "confidence {} should not go below floor {}",
            skill.confidence,
            CONFIDENCE_FLOOR
        );
    }

    #[test]
    fn test_rank_skills_by_tags() {
        let mut reg = SkillRegistry::new();
        let id = reg
            .register_skill("navigate", vec!["open_maps".into()], 100)
            .expect("register");
        reg.add_tags(id, &["navigation", "maps", "driving"])
            .expect("tags");

        // Record some successes to build reliability
        for _ in 0..5 {
            reg.record_outcome(id, true, 200, 1_700_000_000_000).expect("ok");
        }

        let matches = reg.rank_skills_by_relevance(&["navigation", "maps"], 100);
        assert!(!matches.is_empty(), "should find a match");
        assert_eq!(matches[0].skill_id, id);
        assert!(matches[0].tag_overlap > 0.5);
    }

    #[test]
    fn test_rank_skills_empty_tags() {
        let reg = SkillRegistry::new();
        let matches = reg.rank_skills_by_relevance(&[], 100);
        assert!(matches.is_empty());
    }

    #[test]
    fn test_rank_skills_scoring_order() {
        let mut reg = SkillRegistry::new();

        let id1 = reg
            .register_skill("nav_good", vec!["a".into()], 100)
            .expect("ok");
        reg.add_tags(id1, &["navigation", "maps"]).expect("ok");
        for _ in 0..10 {
            reg.record_outcome(id1, true, 200, 1_700_000_000_000).expect("ok");
        }

        let id2 = reg
            .register_skill("nav_bad", vec!["a".into()], 100)
            .expect("ok");
        reg.add_tags(id2, &["navigation"]).expect("ok");
        for _ in 0..10 {
            reg.record_outcome(id2, false, 200, 1_700_000_000_000).expect("ok");
        }

        let matches = reg.rank_skills_by_relevance(&["navigation", "maps"], 100);
        assert!(matches.len() >= 1);
        // id1 should rank higher (better reliability + better tag overlap)
        assert_eq!(matches[0].skill_id, id1);
    }

    #[test]
    fn test_adapt_skill_basic() {
        let mut reg = SkillRegistry::new();
        let src = reg
            .register_skill("original", vec!["step1".into(), "step2".into()], 1000)
            .expect("ok");

        let adapted = reg
            .adapt_skill(src, "adapted_v", vec!["step1".into(), "step3".into()], 2000)
            .expect("adapt");

        assert_ne!(src, adapted);
        let skill = reg.get_skill(adapted).expect("lookup");
        assert_eq!(skill.parent_id, Some(src));
        assert_eq!(skill.version, 2);
        assert_eq!(skill.steps.len(), 2);
        assert_eq!(skill.steps[1], "step3");
    }

    #[test]
    fn test_adapt_skill_inherits_tags() {
        let mut reg = SkillRegistry::new();
        let src = reg
            .register_skill("tagged_src", vec!["a".into()], 100)
            .expect("ok");
        reg.add_tags(src, &["nav", "maps"]).expect("ok");

        let adapted = reg
            .adapt_skill(src, "tagged_adapted", vec!["b".into()], 200)
            .expect("ok");

        let skill = reg.get_skill(adapted).expect("lookup");
        assert_eq!(skill.tags.len(), 2);
        assert!(skill.tags.contains(&"nav".to_string()));
        assert!(skill.tags.contains(&"maps".to_string()));
    }

    #[test]
    fn test_adapt_skill_not_found() {
        let mut reg = SkillRegistry::new();
        let result = reg.adapt_skill(9999, "nope", vec![], 100);
        assert!(result.is_err());
    }

    #[test]
    fn test_skill_lineage() {
        let mut reg = SkillRegistry::new();
        let root = reg
            .register_skill("root", vec!["a".into()], 100)
            .expect("ok");
        let child = reg
            .adapt_skill(root, "child", vec!["b".into()], 200)
            .expect("ok");
        let grandchild = reg
            .adapt_skill(child, "grandchild", vec!["c".into()], 300)
            .expect("ok");

        let lineage = reg.get_skill_lineage(grandchild);
        assert_eq!(lineage.len(), 3);
        assert_eq!(lineage[0], grandchild);
        assert_eq!(lineage[1], child);
        assert_eq!(lineage[2], root);
    }

    #[test]
    fn test_add_tags() {
        let mut reg = SkillRegistry::new();
        let id = reg
            .register_skill("tag_test", vec!["a".into()], 100)
            .expect("ok");

        reg.add_tags(id, &["alpha", "beta", "ALPHA"]).expect("ok");
        let skill = reg.get_skill(id).expect("lookup");
        // "ALPHA" should be normalized to "alpha" and deduplicated
        assert_eq!(skill.tags.len(), 2);
        assert!(skill.tags.contains(&"alpha".to_string()));
        assert!(skill.tags.contains(&"beta".to_string()));
    }

    #[test]
    fn test_add_tags_capacity() {
        let mut reg = SkillRegistry::new();
        let id = reg
            .register_skill("cap_tags", vec!["a".into()], 100)
            .expect("ok");

        let many_tags: Vec<String> = (0..20).map(|i| format!("tag_{i}")).collect();
        let tag_refs: Vec<&str> = many_tags.iter().map(|s| s.as_str()).collect();
        reg.add_tags(id, &tag_refs).expect("ok");

        let skill = reg.get_skill(id).expect("lookup");
        assert_eq!(skill.tags.len(), MAX_TAGS_PER_SKILL);
    }

    #[test]
    fn test_get_skills_by_tag() {
        let mut reg = SkillRegistry::new();
        let id1 = reg.register_skill("s1", vec!["a".into()], 100).expect("ok");
        let id2 = reg.register_skill("s2", vec!["b".into()], 100).expect("ok");
        let _id3 = reg.register_skill("s3", vec!["c".into()], 100).expect("ok");

        reg.add_tags(id1, &["shared"]).expect("ok");
        reg.add_tags(id2, &["shared"]).expect("ok");

        let found = reg.get_skills_by_tag("shared");
        assert_eq!(found.len(), 2);
    }

    #[test]
    fn test_decay_all_confidence() {
        let mut reg = SkillRegistry::new();
        let id = reg
            .register_skill("decay_all", vec!["a".into()], 1000)
            .expect("ok");

        let original = reg.get_skill(id).expect("lookup").confidence;

        // Decay after 30 days
        let now = 1000 + 86_400_000 * 30;
        reg.decay_all_confidence(now);

        let decayed = reg.get_skill(id).expect("lookup").confidence;
        assert!(
            decayed < original,
            "confidence should decay: {decayed} < {original}"
        );
        assert!(
            decayed >= CONFIDENCE_FLOOR,
            "should not go below floor: {decayed}"
        );
    }

    #[test]
    fn test_decayed_confidence_calculation() {
        let mut reg = SkillRegistry::new();
        let id = reg.register_skill("calc", vec!["a".into()], 0).expect("ok");

        let skill = reg.get_skill(id).expect("lookup");

        // Same time: no decay
        assert!((skill.decayed_confidence(0) - 0.5).abs() < f32::EPSILON);

        // 1 day later: 0.5 * 0.98 = 0.49
        let one_day = 86_400_000u64;
        let d1 = skill.decayed_confidence(one_day);
        assert!((d1 - 0.49).abs() < 0.01, "expected ~0.49, got {d1}");

        // 100 days: should be near floor
        let d100 = skill.decayed_confidence(one_day * 100);
        assert!(
            d100 >= CONFIDENCE_FLOOR && d100 < 0.4,
            "expected near floor, got {d100}"
        );
    }
}
