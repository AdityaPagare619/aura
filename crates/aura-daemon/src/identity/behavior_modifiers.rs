//! Personality-derived behavior modifiers.
//!
//! Translates OCEAN scores into concrete behavioral parameters that influence
//! routing, response style, autonomy, and risk tolerance. These modifiers are
//! consumed by the routing classifier, pipeline contextor, and execution engine.
//!
//! # Formulas
//!
//! All outputs are clamped to \[0.0, 1.0\]. The OCEAN inputs are assumed to
//! already be bounded to \[0.1, 0.9\] by `OceanTraits::clamp_all()`.
//!
//! | Output             | Formula                                         |
//! |--------------------|-------------------------------------------------|
//! | proactivity_level  | E×0.4 + O×0.3 + (1−N)×0.3                      |
//! | verbosity_level    | E×0.5 + O×0.3 + A×0.2                           |
//! | autonomy_level     | O×0.3 + C×0.3 + (1−N)×0.2 + E×0.2              |
//! | risk_tolerance     | O×0.4 + (1−N)×0.35 + (1−C)×0.25                |
//! | exploration_drive  | O×0.5 + E×0.3 + (1−C)×0.2                       |

use aura_types::identity::OceanTraits;
use tracing::instrument;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Discrete verbosity level for response generation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerbosityLevel {
    /// Minimal — bullet points, no elaboration.
    Terse,
    /// Standard — short paragraphs, moderate detail.
    Normal,
    /// Detailed — full explanations, examples.
    Verbose,
}

/// Discrete autonomy level for action execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutonomyLevel {
    /// Always ask before acting.
    Supervised,
    /// Ask for non-trivial actions, auto-execute trivial ones.
    Guided,
    /// Only ask for high-risk actions.
    Autonomous,
    /// Only ask for critical/irreversible actions.
    FullAutonomy,
}

/// Weighted priorities for goal selection.
#[derive(Debug, Clone)]
pub struct GoalWeights {
    /// Weight for user-satisfaction goals.
    pub user_satisfaction: f32,
    /// Weight for efficiency goals.
    pub efficiency: f32,
    /// Weight for safety/correctness goals.
    pub safety: f32,
    /// Weight for exploration/learning goals.
    pub exploration: f32,
}

/// Style parameters for response generation.
#[derive(Debug, Clone)]
pub struct ResponseStyleParams {
    /// How proactive AURA should be (0 = reactive, 1 = very proactive).
    pub proactivity: f32,
    /// How verbose responses should be (0 = terse, 1 = verbose).
    pub verbosity: f32,
    /// How much risk AURA is willing to take (0 = risk-averse, 1 = risk-taking).
    pub risk_tolerance: f32,
    /// How exploratory AURA should be (0 = conservative, 1 = explorative).
    pub exploration_drive: f32,
    /// How autonomous AURA should act (0 = always ask, 1 = full autonomy).
    pub autonomy: f32,
    /// Discrete verbosity bucket.
    pub verbosity_level: VerbosityLevel,
    /// Discrete autonomy bucket.
    pub autonomy_level: AutonomyLevel,
}

// ---------------------------------------------------------------------------
// Core functions
// ---------------------------------------------------------------------------

/// Compute the proactivity level from OCEAN traits.
///
/// Formula: `E×0.4 + O×0.3 + (1−N)×0.3`
///
/// High E + High O + Low N → very proactive.
#[instrument(skip_all)]
pub fn proactivity_level(ocean: &OceanTraits) -> f32 {
    (ocean.extraversion * 0.4 + ocean.openness * 0.3 + (1.0 - ocean.neuroticism) * 0.3)
        .clamp(0.0, 1.0)
}

/// Compute the verbosity level as a continuous score.
///
/// Formula: `E×0.5 + O×0.3 + A×0.2`
///
/// Extraverts who are open and agreeable produce more verbose responses.
#[instrument(skip_all)]
pub fn verbosity_score(ocean: &OceanTraits) -> f32 {
    (ocean.extraversion * 0.5 + ocean.openness * 0.3 + ocean.agreeableness * 0.2).clamp(0.0, 1.0)
}

/// Map a continuous verbosity score to a discrete level.
pub fn verbosity_level(score: f32) -> VerbosityLevel {
    if score > 0.65 {
        VerbosityLevel::Verbose
    } else if score < 0.35 {
        VerbosityLevel::Terse
    } else {
        VerbosityLevel::Normal
    }
}

/// Compute the autonomy level as a continuous score.
///
/// Formula: `O×0.3 + C×0.3 + (1−N)×0.2 + E×0.2`
///
/// Open, conscientious, emotionally stable, extraverted → more autonomous.
#[instrument(skip_all)]
pub fn autonomy_score(ocean: &OceanTraits) -> f32 {
    (ocean.openness * 0.3
        + ocean.conscientiousness * 0.3
        + (1.0 - ocean.neuroticism) * 0.2
        + ocean.extraversion * 0.2)
        .clamp(0.0, 1.0)
}

/// Map a continuous autonomy score to a discrete level.
pub fn autonomy_level(score: f32) -> AutonomyLevel {
    if score > 0.75 {
        AutonomyLevel::FullAutonomy
    } else if score > 0.55 {
        AutonomyLevel::Autonomous
    } else if score > 0.35 {
        AutonomyLevel::Guided
    } else {
        AutonomyLevel::Supervised
    }
}

/// Compute risk tolerance from OCEAN traits.
///
/// Formula: `O×0.4 + (1−N)×0.35 + (1−C)×0.25`
///
/// Open, emotionally stable people with lower conscientiousness take more risks.
#[instrument(skip_all)]
pub fn risk_tolerance(ocean: &OceanTraits) -> f32 {
    (ocean.openness * 0.4
        + (1.0 - ocean.neuroticism) * 0.35
        + (1.0 - ocean.conscientiousness) * 0.25)
        .clamp(0.0, 1.0)
}

/// Compute exploration drive from OCEAN traits.
///
/// Formula: `O×0.5 + E×0.3 + (1−C)×0.2`
///
/// High openness is the primary driver of exploration.
#[instrument(skip_all)]
pub fn exploration_drive(ocean: &OceanTraits) -> f32 {
    (ocean.openness * 0.5 + ocean.extraversion * 0.3 + (1.0 - ocean.conscientiousness) * 0.2)
        .clamp(0.0, 1.0)
}

/// Compute goal prioritization weights from OCEAN traits.
///
/// - user_satisfaction: primarily A (agreeable people prioritize user comfort)
/// - efficiency:        primarily C (conscientious people prioritize efficiency)
/// - safety:            primarily N and C (cautious + organized → safety-first)
/// - exploration:       primarily O and E (open + extraverted → explore more)
///
/// Weights are normalized to sum to 1.0.
#[instrument(skip_all)]
pub fn goal_prioritization_weights(ocean: &OceanTraits) -> GoalWeights {
    let raw_satisfaction = ocean.agreeableness * 0.6 + ocean.extraversion * 0.4;
    let raw_efficiency = ocean.conscientiousness * 0.7 + (1.0 - ocean.openness) * 0.3;
    let raw_safety =
        ocean.neuroticism * 0.5 + ocean.conscientiousness * 0.3 + ocean.agreeableness * 0.2;
    let raw_exploration = ocean.openness * 0.6 + ocean.extraversion * 0.4;

    let total = raw_satisfaction + raw_efficiency + raw_safety + raw_exploration;

    if total < f32::EPSILON {
        // Shouldn't happen with valid OCEAN values, but be safe.
        return GoalWeights {
            user_satisfaction: 0.25,
            efficiency: 0.25,
            safety: 0.25,
            exploration: 0.25,
        };
    }

    GoalWeights {
        user_satisfaction: raw_satisfaction / total,
        efficiency: raw_efficiency / total,
        safety: raw_safety / total,
        exploration: raw_exploration / total,
    }
}

/// Compute a complete `ResponseStyleParams` from OCEAN traits.
///
/// This is the primary entry point for the pipeline — call this once
/// per event and pass the result to the response generator.
#[instrument(skip_all)]
pub fn response_style(ocean: &OceanTraits) -> ResponseStyleParams {
    let proactivity = proactivity_level(ocean);
    let verbosity = verbosity_score(ocean);
    let risk = risk_tolerance(ocean);
    let exploration = exploration_drive(ocean);
    let autonomy = autonomy_score(ocean);

    ResponseStyleParams {
        proactivity,
        verbosity,
        risk_tolerance: risk,
        exploration_drive: exploration,
        autonomy,
        verbosity_level: verbosity_level(verbosity),
        autonomy_level: autonomy_level(autonomy),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_proactivity_default_traits() {
        let ocean = OceanTraits::DEFAULT;
        // E=0.50, O=0.85, N=0.25
        // proactivity = 0.50*0.4 + 0.85*0.3 + 0.75*0.3 = 0.20 + 0.255 + 0.225 = 0.68
        let p = proactivity_level(&ocean);
        assert!((p - 0.68).abs() < 0.01, "proactivity={}", p);
    }

    #[test]
    fn test_verbosity_high_extraversion() {
        let ocean = OceanTraits {
            openness: 0.9,
            conscientiousness: 0.5,
            extraversion: 0.9,
            agreeableness: 0.8,
            neuroticism: 0.2,
        };
        let v = verbosity_score(&ocean);
        assert!(v > 0.65, "high E+O+A should be verbose, got {}", v);
        assert_eq!(verbosity_level(v), VerbosityLevel::Verbose);
    }

    #[test]
    fn test_verbosity_low_extraversion() {
        let ocean = OceanTraits {
            openness: 0.2,
            conscientiousness: 0.8,
            extraversion: 0.15,
            agreeableness: 0.2,
            neuroticism: 0.8,
        };
        let v = verbosity_score(&ocean);
        assert!(v < 0.35, "low E+O+A should be terse, got {}", v);
        assert_eq!(verbosity_level(v), VerbosityLevel::Terse);
    }

    #[test]
    fn test_autonomy_high_all() {
        let ocean = OceanTraits {
            openness: 0.9,
            conscientiousness: 0.9,
            extraversion: 0.9,
            agreeableness: 0.5,
            neuroticism: 0.1,
        };
        let a = autonomy_score(&ocean);
        // O=0.9*0.3 + C=0.9*0.3 + (1-0.1)*0.2 + E=0.9*0.2
        // = 0.27 + 0.27 + 0.18 + 0.18 = 0.90
        assert!(
            a > 0.75,
            "high O+C+E, low N should be full autonomy, got {}",
            a
        );
        assert_eq!(autonomy_level(a), AutonomyLevel::FullAutonomy);
    }

    #[test]
    fn test_risk_tolerance_conservative() {
        let ocean = OceanTraits {
            openness: 0.2,
            conscientiousness: 0.9,
            extraversion: 0.5,
            agreeableness: 0.5,
            neuroticism: 0.8,
        };
        let r = risk_tolerance(&ocean);
        // O=0.2*0.4 + (1-0.8)*0.35 + (1-0.9)*0.25
        // = 0.08 + 0.07 + 0.025 = 0.175
        assert!(r < 0.30, "low O, high N+C should be risk-averse, got {}", r);
    }

    #[test]
    fn test_exploration_drive_high_openness() {
        let ocean = OceanTraits {
            openness: 0.9,
            conscientiousness: 0.3,
            extraversion: 0.7,
            agreeableness: 0.5,
            neuroticism: 0.3,
        };
        let e = exploration_drive(&ocean);
        // O=0.9*0.5 + E=0.7*0.3 + (1-0.3)*0.2
        // = 0.45 + 0.21 + 0.14 = 0.80
        assert!(
            e > 0.65,
            "high O, moderate E, low C should explore, got {}",
            e
        );
    }

    #[test]
    fn test_goal_weights_sum_to_one() {
        let ocean = OceanTraits::DEFAULT;
        let gw = goal_prioritization_weights(&ocean);
        let sum = gw.user_satisfaction + gw.efficiency + gw.safety + gw.exploration;
        assert!(
            (sum - 1.0).abs() < 0.001,
            "goal weights should sum to 1.0, got {}",
            sum
        );
    }

    #[test]
    fn test_response_style_complete() {
        let ocean = OceanTraits::DEFAULT;
        let style = response_style(&ocean);
        assert!(style.proactivity >= 0.0 && style.proactivity <= 1.0);
        assert!(style.verbosity >= 0.0 && style.verbosity <= 1.0);
        assert!(style.risk_tolerance >= 0.0 && style.risk_tolerance <= 1.0);
        assert!(style.exploration_drive >= 0.0 && style.exploration_drive <= 1.0);
        assert!(style.autonomy >= 0.0 && style.autonomy <= 1.0);
    }

    #[test]
    fn test_all_values_clamped() {
        // Extreme values (already bounded by OceanTraits but test the functions)
        let ocean = OceanTraits {
            openness: 0.9,
            conscientiousness: 0.9,
            extraversion: 0.9,
            agreeableness: 0.9,
            neuroticism: 0.9,
        };
        assert!(proactivity_level(&ocean) <= 1.0);
        assert!(verbosity_score(&ocean) <= 1.0);
        assert!(autonomy_score(&ocean) <= 1.0);
        assert!(risk_tolerance(&ocean) <= 1.0);
        assert!(exploration_drive(&ocean) <= 1.0);

        let ocean_low = OceanTraits {
            openness: 0.1,
            conscientiousness: 0.1,
            extraversion: 0.1,
            agreeableness: 0.1,
            neuroticism: 0.1,
        };
        assert!(proactivity_level(&ocean_low) >= 0.0);
        assert!(verbosity_score(&ocean_low) >= 0.0);
        assert!(autonomy_score(&ocean_low) >= 0.0);
        assert!(risk_tolerance(&ocean_low) >= 0.0);
        assert!(exploration_drive(&ocean_low) >= 0.0);
    }
}
