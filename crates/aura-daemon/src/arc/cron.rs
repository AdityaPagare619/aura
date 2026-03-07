//! Cron job scheduler for the Arc module (§2 of SPEC-ARC).
//!
//! Implements a sorted-list timer wheel with power-tier gating.
//! Each job stores its interval, last-run timestamp, priority, power-tier
//! minimum, and a closure-less job identifier dispatched by the scheduler.

use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::collections::BinaryHeap;
use tracing::{debug, instrument, trace, warn};

use aura_types::power::PowerTier;

use super::{ArcError, ContextMode, DomainId};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Hard upper bound on registered cron jobs.
const MAX_CRON_JOBS: usize = 64;

// ---------------------------------------------------------------------------
// CronJobId — typed job identifier
// ---------------------------------------------------------------------------

/// Well-known cron job identifiers.
///
/// Each variant corresponds to one of the 31 jobs specified in §2.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum CronJobId {
    // ── Health (1-minute to daily) ──
    MedCheck = 0,
    VitalIngest = 1,
    StepSync = 2,
    SleepInfer = 3,
    HealthScore = 4,
    HealthWeekly = 5,

    // ── Social (5-minute to daily) ──
    ContactUpdate = 6,
    ImportanceRecalc = 7,
    RelationshipHealth = 8,
    GapScan = 9,
    BirthdayCheck = 10,
    SocialScore = 11,
    SocialWeekly = 12,

    // ── Proactive (2-minute to daily) ──
    TriggerEval = 13,
    OpportunityDetect = 14,
    ThreatAccumulate = 15,
    ActionDrain = 16,
    DailyBudgetReset = 17,

    // ── Learning (varies) ──
    PatternObserve = 18,
    PatternAnalyze = 19,
    PatternDeviationCheck = 20,
    HebbianDecay = 21,
    HebbianConsolidate = 22,
    InterestUpdate = 23,
    SkillProgress = 24,

    // ── System / cross-cutting ──
    DomainStatePublish = 25,
    LifeQualityCompute = 26,
    CronSelfCheck = 27,
    MemoryArcFlush = 28,
    WeeklyDigest = 29,
    DeepConsolidation = 30,
    ProactiveTick = 31,
    DreamingTick = 32,
}

impl CronJobId {
    /// All 33 job identifiers.
    pub const ALL: [CronJobId; 33] = [
        CronJobId::MedCheck,
        CronJobId::VitalIngest,
        CronJobId::StepSync,
        CronJobId::SleepInfer,
        CronJobId::HealthScore,
        CronJobId::HealthWeekly,
        CronJobId::ContactUpdate,
        CronJobId::ImportanceRecalc,
        CronJobId::RelationshipHealth,
        CronJobId::GapScan,
        CronJobId::BirthdayCheck,
        CronJobId::SocialScore,
        CronJobId::SocialWeekly,
        CronJobId::TriggerEval,
        CronJobId::OpportunityDetect,
        CronJobId::ThreatAccumulate,
        CronJobId::ActionDrain,
        CronJobId::DailyBudgetReset,
        CronJobId::PatternObserve,
        CronJobId::PatternAnalyze,
        CronJobId::PatternDeviationCheck,
        CronJobId::HebbianDecay,
        CronJobId::HebbianConsolidate,
        CronJobId::InterestUpdate,
        CronJobId::SkillProgress,
        CronJobId::DomainStatePublish,
        CronJobId::LifeQualityCompute,
        CronJobId::CronSelfCheck,
        CronJobId::MemoryArcFlush,
        CronJobId::WeeklyDigest,
        CronJobId::DeepConsolidation,
        CronJobId::ProactiveTick,
        CronJobId::DreamingTick,
    ];
}

impl std::fmt::Display for CronJobId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self)
    }
}

// ---------------------------------------------------------------------------
// CronJob — definition of a single scheduled job
// ---------------------------------------------------------------------------

/// A single cron job definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CronJob {
    pub id: CronJobId,
    /// Human-readable name.
    pub name: String,
    /// Owning domain (for budget/reporting).
    pub domain: DomainId,
    /// Interval in seconds between runs.
    pub interval_secs: u32,
    /// Unix-epoch seconds of the last successful run (0 = never).
    pub last_run_at: i64,
    /// Lower number = higher priority.
    pub priority: u8,
    /// Minimum power tier required to execute.
    pub power_tier: PowerTier,
    /// Whether this job is currently enabled.
    pub enabled: bool,
    /// Context modes where this job is suppressed.
    pub blocked_modes: Vec<ContextMode>,
}

impl CronJob {
    /// Next eligible fire time (unix epoch seconds).
    #[must_use]
    pub fn next_fire_at(&self) -> i64 {
        self.last_run_at + self.interval_secs as i64
    }

    /// Whether the job is due at `now` given the current `power_tier` and
    /// `context_mode`.
    #[must_use]
    pub fn is_due(&self, now: i64, current_tier: PowerTier, mode: ContextMode) -> bool {
        if !self.enabled {
            return false;
        }
        if self.blocked_modes.contains(&mode) {
            return false;
        }
        // Power tier comparison: lower enum ordinal = higher tier.
        // A job requiring P2Normal can run when current is P0Always..P2Normal.
        if (current_tier as u8) > (self.power_tier as u8) {
            return false;
        }
        now >= self.next_fire_at()
    }
}

// ---------------------------------------------------------------------------
// Pending fire — priority queue element
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct PendingFire {
    job_id: CronJobId,
    priority: u8,
    next_fire: i64,
}

impl PartialEq for PendingFire {
    fn eq(&self, other: &Self) -> bool {
        self.priority == other.priority && self.next_fire == other.next_fire
    }
}

impl Eq for PendingFire {}

impl PartialOrd for PendingFire {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for PendingFire {
    fn cmp(&self, other: &Self) -> Ordering {
        // Lower priority number = higher urgency, comes first (max-heap inversion).
        other
            .priority
            .cmp(&self.priority)
            .then_with(|| self.next_fire.cmp(&other.next_fire))
    }
}

// ---------------------------------------------------------------------------
// CronScheduler — the timer wheel
// ---------------------------------------------------------------------------

/// Manages all registered cron jobs and determines which are due.
#[derive(Debug)]
pub struct CronScheduler {
    jobs: Vec<CronJob>,
}

impl CronScheduler {
    /// Create an empty scheduler.
    #[must_use]
    pub fn new() -> Self {
        Self {
            jobs: Vec::with_capacity(MAX_CRON_JOBS),
        }
    }

    /// Create a scheduler pre-loaded with the default 31 jobs.
    #[must_use]
    pub fn with_defaults() -> Self {
        let mut sched = Self::new();
        for job in default_jobs() {
            // Safety: default_jobs produces exactly 31 jobs, under MAX_CRON_JOBS.
            let _ = sched.register(job);
        }
        sched
    }

    /// Register a new cron job.  Fails if capacity is exceeded.
    pub fn register(&mut self, job: CronJob) -> Result<(), ArcError> {
        if self.jobs.len() >= MAX_CRON_JOBS {
            return Err(ArcError::CapacityExceeded {
                collection: "cron_jobs".into(),
                max: MAX_CRON_JOBS,
            });
        }
        debug!(job_id = %job.id, interval = job.interval_secs, "registered cron job");
        self.jobs.push(job);
        Ok(())
    }

    /// Collect all jobs that are due right now, ordered by priority.
    ///
    /// Returns a `Vec` of `CronJobId`s. The caller is responsible for
    /// dispatching each job and calling [`mark_run`] on completion.
    #[instrument(name = "cron_tick", skip(self), fields(due_count))]
    pub fn tick(&self, now: i64, current_tier: PowerTier, mode: ContextMode) -> Vec<CronJobId> {
        let mut heap = BinaryHeap::<PendingFire>::new();

        for job in &self.jobs {
            if job.is_due(now, current_tier, mode) {
                heap.push(PendingFire {
                    job_id: job.id,
                    priority: job.priority,
                    next_fire: job.next_fire_at(),
                });
            }
        }

        let mut result = Vec::with_capacity(heap.len());
        while let Some(pf) = heap.pop() {
            result.push(pf.job_id);
        }
        trace!(due_count = result.len(), "cron tick complete");
        result
    }

    /// Mark a job as having run at `now`.
    pub fn mark_run(&mut self, id: CronJobId, now: i64) {
        if let Some(job) = self.jobs.iter_mut().find(|j| j.id == id) {
            job.last_run_at = now;
        }
    }

    /// Enable or disable a job.
    pub fn set_enabled(&mut self, id: CronJobId, enabled: bool) {
        if let Some(job) = self.jobs.iter_mut().find(|j| j.id == id) {
            job.enabled = enabled;
        }
    }

    /// Number of registered jobs.
    #[must_use]
    pub fn job_count(&self) -> usize {
        self.jobs.len()
    }

    /// Read-only access to all jobs.
    #[must_use]
    pub fn jobs(&self) -> &[CronJob] {
        &self.jobs
    }
}

impl Default for CronScheduler {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Default job definitions (§2)
// ---------------------------------------------------------------------------

/// Build the default set of 31 cron jobs.
fn default_jobs() -> Vec<CronJob> {
    vec![
        // ── Health ──
        cj(
            CronJobId::MedCheck,
            "medication_check",
            DomainId::Health,
            60,
            10,
            PowerTier::P0Always,
            &[],
        ),
        cj(
            CronJobId::VitalIngest,
            "vital_ingest",
            DomainId::Health,
            300,
            40,
            PowerTier::P1IdlePlus,
            &[],
        ),
        cj(
            CronJobId::StepSync,
            "step_sync",
            DomainId::Health,
            900,
            80,
            PowerTier::P1IdlePlus,
            &[ContextMode::Sleeping],
        ),
        cj(
            CronJobId::SleepInfer,
            "sleep_infer",
            DomainId::Health,
            3600,
            60,
            PowerTier::P2Normal,
            &[],
        ),
        cj(
            CronJobId::HealthScore,
            "health_score_compute",
            DomainId::Health,
            3600,
            70,
            PowerTier::P2Normal,
            &[],
        ),
        cj(
            CronJobId::HealthWeekly,
            "health_weekly_report",
            DomainId::Health,
            86400 * 7,
            120,
            PowerTier::P3Charging,
            &[],
        ),
        // ── Social ──
        cj(
            CronJobId::ContactUpdate,
            "contact_update",
            DomainId::Social,
            300,
            50,
            PowerTier::P1IdlePlus,
            &[ContextMode::Sleeping],
        ),
        cj(
            CronJobId::ImportanceRecalc,
            "importance_recalc",
            DomainId::Social,
            3600,
            70,
            PowerTier::P2Normal,
            &[],
        ),
        cj(
            CronJobId::RelationshipHealth,
            "relationship_health",
            DomainId::Social,
            21600,
            60,
            PowerTier::P1IdlePlus,
            &[],
        ),
        cj(
            CronJobId::GapScan,
            "social_gap_scan",
            DomainId::Social,
            21600,
            80,
            PowerTier::P1IdlePlus,
            &[ContextMode::Sleeping],
        ),
        cj(
            CronJobId::BirthdayCheck,
            "birthday_check",
            DomainId::Social,
            86400,
            30,
            PowerTier::P2Normal,
            &[],
        ),
        cj(
            CronJobId::SocialScore,
            "social_score_compute",
            DomainId::Social,
            3600,
            70,
            PowerTier::P2Normal,
            &[],
        ),
        cj(
            CronJobId::SocialWeekly,
            "social_weekly_report",
            DomainId::Social,
            86400 * 7,
            120,
            PowerTier::P3Charging,
            &[],
        ),
        // ── Proactive ──
        cj(
            CronJobId::TriggerEval,
            "trigger_rule_eval",
            DomainId::Productivity,
            120,
            20,
            PowerTier::P1IdlePlus,
            &[],
        ),
        cj(
            CronJobId::OpportunityDetect,
            "opportunity_detect",
            DomainId::Productivity,
            900,
            90,
            PowerTier::P2Normal,
            &[ContextMode::Sleeping, ContextMode::DoNotDisturb],
        ),
        cj(
            CronJobId::ThreatAccumulate,
            "threat_accumulate",
            DomainId::Health,
            120,
            30,
            PowerTier::P0Always,
            &[],
        ),
        cj(
            CronJobId::ActionDrain,
            "action_drain",
            DomainId::Productivity,
            60,
            15,
            PowerTier::P0Always,
            &[],
        ),
        cj(
            CronJobId::DailyBudgetReset,
            "daily_budget_reset",
            DomainId::Productivity,
            86400,
            100,
            PowerTier::P0Always,
            &[],
        ),
        // ── Learning ──
        cj(
            CronJobId::PatternObserve,
            "pattern_observe",
            DomainId::Learning,
            1,
            5,
            PowerTier::P0Always,
            &[],
        ),
        cj(
            CronJobId::PatternAnalyze,
            "pattern_analyze",
            DomainId::Learning,
            86400 * 7,
            110,
            PowerTier::P3Charging,
            &[],
        ),
        cj(
            CronJobId::PatternDeviationCheck,
            "pattern_deviation_check",
            DomainId::Learning,
            1800,
            70,
            PowerTier::P1IdlePlus,
            &[ContextMode::Sleeping],
        ),
        cj(
            CronJobId::HebbianDecay,
            "hebbian_decay",
            DomainId::Learning,
            3600,
            80,
            PowerTier::P2Normal,
            &[],
        ),
        cj(
            CronJobId::HebbianConsolidate,
            "hebbian_consolidate",
            DomainId::Learning,
            86400,
            100,
            PowerTier::P3Charging,
            &[],
        ),
        cj(
            CronJobId::InterestUpdate,
            "interest_update",
            DomainId::Learning,
            21600,
            90,
            PowerTier::P2Normal,
            &[],
        ),
        cj(
            CronJobId::SkillProgress,
            "skill_progress",
            DomainId::Learning,
            86400,
            100,
            PowerTier::P2Normal,
            &[],
        ),
        // ── System / cross-cutting ──
        cj(
            CronJobId::DomainStatePublish,
            "domain_state_publish",
            DomainId::Health,
            300,
            40,
            PowerTier::P1IdlePlus,
            &[],
        ),
        cj(
            CronJobId::LifeQualityCompute,
            "life_quality_compute",
            DomainId::Health,
            3600,
            60,
            PowerTier::P2Normal,
            &[],
        ),
        cj(
            CronJobId::CronSelfCheck,
            "cron_self_check",
            DomainId::Health,
            900,
            50,
            PowerTier::P0Always,
            &[],
        ),
        cj(
            CronJobId::MemoryArcFlush,
            "memory_arc_flush",
            DomainId::Health,
            1800,
            70,
            PowerTier::P2Normal,
            &[],
        ),
        cj(
            CronJobId::WeeklyDigest,
            "weekly_digest",
            DomainId::Health,
            86400 * 7,
            130,
            PowerTier::P3Charging,
            &[],
        ),
        cj(
            CronJobId::DeepConsolidation,
            "deep_consolidation",
            DomainId::Health,
            86400,
            140,
            PowerTier::P4DeepWork,
            &[],
        ),
        cj(
            CronJobId::ProactiveTick,
            "proactive_tick",
            DomainId::Productivity,
            300,
            25,
            PowerTier::P1IdlePlus,
            &[],
        ),
        cj(
            CronJobId::DreamingTick,
            "dreaming_tick",
            DomainId::Learning,
            300,
            35,
            PowerTier::P3Charging,
            &[ContextMode::Sleeping, ContextMode::DoNotDisturb],
        ),
    ]
}

/// Helper to construct a [`CronJob`] concisely.
fn cj(
    id: CronJobId,
    name: &str,
    domain: DomainId,
    interval_secs: u32,
    priority: u8,
    power_tier: PowerTier,
    blocked: &[ContextMode],
) -> CronJob {
    CronJob {
        id,
        name: name.into(),
        domain,
        interval_secs,
        last_run_at: 0,
        priority,
        power_tier,
        enabled: true,
        blocked_modes: blocked.to_vec(),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_jobs_count() {
        let jobs = default_jobs();
        assert_eq!(jobs.len(), 33, "spec requires exactly 33 cron jobs");
    }

    #[test]
    fn test_scheduler_with_defaults() {
        let sched = CronScheduler::with_defaults();
        assert_eq!(sched.job_count(), 33);
    }

    #[test]
    fn test_capacity_limit() {
        let mut sched = CronScheduler::new();
        for i in 0..MAX_CRON_JOBS {
            // Re-use MedCheck id, not realistic but tests capacity
            let job = CronJob {
                id: CronJobId::MedCheck,
                name: format!("job_{i}"),
                domain: DomainId::Health,
                interval_secs: 60,
                last_run_at: 0,
                priority: 50,
                power_tier: PowerTier::P0Always,
                enabled: true,
                blocked_modes: vec![],
            };
            assert!(sched.register(job).is_ok());
        }
        // One more should fail
        let extra = CronJob {
            id: CronJobId::MedCheck,
            name: "overflow".into(),
            domain: DomainId::Health,
            interval_secs: 60,
            last_run_at: 0,
            priority: 50,
            power_tier: PowerTier::P0Always,
            enabled: true,
            blocked_modes: vec![],
        };
        assert!(sched.register(extra).is_err());
    }

    #[test]
    fn test_job_is_due_basic() {
        let job = cj(
            CronJobId::MedCheck,
            "med_check",
            DomainId::Health,
            60,
            10,
            PowerTier::P0Always,
            &[],
        );
        // last_run_at=0, interval=60 → next_fire_at=60
        assert!(job.is_due(60, PowerTier::P0Always, ContextMode::Default));
        assert!(job.is_due(100, PowerTier::P0Always, ContextMode::Default));
        assert!(!job.is_due(30, PowerTier::P0Always, ContextMode::Default));
    }

    #[test]
    fn test_job_power_tier_gating() {
        let job = cj(
            CronJobId::HealthScore,
            "health_score",
            DomainId::Health,
            3600,
            70,
            PowerTier::P2Normal,
            &[],
        );
        // P2Normal job should run when current tier is P0Always (better)
        assert!(job.is_due(5000, PowerTier::P0Always, ContextMode::Default));
        assert!(job.is_due(5000, PowerTier::P2Normal, ContextMode::Default));
        // Should NOT run when current tier is P3Charging (worse)
        assert!(!job.is_due(5000, PowerTier::P3Charging, ContextMode::Default));
    }

    #[test]
    fn test_job_blocked_context() {
        let job = cj(
            CronJobId::StepSync,
            "step_sync",
            DomainId::Health,
            900,
            80,
            PowerTier::P1IdlePlus,
            &[ContextMode::Sleeping],
        );
        assert!(job.is_due(2000, PowerTier::P0Always, ContextMode::Active));
        assert!(!job.is_due(2000, PowerTier::P0Always, ContextMode::Sleeping));
    }

    #[test]
    fn test_tick_returns_priority_order() {
        let mut sched = CronScheduler::new();

        // Low priority (high number)
        let _ = sched.register(cj(
            CronJobId::HealthWeekly,
            "weekly",
            DomainId::Health,
            10,
            120,
            PowerTier::P0Always,
            &[],
        ));
        // High priority (low number)
        let _ = sched.register(cj(
            CronJobId::MedCheck,
            "med",
            DomainId::Health,
            10,
            10,
            PowerTier::P0Always,
            &[],
        ));

        let due = sched.tick(20, PowerTier::P0Always, ContextMode::Default);
        assert_eq!(due.len(), 2);
        assert_eq!(due[0], CronJobId::MedCheck); // priority 10 first
        assert_eq!(due[1], CronJobId::HealthWeekly); // priority 120 second
    }

    #[test]
    fn test_mark_run() {
        let mut sched = CronScheduler::new();
        let _ = sched.register(cj(
            CronJobId::MedCheck,
            "med",
            DomainId::Health,
            60,
            10,
            PowerTier::P0Always,
            &[],
        ));
        assert!(
            sched
                .tick(100, PowerTier::P0Always, ContextMode::Default)
                .len()
                == 1
        );

        sched.mark_run(CronJobId::MedCheck, 100);
        // Should not be due until t=160
        assert!(sched
            .tick(120, PowerTier::P0Always, ContextMode::Default)
            .is_empty());
        assert!(
            sched
                .tick(160, PowerTier::P0Always, ContextMode::Default)
                .len()
                == 1
        );
    }

    #[test]
    fn test_set_enabled() {
        let mut sched = CronScheduler::new();
        let _ = sched.register(cj(
            CronJobId::MedCheck,
            "med",
            DomainId::Health,
            60,
            10,
            PowerTier::P0Always,
            &[],
        ));
        sched.set_enabled(CronJobId::MedCheck, false);
        assert!(sched
            .tick(100, PowerTier::P0Always, ContextMode::Default)
            .is_empty());

        sched.set_enabled(CronJobId::MedCheck, true);
        assert_eq!(
            sched
                .tick(100, PowerTier::P0Always, ContextMode::Default)
                .len(),
            1
        );
    }
}
