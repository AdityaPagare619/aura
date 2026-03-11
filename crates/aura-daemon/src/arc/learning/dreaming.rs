//! Dreaming engine — autonomous exploration during idle/charging periods.
//!
//! # Architecture (SPEC-ARC §8 — DREAMING State)
//!
//! AURA enters DREAMING state when all conditions are met:
//! - Device is **charging**
//! - Screen is **off**
//! - Battery is **> 30%**
//! - Thermal state is **nominal** (Cool or Warm)
//!
//! ## Phases (executed in order)
//!
//! 1. **Maintenance** — memory consolidation, pattern aging, dead-link cleanup.
//! 2. **ETG Verification** — verify stored execution trace graphs still work.
//! 3. **Exploration** — discover new app capabilities and UI paths.
//! 4. **Annotation** — label discovered elements, update knowledge base.
//! 5. **Cleanup** — prune stale data, compact storage.
//!
//! ## Safety Invariants
//!
//! - **NEVER modify user data** — all exploration is read-only / sandboxed.
//! - Depth limit of 5 navigation steps per exploration.
//! - App allowlist constrains which apps can be explored.
//! - Phase budget limits prevent runaway resource consumption.
//! - Thermal and battery checks at every phase transition.

use std::collections::HashMap;

use aura_types::memory::Episode;
use serde::{Deserialize, Serialize};
use tracing::{debug, info, instrument, warn};

use super::super::ArcError;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Minimum battery percentage required to enter DREAMING state.
pub const MIN_BATTERY_PERCENT: u8 = 30;

/// Maximum navigation depth during exploration phase.
pub const MAX_EXPLORATION_DEPTH: u8 = 5;

/// Maximum number of apps in the exploration allowlist.
pub const MAX_ALLOWLIST_SIZE: usize = 50;

/// Maximum number of discovered capabilities tracked.
pub const MAX_DISCOVERED_CAPABILITIES: usize = 512;

/// Maximum number of exploration sessions retained in history.
pub const MAX_SESSION_HISTORY: usize = 100;

/// Maximum time budget per phase (milliseconds) — 5 minutes.
pub const PHASE_TIME_BUDGET_MS: u64 = 5 * 60 * 1000;

/// Maximum total dreaming session time (milliseconds) — 30 minutes.
pub const MAX_SESSION_DURATION_MS: u64 = 30 * 60 * 1000;

/// Milliseconds in one day.
const MS_PER_DAY: u64 = 24 * 60 * 60 * 1000;

// ---------------------------------------------------------------------------
// DreamPhase
// ---------------------------------------------------------------------------

/// The five ordered phases of a dreaming session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DreamPhase {
    /// Memory consolidation, pattern aging, dead-link cleanup.
    Maintenance,
    /// Verify stored execution trace graphs still work.
    EtgVerification,
    /// Discover new app capabilities and UI paths.
    Exploration,
    /// Label discovered elements, update knowledge base.
    Annotation,
    /// Prune stale data, compact storage.
    Cleanup,
}

impl DreamPhase {
    /// All phases in execution order.
    pub const ALL: [DreamPhase; 5] = [
        DreamPhase::Maintenance,
        DreamPhase::EtgVerification,
        DreamPhase::Exploration,
        DreamPhase::Annotation,
        DreamPhase::Cleanup,
    ];

    /// Get the next phase in sequence, or `None` if this is the last phase.
    #[must_use]
    pub fn next(self) -> Option<DreamPhase> {
        match self {
            DreamPhase::Maintenance => Some(DreamPhase::EtgVerification),
            DreamPhase::EtgVerification => Some(DreamPhase::Exploration),
            DreamPhase::Exploration => Some(DreamPhase::Annotation),
            DreamPhase::Annotation => Some(DreamPhase::Cleanup),
            DreamPhase::Cleanup => None,
        }
    }

    /// Display name for logging.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            DreamPhase::Maintenance => "maintenance",
            DreamPhase::EtgVerification => "etg_verification",
            DreamPhase::Exploration => "exploration",
            DreamPhase::Annotation => "annotation",
            DreamPhase::Cleanup => "cleanup",
        }
    }
}

// ---------------------------------------------------------------------------
// DreamingConditions
// ---------------------------------------------------------------------------

/// Snapshot of device conditions for DREAMING eligibility.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DreamingConditions {
    /// Whether the device is charging.
    pub is_charging: bool,
    /// Whether the screen is off.
    pub screen_off: bool,
    /// Battery percentage (0–100).
    pub battery_percent: u8,
    /// Whether thermal state is nominal (Cool or Warm).
    pub thermal_nominal: bool,
    /// Current timestamp in milliseconds.
    pub now_ms: u64,
}

impl DreamingConditions {
    /// Check whether all conditions for DREAMING are met.
    #[must_use]
    pub fn can_dream(&self) -> bool {
        self.is_charging
            && self.screen_off
            && self.battery_percent > MIN_BATTERY_PERCENT
            && self.thermal_nominal
    }
}

// ---------------------------------------------------------------------------
// DiscoveredCapability
// ---------------------------------------------------------------------------

/// A capability discovered during exploration of an app.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveredCapability {
    /// The app package or identifier where this capability was found.
    pub app_id: String,
    /// Human-readable description of the capability.
    pub description: String,
    /// The UI path taken to reach this capability (list of element descriptors).
    pub ui_path: Vec<String>,
    /// Confidence that this capability is correctly identified [0.0, 1.0].
    pub confidence: f32,
    /// Whether this has been verified by a subsequent exploration session.
    pub verified: bool,
    /// Timestamp (ms) of discovery.
    pub discovered_ms: u64,
    /// Timestamp (ms) of last verification.
    pub last_verified_ms: u64,
}

// ---------------------------------------------------------------------------
// CapabilityGap
// ---------------------------------------------------------------------------

/// A gap detected from repeated task failures (§8.6 — Learning from Failure).
///
/// When 3+ failures occur on the same task pattern, a capability gap is registered.
/// The dreaming engine will attempt to fill this gap during exploration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapabilityGap {
    /// Description of the failed task or capability.
    pub description: String,
    /// Number of failures that led to this gap detection.
    pub failure_count: u32,
    /// The app contexts where failures occurred.
    pub app_contexts: Vec<String>,
    /// Whether the dreaming engine has attempted to fill this gap.
    pub exploration_attempted: bool,
    /// Whether the gap has been successfully resolved.
    pub resolved: bool,
    /// Timestamp (ms) of gap detection.
    pub detected_ms: u64,
}

/// Minimum failures before registering a capability gap.
pub const MIN_FAILURES_FOR_GAP: u32 = 3;

/// Minimum success rate for pathway survival (below this, pathway is pruned).
pub const MIN_PATHWAY_SUCCESS_RATE: f32 = 0.10;

/// Maximum ETG traces stored for consolidation.
pub const MAX_ETG_TRACES: usize = 1000;

// ---------------------------------------------------------------------------
// EtgTrace
// ---------------------------------------------------------------------------

/// An ETG (Execution Trace Graph) trace for memory consolidation.
/// Represents a successful action pathway that can be strengthened.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EtgTrace {
    /// Unique trace identifier.
    pub trace_id: u64,
    /// The action or goal this trace represents.
    pub action_name: String,
    /// Number of successful executions.
    pub success_count: u32,
    /// Number of failed executions.
    pub failure_count: u32,
    /// Last execution timestamp (ms).
    pub last_executed_ms: u64,
    /// Confidence score [0.0, 1.0].
    pub confidence: f32,
    /// Whether this trace has been consolidated recently.
    pub consolidated: bool,
}

impl EtgTrace {
    /// Calculate success rate for this trace.
    /// Uses Bayesian smoothing: prior of 1 success + 1 failure (starts at 0.5
    /// and converges to the actual rate as observations accumulate).
    #[must_use]
    pub fn success_rate(&self) -> f32 {
        let total = self.success_count + self.failure_count;
        if total == 0 {
            return 0.5; // Default when no data
        }
        // Bayesian smoothing with prior strength of 2 (1 pseudo-success + 1 pseudo-failure)
        (self.success_count as f32 + 1.0) / (total as f32 + 2.0)
    }

    /// Whether this trace should be pruned based on exposure-aware thresholds.
    ///
    /// Instead of a flat success rate threshold, the pruning criterion adapts:
    /// - Low-observation pathways get a lenient threshold (might just be unlucky)
    /// - High-observation pathways with consistently poor results are pruned aggressively
    /// Formula: threshold = MIN_PATHWAY_SUCCESS_RATE × (1 + 20 / (total + 20))
    /// At 5 obs → threshold ≈ 0.16 (lenient); at 100 obs → threshold ≈ 0.12 (strict)
    #[must_use]
    pub fn should_prune(&self) -> bool {
        let total = self.success_count + self.failure_count;
        if total < 5 {
            return false; // Not enough data to make a call
        }
        // Exposure-scaled threshold: lenient for few observations, strict for many
        let adaptive_threshold = MIN_PATHWAY_SUCCESS_RATE * (1.0 + 20.0 / (total as f32 + 20.0));
        self.success_rate() <= adaptive_threshold
    }
}

// ---------------------------------------------------------------------------
// ConsolidationStage
// ---------------------------------------------------------------------------

/// The four stages of memory consolidation during dreaming.
/// These run within the existing 5-phase framework.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ConsolidationStage {
    /// Load ETG traces into working memory.
    Sensorimotor,
    /// Strengthen successful pathways, prune weak ones.
    Consolidation,
    /// Replay successful traces to strengthen pathways.
    Replay,
    /// Generate insights from pattern analysis.
    Awake,
}

impl ConsolidationStage {
    /// All stages in order.
    pub const ALL: [ConsolidationStage; 4] = [
        ConsolidationStage::Sensorimotor,
        ConsolidationStage::Consolidation,
        ConsolidationStage::Replay,
        ConsolidationStage::Awake,
    ];

    /// Get next stage, or None if complete.
    #[must_use]
    pub fn next(self) -> Option<ConsolidationStage> {
        match self {
            ConsolidationStage::Sensorimotor => Some(ConsolidationStage::Consolidation),
            ConsolidationStage::Consolidation => Some(ConsolidationStage::Replay),
            ConsolidationStage::Replay => Some(ConsolidationStage::Awake),
            ConsolidationStage::Awake => None,
        }
    }

    /// Display name.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            ConsolidationStage::Sensorimotor => "sensorimotor",
            ConsolidationStage::Consolidation => "consolidation",
            ConsolidationStage::Replay => "replay",
            ConsolidationStage::Awake => "awake",
        }
    }
}

// ---------------------------------------------------------------------------
// DreamInsight
// ---------------------------------------------------------------------------

/// An insight generated during the Awake phase.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DreamInsight {
    /// Description of the insight.
    pub description: String,
    /// Confidence in the insight [0.0, 1.0].
    pub confidence: f32,
    /// Related action names.
    pub related_actions: Vec<String>,
    /// Timestamp when generated (ms).
    pub generated_ms: u64,
}

// ---------------------------------------------------------------------------
// PhaseReport
// ---------------------------------------------------------------------------

/// Result of executing a single dreaming phase.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PhaseReport {
    /// Which phase was executed.
    pub phase: Option<DreamPhase>,
    /// Duration of this phase in milliseconds.
    pub duration_ms: u64,
    /// Number of items processed (phase-specific meaning).
    pub items_processed: usize,
    /// Number of items discovered or created.
    pub items_created: usize,
    /// Number of items pruned or cleaned up.
    pub items_pruned: usize,
    /// Whether the phase completed successfully.
    pub completed: bool,
    /// Reason for early termination, if any.
    pub abort_reason: Option<String>,
}

// ---------------------------------------------------------------------------
// DreamSession
// ---------------------------------------------------------------------------

/// Record of a complete dreaming session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DreamSession {
    /// Unique session identifier.
    pub session_id: u64,
    /// Timestamp (ms) when the session started.
    pub started_ms: u64,
    /// Timestamp (ms) when the session ended.
    pub ended_ms: u64,
    /// Reports for each phase executed.
    pub phase_reports: Vec<PhaseReport>,
    /// Whether the session completed all phases.
    pub fully_completed: bool,
    /// Reason for early termination, if any.
    pub abort_reason: Option<String>,
}

impl DreamSession {
    /// Total duration of the session in milliseconds.
    #[must_use]
    pub fn duration_ms(&self) -> u64 {
        self.ended_ms.saturating_sub(self.started_ms)
    }

    /// Total items processed across all phases.
    #[must_use]
    pub fn total_items_processed(&self) -> usize {
        self.phase_reports.iter().map(|r| r.items_processed).sum()
    }

    /// Total items created across all phases.
    #[must_use]
    pub fn total_items_created(&self) -> usize {
        self.phase_reports.iter().map(|r| r.items_created).sum()
    }
}

// ---------------------------------------------------------------------------
// DreamingEngine
// ---------------------------------------------------------------------------

/// The dreaming engine orchestrates autonomous exploration during idle periods.
///
/// It checks device conditions, runs through the five dreaming phases in order,
/// and records all discoveries. Safety invariants are enforced at every step:
/// - Read-only exploration (never modify user data)
/// - App allowlist
/// - Depth limits
/// - Phase time budgets
/// - Thermal and battery checks at phase transitions
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DreamingEngine {
    /// Apps allowed for exploration (package IDs or identifiers).
    app_allowlist: Vec<String>,
    /// Capabilities discovered during exploration sessions.
    discovered_capabilities: HashMap<String, DiscoveredCapability>,
    /// Detected capability gaps from repeated failures.
    capability_gaps: Vec<CapabilityGap>,
    /// ETG traces for memory consolidation.
    etg_traces: HashMap<u64, EtgTrace>,
    /// Insights generated during dreaming.
    insights: Vec<DreamInsight>,
    /// History of completed dreaming sessions.
    session_history: Vec<DreamSession>,
    /// Monotonic session counter.
    session_counter: u64,
    /// Monotonic trace counter.
    trace_counter: u64,
    /// Whether a dreaming session is currently in progress.
    is_dreaming: bool,
    /// Current phase if dreaming is active.
    current_phase: Option<DreamPhase>,
    /// Current consolidation stage within the current phase.
    current_consolidation_stage: Option<ConsolidationStage>,
    /// Timestamp (ms) when current session started.
    current_session_start_ms: u64,
    /// Timestamp (ms) when current phase started.
    current_phase_start_ms: u64,
    /// Phase reports for the current session.
    current_phase_reports: Vec<PhaseReport>,
}

impl DreamingEngine {
    /// Create a new dreaming engine with an empty state.
    #[must_use]
    pub fn new() -> Self {
        Self {
            app_allowlist: Vec::with_capacity(16),
            discovered_capabilities: HashMap::with_capacity(64),
            capability_gaps: Vec::with_capacity(16),
            etg_traces: HashMap::with_capacity(128),
            insights: Vec::with_capacity(32),
            session_history: Vec::with_capacity(16),
            session_counter: 0,
            trace_counter: 0,
            is_dreaming: false,
            current_phase: None,
            current_consolidation_stage: None,
            current_session_start_ms: 0,
            current_phase_start_ms: 0,
            current_phase_reports: Vec::new(),
        }
    }

    // -- accessors ----------------------------------------------------------

    /// Whether a dreaming session is currently active.
    #[must_use]
    pub fn is_dreaming(&self) -> bool {
        self.is_dreaming
    }

    /// Current dreaming phase, if active.
    #[must_use]
    pub fn current_phase(&self) -> Option<DreamPhase> {
        self.current_phase
    }

    /// Number of apps in the allowlist.
    #[must_use]
    pub fn allowlist_size(&self) -> usize {
        self.app_allowlist.len()
    }

    /// Number of discovered capabilities.
    #[must_use]
    pub fn capability_count(&self) -> usize {
        self.discovered_capabilities.len()
    }

    /// Number of detected capability gaps.
    #[must_use]
    pub fn gap_count(&self) -> usize {
        self.capability_gaps.len()
    }

    /// Number of completed dreaming sessions in history.
    #[must_use]
    pub fn session_count(&self) -> usize {
        self.session_history.len()
    }

    /// Number of ETG traces stored.
    #[must_use]
    pub fn trace_count(&self) -> usize {
        self.etg_traces.len()
    }

    /// Number of insights generated.
    #[must_use]
    pub fn insight_count(&self) -> usize {
        self.insights.len()
    }

    /// Get an ETG trace by ID.
    #[must_use]
    pub fn get_trace(&self, trace_id: u64) -> Option<&EtgTrace> {
        self.etg_traces.get(&trace_id)
    }

    /// Iterate over all insights.
    #[must_use]
    pub fn insights(&self) -> &[DreamInsight] {
        &self.insights
    }

    /// Current consolidation stage, if in a dreaming session.
    #[must_use]
    pub fn current_consolidation_stage(&self) -> Option<ConsolidationStage> {
        self.current_consolidation_stage
    }

    /// Get a discovered capability by key (app_id + description hash).
    #[must_use]
    pub fn get_capability(&self, key: &str) -> Option<&DiscoveredCapability> {
        self.discovered_capabilities.get(key)
    }

    /// Iterate over all capability gaps.
    #[must_use]
    pub fn gaps(&self) -> &[CapabilityGap] {
        &self.capability_gaps
    }

    /// Iterate over session history.
    #[must_use]
    pub fn sessions(&self) -> &[DreamSession] {
        &self.session_history
    }

    /// Check if an app is in the allowlist.
    #[must_use]
    pub fn is_app_allowed(&self, app_id: &str) -> bool {
        self.app_allowlist.iter().any(|a| a == app_id)
    }

    // -- allowlist management -----------------------------------------------

    /// Add an app to the exploration allowlist.
    ///
    /// Returns `Err` if the allowlist is at capacity.
    pub fn add_to_allowlist(&mut self, app_id: &str) -> Result<(), ArcError> {
        if self.app_allowlist.iter().any(|a| a == app_id) {
            return Ok(()); // Already present
        }
        if self.app_allowlist.len() >= MAX_ALLOWLIST_SIZE {
            return Err(ArcError::CapacityExceeded {
                collection: "dreaming_allowlist".into(),
                max: MAX_ALLOWLIST_SIZE,
            });
        }
        self.app_allowlist.push(app_id.to_owned());
        debug!(app_id, "added to dreaming allowlist");
        Ok(())
    }

    /// Remove an app from the exploration allowlist.
    pub fn remove_from_allowlist(&mut self, app_id: &str) {
        self.app_allowlist.retain(|a| a != app_id);
    }

    // -- capability gap management ------------------------------------------

    /// Record a task failure. If the same description has reached
    /// [`MIN_FAILURES_FOR_GAP`], register a capability gap.
    #[instrument(skip_all, fields(description = %description))]
    pub fn record_failure(
        &mut self,
        description: &str,
        app_context: &str,
        now_ms: u64,
    ) -> Option<&CapabilityGap> {
        // Find existing gap or count
        if let Some(gap) = self
            .capability_gaps
            .iter_mut()
            .find(|g| g.description == description)
        {
            gap.failure_count = gap.failure_count.saturating_add(1);
            if !gap.app_contexts.contains(&app_context.to_owned()) {
                gap.app_contexts.push(app_context.to_owned());
            }
            debug!(
                description,
                failures = gap.failure_count,
                "updated capability gap"
            );
            return self
                .capability_gaps
                .iter()
                .find(|g| g.description == description);
        }

        // New gap candidate
        let gap = CapabilityGap {
            description: description.to_owned(),
            failure_count: 1,
            app_contexts: vec![app_context.to_owned()],
            exploration_attempted: false,
            resolved: false,
            detected_ms: now_ms,
        };
        self.capability_gaps.push(gap);
        debug!(description, "new capability gap candidate recorded");
        self.capability_gaps.last()
    }

    /// Get unresolved capability gaps with enough failures to warrant exploration.
    #[must_use]
    pub fn actionable_gaps(&self) -> Vec<&CapabilityGap> {
        self.capability_gaps
            .iter()
            .filter(|g| g.failure_count >= MIN_FAILURES_FOR_GAP && !g.resolved)
            .collect()
    }

    // -- discovery management -----------------------------------------------

    /// Record a discovered capability.
    ///
    /// The key is derived from `app_id` + `description`. If a capability with
    /// the same key already exists, it is updated (verified, confidence refreshed).
    pub fn record_discovery(
        &mut self,
        app_id: &str,
        description: &str,
        ui_path: Vec<String>,
        confidence: f32,
        now_ms: u64,
    ) -> Result<(), ArcError> {
        let key = format!("{app_id}:{description}");

        if let Some(existing) = self.discovered_capabilities.get_mut(&key) {
            existing.verified = true;
            existing.last_verified_ms = now_ms;
            existing.confidence = (existing.confidence * 0.7 + confidence * 0.3).clamp(0.0, 1.0);
            if !ui_path.is_empty() {
                existing.ui_path = ui_path;
            }
            debug!(key = key.as_str(), "capability re-verified");
            return Ok(());
        }

        if self.discovered_capabilities.len() >= MAX_DISCOVERED_CAPABILITIES {
            // Evict lowest-confidence unverified capability
            self.evict_weakest_capability();
        }

        let capability = DiscoveredCapability {
            app_id: app_id.to_owned(),
            description: description.to_owned(),
            ui_path,
            confidence: confidence.clamp(0.0, 1.0),
            verified: false,
            discovered_ms: now_ms,
            last_verified_ms: now_ms,
        };
        self.discovered_capabilities.insert(key.clone(), capability);
        debug!(key = key.as_str(), "new capability discovered");

        // Check if this resolves any capability gaps
        for gap in &mut self.capability_gaps {
            if !gap.resolved && gap.description.contains(description) {
                gap.resolved = true;
                info!(
                    gap_desc = gap.description.as_str(),
                    "capability gap resolved by discovery"
                );
            }
        }

        Ok(())
    }

    // -- session lifecycle --------------------------------------------------

    /// Attempt to start a new dreaming session.
    ///
    /// Checks conditions and transitions to the Maintenance phase if eligible.
    #[instrument(skip_all)]
    pub fn try_start_session(&mut self, conditions: &DreamingConditions) -> Result<bool, ArcError> {
        if self.is_dreaming {
            debug!("already in a dreaming session");
            return Ok(false);
        }

        if !conditions.can_dream() {
            debug!(
                charging = conditions.is_charging,
                screen_off = conditions.screen_off,
                battery = conditions.battery_percent,
                thermal_ok = conditions.thermal_nominal,
                "dreaming conditions not met"
            );
            return Ok(false);
        }

        self.session_counter += 1;
        self.is_dreaming = true;
        self.current_phase = Some(DreamPhase::Maintenance);
        self.current_session_start_ms = conditions.now_ms;
        self.current_phase_start_ms = conditions.now_ms;
        self.current_phase_reports.clear();

        info!(
            session_id = self.session_counter,
            "dreaming session started"
        );
        Ok(true)
    }

    /// Execute the Maintenance phase with the full 4-stage consolidation cycle.
    /// This is called when the current phase is DreamPhase::Maintenance.
    /// Returns a PhaseReport with the results.
    #[instrument(skip_all)]
    pub fn execute_maintenance(&mut self, now_ms: u64) -> Result<PhaseReport, ArcError> {
        if !self.is_dreaming {
            return Err(ArcError::DomainError {
                domain: super::super::DomainId::Learning,
                detail: "not in a dreaming session".into(),
            });
        }

        if self.current_phase != Some(DreamPhase::Maintenance) {
            return Err(ArcError::DomainError {
                domain: super::super::DomainId::Learning,
                detail: "not in Maintenance phase".into(),
            });
        }

        let start_ms = now_ms;

        // Run the full 4-stage consolidation cycle
        let (processed, created, pruned) = self.run_consolidation_cycle(now_ms);

        let duration_ms = now_ms.saturating_sub(start_ms);

        info!(
            processed,
            created, pruned, duration_ms, "maintenance phase complete"
        );

        Ok(PhaseReport {
            phase: Some(DreamPhase::Maintenance),
            duration_ms,
            items_processed: processed,
            items_created: created,
            items_pruned: pruned,
            completed: true,
            abort_reason: None,
        })
    }

    /// Check if the current phase has exceeded its time budget.
    #[must_use]
    pub fn is_phase_overtime(&self, now_ms: u64) -> bool {
        if !self.is_dreaming {
            return false;
        }
        now_ms.saturating_sub(self.current_phase_start_ms) > PHASE_TIME_BUDGET_MS
    }

    /// Check if the entire session has exceeded its time budget.
    #[must_use]
    pub fn is_session_overtime(&self, now_ms: u64) -> bool {
        if !self.is_dreaming {
            return false;
        }
        now_ms.saturating_sub(self.current_session_start_ms) > MAX_SESSION_DURATION_MS
    }

    /// Complete the current phase and advance to the next one.
    ///
    /// Returns the next phase, or `None` if all phases are done.
    /// Re-checks conditions at each transition for safety.
    #[instrument(skip_all)]
    pub fn advance_phase(
        &mut self,
        report: PhaseReport,
        conditions: &DreamingConditions,
    ) -> Result<Option<DreamPhase>, ArcError> {
        if !self.is_dreaming {
            return Err(ArcError::DomainError {
                domain: super::super::DomainId::Learning,
                detail: "not in a dreaming session".into(),
            });
        }

        self.current_phase_reports.push(report);

        // Safety: re-check conditions at phase transition
        if !conditions.can_dream() {
            let reason = "conditions no longer met at phase transition".to_owned();
            self.abort_session(conditions.now_ms, &reason);
            info!(reason = reason.as_str(), "dreaming session aborted");
            return Ok(None);
        }

        // Safety: check session-level time budget
        if self.is_session_overtime(conditions.now_ms) {
            let reason = "session time budget exceeded".to_owned();
            self.abort_session(conditions.now_ms, &reason);
            return Ok(None);
        }

        // Advance to next phase
        let current = self.current_phase.ok_or_else(|| ArcError::DomainError {
            domain: super::super::DomainId::Learning,
            detail: "no current phase".into(),
        })?;

        match current.next() {
            Some(next) => {
                self.current_phase = Some(next);
                self.current_phase_start_ms = conditions.now_ms;
                debug!(phase = next.name(), "advanced to next dreaming phase");
                Ok(Some(next))
            }
            None => {
                // All phases complete — finalize session
                self.finalize_session(conditions.now_ms);
                Ok(None)
            }
        }
    }

    /// Abort the current dreaming session with a reason.
    pub fn abort_session(&mut self, now_ms: u64, reason: &str) {
        if !self.is_dreaming {
            return;
        }

        let session = DreamSession {
            session_id: self.session_counter,
            started_ms: self.current_session_start_ms,
            ended_ms: now_ms,
            phase_reports: std::mem::take(&mut self.current_phase_reports),
            fully_completed: false,
            abort_reason: Some(reason.to_owned()),
        };
        self.push_session(session);

        self.is_dreaming = false;
        self.current_phase = None;
        warn!(
            reason,
            session_id = self.session_counter,
            "dreaming session aborted"
        );
    }

    /// Finalize the current session as fully completed.
    fn finalize_session(&mut self, now_ms: u64) {
        let session = DreamSession {
            session_id: self.session_counter,
            started_ms: self.current_session_start_ms,
            ended_ms: now_ms,
            phase_reports: std::mem::take(&mut self.current_phase_reports),
            fully_completed: true,
            abort_reason: None,
        };
        self.push_session(session);

        self.is_dreaming = false;
        self.current_phase = None;
        info!(
            session_id = self.session_counter,
            "dreaming session completed successfully"
        );
    }

    fn push_session(&mut self, session: DreamSession) {
        if self.session_history.len() >= MAX_SESSION_HISTORY {
            self.session_history.remove(0);
        }
        self.session_history.push(session);
    }

    // -- exploration helpers ------------------------------------------------

    /// Get the next apps to explore, prioritizing those with capability gaps.
    ///
    /// Returns up to `limit` app IDs sorted by priority.
    #[must_use]
    pub fn exploration_targets(&self, limit: usize) -> Vec<String> {
        let mut targets: Vec<(String, u32)> = Vec::new();

        // Priority 1: apps with unresolved capability gaps
        for gap in &self.capability_gaps {
            if gap.resolved || gap.failure_count < MIN_FAILURES_FOR_GAP {
                continue;
            }
            for app in &gap.app_contexts {
                if self.is_app_allowed(app) {
                    let entry = targets.iter_mut().find(|(a, _)| a == app);
                    if let Some((_, priority)) = entry {
                        *priority += gap.failure_count;
                    } else {
                        targets.push((app.clone(), gap.failure_count));
                    }
                }
            }
        }

        // Priority 2: allowlisted apps not recently explored
        for app in &self.app_allowlist {
            if !targets.iter().any(|(a, _)| a == app) {
                targets.push((app.clone(), 1));
            }
        }

        // Sort by descending priority
        targets.sort_by_key(|b| std::cmp::Reverse(b.1));

        targets
            .into_iter()
            .take(limit)
            .map(|(app, _)| app)
            .collect()
    }

    /// Get capabilities for a specific app.
    #[must_use]
    pub fn capabilities_for_app(&self, app_id: &str) -> Vec<&DiscoveredCapability> {
        self.discovered_capabilities
            .values()
            .filter(|c| c.app_id == app_id)
            .collect()
    }

    /// Get all verified capabilities.
    #[must_use]
    pub fn verified_capabilities(&self) -> Vec<&DiscoveredCapability> {
        self.discovered_capabilities
            .values()
            .filter(|c| c.verified)
            .collect()
    }

    // -- ETG trace management -------------------------------------------------

    /// Record a successful execution of an action (strengthens pathway).
    pub fn record_trace_success(&mut self, action_name: &str, now_ms: u64) {
        // Find existing trace or create new
        let trace_id = self.find_trace_by_action(action_name);

        if let Some(id) = trace_id {
            if let Some(trace) = self.etg_traces.get_mut(&id) {
                trace.success_count = trace.success_count.saturating_add(1);
                trace.last_executed_ms = now_ms;
                trace.consolidated = false;
                // Update confidence based on success rate
                trace.confidence = trace.success_rate();
            }
        } else {
            // Create new trace
            let trace_id = self.trace_counter;
            self.trace_counter = self.trace_counter.wrapping_add(1);

            let trace = EtgTrace {
                trace_id,
                action_name: action_name.to_owned(),
                success_count: 1,
                failure_count: 0,
                last_executed_ms: now_ms,
                confidence: 0.5,
                consolidated: false,
            };

            // Enforce capacity
            if self.etg_traces.len() >= MAX_ETG_TRACES {
                self.evict_oldest_trace();
            }

            self.etg_traces.insert(trace_id, trace);
            debug!(action = action_name, trace_id, "new ETG trace created");
        }
    }

    /// Record a failed execution of an action.
    pub fn record_trace_failure(&mut self, action_name: &str, now_ms: u64) {
        let trace_id = self.find_trace_by_action(action_name);

        if let Some(id) = trace_id {
            if let Some(trace) = self.etg_traces.get_mut(&id) {
                trace.failure_count = trace.failure_count.saturating_add(1);
                trace.last_executed_ms = now_ms;
                trace.confidence = trace.success_rate();
            }
        } else {
            // Create new trace with failure
            let trace_id = self.trace_counter;
            self.trace_counter = self.trace_counter.wrapping_add(1);

            let trace = EtgTrace {
                trace_id,
                action_name: action_name.to_owned(),
                success_count: 0,
                failure_count: 1,
                last_executed_ms: now_ms,
                confidence: 0.0,
                consolidated: false,
            };

            if self.etg_traces.len() >= MAX_ETG_TRACES {
                self.evict_oldest_trace();
            }

            self.etg_traces.insert(trace_id, trace);
        }
    }

    /// Find trace ID by action name.
    fn find_trace_by_action(&self, action_name: &str) -> Option<u64> {
        self.etg_traces
            .iter()
            .find(|(_, t)| t.action_name == action_name)
            .map(|(id, _)| *id)
    }

    /// Evict the oldest trace to make room.
    fn evict_oldest_trace(&mut self) {
        if let Some(oldest_key) = self
            .etg_traces
            .iter()
            .min_by_key(|(_, t)| t.last_executed_ms)
            .map(|(k, _)| *k)
        {
            self.etg_traces.remove(&oldest_key);
        }
    }

    // -- consolidation --------------------------------------------------------

    /// Run consolidation stage: strengthen successful pathways, prune weak ones.
    /// Returns the number of traces pruned.
    #[instrument(skip(self))]
    pub fn run_consolidation(&mut self) -> usize {
        let mut pruned = 0;

        // First pass: identify traces to prune
        let to_prune: Vec<u64> = self
            .etg_traces
            .iter()
            .filter(|(_, trace)| trace.should_prune())
            .map(|(id, _)| *id)
            .collect();

        // Prune the weak traces
        for trace_id in to_prune {
            if let Some(trace) = self.etg_traces.remove(&trace_id) {
                pruned += 1;
                debug!(trace_id, action = %trace.action_name, "pruned weak pathway");
            }
        }

        // Second pass: strengthen remaining unconsolidated traces
        for trace in self.etg_traces.values_mut() {
            if !trace.consolidated {
                trace.confidence = (trace.confidence * 0.7 + trace.success_rate() * 0.3).min(1.0);
                trace.consolidated = true;
            }
        }

        info!(pruned, "consolidation complete");
        pruned
    }

    /// Run replay stage: strengthen pathways by mental rehearsal.
    /// Returns number of traces replayed.
    #[instrument(skip(self))]
    pub fn run_replay(&mut self) -> usize {
        let mut replayed = 0;

        // Get successful traces (success rate > 0.5)
        let successful_ids: Vec<u64> = self
            .etg_traces
            .iter()
            .filter(|(_, t)| t.success_rate() > 0.5)
            .map(|(id, _)| *id)
            .collect();

        for trace_id in successful_ids {
            if let Some(trace) = self.etg_traces.get_mut(&trace_id) {
                // Mental rehearsal strengthens the pathway
                trace.confidence = (trace.confidence + 0.05).min(1.0);
                trace.consolidated = false; // Mark for next consolidation
                replayed += 1;
                debug!(trace_id, action = %trace.action_name, confidence = trace.confidence, "replayed");
            }
        }

        info!(replayed, "replay complete");
        replayed
    }

    /// Run awake stage: generate insights from pattern analysis.
    /// Returns number of insights generated.
    #[instrument(skip(self))]
    pub fn run_awake(&mut self, now_ms: u64) -> usize {
        let mut generated = 0;

        // Analyze patterns in traces
        let high_confidence_traces: Vec<&EtgTrace> = self
            .etg_traces
            .values()
            .filter(|t| t.confidence > 0.7)
            .collect();

        // Generate insight if we have enough successful patterns
        if high_confidence_traces.len() >= 3 {
            let actions: Vec<String> = high_confidence_traces
                .iter()
                .map(|t| t.action_name.clone())
                .collect();

            let avg_confidence: f32 = high_confidence_traces
                .iter()
                .map(|t| t.confidence)
                .sum::<f32>()
                / high_confidence_traces.len() as f32;

            let action_count = actions.len();
            let insight = DreamInsight {
                description: format!(
                    "Identified {} high-confidence action patterns with {:.0}% average success rate",
                    action_count,
                    avg_confidence * 100.0
                ),
                confidence: avg_confidence,
                related_actions: actions,
                generated_ms: now_ms,
            };

            self.insights.push(insight);
            generated += 1;
            info!(actions = action_count, "generated insight");
        }

        // Generate insight for pruned pathways
        let weak_count = self
            .etg_traces
            .values()
            .filter(|t| t.success_rate() < 0.3)
            .count();

        if weak_count > 0 {
            let insight = DreamInsight {
                description: format!(
                    "Detected {} low-performing action pathways that may need optimization",
                    weak_count
                ),
                confidence: 0.6,
                related_actions: Vec::new(),
                generated_ms: now_ms,
            };
            self.insights.push(insight);
            generated += 1;
        }

        generated
    }

    /// Execute a full consolidation cycle across all 4 stages.
    /// Returns tuple of (processed, created, pruned).
    pub fn run_consolidation_cycle(&mut self, now_ms: u64) -> (usize, usize, usize) {
        // Sensorimotor: load traces (we already have them in memory)
        let processed = self.etg_traces.len();

        // Consolidation: strengthen and prune
        let pruned = self.run_consolidation();

        // Replay: mental rehearsal
        let _replayed = self.run_replay();

        // Awake: generate insights
        let created = self.run_awake(now_ms);

        (processed, created, pruned)
    }

    /// Consolidate with episodic memory data.
    ///
    /// This is the bridge between the DreamingEngine and the episodic memory
    /// store.  The caller fetches recent episodes (async) and passes them in;
    /// we ingest them into our internal trace map so the consolidation cycle
    /// can reason over real interaction history, then run the full cycle and
    /// return any *new* insights produced (for the caller to persist back).
    ///
    /// # Flow
    /// 1. Ingest episodes → create/update `EtgTrace` entries from episode data.
    /// 2. Run `run_consolidation_cycle` (all 4 stages).
    /// 3. Collect and return only the insights created during *this* cycle.
    #[instrument(skip(self, episodes), fields(episode_count = episodes.len()))]
    pub fn consolidate_with_episodes(
        &mut self,
        episodes: &[Episode],
        now_ms: u64,
    ) -> Vec<DreamInsight> {
        let insights_before = self.insights.len();

        // Phase 1: Ingest episodes into EtgTrace map.
        // Each episode's content and tags are scanned to correlate with
        // existing traces or create lightweight trace stubs.
        for ep in episodes {
            // Derive a stable trace id from episode id so repeated
            // consolidation cycles are idempotent for the same episode.
            let trace_id = ep.id;

            if let Some(trace) = self.etg_traces.get_mut(&trace_id) {
                // Episode already ingested in a prior cycle — just freshen it.
                trace.last_executed_ms = trace.last_executed_ms.max(ep.timestamp_ms);
                continue;
            }

            // Determine success/failure heuristic from episode metadata.
            // Positive emotional valence → success, negative → failure.
            let (succ, fail) = if ep.emotional_valence >= 0.0 {
                (1u32, 0u32)
            } else {
                (0u32, 1u32)
            };

            // Build an action name from the first context tag, falling back
            // to a truncated content prefix.
            let action_name = ep
                .context_tags
                .first()
                .cloned()
                .unwrap_or_else(|| {
                    ep.content.chars().take(64).collect::<String>()
                });

            let trace = EtgTrace {
                trace_id,
                action_name,
                success_count: succ,
                failure_count: fail,
                last_executed_ms: ep.timestamp_ms,
                confidence: ep.importance.clamp(0.0, 1.0),
                consolidated: false,
            };

            self.etg_traces.insert(trace_id, trace);
        }

        info!(
            ingested = episodes.len(),
            total_traces = self.etg_traces.len(),
            "episode ingestion complete"
        );

        // Phase 2: Run the full 4-stage consolidation cycle.
        let (processed, created, pruned) = self.run_consolidation_cycle(now_ms);
        info!(processed, created, pruned, "consolidation cycle complete");

        // Phase 3: Return only the insights generated during *this* call.
        self.insights[insights_before..].to_vec()
    }

    /// Get actionable insights (high confidence).
    #[must_use]
    pub fn actionable_insights(&self) -> Vec<&DreamInsight> {
        self.insights
            .iter()
            .filter(|i| i.confidence > 0.7)
            .collect()
    }

    // -- eviction helpers ---------------------------------------------------

    fn evict_weakest_capability(&mut self) {
        // Find lowest-confidence unverified capability
        let weakest_key = self
            .discovered_capabilities
            .iter()
            .filter(|(_, c)| !c.verified)
            .min_by(|a, b| {
                a.1.confidence
                    .partial_cmp(&b.1.confidence)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .map(|(k, _)| k.clone());

        if let Some(key) = weakest_key {
            self.discovered_capabilities.remove(&key);
        } else {
            // All verified — evict the oldest
            let oldest_key = self
                .discovered_capabilities
                .iter()
                .min_by_key(|(_, c)| c.last_verified_ms)
                .map(|(k, _)| k.clone());
            if let Some(key) = oldest_key {
                self.discovered_capabilities.remove(&key);
            }
        }
    }
}

impl Default for DreamingEngine {
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

    fn good_conditions(now_ms: u64) -> DreamingConditions {
        DreamingConditions {
            is_charging: true,
            screen_off: true,
            battery_percent: 80,
            thermal_nominal: true,
            now_ms,
        }
    }

    fn bad_conditions(now_ms: u64) -> DreamingConditions {
        DreamingConditions {
            is_charging: false,
            screen_off: true,
            battery_percent: 80,
            thermal_nominal: true,
            now_ms,
        }
    }

    // -- DreamPhase tests ---------------------------------------------------

    #[test]
    fn test_phase_order() {
        let mut phase = DreamPhase::Maintenance;
        let expected = [
            DreamPhase::EtgVerification,
            DreamPhase::Exploration,
            DreamPhase::Annotation,
            DreamPhase::Cleanup,
        ];
        for exp in &expected {
            phase = phase.next().expect("should have next phase");
            assert_eq!(phase, *exp);
        }
        assert!(phase.next().is_none(), "cleanup should be last");
    }

    #[test]
    fn test_phase_names() {
        assert_eq!(DreamPhase::Maintenance.name(), "maintenance");
        assert_eq!(DreamPhase::EtgVerification.name(), "etg_verification");
        assert_eq!(DreamPhase::Exploration.name(), "exploration");
        assert_eq!(DreamPhase::Annotation.name(), "annotation");
        assert_eq!(DreamPhase::Cleanup.name(), "cleanup");
    }

    #[test]
    fn test_phase_all() {
        assert_eq!(DreamPhase::ALL.len(), 5);
        assert_eq!(DreamPhase::ALL[0], DreamPhase::Maintenance);
        assert_eq!(DreamPhase::ALL[4], DreamPhase::Cleanup);
    }

    // -- DreamingConditions tests -------------------------------------------

    #[test]
    fn test_can_dream_all_met() {
        let c = good_conditions(1000);
        assert!(c.can_dream());
    }

    #[test]
    fn test_cannot_dream_not_charging() {
        let c = DreamingConditions {
            is_charging: false,
            ..good_conditions(1000)
        };
        assert!(!c.can_dream());
    }

    #[test]
    fn test_cannot_dream_screen_on() {
        let c = DreamingConditions {
            screen_off: false,
            ..good_conditions(1000)
        };
        assert!(!c.can_dream());
    }

    #[test]
    fn test_cannot_dream_low_battery() {
        let c = DreamingConditions {
            battery_percent: 20,
            ..good_conditions(1000)
        };
        assert!(!c.can_dream());
    }

    #[test]
    fn test_cannot_dream_thermal_hot() {
        let c = DreamingConditions {
            thermal_nominal: false,
            ..good_conditions(1000)
        };
        assert!(!c.can_dream());
    }

    // -- DreamingEngine basics ----------------------------------------------

    #[test]
    fn test_new_engine() {
        let e = DreamingEngine::new();
        assert!(!e.is_dreaming());
        assert_eq!(e.capability_count(), 0);
        assert_eq!(e.gap_count(), 0);
        assert_eq!(e.session_count(), 0);
        assert_eq!(e.allowlist_size(), 0);
    }

    // -- Allowlist tests ----------------------------------------------------

    #[test]
    fn test_allowlist_add_remove() {
        let mut e = DreamingEngine::new();
        e.add_to_allowlist("com.slack").expect("ok");
        assert!(e.is_app_allowed("com.slack"));
        assert_eq!(e.allowlist_size(), 1);

        // Duplicate add is idempotent
        e.add_to_allowlist("com.slack").expect("ok");
        assert_eq!(e.allowlist_size(), 1);

        e.remove_from_allowlist("com.slack");
        assert!(!e.is_app_allowed("com.slack"));
        assert_eq!(e.allowlist_size(), 0);
    }

    #[test]
    fn test_allowlist_capacity() {
        let mut e = DreamingEngine::new();
        for i in 0..MAX_ALLOWLIST_SIZE {
            e.add_to_allowlist(&format!("app_{i}")).expect("ok");
        }
        let result = e.add_to_allowlist("one_more");
        assert!(result.is_err());
    }

    // -- Capability gap tests -----------------------------------------------

    #[test]
    fn test_record_failure_creates_gap() {
        let mut e = DreamingEngine::new();
        e.record_failure("send_email", "com.gmail", 1000);
        assert_eq!(e.gap_count(), 1);
    }

    #[test]
    fn test_record_failure_accumulates() {
        let mut e = DreamingEngine::new();
        for i in 0..5 {
            e.record_failure("send_email", "com.gmail", 1000 + i * 100);
        }
        assert_eq!(e.gap_count(), 1);
        let gaps = e.actionable_gaps();
        assert_eq!(gaps.len(), 1);
        assert_eq!(gaps[0].failure_count, 5);
    }

    #[test]
    fn test_actionable_gaps_threshold() {
        let mut e = DreamingEngine::new();
        // Only 2 failures — not enough for actionable gap
        e.record_failure("task_a", "app_a", 1000);
        e.record_failure("task_a", "app_a", 2000);
        assert!(e.actionable_gaps().is_empty());

        // Third failure crosses threshold
        e.record_failure("task_a", "app_a", 3000);
        assert_eq!(e.actionable_gaps().len(), 1);
    }

    // -- Discovery tests ----------------------------------------------------

    #[test]
    fn test_record_discovery() {
        let mut e = DreamingEngine::new();
        e.record_discovery(
            "com.slack",
            "send_message",
            vec!["open_app".into(), "tap_compose".into()],
            0.8,
            1000,
        )
        .expect("ok");
        assert_eq!(e.capability_count(), 1);
        assert!(e.get_capability("com.slack:send_message").is_some());
    }

    #[test]
    fn test_record_discovery_reverify() {
        let mut e = DreamingEngine::new();
        e.record_discovery("com.slack", "send_msg", vec![], 0.7, 1000)
            .expect("ok");
        assert!(
            !e.get_capability("com.slack:send_msg")
                .expect("found")
                .verified
        );

        e.record_discovery("com.slack", "send_msg", vec![], 0.9, 2000)
            .expect("ok");
        assert!(
            e.get_capability("com.slack:send_msg")
                .expect("found")
                .verified
        );
    }

    #[test]
    fn test_discovery_resolves_gap() {
        let mut e = DreamingEngine::new();
        for i in 0..4 {
            e.record_failure("send_msg", "com.slack", 1000 + i * 100);
        }
        assert!(!e.gaps()[0].resolved);

        e.record_discovery("com.slack", "send_msg", vec![], 0.9, 5000)
            .expect("ok");
        assert!(e.gaps()[0].resolved);
    }

    #[test]
    fn test_capabilities_for_app() {
        let mut e = DreamingEngine::new();
        e.record_discovery("com.slack", "msg", vec![], 0.8, 1000)
            .expect("ok");
        e.record_discovery("com.slack", "call", vec![], 0.7, 2000)
            .expect("ok");
        e.record_discovery("com.gmail", "send", vec![], 0.9, 3000)
            .expect("ok");

        assert_eq!(e.capabilities_for_app("com.slack").len(), 2);
        assert_eq!(e.capabilities_for_app("com.gmail").len(), 1);
        assert_eq!(e.capabilities_for_app("com.unknown").len(), 0);
    }

    // -- Session lifecycle tests --------------------------------------------

    #[test]
    fn test_start_session_good_conditions() {
        let mut e = DreamingEngine::new();
        let started = e.try_start_session(&good_conditions(1000)).expect("ok");
        assert!(started);
        assert!(e.is_dreaming());
        assert_eq!(e.current_phase(), Some(DreamPhase::Maintenance));
    }

    #[test]
    fn test_start_session_bad_conditions() {
        let mut e = DreamingEngine::new();
        let started = e.try_start_session(&bad_conditions(1000)).expect("ok");
        assert!(!started);
        assert!(!e.is_dreaming());
    }

    #[test]
    fn test_cannot_start_while_dreaming() {
        let mut e = DreamingEngine::new();
        e.try_start_session(&good_conditions(1000)).expect("ok");
        let second = e.try_start_session(&good_conditions(2000)).expect("ok");
        assert!(!second, "should not start a second session");
    }

    #[test]
    fn test_advance_through_all_phases() {
        let mut e = DreamingEngine::new();
        e.try_start_session(&good_conditions(1000)).expect("ok");

        let phases = [
            DreamPhase::EtgVerification,
            DreamPhase::Exploration,
            DreamPhase::Annotation,
            DreamPhase::Cleanup,
        ];

        let mut time = 2000u64;
        for expected_next in &phases {
            let report = PhaseReport {
                phase: e.current_phase(),
                duration_ms: 1000,
                items_processed: 5,
                completed: true,
                ..Default::default()
            };
            let next = e.advance_phase(report, &good_conditions(time)).expect("ok");
            assert_eq!(next, Some(*expected_next));
            time += 1000;
        }

        // Final phase completion
        let report = PhaseReport {
            phase: Some(DreamPhase::Cleanup),
            duration_ms: 500,
            items_processed: 2,
            completed: true,
            ..Default::default()
        };
        let next = e.advance_phase(report, &good_conditions(time)).expect("ok");
        assert!(next.is_none(), "no more phases");
        assert!(!e.is_dreaming());
        assert_eq!(e.session_count(), 1);
        assert!(e.sessions()[0].fully_completed);
    }

    #[test]
    fn test_abort_on_bad_conditions() {
        let mut e = DreamingEngine::new();
        e.try_start_session(&good_conditions(1000)).expect("ok");

        let report = PhaseReport {
            phase: Some(DreamPhase::Maintenance),
            completed: true,
            ..Default::default()
        };
        let next = e.advance_phase(report, &bad_conditions(2000)).expect("ok");
        assert!(next.is_none(), "should abort when conditions degrade");
        assert!(!e.is_dreaming());
        assert_eq!(e.session_count(), 1);
        assert!(!e.sessions()[0].fully_completed);
        assert!(e.sessions()[0].abort_reason.is_some());
    }

    #[test]
    fn test_abort_session_explicit() {
        let mut e = DreamingEngine::new();
        e.try_start_session(&good_conditions(1000)).expect("ok");
        e.abort_session(2000, "user woke up");
        assert!(!e.is_dreaming());
        assert_eq!(e.session_count(), 1);
    }

    #[test]
    fn test_session_overtime() {
        let mut e = DreamingEngine::new();
        e.try_start_session(&good_conditions(0)).expect("ok");
        assert!(!e.is_session_overtime(1000));
        assert!(e.is_session_overtime(MAX_SESSION_DURATION_MS + 1));
    }

    #[test]
    fn test_phase_overtime() {
        let mut e = DreamingEngine::new();
        e.try_start_session(&good_conditions(0)).expect("ok");
        assert!(!e.is_phase_overtime(1000));
        assert!(e.is_phase_overtime(PHASE_TIME_BUDGET_MS + 1));
    }

    // -- Exploration targets ------------------------------------------------

    #[test]
    fn test_exploration_targets_prioritizes_gaps() {
        let mut e = DreamingEngine::new();
        e.add_to_allowlist("com.slack").expect("ok");
        e.add_to_allowlist("com.gmail").expect("ok");
        e.add_to_allowlist("com.calendar").expect("ok");

        // Record failures for slack
        for i in 0..5 {
            e.record_failure("msg_fail", "com.slack", 1000 + i * 100);
        }

        let targets = e.exploration_targets(10);
        assert!(!targets.is_empty());
        // com.slack should be first because it has capability gaps
        assert_eq!(targets[0], "com.slack");
    }

    #[test]
    fn test_exploration_targets_includes_allowlist() {
        let mut e = DreamingEngine::new();
        e.add_to_allowlist("com.slack").expect("ok");
        e.add_to_allowlist("com.gmail").expect("ok");

        let targets = e.exploration_targets(10);
        assert_eq!(targets.len(), 2);
    }

    // -- DreamSession tests -------------------------------------------------

    #[test]
    fn test_session_duration() {
        let s = DreamSession {
            session_id: 1,
            started_ms: 1000,
            ended_ms: 5000,
            phase_reports: vec![],
            fully_completed: true,
            abort_reason: None,
        };
        assert_eq!(s.duration_ms(), 4000);
    }

    #[test]
    fn test_session_totals() {
        let s = DreamSession {
            session_id: 1,
            started_ms: 0,
            ended_ms: 10000,
            phase_reports: vec![
                PhaseReport {
                    items_processed: 10,
                    items_created: 3,
                    ..Default::default()
                },
                PhaseReport {
                    items_processed: 20,
                    items_created: 5,
                    ..Default::default()
                },
            ],
            fully_completed: true,
            abort_reason: None,
        };
        assert_eq!(s.total_items_processed(), 30);
        assert_eq!(s.total_items_created(), 8);
    }

    // -- Serde roundtrip tests ----------------------------------------------

    #[test]
    fn test_serde_dream_phase() {
        for phase in &DreamPhase::ALL {
            let json = serde_json::to_string(phase).expect("serialize");
            let back: DreamPhase = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(*phase, back);
        }
    }

    #[test]
    fn test_serde_conditions() {
        let c = good_conditions(1000);
        let json = serde_json::to_string(&c).expect("serialize");
        let back: DreamingConditions = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.battery_percent, 80);
        assert!(back.can_dream());
    }

    #[test]
    fn test_serde_engine_roundtrip() {
        let mut e = DreamingEngine::new();
        e.add_to_allowlist("com.test").expect("ok");
        e.record_failure("test_gap", "com.test", 1000);
        e.record_discovery("com.test", "feat", vec!["a".into()], 0.9, 2000)
            .expect("ok");

        let json = serde_json::to_string(&e).expect("serialize");
        let back: DreamingEngine = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.allowlist_size(), 1);
        assert_eq!(back.gap_count(), 1);
        assert_eq!(back.capability_count(), 1);
    }

    #[test]
    fn test_session_history_bounded() {
        let mut e = DreamingEngine::new();
        for i in 0..(MAX_SESSION_HISTORY + 10) {
            e.try_start_session(&good_conditions(i as u64 * 100_000))
                .expect("ok");
            e.abort_session(i as u64 * 100_000 + 1000, "test");
        }
        assert!(
            e.session_count() <= MAX_SESSION_HISTORY,
            "session history should be bounded"
        );
    }

    #[test]
    fn test_advance_phase_not_dreaming_errors() {
        let mut e = DreamingEngine::new();
        let report = PhaseReport::default();
        let result = e.advance_phase(report, &good_conditions(1000));
        assert!(result.is_err(), "should error when not dreaming");
    }

    // -- Consolidation cycle tests -----------------------------------------

    #[test]
    fn test_run_consolidation_prunes_weak_pathways() {
        let mut e = DreamingEngine::new();

        // Create traces with success rate < 10% - use same action name to accumulate failures
        for i in 0..10 {
            e.record_trace_failure("weak_action", 1000 + i as u64 * 100);
        }

        // Some traces should be pruned after consolidation
        let pruned = e.run_consolidation();
        assert!(pruned >= 1, "should prune at least one weak pathway");
    }

    #[test]
    fn test_run_consolidation_strengthens_strong_pathways() {
        let mut e = DreamingEngine::new();

        // Create successful traces
        for i in 0..20 {
            e.record_trace_success("strong_action", 1000 + i as u64 * 100);
        }

        let before_conf = e.get_trace(0).map(|t| t.confidence).unwrap_or(0.0);

        e.run_consolidation();

        let after_conf = e.get_trace(0).map(|t| t.confidence).unwrap_or(0.0);
        assert!(
            after_conf >= before_conf,
            "confidence should not decrease after consolidation"
        );
    }

    #[test]
    fn test_run_replay_increases_confidence() {
        let mut e = DreamingEngine::new();

        // Create some successful traces
        for _ in 0..10 {
            e.record_trace_success("action_a", 1000);
            e.record_trace_success("action_b", 2000);
        }

        let before_replay = e.trace_count();
        let replayed = e.run_replay();

        assert!(replayed >= 1, "should replay at least one trace");
        assert_eq!(
            before_replay,
            e.trace_count(),
            "replay should not change trace count"
        );
    }

    #[test]
    fn test_run_awake_generates_insights() {
        let mut e = DreamingEngine::new();

        // Create high-confidence traces
        for _ in 0..10 {
            e.record_trace_success("high_conf_action_1", 1000);
            e.record_trace_success("high_conf_action_2", 2000);
            e.record_trace_success("high_conf_action_3", 3000);
        }

        let insights_before = e.insight_count();
        let generated = e.run_awake(5000);

        assert!(generated >= 1, "should generate at least one insight");
        assert!(
            e.insight_count() > insights_before,
            "new insight should be added"
        );
    }

    #[test]
    fn test_run_awake_generates_weak_pathway_insight() {
        let mut e = DreamingEngine::new();

        // Create low-confidence traces
        for _ in 0..10 {
            e.record_trace_failure("weak_pathway", 1000);
        }

        let insights_before = e.insight_count();
        let generated = e.run_awake(5000);

        // Should generate insight about weak pathways
        assert!(
            generated >= 1,
            "should generate insight about weak pathways"
        );
    }

    #[test]
    fn test_full_consolidation_cycle() {
        let mut e = DreamingEngine::new();

        // Add traces - use same action names to accumulate attempts
        for i in 0..5 {
            e.record_trace_success(&format!("action_{}", i), 1000 + i as u64 * 100);
        }
        // Use same action name for failures to accumulate at least 5 attempts
        for i in 0..5 {
            e.record_trace_failure("weak_action", 2000 + i as u64 * 100);
        }

        let trace_count_before = e.trace_count();
        let (processed, created, pruned) = e.run_consolidation_cycle(5000);

        assert!(processed >= 1, "should process traces");
        assert!(pruned >= 1, "should prune weak pathways");
        assert!(
            e.trace_count() <= trace_count_before,
            "trace count should not increase after pruning"
        );
    }

    #[test]
    fn test_consolidation_stage_order() {
        // Verify all 4 stages are defined
        assert_eq!(ConsolidationStage::ALL.len(), 4);
        assert_eq!(ConsolidationStage::ALL[0], ConsolidationStage::Sensorimotor);
        assert_eq!(
            ConsolidationStage::ALL[1],
            ConsolidationStage::Consolidation
        );
        assert_eq!(ConsolidationStage::ALL[2], ConsolidationStage::Replay);
        assert_eq!(ConsolidationStage::ALL[3], ConsolidationStage::Awake);
    }

    #[test]
    fn test_stage_next_order() {
        let mut stage = ConsolidationStage::Sensorimotor;
        assert_eq!(stage.next(), Some(ConsolidationStage::Consolidation));

        stage = ConsolidationStage::Consolidation;
        assert_eq!(stage.next(), Some(ConsolidationStage::Replay));

        stage = ConsolidationStage::Replay;
        assert_eq!(stage.next(), Some(ConsolidationStage::Awake));

        stage = ConsolidationStage::Awake;
        assert_eq!(stage.next(), None);
    }

    #[test]
    fn test_stage_names() {
        assert_eq!(ConsolidationStage::Sensorimotor.name(), "sensorimotor");
        assert_eq!(ConsolidationStage::Consolidation.name(), "consolidation");
        assert_eq!(ConsolidationStage::Replay.name(), "replay");
        assert_eq!(ConsolidationStage::Awake.name(), "awake");
    }

    #[test]
    fn test_etg_trace_success_rate() {
        let trace = EtgTrace {
            trace_id: 1,
            action_name: "test_action".into(),
            success_count: 8,
            failure_count: 2,
            last_executed_ms: 1000,
            confidence: 0.8,
            consolidated: false,
        };

        assert!((trace.success_rate() - 0.8).abs() < 0.001);
    }

    #[test]
    fn test_etg_trace_should_prune() {
        // High failure rate, enough total attempts
        let trace = EtgTrace {
            trace_id: 1,
            action_name: "weak".into(),
            success_count: 1,
            failure_count: 9,
            last_executed_ms: 1000,
            confidence: 0.1,
            consolidated: false,
        };

        assert!(
            trace.should_prune(),
            "should prune low success rate with enough attempts"
        );
    }

    #[test]
    fn test_etg_trace_should_not_prune_insufficient_data() {
        // Not enough total attempts
        let trace = EtgTrace {
            trace_id: 1,
            action_name: "new".into(),
            success_count: 0,
            failure_count: 2,
            last_executed_ms: 1000,
            confidence: 0.0,
            consolidated: false,
        };

        assert!(
            !trace.should_prune(),
            "should not prune with insufficient data"
        );
    }

    #[test]
    fn test_etg_trace_should_not_prune_high_success() {
        // High success rate
        let trace = EtgTrace {
            trace_id: 1,
            action_name: "strong".into(),
            success_count: 9,
            failure_count: 1,
            last_executed_ms: 1000,
            confidence: 0.9,
            consolidated: false,
        };

        assert!(!trace.should_prune(), "should not prune high success rate");
    }

    #[test]
    fn test_dream_insight_fields() {
        let insight = DreamInsight {
            description: "Test insight".into(),
            confidence: 0.85,
            related_actions: vec!["action1".into(), "action2".into()],
            generated_ms: 1000,
        };

        assert_eq!(insight.description, "Test insight");
        assert!((insight.confidence - 0.85).abs() < 0.001);
        assert_eq!(insight.related_actions.len(), 2);
    }

    #[test]
    fn test_actionable_insights_filter() {
        let mut e = DreamingEngine::new();

        // Add high-confidence insight
        e.insights.push(DreamInsight {
            description: "High confidence".into(),
            confidence: 0.8,
            related_actions: vec![],
            generated_ms: 1000,
        });

        // Add low-confidence insight
        e.insights.push(DreamInsight {
            description: "Low confidence".into(),
            confidence: 0.5,
            related_actions: vec![],
            generated_ms: 2000,
        });

        let actionable = e.actionable_insights();
        assert_eq!(actionable.len(), 1);
        assert!((actionable[0].confidence - 0.8).abs() < 0.001);
    }

    #[test]
    fn test_trace_management_capacity() {
        let mut e = DreamingEngine::new();

        // Add traces up to capacity
        for i in 0..1000 {
            e.record_trace_success(&format!("action_{}", i), 1000 + i as u64);
        }

        // Adding more should trigger eviction
        e.record_trace_success("new_action", 2000);

        // Should still have traces
        assert!(e.trace_count() <= MAX_ETG_TRACES);
    }

    #[test]
    fn test_min_pathway_success_rate_constant() {
        // Verify the constant is 10%
        assert!((MIN_PATHWAY_SUCCESS_RATE - 0.10).abs() < 0.001);
    }

    #[test]
    fn test_dream_conditions_at_boundary() {
        // Exactly at boundary (30%)
        let c = DreamingConditions {
            is_charging: true,
            screen_off: true,
            battery_percent: 30,
            thermal_nominal: true,
            now_ms: 1000,
        };

        assert!(!c.can_dream(), "30% should not be enough (need >30%)");

        // Just above boundary
        let c2 = DreamingConditions {
            battery_percent: 31,
            ..c
        };
        assert!(c2.can_dream(), "31% should be enough");
    }

    #[test]
    fn test_dream_conditions_all_factors() {
        let mut c = good_conditions(1000);
        assert!(c.can_dream());

        c.is_charging = false;
        assert!(!c.can_dream());

        c.is_charging = true;
        c.screen_off = false;
        assert!(!c.can_dream());

        c.screen_off = true;
        c.battery_percent = 25;
        assert!(!c.can_dream());

        c.battery_percent = 80;
        c.thermal_nominal = false;
        assert!(!c.can_dream());
    }
}
