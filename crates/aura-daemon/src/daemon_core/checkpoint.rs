//! State checkpoint — save/load daemon state to/from `state.bin`.
//!
//! Uses bincode 2 with serde compat layer. Atomic writes via
//! write-to-tmp + rename pattern.  Checkpoint size is capped at 64 KB.

use std::{io::Write, path::Path};

#[cfg(test)]
use aura_types::goals::{GoalPriority, GoalSource, GoalStatus};
use aura_types::{
    goals::Goal,
    identity::{DispositionState, OceanTraits, RelationshipStage},
    power::PowerBudget,
};
use serde::{Deserialize, Serialize};

use crate::memory::ConsolidationWeights;

/// Maximum checkpoint file size — architecture hard limit.
const MAX_CHECKPOINT_BYTES: usize = 64 * 1024;

// ---------------------------------------------------------------------------
// CronJobState — lightweight snapshot of cron scheduler state
// ---------------------------------------------------------------------------

/// Persisted cron job state so timers survive restarts.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CronJobState {
    pub job_id: u32,
    pub job_name: String,
    /// Last time this job fired (monotonic ms since boot, or 0 if never).
    pub last_fired_ms: u64,
    /// Next scheduled fire time (0 = needs recalculation on restore).
    pub next_fire_ms: u64,
}

// ---------------------------------------------------------------------------
// TokenCounters — per-model daily counters
// ---------------------------------------------------------------------------

/// Token usage counters for budget enforcement.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TokenCounters {
    /// Tokens used by local (llama.cpp) inference today.
    pub local_tokens: u32,
    /// Tokens used by cloud LLM calls today.
    pub cloud_tokens: u32,
    /// Timestamp of the start of the current accounting day (UTC epoch ms).
    pub day_start_ms: u64,
}

// ---------------------------------------------------------------------------
// DaemonCheckpoint — the full checkpoint payload
// ---------------------------------------------------------------------------

/// Everything the daemon persists across restarts.
///
/// This struct is bincode-encoded and written atomically to `state.bin`.
/// On startup, if the file is corrupt or missing, the daemon boots with defaults.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DaemonCheckpoint {
    /// Schema version — bump when changing the struct layout.
    pub version: u32,

    // --- Identity / personality ---
    pub personality: OceanTraits,
    pub trust_score: f32,
    pub relationship_stage: RelationshipStage,

    // --- Disposition (mood) ---
    pub disposition: DispositionState,

    // --- Active goals ---
    pub goals: Vec<Goal>,

    // --- Cron scheduler state ---
    pub cron_state: Vec<CronJobState>,

    // --- Power budget snapshot ---
    pub power_budget: PowerBudget,

    // --- Token counters ---
    pub token_counters: TokenCounters,

    // --- Monotonic event counter ---
    /// Total `select!` loop iterations since first boot.
    pub select_count: u64,

    // --- Proactive trigger deduplication timestamps ---
    /// Timestamp (ms) of the last dispatched MemoryInsight trigger.
    /// Zero means never dispatched. Used to enforce 24-hour cooldown.
    #[serde(default)]
    pub last_memory_insight_ms: u64,

    // --- Adaptive consolidation weights ---
    /// Learned weights for the consolidation priority formula.
    /// Starts at (recency=0.3, frequency=0.3, importance=0.4) and drifts
    /// as AURA observes which memories the user actually retrieves.
    /// Persisted so learned behavior survives restarts.
    #[serde(default)]
    pub consolidation_weights: ConsolidationWeights,
}

/// Current checkpoint schema version.
pub const CHECKPOINT_VERSION: u32 = 3;

impl Default for DaemonCheckpoint {
    fn default() -> Self {
        Self {
            version: CHECKPOINT_VERSION,
            personality: OceanTraits::default(),
            trust_score: 0.0,
            relationship_stage: RelationshipStage::Stranger,
            disposition: DispositionState::default(),
            goals: Vec::new(),
            cron_state: Vec::new(),
            power_budget: PowerBudget::default(),
            token_counters: TokenCounters::default(),
            select_count: 0,
            last_memory_insight_ms: 0,
            consolidation_weights: ConsolidationWeights::default(),
        }
    }
}

// ---------------------------------------------------------------------------
// Encode / Decode helpers  (bincode 2.0.0-rc.3 serde compat)
// ---------------------------------------------------------------------------

/// Bincode 2 configuration — standard, fixed-int, little-endian.
fn bincode_config() -> impl bincode::config::Config {
    bincode::config::standard()
}

/// Encode a checkpoint to bytes via bincode 2 serde compat layer.
pub fn encode_checkpoint(cp: &DaemonCheckpoint) -> Result<Vec<u8>, CheckpointError> {
    let bytes = bincode::serde::encode_to_vec(cp, bincode_config()).map_err(|e| {
        tracing::error!(error = %e, "checkpoint encode failed");
        CheckpointError::EncodeFailed(e.to_string())
    })?;

    if bytes.len() > MAX_CHECKPOINT_BYTES {
        tracing::error!(
            size = bytes.len(),
            max = MAX_CHECKPOINT_BYTES,
            "checkpoint exceeds size limit"
        );
        return Err(CheckpointError::TooLarge {
            size: bytes.len(),
            max: MAX_CHECKPOINT_BYTES,
        });
    }

    Ok(bytes)
}

/// Decode a checkpoint from bytes via bincode 2 serde compat layer.
///
/// If the checkpoint version is older than `CHECKPOINT_VERSION`, attempts
/// sequential migration (v1→v2→...→current). If migration fails or the
/// version is too old to be handled, falls back to defaults.
pub fn decode_checkpoint(bytes: &[u8]) -> Result<DaemonCheckpoint, CheckpointError> {
    let (mut cp, _len): (DaemonCheckpoint, usize) =
        bincode::serde::decode_from_slice(bytes, bincode_config()).map_err(|e| {
            tracing::error!(error = %e, "checkpoint decode failed");
            CheckpointError::DecodeFailed(e.to_string())
        })?;

    if cp.version == CHECKPOINT_VERSION {
        return Ok(cp);
    }

    // Attempt sequential migration from stored version → current.
    if cp.version < CHECKPOINT_VERSION {
        tracing::info!(
            from = cp.version,
            to = CHECKPOINT_VERSION,
            "migrating checkpoint schema"
        );
        cp = migrate_checkpoint(cp)?;
        return Ok(cp);
    }

    // Future version (downgrade not supported).
    tracing::error!(
        found = cp.version,
        expected = CHECKPOINT_VERSION,
        "checkpoint from newer AURA version — cannot downgrade"
    );
    Err(CheckpointError::VersionMismatch {
        found: cp.version,
        expected: CHECKPOINT_VERSION,
    })
}

/// Sequentially migrate a checkpoint from its stored version to
/// `CHECKPOINT_VERSION`. Each migration step handles exactly one
/// version bump (v1→v2, v2→v3, etc.).
///
/// To add a new migration when bumping CHECKPOINT_VERSION:
/// 1. Add a new match arm for the old version
/// 2. Apply field transformations / defaults
/// 3. Bump cp.version
///
/// Currently v1 is the latest, so no migrations are needed.
/// This framework ensures future schema changes have a safe upgrade path.
fn migrate_checkpoint(mut cp: DaemonCheckpoint) -> Result<DaemonCheckpoint, CheckpointError> {
    while cp.version < CHECKPOINT_VERSION {
        match cp.version {
            // v1→v2: last_memory_insight_ms added (serde default handles it).
            1 => {
                cp.last_memory_insight_ms = 0;
                cp.version = 2;
                tracing::info!("migrated checkpoint v1→v2");
            },
            // v2→v3: consolidation_weights added (serde default handles it).
            2 => {
                cp.consolidation_weights = ConsolidationWeights::default();
                cp.version = 3;
                tracing::info!("migrated checkpoint v2→v3: consolidation_weights initialized");
            },
            unsupported => {
                tracing::warn!(
                    version = unsupported,
                    "no migration path from this version — resetting to defaults"
                );
                return Ok(DaemonCheckpoint::default());
            },
        }
    }
    Ok(cp)
}

// ---------------------------------------------------------------------------
// Atomic file I/O
// ---------------------------------------------------------------------------

/// Save checkpoint atomically: write `.tmp`, then rename.
pub fn save_checkpoint(cp: &DaemonCheckpoint, path: &Path) -> Result<(), CheckpointError> {
    let bytes = encode_checkpoint(cp)?;

    let tmp_path = path.with_extension("bin.tmp");

    // Write to temp file
    let mut file = std::fs::File::create(&tmp_path).map_err(|e| {
        tracing::error!(error = %e, path = %tmp_path.display(), "failed to create tmp checkpoint");
        CheckpointError::IoError(e.to_string())
    })?;
    file.write_all(&bytes).map_err(|e| {
        tracing::error!(error = %e, "failed to write checkpoint bytes");
        CheckpointError::IoError(e.to_string())
    })?;
    file.sync_all().map_err(|e| {
        tracing::error!(error = %e, "failed to sync checkpoint");
        CheckpointError::IoError(e.to_string())
    })?;
    drop(file);

    // Atomic rename
    std::fs::rename(&tmp_path, path).map_err(|e| {
        tracing::error!(error = %e, "failed to rename tmp checkpoint");
        CheckpointError::IoError(e.to_string())
    })?;

    tracing::info!(size = bytes.len(), path = %path.display(), "checkpoint saved");
    Ok(())
}

/// Load checkpoint from disk.  Returns `Ok(None)` if the file doesn't exist.
/// Returns `Ok(default)` if the file is corrupt (graceful degradation).
pub fn load_checkpoint(path: &Path) -> Result<DaemonCheckpoint, CheckpointError> {
    if !path.exists() {
        tracing::info!(path = %path.display(), "no checkpoint file — starting fresh");
        return Ok(DaemonCheckpoint::default());
    }

    let bytes = std::fs::read(path).map_err(|e| {
        tracing::error!(error = %e, path = %path.display(), "failed to read checkpoint");
        CheckpointError::IoError(e.to_string())
    })?;

    match decode_checkpoint(&bytes) {
        Ok(cp) => {
            tracing::info!(
                version = cp.version,
                goals = cp.goals.len(),
                cron_jobs = cp.cron_state.len(),
                select_count = cp.select_count,
                "checkpoint restored"
            );
            Ok(cp)
        },
        Err(e) => {
            tracing::warn!(
                error = %e,
                path = %path.display(),
                "corrupt checkpoint — falling back to defaults"
            );
            Ok(DaemonCheckpoint::default())
        },
    }
}

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

#[derive(Debug, thiserror::Error)]
pub enum CheckpointError {
    #[error("encode failed: {0}")]
    EncodeFailed(String),

    #[error("decode failed: {0}")]
    DecodeFailed(String),

    #[error("checkpoint too large: {size} bytes (max {max})")]
    TooLarge { size: usize, max: usize },

    #[error("version mismatch: found {found}, expected {expected}")]
    VersionMismatch { found: u32, expected: u32 },

    #[error("I/O error: {0}")]
    IoError(String),
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_roundtrip_default_checkpoint() {
        let cp = DaemonCheckpoint::default();
        let bytes = encode_checkpoint(&cp).expect("encode should succeed");
        let restored = decode_checkpoint(&bytes).expect("decode should succeed");

        assert_eq!(restored.version, CHECKPOINT_VERSION);
        assert_eq!(restored.select_count, 0);
        assert!(restored.goals.is_empty());
        assert!(restored.cron_state.is_empty());
        assert!((restored.trust_score - 0.0).abs() < f32::EPSILON);
    }

    #[test]
    fn test_roundtrip_with_data() {
        let mut cp = DaemonCheckpoint::default();
        cp.select_count = 42_000;
        cp.trust_score = 0.75;
        cp.relationship_stage = RelationshipStage::Friend;
        cp.goals.push(Goal {
            id: 1,
            description: "Learn Rust".to_string(),
            priority: GoalPriority::High,
            status: GoalStatus::Active,
            steps: Vec::new(),
            created_ms: 1_700_000_000_000,
            deadline_ms: Some(1_700_100_000_000),
            parent_goal: None,
            source: GoalSource::UserExplicit,
        });
        cp.cron_state.push(CronJobState {
            job_id: 7,
            job_name: "weekly_health_report".to_string(),
            last_fired_ms: 100_000,
            next_fire_ms: 700_000,
        });
        cp.token_counters.local_tokens = 12_345;
        cp.token_counters.cloud_tokens = 678;

        let bytes = encode_checkpoint(&cp).expect("encode should succeed");
        assert!(
            bytes.len() <= MAX_CHECKPOINT_BYTES,
            "checkpoint must fit in 64KB"
        );

        let restored = decode_checkpoint(&bytes).expect("decode should succeed");
        assert_eq!(restored.select_count, 42_000);
        assert!((restored.trust_score - 0.75).abs() < f32::EPSILON);
        assert_eq!(restored.relationship_stage, RelationshipStage::Friend);
        assert_eq!(restored.goals.len(), 1);
        assert_eq!(restored.goals[0].description, "Learn Rust");
        assert_eq!(restored.cron_state.len(), 1);
        assert_eq!(restored.cron_state[0].job_name, "weekly_health_report");
        assert_eq!(restored.token_counters.local_tokens, 12_345);
    }

    #[test]
    fn test_size_limit_enforced() {
        let mut cp = DaemonCheckpoint::default();
        // Stuff enough goals to exceed 64KB.
        for i in 0..5000 {
            cp.goals.push(Goal {
                id: i,
                description: format!("Goal {i} with a fairly long description to inflate size"),
                priority: GoalPriority::Medium,
                status: GoalStatus::Pending,
                steps: Vec::new(),
                created_ms: 0,
                deadline_ms: None,
                parent_goal: None,
                source: GoalSource::UserExplicit,
            });
        }
        let result = encode_checkpoint(&cp);
        assert!(result.is_err(), "should reject checkpoint exceeding 64KB");
    }

    #[test]
    fn test_corrupt_bytes_returns_error() {
        let garbage = vec![0xFF, 0xFE, 0xFD, 0xFC, 0x00, 0x01];
        let result = decode_checkpoint(&garbage);
        assert!(result.is_err(), "corrupt bytes should fail to decode");
    }

    #[test]
    fn test_atomic_save_and_load() {
        let dir = tempfile::tempdir().expect("tempdir should work");
        let path = dir.path().join("state.bin");

        let mut cp = DaemonCheckpoint::default();
        cp.select_count = 999;
        cp.trust_score = 0.42;

        save_checkpoint(&cp, &path).expect("save should succeed");
        assert!(path.exists(), "checkpoint file should exist");

        let loaded = load_checkpoint(&path).expect("load should succeed");
        assert_eq!(loaded.select_count, 999);
        assert!((loaded.trust_score - 0.42).abs() < f32::EPSILON);
    }

    #[test]
    fn test_load_missing_file_returns_default() {
        let dir = tempfile::tempdir().expect("tempdir should work");
        let path = dir.path().join("nonexistent.bin");

        let cp = load_checkpoint(&path).expect("missing file should return default");
        assert_eq!(cp.version, CHECKPOINT_VERSION);
        assert_eq!(cp.select_count, 0);
    }

    #[test]
    fn test_load_corrupt_file_returns_default() {
        let dir = tempfile::tempdir().expect("tempdir should work");
        let path = dir.path().join("state.bin");
        std::fs::write(&path, b"this is not valid bincode").expect("write garbage");

        let cp = load_checkpoint(&path).expect("corrupt file should return default");
        assert_eq!(cp.version, CHECKPOINT_VERSION);
        assert_eq!(cp.select_count, 0);
    }
}
