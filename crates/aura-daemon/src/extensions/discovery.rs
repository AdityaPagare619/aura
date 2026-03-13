/// Smart Extension Discovery (E5)
/// Rather than a traditional "App Store", AURA observes user needs
/// and suggests Abilities or Skills proactively.
pub struct ExtensionDiscovery;

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

impl ExtensionDiscovery {
    pub fn new() -> Self {
        Self
    }

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
}
