//! AURA v4 Arc Module — Bio-inspired behavioral arc system.
//!
//! The Arc is AURA's "higher brain functions" layer: proactive health awareness,
//! social intelligence, behavioral pattern learning, and continuous self-improvement.
//!
//! # Module Organisation (§8.1 of SPEC-ARC-ARCHITECTURE-MAPPING)
//!
//! | Sub-module   | Purpose                                      |
//! |--------------|----------------------------------------------|
//! | `health`     | Health domain: meds, vitals, fitness, sleep   |
//! | `social`     | Social domain: contacts, relationships, graph |
//! | `proactive`  | Proactive engine: triggers, threats, budget    |
//! | `learning`   | Pattern learning + Hebbian concept learning    |
//! | `cron`       | Timer-wheel cron job scheduler                 |

pub mod cron;
pub mod health;
pub mod learning;
pub mod proactive;
pub mod social;

// Re-export key types at module root.
pub use cron::{CronJob, CronScheduler};
pub use health::HealthDomain;
pub use learning::LearningEngine;
pub use proactive::ProactiveEngine;
pub use social::SocialDomain;

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;
use tracing::{debug, info, instrument, warn};

use aura_types::errors::{AuraError, MemError};

// ---------------------------------------------------------------------------
// Core enums — canonical definitions (§8.2)
// ---------------------------------------------------------------------------

/// 10 life domains AURA manages.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum DomainId {
    Health = 0,
    Social = 1,
    Productivity = 2,
    Finance = 3,
    Lifestyle = 4,
    Entertainment = 5,
    Learning = 6,
    Communication = 7,
    Environment = 8,
    PersonalGrowth = 9,
}

impl DomainId {
    /// All domain variants for iteration.
    pub const ALL: [DomainId; 10] = [
        DomainId::Health,
        DomainId::Social,
        DomainId::Productivity,
        DomainId::Finance,
        DomainId::Lifestyle,
        DomainId::Entertainment,
        DomainId::Learning,
        DomainId::Communication,
        DomainId::Environment,
        DomainId::PersonalGrowth,
    ];

    /// Default weight for life-quality index (§6.5.2).
    #[must_use]
    pub fn default_weight(self) -> f32 {
        match self {
            DomainId::Health => 1.0,
            DomainId::Social => 0.8,
            DomainId::Productivity => 0.7,
            DomainId::Finance => 0.6,
            DomainId::Lifestyle => 0.5,
            DomainId::Learning => 0.4,
            DomainId::PersonalGrowth => 0.4,
            DomainId::Communication => 0.3,
            DomainId::Entertainment => 0.2,
            DomainId::Environment => 0.2,
        }
    }
}

impl std::fmt::Display for DomainId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let name = match self {
            DomainId::Health => "Health",
            DomainId::Social => "Social",
            DomainId::Productivity => "Productivity",
            DomainId::Finance => "Finance",
            DomainId::Lifestyle => "Lifestyle",
            DomainId::Entertainment => "Entertainment",
            DomainId::Learning => "Learning",
            DomainId::Communication => "Communication",
            DomainId::Environment => "Environment",
            DomainId::PersonalGrowth => "PersonalGrowth",
        };
        f.write_str(name)
    }
}

/// Domain lifecycle state machine (§8.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DomainLifecycle {
    /// No data observed for this domain.
    Dormant,
    /// Data observed but below statistical significance.
    Initializing,
    /// Sufficient data, domain fully operational.
    Active,
    /// Data stale or system error, partial function.
    Degraded,
}

/// Context modes affecting all domain behaviour (§8.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum ContextMode {
    Default = 0,
    DoNotDisturb = 1,
    Sleeping = 2,
    Active = 3,
    Driving = 4,
    Custom1 = 5,
    Custom2 = 6,
    Custom3 = 7,
}

// ---------------------------------------------------------------------------
// ArcError — module-local error type
// ---------------------------------------------------------------------------

/// Errors originating within the Arc module.
///
/// Converted to [`AuraError`] at API boundaries via the `From` impl.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ArcError {
    /// A domain has not accumulated enough data.
    InsufficientData { domain: DomainId, detail: String },
    /// An operation was rejected because of power-tier constraints.
    PowerTierBlocked { required: String, current: String },
    /// Capacity limit reached for a bounded collection.
    CapacityExceeded { collection: String, max: usize },
    /// A required lookup failed.
    NotFound { entity: String, id: u64 },
    /// Serialization / deserialization failure.
    SerdeError(String),
    /// Generic domain logic error.
    DomainError { domain: DomainId, detail: String },
}

impl std::fmt::Display for ArcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ArcError::InsufficientData { domain, detail } => {
                write!(f, "arc: insufficient data in {domain}: {detail}")
            }
            ArcError::PowerTierBlocked { required, current } => {
                write!(
                    f,
                    "arc: power tier blocked (need {required}, have {current})"
                )
            }
            ArcError::CapacityExceeded { collection, max } => {
                write!(f, "arc: capacity exceeded for {collection} (max {max})")
            }
            ArcError::NotFound { entity, id } => {
                write!(f, "arc: {entity} not found: {id}")
            }
            ArcError::SerdeError(msg) => write!(f, "arc: serde error: {msg}"),
            ArcError::DomainError { domain, detail } => {
                write!(f, "arc: domain error in {domain}: {detail}")
            }
        }
    }
}

impl std::error::Error for ArcError {}

impl From<ArcError> for AuraError {
    fn from(e: ArcError) -> Self {
        AuraError::Memory(MemError::DatabaseError(e.to_string()))
    }
}

// ---------------------------------------------------------------------------
// DomainSnapshot — cross-domain shared state (§10.6)
// ---------------------------------------------------------------------------

/// A snapshot of a single domain's current state, used for cross-domain reads.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DomainSnapshot {
    pub health_score: f32,
    pub lifecycle: DomainLifecycle,
    /// Domain-specific KPIs. Keys are metric names, values are current readings.
    pub key_metrics: HashMap<String, f64>,
    /// Unix-epoch seconds when this snapshot was last refreshed.
    pub updated_at: i64,
}

impl Default for DomainSnapshot {
    fn default() -> Self {
        Self {
            health_score: 0.5,
            lifecycle: DomainLifecycle::Dormant,
            key_metrics: HashMap::new(),
            updated_at: 0,
        }
    }
}

// ---------------------------------------------------------------------------
// DomainStateStore — concurrent read/write map (§10.6)
// ---------------------------------------------------------------------------

/// Thread-safe, read-mostly store of per-domain snapshots.
///
/// Uses an `RwLock<HashMap>` rather than `DashMap` to keep dependencies small.
/// Reads are frequent and fast; writes happen at most once per evaluation cycle.
#[derive(Debug, Clone)]
pub struct DomainStateStore {
    states: Arc<std::sync::RwLock<HashMap<DomainId, DomainSnapshot>>>,
}

impl DomainStateStore {
    /// Create a new store with default (Dormant) snapshots for all 10 domains.
    #[must_use]
    pub fn new() -> Self {
        let mut map = HashMap::with_capacity(10);
        for &domain in &DomainId::ALL {
            map.insert(domain, DomainSnapshot::default());
        }
        Self {
            states: Arc::new(std::sync::RwLock::new(map)),
        }
    }

    /// Read a single metric from a domain snapshot.
    ///
    /// Returns `None` if the domain has no snapshot or the key is absent.
    pub fn get_metric(&self, domain: DomainId, key: &str) -> Option<f64> {
        let guard = self.states.read().ok()?;
        guard.get(&domain)?.key_metrics.get(key).copied()
    }

    /// Read the health score for a domain.
    pub fn get_health_score(&self, domain: DomainId) -> Option<f32> {
        let guard = self.states.read().ok()?;
        guard.get(&domain).map(|s| s.health_score)
    }

    /// Update a domain's snapshot atomically.
    pub fn update(
        &self,
        domain: DomainId,
        score: f32,
        lifecycle: DomainLifecycle,
        metrics: HashMap<String, f64>,
        now: i64,
    ) -> Result<(), ArcError> {
        let mut guard = self.states.write().map_err(|_| ArcError::DomainError {
            domain,
            detail: "state store lock poisoned".into(),
        })?;
        guard.insert(
            domain,
            DomainSnapshot {
                health_score: score.clamp(0.0, 1.0),
                lifecycle,
                key_metrics: metrics,
                updated_at: now,
            },
        );
        Ok(())
    }
}

impl Default for DomainStateStore {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Life Quality Index (§6.5.2)
// ---------------------------------------------------------------------------

/// Compute the overall life-quality index from per-domain scores.
///
/// `custom_weights` optionally overrides the default weights in [`DomainId::default_weight`].
#[must_use]
pub fn compute_life_quality(
    domain_scores: &HashMap<DomainId, f32>,
    custom_weights: Option<&HashMap<DomainId, f32>>,
) -> f32 {
    let mut weighted_sum = 0.0_f32;
    let mut weight_sum = 0.0_f32;

    for &domain in &DomainId::ALL {
        let w = custom_weights
            .and_then(|cw| cw.get(&domain))
            .copied()
            .unwrap_or_else(|| domain.default_weight());
        let score = domain_scores.get(&domain).copied().unwrap_or(0.5);
        weighted_sum += w * score;
        weight_sum += w;
    }

    if weight_sum > 0.0 {
        (weighted_sum / weight_sum).clamp(0.0, 1.0)
    } else {
        0.5
    }
}

// ---------------------------------------------------------------------------
// ArcManager — top-level coordinator
// ---------------------------------------------------------------------------

/// The top-level coordinator for the entire Arc module.
///
/// Owns the cron scheduler, domain engines, and shared state store.
/// Constructed during daemon startup and driven by the main event loop.
pub struct ArcManager {
    pub state_store: DomainStateStore,
    pub scheduler: CronScheduler,
    pub health: HealthDomain,
    pub social: SocialDomain,
    pub proactive: ProactiveEngine,
    pub learning: LearningEngine,
    pub context_mode: ContextMode,
    created_at: Instant,
}

impl ArcManager {
    /// Construct a new `ArcManager` with default sub-systems.
    #[instrument(name = "arc_manager_new", skip_all)]
    pub fn new() -> Self {
        info!("initialising Arc module");
        Self {
            state_store: DomainStateStore::new(),
            scheduler: CronScheduler::new(),
            health: HealthDomain::new(),
            social: SocialDomain::new(),
            proactive: ProactiveEngine::new(),
            learning: LearningEngine::new(),
            context_mode: ContextMode::Default,
            created_at: Instant::now(),
        }
    }

    /// Switch context mode (e.g., when DnD is toggled).
    pub fn set_context_mode(&mut self, mode: ContextMode) {
        debug!(old = ?self.context_mode, new = ?mode, "context mode changed");
        self.context_mode = mode;
    }

    /// Elapsed time since the Arc manager was created.
    #[must_use]
    pub fn uptime_secs(&self) -> u64 {
        self.created_at.elapsed().as_secs()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_domain_id_all_count() {
        assert_eq!(DomainId::ALL.len(), 10);
    }

    #[test]
    fn test_domain_id_display() {
        assert_eq!(DomainId::Health.to_string(), "Health");
        assert_eq!(DomainId::PersonalGrowth.to_string(), "PersonalGrowth");
    }

    #[test]
    fn test_domain_default_weights_sum() {
        let sum: f32 = DomainId::ALL.iter().map(|d| d.default_weight()).sum();
        // Expected: 1.0+0.8+0.7+0.6+0.5+0.4+0.4+0.3+0.2+0.2 = 5.1
        assert!((sum - 5.1).abs() < 0.01, "got {sum}");
    }

    #[test]
    fn test_life_quality_neutral() {
        let scores = HashMap::new(); // all default to 0.5
        let q = compute_life_quality(&scores, None);
        assert!((q - 0.5).abs() < 0.01, "got {q}");
    }

    #[test]
    fn test_life_quality_perfect() {
        let mut scores = HashMap::new();
        for &d in &DomainId::ALL {
            scores.insert(d, 1.0);
        }
        let q = compute_life_quality(&scores, None);
        assert!((q - 1.0).abs() < 0.01, "got {q}");
    }

    #[test]
    fn test_life_quality_custom_weights() {
        let mut scores = HashMap::new();
        scores.insert(DomainId::Health, 1.0);
        // Rest default to 0.5

        let mut weights = HashMap::new();
        weights.insert(DomainId::Health, 10.0);
        // Other domains keep default low weights

        let q = compute_life_quality(&scores, Some(&weights));
        // Health dominates → should be closer to 1.0
        assert!(q > 0.7, "got {q}");
    }

    #[test]
    fn test_domain_state_store_read_write() {
        let store = DomainStateStore::new();

        // Default health score is 0.5
        let score = store.get_health_score(DomainId::Health);
        assert_eq!(score, Some(0.5));

        // Update
        let mut metrics = HashMap::new();
        metrics.insert("med_adherence".into(), 0.95);
        store
            .update(
                DomainId::Health,
                0.85,
                DomainLifecycle::Active,
                metrics,
                1000,
            )
            .expect("update should succeed");

        assert_eq!(store.get_health_score(DomainId::Health), Some(0.85));
        assert_eq!(
            store.get_metric(DomainId::Health, "med_adherence"),
            Some(0.95)
        );
        assert_eq!(store.get_metric(DomainId::Health, "nonexistent"), None);
    }

    #[test]
    fn test_domain_state_store_clamping() {
        let store = DomainStateStore::new();
        store
            .update(
                DomainId::Social,
                1.5,
                DomainLifecycle::Active,
                HashMap::new(),
                0,
            )
            .expect("update should succeed");
        assert_eq!(store.get_health_score(DomainId::Social), Some(1.0));

        store
            .update(
                DomainId::Social,
                -0.5,
                DomainLifecycle::Active,
                HashMap::new(),
                0,
            )
            .expect("update should succeed");
        assert_eq!(store.get_health_score(DomainId::Social), Some(0.0));
    }

    #[test]
    fn test_arc_error_display() {
        let e = ArcError::CapacityExceeded {
            collection: "contacts".into(),
            max: 500,
        };
        assert!(e.to_string().contains("500"));
        assert!(e.to_string().contains("contacts"));
    }

    #[test]
    fn test_arc_error_into_aura_error() {
        let e = ArcError::NotFound {
            entity: "contact".into(),
            id: 42,
        };
        let aura: AuraError = e.into();
        let msg = aura.to_string();
        assert!(msg.contains("contact") || msg.contains("42"));
    }
}
