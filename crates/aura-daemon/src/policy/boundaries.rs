//! Dynamic Boundary Reasoning — 3-level boundary system for contextual ethics.
//!
//! AURA's autonomy must be bounded by user safety, transparency, and consent.
//! The boundary system enforces three tiers of rules:
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────┐
//! │ Level 1: ABSOLUTE — hardcoded, NEVER overridden             │
//! │   "Never delete all data", "Never impersonate user"         │
//! ├─────────────────────────────────────────────────────────────┤
//! │ Level 2: CONDITIONAL — requires user confirmation            │
//! │   "Financial tx → confirm", "Send msg → review first"       │
//! ├─────────────────────────────────────────────────────────────┤
//! │ Level 3: LEARNED — adapted from user behavior               │
//! │   "User always denies X → stop asking"                      │
//! │   "User confirms Y fast → auto-approve"                     │
//! └─────────────────────────────────────────────────────────────┘
//! ```
//!
//! # Evaluation Order
//!
//! 1. **Level 1 (Absolute)**: If action matches → `DenyAbsolute`, STOP.
//! 2. **Level 3 (Learned)**: If high-confidence prediction → use it. (Checked before L2 because
//!    consistent user confirmation = auto-approve.)
//! 3. **Level 2 (Conditional)**: If action matches → `AllowWithConfirmation`.
//! 4. **Default** → `Allow` (things not covered by any rule).
//!
//! Level 3 can NEVER override Level 1.
//!
//! # TRUTH Protocol
//!
//! Every boundary decision must survive: "Does this help the user connect
//! more IRL, or isolate them?" AURA can refuse harmful actions.

use std::collections::VecDeque;

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum number of conditional rules.
const MAX_CONDITIONAL_RULES: usize = 128;

/// Maximum number of learned boundaries.
const MAX_LEARNED_BOUNDARIES: usize = 256;

/// Maximum number of decision log entries.
const MAX_DECISION_LOG_ENTRIES: usize = 512;

/// Minimum observation count before a learned boundary is trusted.
const MIN_OBSERVATIONS_FOR_CONFIDENCE: u32 = 5;

/// Confidence threshold for auto-approve/deny from learned boundaries.
const LEARNED_CONFIDENCE_THRESHOLD: f32 = 0.85;

/// Fast confirmation threshold (ms) — if user confirms within this time,
/// they probably want auto-approve in the future.
const FAST_CONFIRMATION_MS: u64 = 1500;

/// Default cooldown for conditional rules (ms) — how long after the last
/// confirmation before re-asking. 5 minutes.
const DEFAULT_COOLDOWN_MS: u64 = 300_000;

/// Milliseconds in one day — used for `FrequencyLimited` boundary checks.
const MS_PER_DAY: u64 = 86_400_000;

// ---------------------------------------------------------------------------
// BoundedVec<T> (local, independent of vault module)
// ---------------------------------------------------------------------------

/// A vector with a hard upper bound on capacity.
///
/// When the bound is reached, the oldest element (index 0) is evicted.
/// This guarantees bounded memory usage on mobile (4–8 GB RAM).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BoundedVec<T> {
    inner: VecDeque<T>,
    capacity: usize,
}

impl<T> BoundedVec<T> {
    /// Create a new bounded vector with the given maximum capacity.
    pub fn new(capacity: usize) -> Self {
        Self {
            inner: VecDeque::with_capacity(capacity.min(256)),
            capacity,
        }
    }

    /// Push an element, evicting the oldest if at capacity. O(1) amortized.
    pub fn push(&mut self, item: T) {
        if self.inner.len() >= self.capacity {
            self.inner.pop_front();
        }
        self.inner.push_back(item);
    }

    /// Number of elements currently stored.
    #[must_use]
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    /// Whether the collection is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// Iterate over all elements.
    pub fn iter(&self) -> std::collections::vec_deque::Iter<'_, T> {
        self.inner.iter()
    }

    /// Mutable iteration.
    pub fn iter_mut(&mut self) -> std::collections::vec_deque::IterMut<'_, T> {
        self.inner.iter_mut()
    }

    /// Retain only elements matching a predicate.
    pub fn retain<F: FnMut(&T) -> bool>(&mut self, f: F) {
        self.inner.retain(f);
    }

    /// Collect the contents as a `Vec<T>` (for testing/serialization).
    #[must_use]
    pub fn to_vec(&self) -> Vec<T>
    where
        T: Clone, {
        self.inner.iter().cloned().collect()
    }
}

// ---------------------------------------------------------------------------
// BoundaryLevel
// ---------------------------------------------------------------------------

/// The level of a boundary rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum BoundaryLevel {
    /// Level 1: Hardcoded, immutable, NEVER overridden.
    Absolute = 1,
    /// Level 2: Requires user confirmation before proceeding.
    Conditional = 2,
    /// Level 3: Adapted from user behavior over time.
    Learned = 3,
}

impl std::fmt::Display for BoundaryLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Absolute => write!(f, "L1:Absolute"),
            Self::Conditional => write!(f, "L2:Conditional"),
            Self::Learned => write!(f, "L3:Learned"),
        }
    }
}

// ---------------------------------------------------------------------------
// BoundaryDecision
// ---------------------------------------------------------------------------

/// The outcome of evaluating an action against the boundary system.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum BoundaryDecision {
    /// Action is allowed without any confirmation.
    Allow,
    /// Action is allowed but requires user confirmation first.
    AllowWithConfirmation {
        /// Why confirmation is needed.
        reason: String,
        /// The prompt to show the user.
        confirmation_prompt: String,
    },
    /// Action is denied by a conditional or learned rule.
    Deny {
        /// Why the action was denied.
        reason: String,
        /// Which level triggered the denial.
        level: BoundaryLevel,
    },
    /// Action is absolutely denied — hardcoded, immutable, NEVER overridden.
    DenyAbsolute {
        /// Why the action was denied.
        reason: String,
        /// The rule ID that triggered the denial.
        rule_id: &'static str,
    },
}

impl BoundaryDecision {
    /// Whether this decision allows the action (possibly with confirmation).
    #[must_use]
    pub fn is_allowed(&self) -> bool {
        matches!(self, Self::Allow | Self::AllowWithConfirmation { .. })
    }

    /// Whether this decision requires user confirmation.
    #[must_use]
    pub fn needs_confirmation(&self) -> bool {
        matches!(self, Self::AllowWithConfirmation { .. })
    }

    /// Whether this is an absolute denial (Level 1).
    #[must_use]
    pub fn is_absolute_deny(&self) -> bool {
        matches!(self, Self::DenyAbsolute { .. })
    }

    /// Human-readable summary of the decision.
    #[must_use]
    pub fn summary(&self) -> String {
        match self {
            Self::Allow => "allowed".to_string(),
            Self::AllowWithConfirmation { reason, .. } => {
                format!("allowed with confirmation: {reason}")
            },
            Self::Deny { reason, level } => format!("denied ({level}): {reason}"),
            Self::DenyAbsolute { reason, rule_id } => {
                format!("ABSOLUTE DENY [{rule_id}]: {reason}")
            },
        }
    }
}

// ---------------------------------------------------------------------------
// AbsoluteRule (Level 1)
// ---------------------------------------------------------------------------

/// A hardcoded, immutable rule that can NEVER be overridden.
///
/// These rules protect user safety and AURA's ethical integrity.
/// They are compiled into the binary and cannot be modified at runtime.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AbsoluteRule {
    /// Unique rule identifier (e.g., "abs_001").
    pub id: &'static str,
    /// Human-readable description of what this rule prevents.
    pub description: &'static str,
    /// Action pattern to match (substring match against action string).
    pub pattern: &'static str,
}

/// All absolute rules — hardcoded into the binary, immutable at runtime.
///
/// These represent AURA's ethical bedrock. No user preference, learned
/// behavior, or runtime configuration can override these.
const ABSOLUTE_RULES: &[AbsoluteRule] = &[
    AbsoluteRule {
        id: "abs_001",
        description: "Never delete all user data",
        pattern: "delete_all_data",
    },
    AbsoluteRule {
        id: "abs_002",
        description: "Never disable security features",
        pattern: "disable_security",
    },
    AbsoluteRule {
        id: "abs_003",
        description: "Never share data externally without consent",
        pattern: "share_external",
    },
    AbsoluteRule {
        id: "abs_004",
        description: "Never execute untrusted code",
        pattern: "execute_untrusted",
    },
    AbsoluteRule {
        id: "abs_005",
        description: "Never impersonate user legally/financially",
        pattern: "impersonate_legal",
    },
    AbsoluteRule {
        id: "abs_006",
        description: "Never access camera/mic without explicit consent",
        pattern: "access_camera_mic",
    },
    AbsoluteRule {
        id: "abs_007",
        description: "Never send Tier 3 data to external services",
        pattern: "send_critical_external",
    },
    AbsoluteRule {
        id: "abs_008",
        description: "Never modify system security settings",
        pattern: "modify_security_settings",
    },
    AbsoluteRule {
        id: "abs_009",
        description: "Never factory reset device",
        pattern: "factory_reset",
    },
    AbsoluteRule {
        id: "abs_010",
        description: "Never install from unknown sources without explicit consent",
        pattern: "install_unknown_source",
    },
    AbsoluteRule {
        id: "abs_011",
        description: "Never disable accessibility service",
        pattern: "disable_accessibility",
    },
    AbsoluteRule {
        id: "abs_012",
        description: "Never override TRUTH protocol",
        pattern: "override_truth",
    },
    AbsoluteRule {
        id: "abs_013",
        description: "Never hide actions from user",
        pattern: "hide_action",
    },
    AbsoluteRule {
        id: "abs_014",
        description: "Never manipulate user emotionally",
        pattern: "emotional_manipulation",
    },
    AbsoluteRule {
        id: "abs_015",
        description: "Never create addiction patterns",
        pattern: "create_addiction",
    },
];

// ---------------------------------------------------------------------------
// ConditionalRule (Level 2)
// ---------------------------------------------------------------------------

/// A rule that requires user confirmation before the action proceeds.
///
/// These are configurable at runtime and can have cooldown periods
/// (if the user recently confirmed, don't ask again).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConditionalRule {
    /// Unique rule identifier.
    pub id: String,
    /// Human-readable description.
    pub description: String,
    /// Action pattern to match (substring match).
    pub action_pattern: String,
    /// The prompt shown to the user for confirmation.
    pub confirmation_prompt: String,
    /// Whether biometric auth is required in addition to confirmation.
    pub requires_biometric: bool,
    /// Cooldown period (ms) after last confirmation. If the user confirmed
    /// within this window, skip re-confirmation.
    pub cooldown_ms: u64,
    /// Timestamp (epoch ms) of the last user confirmation, if any.
    pub last_confirmed_ms: Option<u64>,
}

impl ConditionalRule {
    /// Whether the cooldown has elapsed since the last confirmation.
    #[must_use]
    pub fn is_cooldown_elapsed(&self, now_ms: u64) -> bool {
        match self.last_confirmed_ms {
            Some(last) => now_ms.saturating_sub(last) >= self.cooldown_ms,
            None => true, // never confirmed → ask
        }
    }

    /// Record that the user confirmed this rule.
    pub fn record_confirmation(&mut self, now_ms: u64) {
        self.last_confirmed_ms = Some(now_ms);
    }
}

// ---------------------------------------------------------------------------
// UserPreference (Level 3)
// ---------------------------------------------------------------------------

/// Learned user preference for a particular action pattern.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum UserPreference {
    /// User always confirms fast → auto-approve in the future.
    AlwaysAllow,
    /// User always denies → stop asking.
    AlwaysDeny,
    /// Only allow during certain hours (24h format, inclusive range).
    TimeRestricted {
        /// (start_hour, end_hour) — e.g., (9, 17) = 9am–5pm.
        allowed_hours: (u8, u8),
    },
    /// Maximum number of times per day.
    FrequencyLimited {
        /// Max occurrences per day.
        max_per_day: u32,
    },
    /// Only allow in a specific context.
    RequiresContext(String),
}

impl std::fmt::Display for UserPreference {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AlwaysAllow => write!(f, "AlwaysAllow"),
            Self::AlwaysDeny => write!(f, "AlwaysDeny"),
            Self::TimeRestricted { allowed_hours } => {
                write!(
                    f,
                    "TimeRestricted({:02}:00-{:02}:00)",
                    allowed_hours.0, allowed_hours.1
                )
            },
            Self::FrequencyLimited { max_per_day } => {
                write!(f, "FrequencyLimited({max_per_day}/day)")
            },
            Self::RequiresContext(ctx) => write!(f, "RequiresContext({ctx})"),
        }
    }
}

// ---------------------------------------------------------------------------
// LearnedBoundary (Level 3)
// ---------------------------------------------------------------------------

/// A boundary learned from observing user behavior over time.
///
/// These are created by `learn_from_history()` when patterns emerge
/// in the decision log — e.g., user always confirms financial transactions
/// within 1 second → auto-approve.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LearnedBoundary {
    /// Action pattern this boundary applies to.
    pub action_pattern: String,
    /// What the user prefers for this action.
    pub user_preference: UserPreference,
    /// Confidence in this preference (0.0–1.0).
    pub confidence: f32,
    /// Number of observations that led to this boundary.
    pub observation_count: u32,
    /// When this boundary was last updated (epoch ms).
    pub last_updated_ms: u64,
}

// ---------------------------------------------------------------------------
// BoundaryContext
// ---------------------------------------------------------------------------

/// Contextual information provided when evaluating an action.
///
/// This allows the boundary reasoner to make context-sensitive decisions
/// — e.g., deny financial transactions at 3am, or allow auto-approve
/// for trusted apps.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BoundaryContext {
    /// Current hour (0–23).
    pub current_hour: u8,
    /// Relationship/trust stage (0–4): 0 = new, 4 = deeply trusted.
    pub relationship_stage: u8,
    /// Category of the action (e.g., "financial", "messaging").
    pub action_category: String,
    /// Whether the action involves money.
    pub involves_money: bool,
    /// Whether the action involves sending messages.
    pub involves_messaging: bool,
    /// Whether the action accesses stored data.
    pub involves_data_access: bool,
    /// The data tier being accessed, if any (0–3, matching DataTier values).
    /// Uses `Option<u8>` instead of DataTier to keep modules independent.
    pub data_tier: Option<u8>,
    /// Target application, if applicable.
    pub target_app: Option<String>,
    /// Number of denials in the last hour.
    pub recent_denials: u32,
}

impl Default for BoundaryContext {
    fn default() -> Self {
        Self {
            current_hour: 12,
            relationship_stage: 0,
            action_category: String::new(),
            involves_money: false,
            involves_messaging: false,
            involves_data_access: false,
            data_tier: None,
            target_app: None,
            recent_denials: 0,
        }
    }
}

// ---------------------------------------------------------------------------
// BoundaryDecisionLog
// ---------------------------------------------------------------------------

/// A log entry recording a boundary decision and (optionally) the user's
/// subsequent response. Used by Level 3 learning to detect patterns.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BoundaryDecisionLog {
    /// When the decision was made (epoch ms).
    pub timestamp_ms: u64,
    /// The action that was evaluated.
    pub action: String,
    /// Summary of the decision.
    pub decision: String,
    /// Which level triggered the decision.
    pub level: BoundaryLevel,
    /// Whether the user allowed or denied (filled later).
    pub user_response: Option<bool>,
    /// How long the user took to respond (ms), if applicable.
    pub response_time_ms: Option<u64>,
}

// ---------------------------------------------------------------------------
// BoundaryStats
// ---------------------------------------------------------------------------

/// Aggregate statistics about the boundary system.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BoundaryStats {
    /// Number of absolute rules (always == ABSOLUTE_RULES.len()).
    pub absolute_rule_count: usize,
    /// Number of conditional rules.
    pub conditional_rule_count: usize,
    /// Number of learned boundaries.
    pub learned_boundary_count: usize,
    /// Number of decision log entries.
    pub decision_log_size: usize,
    /// Total absolute denials ever issued.
    pub total_absolute_denials: u64,
    /// Total conditional confirmations requested.
    pub total_confirmations_requested: u64,
    /// Total actions auto-approved by Level 3 learning.
    pub total_auto_approved: u64,
    /// Total actions auto-denied by Level 3 learning.
    pub total_auto_denied: u64,
}

// ---------------------------------------------------------------------------
// BoundaryReasoner
// ---------------------------------------------------------------------------

/// The Dynamic Boundary Reasoning engine.
///
/// Evaluates actions against a 3-level rule hierarchy:
/// 1. Absolute (hardcoded, immutable) → deny
/// 2. Learned (from user behavior) → auto-approve or auto-deny
/// 3. Conditional (requires confirmation) → confirm
/// 4. Default → allow
///
/// Level 3 is checked BEFORE Level 2 because if the user has consistently
/// confirmed something, we can auto-approve it (better UX). But Level 3
/// can NEVER override Level 1.
pub struct BoundaryReasoner {
    /// Hardcoded absolute rules (~15 rules, compiled in).
    /// SECURITY: &'static slice — compile-time immutable, cannot be mutated at runtime.
    /// GAP-CRIT-001: Vec allowed runtime mutation of ethical bedrock rules.
    absolute_rules: &'static [AbsoluteRule],
    /// Configurable conditional rules (bounded).
    conditional_rules: BoundedVec<ConditionalRule>,
    /// Learned user preferences (bounded).
    learned_boundaries: BoundedVec<LearnedBoundary>,
    /// Decision log for learning and transparency (bounded).
    decision_log: BoundedVec<BoundaryDecisionLog>,
    /// Running counters for stats.
    total_absolute_denials: u64,
    total_confirmations_requested: u64,
    total_auto_approved: u64,
    total_auto_denied: u64,
}

impl BoundaryReasoner {
    /// Create a new boundary reasoner with all absolute rules hardcoded
    /// and a default set of conditional rules.
    pub fn new() -> Self {
        let mut reasoner = Self {
            absolute_rules: ABSOLUTE_RULES,
            conditional_rules: BoundedVec::new(MAX_CONDITIONAL_RULES),
            learned_boundaries: BoundedVec::new(MAX_LEARNED_BOUNDARIES),
            decision_log: BoundedVec::new(MAX_DECISION_LOG_ENTRIES),
            total_absolute_denials: 0,
            total_confirmations_requested: 0,
            total_auto_approved: 0,
            total_auto_denied: 0,
        };

        // Initialize default conditional rules (Level 2).
        reasoner.init_default_conditional_rules();

        reasoner
    }

    /// The core decision method — evaluates an action against all three
    /// levels and returns the appropriate boundary decision.
    ///
    /// # Evaluation order
    ///
    /// 1. Level 1 (Absolute) → `DenyAbsolute` if matched.
    /// 2. Level 3 (Learned) → auto-approve/deny if high confidence.
    /// 3. Level 2 (Conditional) → `AllowWithConfirmation` if matched.
    /// 4. Default → `Allow`.
    #[must_use]
    pub fn evaluate(&self, action: &str, context: &BoundaryContext) -> BoundaryDecision {
        let lower_action = action.to_ascii_lowercase();

        // --- Level 1: Absolute (NEVER overridden) ---
        for rule in self.absolute_rules {
            if matches_action_pattern(&lower_action, rule.pattern) {
                tracing::warn!(
                    target: "BOUNDARY",
                    rule_id = rule.id,
                    action = action,
                    "ABSOLUTE DENY: {}",
                    rule.description
                );
                return BoundaryDecision::DenyAbsolute {
                    reason: rule.description.to_string(),
                    rule_id: rule.id,
                };
            }
        }

        // --- Level 3: Learned (checked BEFORE Level 2 for better UX) ---
        if let Some(decision) = self.check_learned(&lower_action, context) {
            return decision;
        }

        // --- Level 2: Conditional ---
        if let Some(prompt) = self.check_conditional(&lower_action, context) {
            return prompt;
        }

        // --- Default: Allow ---
        BoundaryDecision::Allow
    }

    /// Add a new conditional rule.
    pub fn add_conditional_rule(&mut self, rule: ConditionalRule) {
        tracing::info!(
            target: "BOUNDARY",
            rule_id = rule.id.as_str(),
            pattern = rule.action_pattern.as_str(),
            "added conditional rule"
        );
        self.conditional_rules.push(rule);
    }

    /// Remove a conditional rule by ID. Returns `true` if found and removed.
    pub fn remove_conditional_rule(&mut self, id: &str) -> bool {
        let before = self.conditional_rules.len();
        self.conditional_rules.retain(|r| r.id != id);
        let removed = self.conditional_rules.len() < before;
        if removed {
            tracing::info!(
                target: "BOUNDARY",
                rule_id = id,
                "removed conditional rule"
            );
        }
        removed
    }

    /// Record the user's response to a boundary decision, feeding Level 3
    /// learning. Call this after the user confirms or denies an action.
    pub fn record_user_response(&mut self, action: &str, allowed: bool, response_time_ms: u64) {
        let now_ms = current_timestamp_ms();

        // Update the most recent decision log entry for this action.
        for entry in self.decision_log.iter_mut().rev() {
            if entry.action == action && entry.user_response.is_none() {
                entry.user_response = Some(allowed);
                entry.response_time_ms = Some(response_time_ms);
                break;
            }
        }

        // If user confirmed a conditional rule, update its cooldown.
        if allowed {
            for rule in self.conditional_rules.iter_mut() {
                if action.to_ascii_lowercase().contains(&rule.action_pattern) {
                    rule.record_confirmation(now_ms);
                }
            }
        }

        tracing::debug!(
            target: "BOUNDARY",
            action = action,
            allowed = allowed,
            response_time_ms = response_time_ms,
            "recorded user response"
        );
    }

    /// Analyze the decision log and create/update learned boundaries.
    ///
    /// Scans for consistent patterns:
    /// - Action always allowed with fast response → AlwaysAllow
    /// - Action always denied → AlwaysDeny
    /// - Action only allowed during certain hours → TimeRestricted
    pub fn learn_from_history(&mut self) {
        use std::collections::HashMap;

        // Group log entries by action pattern.
        let mut action_stats: HashMap<String, ActionLearningStats> = HashMap::new();

        for entry in self.decision_log.iter() {
            if let Some(response) = entry.user_response {
                let stats = action_stats
                    .entry(entry.action.clone())
                    .or_default();
                stats.total += 1;
                if response {
                    stats.allowed += 1;
                    if let Some(rt) = entry.response_time_ms {
                        if rt < FAST_CONFIRMATION_MS {
                            stats.fast_confirms += 1;
                        }
                    }
                } else {
                    stats.denied += 1;
                }
            }
        }

        let now_ms = current_timestamp_ms();

        for (action, stats) in &action_stats {
            if stats.total < MIN_OBSERVATIONS_FOR_CONFIDENCE {
                continue;
            }

            let allow_rate = stats.allowed as f32 / stats.total as f32;
            let fast_rate = if stats.allowed > 0 {
                stats.fast_confirms as f32 / stats.allowed as f32
            } else {
                0.0
            };

            // Pattern: always fast-approve → AlwaysAllow
            if allow_rate > 0.95 && fast_rate > 0.8 {
                self.upsert_learned_boundary(
                    action.clone(),
                    UserPreference::AlwaysAllow,
                    allow_rate * fast_rate,
                    stats.total,
                    now_ms,
                );
                continue;
            }

            // Pattern: always deny → AlwaysDeny
            let deny_rate = stats.denied as f32 / stats.total as f32;
            if deny_rate > 0.95 {
                self.upsert_learned_boundary(
                    action.clone(),
                    UserPreference::AlwaysDeny,
                    deny_rate,
                    stats.total,
                    now_ms,
                );
                continue;
            }
        }

        tracing::info!(
            target: "BOUNDARY",
            learned_count = self.learned_boundaries.len(),
            "learning pass complete"
        );
    }

    /// Fast check: does this action match any absolute (Level 1) rule?
    #[must_use]
    pub fn is_absolute_deny(action: &str) -> bool {
        let lower = action.to_ascii_lowercase();
        ABSOLUTE_RULES.iter().any(|r| lower.contains(r.pattern))
    }

    /// Check if this action needs user confirmation (Level 2).
    ///
    /// Returns the confirmation prompt if needed, or `None`.
    #[must_use]
    pub fn needs_confirmation(&self, action: &str, context: &BoundaryContext) -> Option<String> {
        match self.check_conditional(&action.to_ascii_lowercase(), context) {
            Some(BoundaryDecision::AllowWithConfirmation {
                confirmation_prompt,
                ..
            }) => Some(confirmation_prompt),
            _ => None,
        }
    }

    /// Level 3 prediction: would the user likely allow this action?
    ///
    /// Returns `Some(true)` for predicted allow, `Some(false)` for predicted
    /// deny, or `None` if no confident prediction exists.
    #[must_use]
    pub fn user_would_allow(&self, action: &str, _context: &BoundaryContext) -> Option<bool> {
        let lower = action.to_ascii_lowercase();
        for boundary in self.learned_boundaries.iter() {
            if lower.contains(&boundary.action_pattern)
                && boundary.confidence >= LEARNED_CONFIDENCE_THRESHOLD
                && boundary.observation_count >= MIN_OBSERVATIONS_FOR_CONFIDENCE
            {
                return match &boundary.user_preference {
                    UserPreference::AlwaysAllow => Some(true),
                    UserPreference::AlwaysDeny => Some(false),
                    _ => None,
                };
            }
        }
        None
    }

    /// Aggregate statistics about the boundary system.
    #[must_use]
    pub fn stats(&self) -> BoundaryStats {
        BoundaryStats {
            absolute_rule_count: self.absolute_rules.len(),
            conditional_rule_count: self.conditional_rules.len(),
            learned_boundary_count: self.learned_boundaries.len(),
            decision_log_size: self.decision_log.len(),
            total_absolute_denials: self.total_absolute_denials,
            total_confirmations_requested: self.total_confirmations_requested,
            total_auto_approved: self.total_auto_approved,
            total_auto_denied: self.total_auto_denied,
        }
    }

    /// Transparency: export a human-readable summary of all active rules.
    ///
    /// The user can inspect this at any time — AURA hides nothing.
    #[must_use]
    pub fn export_rules_summary(&self) -> String {
        let mut out = String::with_capacity(2048);

        out.push_str("=== AURA Boundary Rules ===\n\n");

        out.push_str("--- Level 1: ABSOLUTE (never overridden) ---\n");
        for rule in self.absolute_rules {
            out.push_str(&format!(
                "  [{}] {} (pattern: {})\n",
                rule.id, rule.description, rule.pattern
            ));
        }

        out.push_str("\n--- Level 2: CONDITIONAL (requires confirmation) ---\n");
        for rule in self.conditional_rules.iter() {
            let bio = if rule.requires_biometric {
                " [biometric]"
            } else {
                ""
            };
            out.push_str(&format!(
                "  [{}] {}{} (pattern: {}, cooldown: {}ms)\n",
                rule.id, rule.description, bio, rule.action_pattern, rule.cooldown_ms
            ));
        }

        out.push_str("\n--- Level 3: LEARNED (from your behavior) ---\n");
        if self.learned_boundaries.is_empty() {
            out.push_str("  (no learned boundaries yet)\n");
        }
        for boundary in self.learned_boundaries.iter() {
            out.push_str(&format!(
                "  {} → {} (confidence: {:.0}%, observations: {})\n",
                boundary.action_pattern,
                boundary.user_preference,
                boundary.confidence * 100.0,
                boundary.observation_count,
            ));
        }

        out
    }

    /// Log a boundary decision for auditing and learning.
    pub fn log_decision(
        &mut self,
        action: &str,
        decision: &BoundaryDecision,
        level: BoundaryLevel,
    ) {
        self.decision_log.push(BoundaryDecisionLog {
            timestamp_ms: current_timestamp_ms(),
            action: action.to_string(),
            decision: decision.summary(),
            level,
            user_response: None,
            response_time_ms: None,
        });

        match decision {
            BoundaryDecision::DenyAbsolute { .. } => {
                self.total_absolute_denials = self.total_absolute_denials.saturating_add(1);
            },
            BoundaryDecision::AllowWithConfirmation { .. } => {
                self.total_confirmations_requested =
                    self.total_confirmations_requested.saturating_add(1);
            },
            _ => {},
        }
    }

    // --- Private helpers ---

    /// Check learned boundaries (Level 3).
    fn check_learned(
        &self,
        lower_action: &str,
        context: &BoundaryContext,
    ) -> Option<BoundaryDecision> {
        for boundary in self.learned_boundaries.iter() {
            if !lower_action.contains(&boundary.action_pattern) {
                continue;
            }
            if boundary.confidence < LEARNED_CONFIDENCE_THRESHOLD {
                continue;
            }
            if boundary.observation_count < MIN_OBSERVATIONS_FOR_CONFIDENCE {
                continue;
            }

            match &boundary.user_preference {
                UserPreference::AlwaysAllow => {
                    tracing::debug!(
                        target: "BOUNDARY",
                        pattern = boundary.action_pattern.as_str(),
                        confidence = boundary.confidence,
                        "Level 3: auto-approve (learned)"
                    );
                    return Some(BoundaryDecision::Allow);
                },
                UserPreference::AlwaysDeny => {
                    tracing::debug!(
                        target: "BOUNDARY",
                        pattern = boundary.action_pattern.as_str(),
                        confidence = boundary.confidence,
                        "Level 3: auto-deny (learned)"
                    );
                    return Some(BoundaryDecision::Deny {
                        reason: format!(
                            "learned: you usually deny '{}' (confidence: {:.0}%)",
                            boundary.action_pattern,
                            boundary.confidence * 100.0
                        ),
                        level: BoundaryLevel::Learned,
                    });
                },
                UserPreference::TimeRestricted { allowed_hours } => {
                    let (start, end) = *allowed_hours;
                    let hour = context.current_hour;
                    let in_range = if start <= end {
                        hour >= start && hour <= end
                    } else {
                        // Wraps midnight, e.g., (22, 6)
                        hour >= start || hour <= end
                    };
                    if !in_range {
                        return Some(BoundaryDecision::Deny {
                            reason: format!(
                                "learned: you only allow this during {:02}:00-{:02}:00",
                                start, end
                            ),
                            level: BoundaryLevel::Learned,
                        });
                    }
                },
                UserPreference::FrequencyLimited { max_per_day } => {
                    // Count today's occurrences of this action in the decision log.
                    let now_ms = current_timestamp_ms();
                    let day_start_ms = now_ms.saturating_sub(now_ms % MS_PER_DAY);
                    let today_count = self
                        .decision_log
                        .iter()
                        .filter(|entry| {
                            entry.timestamp_ms >= day_start_ms
                                && matches_action_pattern(
                                    &entry.action.to_ascii_lowercase(),
                                    &boundary.action_pattern,
                                )
                        })
                        .count() as u32;

                    if today_count >= *max_per_day {
                        tracing::info!(
                            target: "BOUNDARY",
                            pattern = boundary.action_pattern.as_str(),
                            today_count = today_count,
                            max_per_day = *max_per_day,
                            "Level 3: frequency limit exceeded"
                        );
                        return Some(BoundaryDecision::Deny {
                            reason: format!(
                                "learned: frequency limit reached ({today_count}/{max_per_day} today)"
                            ),
                            level: BoundaryLevel::Learned,
                        });
                    }
                },
                UserPreference::RequiresContext(required_ctx) => {
                    if !context.action_category.contains(required_ctx.as_str()) {
                        return Some(BoundaryDecision::Deny {
                            reason: format!(
                                "learned: you only allow this in context '{required_ctx}'"
                            ),
                            level: BoundaryLevel::Learned,
                        });
                    }
                },
            }
        }
        None
    }

    /// Check conditional rules (Level 2).
    fn check_conditional(
        &self,
        lower_action: &str,
        _context: &BoundaryContext,
    ) -> Option<BoundaryDecision> {
        let now_ms = current_timestamp_ms();

        for rule in self.conditional_rules.iter() {
            if lower_action.contains(&rule.action_pattern) {
                // Skip if within cooldown.
                if !rule.is_cooldown_elapsed(now_ms) {
                    tracing::debug!(
                        target: "BOUNDARY",
                        rule_id = rule.id.as_str(),
                        "cooldown active, skipping confirmation"
                    );
                    continue;
                }

                tracing::info!(
                    target: "BOUNDARY",
                    rule_id = rule.id.as_str(),
                    pattern = rule.action_pattern.as_str(),
                    "Level 2: confirmation required"
                );
                return Some(BoundaryDecision::AllowWithConfirmation {
                    reason: rule.description.clone(),
                    confirmation_prompt: rule.confirmation_prompt.clone(),
                });
            }
        }
        None
    }

    /// Insert or update a learned boundary.
    fn upsert_learned_boundary(
        &mut self,
        action_pattern: String,
        preference: UserPreference,
        confidence: f32,
        observations: u32,
        now_ms: u64,
    ) {
        // Check if we already have a boundary for this pattern.
        for boundary in self.learned_boundaries.iter_mut() {
            if boundary.action_pattern == action_pattern {
                boundary.user_preference = preference;
                boundary.confidence = confidence;
                boundary.observation_count = observations;
                boundary.last_updated_ms = now_ms;
                return;
            }
        }

        // New learned boundary.
        self.learned_boundaries.push(LearnedBoundary {
            action_pattern,
            user_preference: preference,
            confidence,
            observation_count: observations,
            last_updated_ms: now_ms,
        });
    }

    /// Initialize default conditional rules (Level 2).
    fn init_default_conditional_rules(&mut self) {
        let defaults = [
            (
                "cond_001",
                "Financial transaction",
                "financial_transaction",
                "AURA wants to make a financial transaction. Amount and details will be shown. Allow?",
                true,
            ),
            (
                "cond_002",
                "Send message",
                "send_message",
                "AURA wants to send a message on your behalf. Review the content before sending?",
                false,
            ),
            (
                "cond_003",
                "Install application",
                "install_app",
                "AURA wants to install an application. Approve the source and permissions?",
                false,
            ),
            (
                "cond_004",
                "Access sensitive data (Tier 2+)",
                "access_sensitive",
                "AURA needs to access sensitive data. Verify your identity to proceed.",
                true,
            ),
            (
                "cond_005",
                "Contact new person",
                "contact_new",
                "AURA wants to contact someone for the first time. Confirm recipient?",
                false,
            ),
            (
                "cond_006",
                "Make purchase",
                "make_purchase",
                "AURA wants to make a purchase. Review details and confirm?",
                true,
            ),
            (
                "cond_007",
                "Change system settings",
                "change_settings",
                "AURA wants to change a system setting. Review and approve?",
                false,
            ),
            (
                "cond_008",
                "Access location history",
                "access_location",
                "AURA needs access to your location history. Allow?",
                false,
            ),
        ];

        for (id, desc, pattern, prompt, biometric) in defaults {
            self.conditional_rules.push(ConditionalRule {
                id: id.to_string(),
                description: desc.to_string(),
                action_pattern: pattern.to_string(),
                confirmation_prompt: prompt.to_string(),
                requires_biometric: biometric,
                cooldown_ms: DEFAULT_COOLDOWN_MS,
                last_confirmed_ms: None,
            });
        }
    }
}

impl Default for BoundaryReasoner {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// ActionLearningStats (private helper for learn_from_history)
// ---------------------------------------------------------------------------

/// Aggregated stats for learning from decision log.
#[derive(Default)]
struct ActionLearningStats {
    total: u32,
    allowed: u32,
    denied: u32,
    fast_confirms: u32,
}

// ---------------------------------------------------------------------------
// Pattern matching helper
// ---------------------------------------------------------------------------

/// Check if `action` matches `pattern` at word boundaries (delimited by `_`
/// or `/`). This prevents false positives like "unhide_action" matching
/// pattern "hide_action".
///
/// Rules:
/// - Exact match always succeeds.
/// - Pattern must appear in action where the character before the match (if any) is `_` or `/`, AND
///   the character after the match (if any) is `_` or `/`.
#[must_use]
fn matches_action_pattern(action: &str, pattern: &str) -> bool {
    if action == pattern {
        return true;
    }
    let pat_len = pattern.len();
    if pat_len == 0 || pat_len > action.len() {
        return false;
    }
    let action_bytes = action.as_bytes();
    let _pat_bytes = pattern.as_bytes();

    let mut start = 0;
    while start + pat_len <= action_bytes.len() {
        if let Some(pos) = action[start..].find(pattern) {
            let abs_pos = start + pos;
            let before_ok = abs_pos == 0 || matches!(action_bytes[abs_pos - 1], b'_' | b'/');
            let after_pos = abs_pos + pat_len;
            let after_ok = after_pos == action_bytes.len()
                || matches!(action_bytes[after_pos], b'_' | b'/' | b' ' | b'-' | b'.');
            if before_ok && after_ok {
                return true;
            }
            // Move past this occurrence and keep searching.
            start = abs_pos + 1;
        } else {
            break;
        }
    }
    false
}

// ---------------------------------------------------------------------------
// Timestamp helper
// ---------------------------------------------------------------------------

/// Returns the current timestamp in milliseconds since Unix epoch.
fn current_timestamp_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn default_context() -> BoundaryContext {
        BoundaryContext::default()
    }

    // --- BoundaryLevel tests ---

    #[test]
    fn test_boundary_level_display() {
        assert_eq!(BoundaryLevel::Absolute.to_string(), "L1:Absolute");
        assert_eq!(BoundaryLevel::Conditional.to_string(), "L2:Conditional");
        assert_eq!(BoundaryLevel::Learned.to_string(), "L3:Learned");
    }

    // --- Absolute rules (Level 1) tests ---

    #[test]
    fn test_absolute_deny_delete_all_data() {
        let reasoner = BoundaryReasoner::new();
        let ctx = default_context();
        let decision = reasoner.evaluate("delete_all_data", &ctx);
        assert!(decision.is_absolute_deny());
    }

    #[test]
    fn test_absolute_deny_factory_reset() {
        let reasoner = BoundaryReasoner::new();
        let ctx = default_context();
        let decision = reasoner.evaluate("factory_reset", &ctx);
        assert!(decision.is_absolute_deny());
    }

    #[test]
    fn test_absolute_deny_emotional_manipulation() {
        let reasoner = BoundaryReasoner::new();
        let ctx = default_context();
        let decision = reasoner.evaluate("emotional_manipulation tactic", &ctx);
        assert!(decision.is_absolute_deny());
    }

    #[test]
    fn test_absolute_deny_all_rules() {
        let reasoner = BoundaryReasoner::new();
        let ctx = default_context();

        let patterns = [
            "delete_all_data",
            "disable_security",
            "share_external",
            "execute_untrusted",
            "impersonate_legal",
            "access_camera_mic",
            "send_critical_external",
            "modify_security_settings",
            "factory_reset",
            "install_unknown_source",
            "disable_accessibility",
            "override_truth",
            "hide_action",
            "emotional_manipulation",
            "create_addiction",
        ];

        for pattern in patterns {
            let decision = reasoner.evaluate(pattern, &ctx);
            assert!(
                decision.is_absolute_deny(),
                "expected absolute deny for '{pattern}'"
            );
        }
    }

    #[test]
    fn test_absolute_deny_case_insensitive() {
        let reasoner = BoundaryReasoner::new();
        let ctx = default_context();
        let decision = reasoner.evaluate("FACTORY_RESET", &ctx);
        assert!(decision.is_absolute_deny());
    }

    #[test]
    fn test_is_absolute_deny_static() {
        assert!(BoundaryReasoner::is_absolute_deny("delete_all_data"));
        assert!(BoundaryReasoner::is_absolute_deny("factory_reset"));
        assert!(!BoundaryReasoner::is_absolute_deny("open_app"));
    }

    // --- Conditional rules (Level 2) tests ---

    #[test]
    fn test_conditional_financial_transaction() {
        let reasoner = BoundaryReasoner::new();
        let ctx = default_context();
        let decision = reasoner.evaluate("financial_transaction", &ctx);
        assert!(decision.needs_confirmation());
    }

    #[test]
    fn test_conditional_send_message() {
        let reasoner = BoundaryReasoner::new();
        let ctx = default_context();
        let decision = reasoner.evaluate("send_message to friend", &ctx);
        assert!(decision.needs_confirmation());
    }

    #[test]
    fn test_conditional_install_app() {
        let reasoner = BoundaryReasoner::new();
        let ctx = default_context();
        let decision = reasoner.evaluate("install_app com.example", &ctx);
        assert!(decision.needs_confirmation());
    }

    #[test]
    fn test_conditional_needs_confirmation() {
        let reasoner = BoundaryReasoner::new();
        let ctx = default_context();
        let prompt = reasoner.needs_confirmation("make_purchase item_123", &ctx);
        assert!(prompt.is_some());
    }

    #[test]
    fn test_add_and_remove_conditional_rule() {
        let mut reasoner = BoundaryReasoner::new();
        let ctx = default_context();

        let rule = ConditionalRule {
            id: "custom_001".to_string(),
            description: "Custom rule".to_string(),
            action_pattern: "custom_action".to_string(),
            confirmation_prompt: "Allow custom action?".to_string(),
            requires_biometric: false,
            cooldown_ms: 0,
            last_confirmed_ms: None,
        };

        reasoner.add_conditional_rule(rule);
        let decision = reasoner.evaluate("custom_action test", &ctx);
        assert!(decision.needs_confirmation());

        let removed = reasoner.remove_conditional_rule("custom_001");
        assert!(removed);

        let decision2 = reasoner.evaluate("custom_action test", &ctx);
        assert!(decision2.is_allowed() && !decision2.needs_confirmation());
    }

    #[test]
    fn test_conditional_cooldown() {
        let rule = ConditionalRule {
            id: "test".to_string(),
            description: "test".to_string(),
            action_pattern: "test".to_string(),
            confirmation_prompt: "test?".to_string(),
            requires_biometric: false,
            cooldown_ms: 60_000,
            last_confirmed_ms: Some(current_timestamp_ms()),
        };

        // Just confirmed → cooldown not elapsed.
        assert!(!rule.is_cooldown_elapsed(current_timestamp_ms()));

        // Way in the future → cooldown elapsed.
        assert!(rule.is_cooldown_elapsed(current_timestamp_ms() + 120_000));
    }

    // --- Default allow tests ---

    #[test]
    fn test_default_allow() {
        let reasoner = BoundaryReasoner::new();
        let ctx = default_context();
        let decision = reasoner.evaluate("open_app com.browser", &ctx);
        assert!(decision.is_allowed());
        assert!(!decision.needs_confirmation());
    }

    #[test]
    fn test_default_allow_tap() {
        let reasoner = BoundaryReasoner::new();
        let ctx = default_context();
        let decision = reasoner.evaluate("tap 100 200", &ctx);
        assert!(decision.is_allowed());
    }

    // --- Learned boundaries (Level 3) tests ---

    #[test]
    fn test_learned_always_deny() {
        let mut reasoner = BoundaryReasoner::new();
        let ctx = default_context();

        // Simulate: user denied "spam_action" many times.
        for _ in 0..10 {
            reasoner.decision_log.push(BoundaryDecisionLog {
                timestamp_ms: current_timestamp_ms(),
                action: "spam_action".to_string(),
                decision: "conditional".to_string(),
                level: BoundaryLevel::Conditional,
                user_response: Some(false),
                response_time_ms: Some(500),
            });
        }

        reasoner.learn_from_history();

        let decision = reasoner.evaluate("spam_action something", &ctx);
        assert!(!decision.is_allowed());
        assert!(matches!(
            decision,
            BoundaryDecision::Deny {
                level: BoundaryLevel::Learned,
                ..
            }
        ));
    }

    #[test]
    fn test_learned_always_allow() {
        let mut reasoner = BoundaryReasoner::new();
        let ctx = default_context();

        // Simulate: user fast-confirmed "quick_action" many times.
        for _ in 0..10 {
            reasoner.decision_log.push(BoundaryDecisionLog {
                timestamp_ms: current_timestamp_ms(),
                action: "quick_action".to_string(),
                decision: "conditional".to_string(),
                level: BoundaryLevel::Conditional,
                user_response: Some(true),
                response_time_ms: Some(500), // fast
            });
        }

        reasoner.learn_from_history();

        let decision = reasoner.evaluate("quick_action something", &ctx);
        assert!(decision.is_allowed());
        assert!(!decision.needs_confirmation());
    }

    #[test]
    fn test_learned_never_overrides_absolute() {
        let mut reasoner = BoundaryReasoner::new();
        let ctx = default_context();

        // Even if we somehow learned to allow "factory_reset",
        // Level 1 must ALWAYS win.
        reasoner.learned_boundaries.push(LearnedBoundary {
            action_pattern: "factory_reset".to_string(),
            user_preference: UserPreference::AlwaysAllow,
            confidence: 1.0,
            observation_count: 100,
            last_updated_ms: current_timestamp_ms(),
        });

        let decision = reasoner.evaluate("factory_reset", &ctx);
        assert!(decision.is_absolute_deny());
    }

    #[test]
    fn test_user_would_allow() {
        let mut reasoner = BoundaryReasoner::new();
        let ctx = default_context();

        assert_eq!(reasoner.user_would_allow("anything", &ctx), None);

        reasoner.learned_boundaries.push(LearnedBoundary {
            action_pattern: "frequent_action".to_string(),
            user_preference: UserPreference::AlwaysAllow,
            confidence: 0.95,
            observation_count: 20,
            last_updated_ms: current_timestamp_ms(),
        });

        assert_eq!(
            reasoner.user_would_allow("frequent_action", &ctx),
            Some(true)
        );
    }

    #[test]
    fn test_user_would_allow_deny() {
        let mut reasoner = BoundaryReasoner::new();
        let ctx = default_context();

        reasoner.learned_boundaries.push(LearnedBoundary {
            action_pattern: "annoying_action".to_string(),
            user_preference: UserPreference::AlwaysDeny,
            confidence: 0.98,
            observation_count: 15,
            last_updated_ms: current_timestamp_ms(),
        });

        assert_eq!(
            reasoner.user_would_allow("annoying_action", &ctx),
            Some(false)
        );
    }

    #[test]
    fn test_low_confidence_not_used() {
        let mut reasoner = BoundaryReasoner::new();
        let ctx = default_context();

        // Low confidence → should not be used, fall through to default Allow.
        reasoner.learned_boundaries.push(LearnedBoundary {
            action_pattern: "uncertain_action".to_string(),
            user_preference: UserPreference::AlwaysDeny,
            confidence: 0.5,
            observation_count: 3,
            last_updated_ms: current_timestamp_ms(),
        });

        let decision = reasoner.evaluate("uncertain_action", &ctx);
        assert!(decision.is_allowed());
    }

    // --- Time-restricted learned boundary test ---

    #[test]
    fn test_learned_time_restricted() {
        let mut reasoner = BoundaryReasoner::new();

        reasoner.learned_boundaries.push(LearnedBoundary {
            action_pattern: "morning_only".to_string(),
            user_preference: UserPreference::TimeRestricted {
                allowed_hours: (9, 17),
            },
            confidence: 0.95,
            observation_count: 20,
            last_updated_ms: current_timestamp_ms(),
        });

        // During allowed hours → no restriction from this boundary.
        let mut ctx = default_context();
        ctx.current_hour = 12;
        let decision = reasoner.evaluate("morning_only task", &ctx);
        // TimeRestricted doesn't produce Allow, just doesn't deny. Falls through.
        assert!(decision.is_allowed());

        // Outside allowed hours → denied.
        ctx.current_hour = 3;
        let decision = reasoner.evaluate("morning_only task", &ctx);
        assert!(!decision.is_allowed());
    }

    // --- Record user response test ---

    #[test]
    fn test_record_user_response() {
        let mut reasoner = BoundaryReasoner::new();

        reasoner.decision_log.push(BoundaryDecisionLog {
            timestamp_ms: current_timestamp_ms(),
            action: "test_action".to_string(),
            decision: "conditional".to_string(),
            level: BoundaryLevel::Conditional,
            user_response: None,
            response_time_ms: None,
        });

        reasoner.record_user_response("test_action", true, 800);

        let entry = reasoner.decision_log.iter().last().unwrap();
        assert_eq!(entry.user_response, Some(true));
        assert_eq!(entry.response_time_ms, Some(800));
    }

    // --- Stats & export tests ---

    #[test]
    fn test_stats() {
        let reasoner = BoundaryReasoner::new();
        let stats = reasoner.stats();
        assert_eq!(stats.absolute_rule_count, ABSOLUTE_RULES.len());
        assert_eq!(stats.conditional_rule_count, 8); // 8 defaults
        assert_eq!(stats.learned_boundary_count, 0);
    }

    #[test]
    fn test_export_rules_summary() {
        let reasoner = BoundaryReasoner::new();
        let summary = reasoner.export_rules_summary();
        assert!(summary.contains("ABSOLUTE"));
        assert!(summary.contains("CONDITIONAL"));
        assert!(summary.contains("LEARNED"));
        assert!(summary.contains("abs_001"));
        assert!(summary.contains("cond_001"));
    }

    // --- BoundaryDecision tests ---

    #[test]
    fn test_decision_is_allowed() {
        assert!(BoundaryDecision::Allow.is_allowed());
        assert!(BoundaryDecision::AllowWithConfirmation {
            reason: "test".to_string(),
            confirmation_prompt: "test?".to_string()
        }
        .is_allowed());
        assert!(!BoundaryDecision::Deny {
            reason: "no".to_string(),
            level: BoundaryLevel::Conditional
        }
        .is_allowed());
        assert!(!BoundaryDecision::DenyAbsolute {
            reason: "never".to_string(),
            rule_id: "abs_001"
        }
        .is_allowed());
    }

    #[test]
    fn test_decision_summary() {
        let d = BoundaryDecision::DenyAbsolute {
            reason: "test".to_string(),
            rule_id: "abs_001",
        };
        assert!(d.summary().contains("abs_001"));
    }

    // --- Log decision test ---

    #[test]
    fn test_log_decision_increments_counters() {
        let mut reasoner = BoundaryReasoner::new();

        let decision = BoundaryDecision::DenyAbsolute {
            reason: "test".to_string(),
            rule_id: "abs_001",
        };
        reasoner.log_decision("delete_all_data", &decision, BoundaryLevel::Absolute);
        assert_eq!(reasoner.stats().total_absolute_denials, 1);

        let decision2 = BoundaryDecision::AllowWithConfirmation {
            reason: "test".to_string(),
            confirmation_prompt: "ok?".to_string(),
        };
        reasoner.log_decision("send_message", &decision2, BoundaryLevel::Conditional);
        assert_eq!(reasoner.stats().total_confirmations_requested, 1);
    }

    // --- UserPreference display test ---

    #[test]
    fn test_user_preference_display() {
        assert_eq!(UserPreference::AlwaysAllow.to_string(), "AlwaysAllow");
        assert_eq!(UserPreference::AlwaysDeny.to_string(), "AlwaysDeny");
        assert!(UserPreference::TimeRestricted {
            allowed_hours: (9, 17)
        }
        .to_string()
        .contains("09:00-17:00"));
    }

    // --- BoundedVec tests ---

    #[test]
    fn test_bounded_vec_eviction() {
        let mut bv: BoundedVec<u32> = BoundedVec::new(3);
        bv.push(1);
        bv.push(2);
        bv.push(3);
        bv.push(4);
        assert_eq!(bv.len(), 3);
        assert_eq!(bv.to_vec(), vec![2, 3, 4]);
    }

    #[test]
    fn test_bounded_vec_retain() {
        let mut bv: BoundedVec<u32> = BoundedVec::new(10);
        bv.push(1);
        bv.push(2);
        bv.push(3);
        bv.retain(|x| x % 2 != 0);
        assert_eq!(bv.to_vec(), vec![1, 3]);
    }
}
