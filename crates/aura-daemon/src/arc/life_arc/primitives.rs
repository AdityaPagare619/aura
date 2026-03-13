//! Shared primitives for the Life Arc subsystem.
//!
//! These types are the backbone of all four life arcs (financial, relationship,
//! health, growth). They are kept minimal and data-only — NO reasoning happens
//! here. The LLM reasons; the arc subsystem tracks facts and scores trajectories.

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// ArcHealth — universal arc health level
// ---------------------------------------------------------------------------

/// Universal health levels for any life arc.
///
/// Each arc maps its specific scored conditions to one of these four levels.
/// The LLM receives both the level AND the raw context to reason about.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ArcHealth {
    /// Strong positive momentum in this arc.
    Thriving,
    /// Sustainable trajectory, no major concerns.
    Stable,
    /// Declining trend, worth gentle attention.
    AtRisk,
    /// Significant concern — proactive nudge may be appropriate.
    NeedsAttention,
}

impl ArcHealth {
    /// Score threshold to qualify as Thriving (≥ this value).
    pub const THRESHOLD_THRIVING: f32 = 0.75;
    /// Score threshold to qualify as Stable (≥ this value, < Thriving).
    pub const THRESHOLD_STABLE: f32 = 0.50;
    /// Score threshold to qualify as AtRisk (≥ this value, < Stable).
    pub const THRESHOLD_AT_RISK: f32 = 0.30;
    // Below THRESHOLD_AT_RISK → NeedsAttention.

    /// Derive health level from a normalised score in `[0.0, 1.0]`.
    ///
    /// Day-zero safe: a score of 0.5 (neutral default) maps to `Stable`.
    #[must_use]
    pub fn from_score(score: f32) -> Self {
        let clamped = score.clamp(0.0, 1.0);
        if clamped >= Self::THRESHOLD_THRIVING {
            ArcHealth::Thriving
        } else if clamped >= Self::THRESHOLD_STABLE {
            ArcHealth::Stable
        } else if clamped >= Self::THRESHOLD_AT_RISK {
            ArcHealth::AtRisk
        } else {
            ArcHealth::NeedsAttention
        }
    }

    /// Human-readable label (for LLM context injection — NOT advice).
    #[must_use]
    pub fn label(&self) -> &'static str {
        match self {
            ArcHealth::Thriving => "thriving",
            ArcHealth::Stable => "stable",
            ArcHealth::AtRisk => "at-risk",
            ArcHealth::NeedsAttention => "needs-attention",
        }
    }

    /// Whether this health level warrants considering a proactive trigger.
    #[must_use]
    pub fn warrants_proactive(&self) -> bool {
        matches!(self, ArcHealth::AtRisk | ArcHealth::NeedsAttention)
    }
}

// ---------------------------------------------------------------------------
// ArcType — identifies which arc a trigger came from
// ---------------------------------------------------------------------------

/// Identifies the life arc that generated a proactive trigger or context.
///
/// `Custom(String)` ensures the arc system handles infinite user dimensions —
/// users may define arcs beyond the four built-in ones.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ArcType {
    Financial,
    Relationship,
    Health,
    Growth,
    /// User-defined arc with an arbitrary label.
    Custom(String),
}

impl ArcType {
    /// Short identifier string, suitable for logging and context headers.
    #[must_use]
    pub fn as_str(&self) -> &str {
        match self {
            ArcType::Financial => "financial",
            ArcType::Relationship => "relationship",
            ArcType::Health => "health",
            ArcType::Growth => "growth",
            ArcType::Custom(s) => s.as_str(),
        }
    }
}

// ---------------------------------------------------------------------------
// ProactiveTrigger — when AURA should surface something to the user
// ---------------------------------------------------------------------------

/// A proactive trigger produced by a life arc when conditions are met.
///
/// The trigger contains only FACTS for the LLM to reason about.
/// The LLM decides WHAT to say; this struct decides WHEN and WHETHER to trigger.
///
/// Deduplication: max 1 trigger per arc per 24 hours, enforced by each arc
/// via `last_trigger_ms` comparison before calling `check_proactive_trigger`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProactiveTrigger {
    /// Which arc produced this trigger.
    pub arc_type: ArcType,
    /// Current health level of the arc.
    pub health: ArcHealth,
    /// Unix millisecond timestamp when this trigger was generated.
    pub triggered_at_ms: u64,
    /// Raw factual context for LLM injection.
    ///
    /// This is NOT pre-packaged advice — it is structured facts that the LLM
    /// uses to reason about what (if anything) to say to the user.
    pub context_for_llm: String,
}

/// One day in milliseconds — used for trigger deduplication.
pub const ONE_DAY_MS: u64 = 86_400_000;

/// Check whether enough time has passed since the last trigger to allow
/// a new one (minimum 24 hours between triggers per arc).
#[must_use]
pub fn trigger_cooldown_elapsed(last_trigger_ms: u64, now_ms: u64) -> bool {
    // Day-zero safe: if last_trigger_ms == 0, we have never triggered — allow.
    if last_trigger_ms == 0 {
        return false; // No trigger yet — but day-zero means insufficient data anyway
    }
    now_ms.saturating_sub(last_trigger_ms) >= ONE_DAY_MS
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_arc_health_from_score_boundaries() {
        assert_eq!(ArcHealth::from_score(1.0), ArcHealth::Thriving);
        assert_eq!(ArcHealth::from_score(0.75), ArcHealth::Thriving);
        assert_eq!(ArcHealth::from_score(0.74), ArcHealth::Stable);
        assert_eq!(ArcHealth::from_score(0.50), ArcHealth::Stable);
        assert_eq!(ArcHealth::from_score(0.49), ArcHealth::AtRisk);
        assert_eq!(ArcHealth::from_score(0.30), ArcHealth::AtRisk);
        assert_eq!(ArcHealth::from_score(0.29), ArcHealth::NeedsAttention);
        assert_eq!(ArcHealth::from_score(0.0), ArcHealth::NeedsAttention);
    }

    #[test]
    fn test_arc_health_day_zero_neutral() {
        // 0.5 is the neutral default — must map to Stable (not AtRisk)
        assert_eq!(ArcHealth::from_score(0.5), ArcHealth::Stable);
    }

    #[test]
    fn test_arc_health_clamps_out_of_range() {
        assert_eq!(ArcHealth::from_score(2.0), ArcHealth::Thriving);
        assert_eq!(ArcHealth::from_score(-1.0), ArcHealth::NeedsAttention);
    }

    #[test]
    fn test_warrants_proactive() {
        assert!(!ArcHealth::Thriving.warrants_proactive());
        assert!(!ArcHealth::Stable.warrants_proactive());
        assert!(ArcHealth::AtRisk.warrants_proactive());
        assert!(ArcHealth::NeedsAttention.warrants_proactive());
    }

    #[test]
    fn test_arc_type_as_str() {
        assert_eq!(ArcType::Financial.as_str(), "financial");
        assert_eq!(ArcType::Custom("career".into()).as_str(), "career");
    }

    #[test]
    fn test_trigger_cooldown() {
        // Never triggered → return false (day-zero: no data, no trigger)
        assert!(!trigger_cooldown_elapsed(0, 1_000_000));
        // Triggered 23 hours ago → not yet elapsed
        assert!(!trigger_cooldown_elapsed(1_000_000, 1_000_000 + 82_800_000));
        // Triggered exactly 24 hours ago → elapsed
        assert!(trigger_cooldown_elapsed(1_000_000, 1_000_000 + ONE_DAY_MS));
        // Triggered 25 hours ago → elapsed
        assert!(trigger_cooldown_elapsed(1_000_000, 1_000_000 + ONE_DAY_MS + 3_600_000));
    }
}
