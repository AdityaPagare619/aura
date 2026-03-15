//! Smart Extension Discovery (E5)
//!
//! Two responsibilities:
//!
//! 1. **Gap/Routine Signals** — Observe user needs and emit structured signals for the LLM to
//!    reason about (suggesting new extensions or automations).
//!
//! 2. **Capability Reporting** — Report what extensions are loaded, what permissions they hold, and
//!    their sandbox status.
//!
//! Rust NEVER generates suggestion text or makes content-based decisions.
//! All signals are structured data for the LLM.

use aura_types::manifest::ExecutionTier;

use super::{loader::ExtensionSummary, sandbox::SandboxState};

/// Smart Extension Discovery (E5)
///
/// Rather than a traditional "App Store", AURA observes user needs
/// and suggests Abilities or Skills proactively. Also provides
/// capability introspection for transparency.
pub struct ExtensionDiscovery;

// ---------------------------------------------------------------------------
// Signals (for LLM consumption)
// ---------------------------------------------------------------------------

/// Structured signal describing a capability gap.
///
/// The LLM receives this and decides what suggestion (if any) to surface.
/// Rust NEVER generates suggestion text — that is the LLM's role.
#[derive(Debug, Clone)]
pub struct GapSignal {
    /// The domain in which a capability is missing (e.g., "finance").
    pub domain: String,
    /// The action that was attempted but could not be fulfilled.
    pub missing_action: String,
}

/// Structured signal describing a recurring user routine.
///
/// The LLM receives this and decides whether to offer Recipe automation.
/// Rust NEVER generates natural-language suggestions.
#[derive(Debug, Clone)]
pub struct RoutineSignal {
    /// The name of the recurring routine as observed.
    pub routine_name: String,
    /// The ordered steps comprising the routine.
    pub steps: Vec<String>,
}

/// Structured report of all loaded extensions and their capabilities.
///
/// Emitted for the LLM to reason about what AURA can currently do,
/// and for the user to inspect via transparency features.
#[derive(Debug, Clone)]
pub struct CapabilityReport {
    /// Total number of registered extensions.
    pub total_extensions: usize,
    /// Number of active (healthy) extensions.
    pub active_extensions: usize,
    /// Number of suspended or disabled extensions.
    pub degraded_extensions: usize,
    /// Per-extension summaries.
    pub extensions: Vec<ExtensionCapability>,
}

/// Capability summary for a single extension.
#[derive(Debug, Clone)]
pub struct ExtensionCapability {
    /// Extension identifier.
    pub id: String,
    /// Human-readable name.
    pub name: String,
    /// Current sandbox state.
    pub state: SandboxState,
    /// Execution tier.
    pub tier: ExecutionTier,
    /// Number of granted permissions.
    pub permission_count: usize,
    /// Total permission checks performed.
    pub total_checks: u64,
    /// Total permission denials.
    pub total_denials: u64,
    /// Health indicator: denial rate (denials / checks).
    pub denial_rate: f64,
}

impl ExtensionDiscovery {
    pub fn new() -> Self {
        Self
    }

    // -----------------------------------------------------------------------
    // Gap & Routine Signals (existing functionality, preserved)
    // -----------------------------------------------------------------------

    /// Called when the ETG or prediction engine identifies a repeated failure
    /// or a missing capability in a specific domain.
    ///
    /// Returns a structured `GapSignal` for the LLM to reason about.
    /// The LLM decides whether and how to surface a suggestion to the user.
    /// Rust performs no keyword matching and generates no suggestion text.
    pub async fn analyze_gap(&self, domain: &str, missing_action: &str) -> Option<GapSignal> {
        // Only emit a signal if both fields are non-empty — structural guard only.
        // No content-based routing or keyword matching.
        if domain.is_empty() || missing_action.is_empty() {
            return None;
        }

        Some(GapSignal {
            domain: domain.to_string(),
            missing_action: missing_action.to_string(),
        })
    }

    /// Evaluates if a newly encountered user routine would benefit from a Recipe.
    ///
    /// Returns a structured `RoutineSignal` for the LLM to reason about.
    /// The LLM decides whether to offer automation and what to say.
    /// Rust performs no threshold-based decisions and generates no prompt text.
    pub fn suggest_recipe_for_routine(
        &self,
        routine_name: &str,
        steps: &[&str],
    ) -> Option<RoutineSignal> {
        // Only emit a signal if there is something to act on.
        // No hardcoded step-count thresholds or response strings.
        if routine_name.is_empty() || steps.is_empty() {
            return None;
        }

        Some(RoutineSignal {
            routine_name: routine_name.to_string(),
            steps: steps.iter().map(|s| s.to_string()).collect(),
        })
    }

    // -----------------------------------------------------------------------
    // Capability Reporting (new functionality)
    // -----------------------------------------------------------------------

    /// Build a capability report from extension summaries.
    ///
    /// Called by the daemon to provide the LLM and user with visibility
    /// into what extensions are loaded and their health status.
    pub fn build_capability_report(&self, summaries: &[ExtensionSummary]) -> CapabilityReport {
        let mut extensions = Vec::with_capacity(summaries.len());
        let mut active = 0;
        let mut degraded = 0;

        for summary in summaries {
            let denial_rate = if summary.total_checks > 0 {
                summary.total_denials as f64 / summary.total_checks as f64
            } else {
                0.0
            };

            match summary.state {
                SandboxState::Active => active += 1,
                SandboxState::Suspended | SandboxState::Disabled => degraded += 1,
                _ => {},
            }

            extensions.push(ExtensionCapability {
                id: summary.id.clone(),
                name: summary.name.clone(),
                state: summary.state,
                tier: summary.tier,
                permission_count: summary.permission_count,
                total_checks: summary.total_checks,
                total_denials: summary.total_denials,
                denial_rate,
            });
        }

        CapabilityReport {
            total_extensions: summaries.len(),
            active_extensions: active,
            degraded_extensions: degraded,
            extensions,
        }
    }

    /// Check if any extensions have high denial rates (possible misconfiguration
    /// or attempted abuse).
    ///
    /// Returns extension IDs with denial rates above the threshold.
    /// The LLM decides what to do with this information.
    pub fn find_high_denial_extensions(
        &self,
        summaries: &[ExtensionSummary],
        threshold: f64,
    ) -> Vec<String> {
        summaries
            .iter()
            .filter(|s| {
                s.total_checks > 5 // Minimum sample size.
                    && (s.total_denials as f64 / s.total_checks as f64) > threshold
            })
            .map(|s| s.id.clone())
            .collect()
    }
}

impl Default for ExtensionDiscovery {
    fn default() -> Self {
        Self::new()
    }
}
