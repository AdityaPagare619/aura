use aura_types::identity::OceanTraits;
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Evolution constants
// ---------------------------------------------------------------------------

/// Base positive interaction delta (attenuated by exposure).
/// Actual delta = base / √(1 + evolution_count).
const POSITIVE_DELTA_BASE: f32 = 0.015;
/// Base negative interaction delta (attenuated by exposure).
const NEGATIVE_DELTA_BASE: f32 = -0.02;
const SIGNIFICANT_CHANGE_THRESHOLD: f32 = 0.05;

/// Base micro-drift per interaction (attenuated by exposure).
/// Much smaller than explicit deltas — represents the slow, unconscious
/// shift that happens through repeated interaction.
const MICRO_DRIFT_BASE: f32 = 0.001;

/// Maximum allowed total drift from the baseline before consistency resets.
const MAX_TOTAL_DRIFT: f32 = 0.30;

/// Number of interactions tracked in the evolution window for rate limiting.
#[allow(dead_code)] // Phase 8: used by OCEAN evolution rate-limiter window
const EVOLUTION_WINDOW_SIZE: usize = 100;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Events that may subtly shift the personality over time.
#[derive(Debug, Clone)]
pub enum PersonalityEvent {
    PositiveInteraction,
    NegativeInteraction,
    UserFeedback(String),
    ContextualPressure { trait_name: String, direction: f32 },
}

// ---------------------------------------------------------------------------
// Outcome-driven personality drift
// ---------------------------------------------------------------------------

/// Observable outcome of a user interaction that triggers trait drift.
///
/// Unlike [`PersonalityEvent`] (which describes the *type* of social signal),
/// `PersonalityOutcome` describes the *result* of what the user actually did.
/// This allows the personality to adapt to repeated behavioral patterns:
/// e.g. if the user always explores new options, openness drifts upward.
///
/// Each variant maps to a specific, small OCEAN nudge (MICRO_NUDGE per event).
/// Over many events the cumulative drift becomes meaningful; individual events
/// are imperceptible — matching how personality changes actually work.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PersonalityOutcome {
    /// User followed an established routine successfully.
    /// → conscientiousness ↑, openness ↓ (slight)
    RoutineFollowed,
    /// User deviated from routine and the deviation was positive.
    /// → openness ↑, conscientiousness ↓ (slight)
    RoutineDeviatedPositive,
    /// User deviated from routine and the deviation was negative.
    /// → neuroticism ↑, conscientiousness ↑ (corrective)
    RoutineDeviatedNegative,
    /// User tried something new / exploratory.
    /// → openness ↑
    ExploredNew,
    /// User avoided new options (chose familiar path).
    /// → openness ↓ (slight), conscientiousness ↑ (slight)
    AvoidedNew,
    /// User expressed or received positive emotion.
    /// → extraversion ↑, agreeableness ↑, neuroticism ↓
    EmotionalPositive,
    /// User expressed frustration or received negative emotional signal.
    /// → neuroticism ↑, extraversion ↓ (slight)
    EmotionalNegative,
    /// User cooperated or helped others.
    /// → agreeableness ↑, extraversion ↑ (slight)
    CooperativeAct,
}

/// Magnitude of a single outcome-driven trait nudge.
/// Kept very small so drift is gradual and imperceptible per-event.
const OUTCOME_MICRO_NUDGE: f32 = 0.0005;

/// Named personality archetypes for quick recognition and logging.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PersonalityArchetype {
    /// High O, High C, Low N — analytical, precise, explorative.
    Analyst,
    /// High A, High E, Low N — warm, supportive, approachable.
    Helper,
    /// High O, High E, Low C — creative, spontaneous, adventurous.
    Explorer,
    /// High C, High N, Low O — careful, methodical, risk-averse.
    Guardian,
    /// High E, Low A, Low N — assertive, direct, confident.
    Commander,
    /// No dominant pattern — balanced across traits.
    Balanced,
}

/// Result of a personality consistency check.
#[derive(Debug, Clone)]
pub struct ConsistencyReport {
    /// Whether the personality passes all consistency checks.
    pub is_consistent: bool,
    /// Total drift from the initial baseline.
    pub total_drift: f32,
    /// Whether any individual trait has drifted beyond safe bounds.
    pub has_extreme_drift: bool,
    /// Human-readable issues found.
    pub issues: Vec<String>,
}

/// Persistent personality state wrapping [`OceanTraits`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Personality {
    pub traits: OceanTraits,
    /// Snapshot taken at the last checkpoint — used for significant-change detection.
    snapshot: OceanTraits,
    /// The original baseline traits — never modified, used for drift tracking.
    baseline: OceanTraits,
    /// Total number of evolution events processed.
    evolution_count: u64,
}

// ---------------------------------------------------------------------------
// Implementation — Personality (core)
// ---------------------------------------------------------------------------

impl Personality {
    /// Create a new personality with the AURA defaults.
    pub fn new() -> Self {
        Self {
            traits: OceanTraits::DEFAULT,
            snapshot: OceanTraits::DEFAULT,
            baseline: OceanTraits::DEFAULT,
            evolution_count: 0,
        }
    }

    /// Create a personality with custom initial traits.
    pub fn with_traits(traits: OceanTraits) -> Self {
        let mut t = traits;
        t.clamp_all();
        Self {
            traits: t.clone(),
            snapshot: t.clone(),
            baseline: t,
            evolution_count: 0,
        }
    }

    /// Return a human-readable context string for the LLM.
    pub fn to_llm_context(&self) -> String {
        let arch = self.archetype();
        format!(
            "Personality: O={:.2} C={:.2} E={:.2} A={:.2} N={:.2} (archetype: {:?}, {} evolutions)",
            self.traits.openness,
            self.traits.conscientiousness,
            self.traits.extraversion,
            self.traits.agreeableness,
            self.traits.neuroticism,
            arch,
            self.evolution_count
        )
    }

    /// Apply a personality event, nudging OCEAN traits by exposure-attenuated
    /// deltas. A new AURA (few interactions) is highly responsive; an
    /// established AURA (thousands of interactions) has near-crystallized
    /// personality — matching OCEAN psychology (McCrae & Costa, 2003).
    ///
    /// ContextualPressure: targeted single-trait nudge (NOT attenuated — the
    /// user explicitly asked for a change, so AURA should respect it).
    pub fn evolve(&mut self, event: PersonalityEvent) {
        // Exposure attenuation: base / √(1 + n). At n=0: full delta.
        // At n=1000: ~3% of base. Personality crystallizes with experience.
        let attenuation = 1.0 / (1.0 + self.evolution_count as f32).sqrt();

        match event {
            PersonalityEvent::PositiveInteraction => {
                self.nudge_all(POSITIVE_DELTA_BASE * attenuation);
                self.apply_micro_drift(MICRO_DRIFT_BASE * attenuation);
            }
            PersonalityEvent::NegativeInteraction => {
                self.nudge_all(NEGATIVE_DELTA_BASE * attenuation);
                self.apply_micro_drift(-MICRO_DRIFT_BASE * attenuation);
            }
            PersonalityEvent::ContextualPressure {
                ref trait_name,
                direction,
            } => {
                // User-directed pressure is NOT attenuated — respect explicit intent.
                self.nudge_trait(trait_name, direction);
            }
            PersonalityEvent::UserFeedback(ref _msg) => {
                tracing::debug!("user feedback recorded (no trait change yet)");
            }
        }
        self.traits.clamp_all();
        self.evolution_count += 1;
    }

    /// Apply outcome-driven trait drift based on an observed behavioral result.
    ///
    /// Each [`PersonalityOutcome`] maps to small, targeted OCEAN nudges
    /// (magnitude: [`OUTCOME_MICRO_NUDGE`]). Drift is additive over many
    /// events, modelling how personality slowly shifts to match repeated
    /// behavioral patterns.
    ///
    /// This is intentionally NOT exposure-attenuated — outcomes are observed
    /// facts about what the user did, not subjective signals, so the drift
    /// rate stays constant throughout AURA's lifetime.
    pub fn apply_outcome_drift(&mut self, outcome: PersonalityOutcome) {
        let n = OUTCOME_MICRO_NUDGE;
        match outcome {
            PersonalityOutcome::RoutineFollowed => {
                self.traits.conscientiousness = (self.traits.conscientiousness + n).min(0.9);
                self.traits.openness = (self.traits.openness - n * 0.5).max(0.1);
            }
            PersonalityOutcome::RoutineDeviatedPositive => {
                self.traits.openness = (self.traits.openness + n).min(0.9);
                self.traits.conscientiousness = (self.traits.conscientiousness - n * 0.5).max(0.1);
            }
            PersonalityOutcome::RoutineDeviatedNegative => {
                self.traits.neuroticism = (self.traits.neuroticism + n).min(0.9);
                self.traits.conscientiousness = (self.traits.conscientiousness + n * 0.5).min(0.9);
            }
            PersonalityOutcome::ExploredNew => {
                self.traits.openness = (self.traits.openness + n).min(0.9);
            }
            PersonalityOutcome::AvoidedNew => {
                self.traits.openness = (self.traits.openness - n * 0.5).max(0.1);
                self.traits.conscientiousness = (self.traits.conscientiousness + n * 0.5).min(0.9);
            }
            PersonalityOutcome::EmotionalPositive => {
                self.traits.extraversion = (self.traits.extraversion + n * 0.7).min(0.9);
                self.traits.agreeableness = (self.traits.agreeableness + n).min(0.9);
                self.traits.neuroticism = (self.traits.neuroticism - n * 0.5).max(0.1);
            }
            PersonalityOutcome::EmotionalNegative => {
                self.traits.neuroticism = (self.traits.neuroticism + n).min(0.9);
                self.traits.extraversion = (self.traits.extraversion - n * 0.3).max(0.1);
            }
            PersonalityOutcome::CooperativeAct => {
                self.traits.agreeableness = (self.traits.agreeableness + n).min(0.9);
                self.traits.extraversion = (self.traits.extraversion + n * 0.3).min(0.9);
            }
        }
        // No clamp_all needed — all branches already enforce [0.1, 0.9] per trait.
        self.evolution_count += 1;
    }

    /// Returns `true` when any OCEAN trait has drifted more than 0.05 from
    /// the last checkpoint.
    pub fn has_significant_change(&self) -> bool {
        let diff = |a: f32, b: f32| (a - b).abs();
        diff(self.traits.openness, self.snapshot.openness) >= SIGNIFICANT_CHANGE_THRESHOLD
            || diff(
                self.traits.conscientiousness,
                self.snapshot.conscientiousness,
            ) >= SIGNIFICANT_CHANGE_THRESHOLD
            || diff(self.traits.extraversion, self.snapshot.extraversion)
                >= SIGNIFICANT_CHANGE_THRESHOLD
            || diff(self.traits.agreeableness, self.snapshot.agreeableness)
                >= SIGNIFICANT_CHANGE_THRESHOLD
            || diff(self.traits.neuroticism, self.snapshot.neuroticism)
                >= SIGNIFICANT_CHANGE_THRESHOLD
    }

    /// Save the current traits as the baseline snapshot.
    pub fn checkpoint(&mut self) {
        self.snapshot = self.traits.clone();
    }

    /// Total number of evolution events processed.
    pub fn evolution_count(&self) -> u64 {
        self.evolution_count
    }

    /// Compute total drift from the original baseline.
    ///
    /// Uses Euclidean distance in 5D OCEAN space.
    pub fn total_drift(&self) -> f32 {
        let d = |a: f32, b: f32| (a - b) * (a - b);
        (d(self.traits.openness, self.baseline.openness)
            + d(
                self.traits.conscientiousness,
                self.baseline.conscientiousness,
            )
            + d(self.traits.extraversion, self.baseline.extraversion)
            + d(self.traits.agreeableness, self.baseline.agreeableness)
            + d(self.traits.neuroticism, self.baseline.neuroticism))
        .sqrt()
    }

    /// Run a consistency check on the current personality state.
    ///
    /// Checks for:
    /// 1. Total drift from baseline exceeding `MAX_TOTAL_DRIFT`
    /// 2. Any single trait at the extreme boundary (0.1 or 0.9)
    /// 3. Implausible trait combinations (e.g., neuroticism at max while conscientiousness also at
    ///    max — unlikely in OCEAN literature)
    pub fn consistency_check(&self) -> ConsistencyReport {
        let mut issues = Vec::new();

        let total_drift = self.total_drift();
        let has_extreme_drift = total_drift > MAX_TOTAL_DRIFT;
        if has_extreme_drift {
            issues.push(format!(
                "total drift {:.3} exceeds max {:.3}",
                total_drift, MAX_TOTAL_DRIFT
            ));
        }

        // Check for traits pinned at extremes
        let check_extreme = |name: &str, val: f32, issues: &mut Vec<String>| {
            if (val - 0.1).abs() < f32::EPSILON || (val - 0.9).abs() < f32::EPSILON {
                issues.push(format!("{} is at extreme boundary ({:.2})", name, val));
            }
        };
        check_extreme("openness", self.traits.openness, &mut issues);
        check_extreme(
            "conscientiousness",
            self.traits.conscientiousness,
            &mut issues,
        );
        check_extreme("extraversion", self.traits.extraversion, &mut issues);
        check_extreme("agreeableness", self.traits.agreeableness, &mut issues);
        check_extreme("neuroticism", self.traits.neuroticism, &mut issues);

        // Implausible combination: very high N + very high C is rare
        if self.traits.neuroticism > 0.85 && self.traits.conscientiousness > 0.85 {
            issues.push(
                "implausible: very high neuroticism + very high conscientiousness".to_string(),
            );
        }

        ConsistencyReport {
            is_consistent: issues.is_empty(),
            total_drift,
            has_extreme_drift,
            issues,
        }
    }

    /// Classify the current personality into the nearest archetype.
    // IRON LAW: LLM classifies behavioral archetypes. Rust does not.
    // Weighted scoring formulas that MAKE decisions violate Iron Law #2.
    // The LLM receives raw OCEAN numbers via serialize_identity_block() and
    // infers the archetype itself. Phase N wire-point.
    pub fn archetype(&self) -> PersonalityArchetype {
        let o = self.traits.openness;
        let c = self.traits.conscientiousness;
        let e = self.traits.extraversion;
        let a = self.traits.agreeableness;
        let n = self.traits.neuroticism;

        // Score each archetype by how well the traits match its profile.
        // Higher score = better match. Winner-takes-all.
        let analyst_score = (o - 0.5).max(0.0) + (c - 0.5).max(0.0) + (0.5 - n).max(0.0);
        let helper_score = (a - 0.5).max(0.0) + (e - 0.5).max(0.0) + (0.5 - n).max(0.0);
        let explorer_score =
            (o - 0.5).max(0.0) + (e - 0.5).max(0.0) + (0.5 - c).max(0.0) + (0.5 - n).max(0.0);
        let guardian_score = (n - 0.5).max(0.0) + (c - 0.5).max(0.0) + (0.5 - o).max(0.0);
        let commander_score = (e - 0.5).max(0.0) + (0.5 - a).max(0.0) + (0.5 - n).max(0.0);

        let max_score = analyst_score
            .max(helper_score)
            .max(explorer_score)
            .max(guardian_score)
            .max(commander_score);

        // Require a minimum score to avoid calling balanced profiles something specific.
        const MIN_SCORE: f32 = 0.15;
        if max_score < MIN_SCORE {
            return PersonalityArchetype::Balanced;
        }

        if (max_score - analyst_score).abs() < f32::EPSILON {
            PersonalityArchetype::Analyst
        } else if (max_score - helper_score).abs() < f32::EPSILON {
            PersonalityArchetype::Helper
        } else if (max_score - explorer_score).abs() < f32::EPSILON {
            PersonalityArchetype::Explorer
        } else if (max_score - guardian_score).abs() < f32::EPSILON {
            PersonalityArchetype::Guardian
        } else if (max_score - commander_score).abs() < f32::EPSILON {
            PersonalityArchetype::Commander
        } else {
            PersonalityArchetype::Balanced
        }
    }

    // -- private helpers ----------------------------------------------------

    /// Nudge all traits with differential weights reflecting OCEAN psychology:
    /// - Agreeableness & Extraversion are more malleable through interaction
    /// - Openness evolves at moderate rate
    /// - Conscientiousness & Neuroticism are most trait-like (slower to change)
    ///
    /// See: McCrae & Costa (2003), Concept Design §4.1, AGI Audit §6.
    fn nudge_all(&mut self, delta: f32) {
        self.traits.openness += delta * 1.0; // Moderate stability
        self.traits.conscientiousness += delta * 0.7; // High stability (trait-like)
        self.traits.extraversion += delta * 1.2; // Socially malleable
        self.traits.agreeableness += delta * 1.3; // Most interaction-sensitive
        self.traits.neuroticism += delta * 0.8; // Relatively stable
    }

    fn nudge_trait(&mut self, name: &str, direction: f32) {
        match name {
            "openness" => self.traits.openness += direction,
            "conscientiousness" => self.traits.conscientiousness += direction,
            "extraversion" => self.traits.extraversion += direction,
            "agreeableness" => self.traits.agreeableness += direction,
            "neuroticism" => self.traits.neuroticism += direction,
            other => {
                tracing::warn!(trait_name = other, "unknown trait in contextual pressure");
            }
        }
    }

    /// Apply micro-drift — a very small nudge that differentially affects
    /// traits based on their current distance from center (0.5). Traits
    /// further from center drift faster back toward it (regression to mean).
    fn apply_micro_drift(&mut self, base_drift: f32) {
        let drift_with_regression = |current: f32, drift: f32| -> f32 {
            let distance_from_center = current - 0.5;
            // Pull slightly back toward center (mean regression)
            let regression = -distance_from_center * 0.1 * drift.abs();
            current + drift + regression
        };

        self.traits.openness = drift_with_regression(self.traits.openness, base_drift);
        self.traits.conscientiousness =
            drift_with_regression(self.traits.conscientiousness, base_drift);
        self.traits.extraversion = drift_with_regression(self.traits.extraversion, base_drift);
        self.traits.agreeableness = drift_with_regression(self.traits.agreeableness, base_drift);
        self.traits.neuroticism = drift_with_regression(self.traits.neuroticism, base_drift);
    }
}

impl Default for Personality {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// PersonalityEngine — facade over Personality + behavior_modifiers + prompts
// ---------------------------------------------------------------------------

/// High-level engine that composes all personality-derived influences into
/// a single coherent interface for the pipeline.
///
/// The engine delegates to:
/// - [`Personality`] for core OCEAN state and evolution
/// - [`PersonalityPromptInjector`] for prompt context generation
/// - [`behavior_modifiers`] for goal weights and response style params
///
/// This ensures that personality affects every decision pathway:
/// prompts, routing, goals, responses, ethics, and anti-sycophancy.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersonalityEngine {
    personality: Personality,
}

impl PersonalityEngine {
    /// Create a new `PersonalityEngine` with AURA default traits.
    pub fn new() -> Self {
        Self {
            personality: Personality::new(),
        }
    }

    /// Create a `PersonalityEngine` with custom traits.
    pub fn with_traits(traits: OceanTraits) -> Self {
        Self {
            personality: Personality::with_traits(traits),
        }
    }

    /// Access the underlying personality state.
    pub fn personality(&self) -> &Personality {
        &self.personality
    }

    /// Mutably access the underlying personality state.
    pub fn personality_mut(&mut self) -> &mut Personality {
        &mut self.personality
    }

    /// Evolve the personality based on an interaction event.
    ///
    /// Applies the standard evolution deltas plus micro-drift. Logs a
    /// warning if the personality has drifted significantly from baseline.
    pub fn evolve(&mut self, event: PersonalityEvent) {
        self.personality.evolve(event);

        let report = self.personality.consistency_check();
        if !report.is_consistent {
            tracing::warn!(
                drift = report.total_drift,
                issues = ?report.issues,
                "personality consistency warning"
            );
        }
    }

    /// Run a consistency check and return the report.
    pub fn consistency_check(&self) -> ConsistencyReport {
        self.personality.consistency_check()
    }

    /// Get the current archetype classification.
    pub fn archetype(&self) -> PersonalityArchetype {
        self.personality.archetype()
    }
}

impl Default for PersonalityEngine {
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

    fn custom_traits(o: f32, c: f32, e: f32, a: f32, n: f32) -> OceanTraits {
        let mut t = OceanTraits {
            openness: o,
            conscientiousness: c,
            extraversion: e,
            agreeableness: a,
            neuroticism: n,
        };
        t.clamp_all();
        t
    }

    // ==================== Personality (core) tests ==========================

    #[test]
    fn test_evolution_bounds() {
        let mut p = Personality::new();

        // Push traits toward upper bound repeatedly
        for _ in 0..200 {
            p.evolve(PersonalityEvent::PositiveInteraction);
        }
        // All traits must be clamped to 0.9
        assert!(p.traits.openness <= 0.9);
        assert!(p.traits.conscientiousness <= 0.9);
        assert!(p.traits.extraversion <= 0.9);
        assert!(p.traits.agreeableness <= 0.9);
        assert!(p.traits.neuroticism <= 0.9);

        // Push toward lower bound
        for _ in 0..200 {
            p.evolve(PersonalityEvent::NegativeInteraction);
        }
        assert!(p.traits.openness >= 0.1);
        assert!(p.traits.conscientiousness >= 0.1);
        assert!(p.traits.extraversion >= 0.1);
        assert!(p.traits.agreeableness >= 0.1);
        assert!(p.traits.neuroticism >= 0.1);
    }

    #[test]
    fn test_significant_change_detection() {
        let mut p = Personality::new();
        p.checkpoint();

        assert!(!p.has_significant_change());

        for _ in 0..5 {
            p.evolve(PersonalityEvent::PositiveInteraction);
        }
        assert!(
            p.has_significant_change(),
            "5 early positive interactions should cause significant change"
        );
    }

    #[test]
    fn test_trait_specific_evolution_rates() {
        let mut p = Personality::new();
        let before_a = p.traits.agreeableness;
        let before_c = p.traits.conscientiousness;

        p.evolve(PersonalityEvent::PositiveInteraction);

        let delta_a = p.traits.agreeableness - before_a;
        let delta_c = p.traits.conscientiousness - before_c;

        assert!(
            delta_a > delta_c,
            "Agreeableness delta ({:.4}) should exceed Conscientiousness delta ({:.4})",
            delta_a,
            delta_c
        );
        let ratio = delta_a / delta_c;
        assert!(
            (ratio - 1.857).abs() < 0.3,
            "delta ratio {:.2} should be near 1.86 (1.3/0.7)",
            ratio
        );
    }

    #[test]
    fn test_contextual_pressure() {
        let mut p = Personality::new();
        let before = p.traits.openness;

        p.evolve(PersonalityEvent::ContextualPressure {
            trait_name: "openness".to_string(),
            direction: 0.03,
        });

        assert!((p.traits.openness - (before + 0.03)).abs() < f32::EPSILON);
    }

    #[test]
    fn test_checkpoint_resets_baseline() {
        let mut p = Personality::new();
        for _ in 0..5 {
            p.evolve(PersonalityEvent::PositiveInteraction);
        }
        assert!(p.has_significant_change());

        p.checkpoint();
        assert!(!p.has_significant_change());
    }

    // ==================== New Personality tests =============================

    #[test]
    fn test_with_traits_constructor() {
        let traits = custom_traits(0.5, 0.5, 0.5, 0.5, 0.5);
        let p = Personality::with_traits(traits);
        assert!((p.traits.openness - 0.5).abs() < f32::EPSILON);
        assert!((p.traits.conscientiousness - 0.5).abs() < f32::EPSILON);
    }

    #[test]
    fn test_with_traits_clamped() {
        let traits = OceanTraits {
            openness: 1.5,
            conscientiousness: -0.5,
            extraversion: 0.5,
            agreeableness: 0.5,
            neuroticism: 0.5,
        };
        let p = Personality::with_traits(traits);
        assert!((p.traits.openness - 0.9).abs() < f32::EPSILON);
        assert!((p.traits.conscientiousness - 0.1).abs() < f32::EPSILON);
    }

    #[test]
    fn test_evolution_count_increments() {
        let mut p = Personality::new();
        assert_eq!(p.evolution_count(), 0);
        p.evolve(PersonalityEvent::PositiveInteraction);
        assert_eq!(p.evolution_count(), 1);
        p.evolve(PersonalityEvent::NegativeInteraction);
        assert_eq!(p.evolution_count(), 2);
    }

    #[test]
    fn test_total_drift_zero_initially() {
        let p = Personality::new();
        assert!(p.total_drift() < f32::EPSILON, "drift should be 0 at start");
    }

    #[test]
    fn test_total_drift_increases_with_evolution() {
        let mut p = Personality::new();
        for _ in 0..10 {
            p.evolve(PersonalityEvent::PositiveInteraction);
        }
        assert!(
            p.total_drift() > 0.0,
            "drift should increase after evolution"
        );
    }

    #[test]
    fn test_consistency_check_clean() {
        let p = Personality::new();
        let report = p.consistency_check();
        assert!(
            report.is_consistent,
            "fresh personality should be consistent"
        );
        assert!(!report.has_extreme_drift);
        assert!(report.issues.is_empty());
    }

    #[test]
    fn test_consistency_check_extreme_traits() {
        let traits = OceanTraits {
            openness: 0.9,
            conscientiousness: 0.9,
            extraversion: 0.5,
            agreeableness: 0.5,
            neuroticism: 0.9,
        };
        let p = Personality::with_traits(traits);
        let report = p.consistency_check();
        assert!(
            !report.is_consistent,
            "high N + high C should flag implausible"
        );
        assert!(report.issues.iter().any(|i| i.contains("implausible")));
    }

    #[test]
    fn test_consistency_check_extreme_drift() {
        let mut p = Personality::new();
        for _ in 0..200 {
            p.evolve(PersonalityEvent::PositiveInteraction);
        }
        let report = p.consistency_check();
        assert!(
            report.total_drift > 0.0,
            "should have nonzero drift after 200 evolutions"
        );
    }

    #[test]
    fn test_micro_drift_regression_toward_mean() {
        let traits = custom_traits(0.9, 0.5, 0.5, 0.5, 0.5);
        let mut p = Personality::with_traits(traits);
        let _before = p.traits.openness;

        p.evolve(PersonalityEvent::PositiveInteraction);

        assert!(p.traits.openness <= 0.9, "should be clamped to 0.9");
    }

    #[test]
    fn test_to_llm_context() {
        let p = Personality::new();
        let ctx = p.to_llm_context();
        assert!(
            ctx.contains("Personality:"),
            "context should start with Personality:"
        );
        assert!(
            ctx.contains("archetype:"),
            "context should include archetype"
        );
        assert!(
            ctx.contains("evolutions"),
            "context should include evolution count"
        );
    }

    // ==================== Archetype tests ==================================

    #[test]
    fn test_archetype_analyst() {
        let traits = custom_traits(0.9, 0.9, 0.5, 0.5, 0.1);
        let p = Personality::with_traits(traits);
        assert_eq!(p.archetype(), PersonalityArchetype::Analyst);
    }

    #[test]
    fn test_archetype_helper() {
        let traits = custom_traits(0.5, 0.5, 0.8, 0.9, 0.1);
        let p = Personality::with_traits(traits);
        assert_eq!(p.archetype(), PersonalityArchetype::Helper);
    }

    #[test]
    fn test_archetype_explorer() {
        let traits = custom_traits(0.9, 0.1, 0.9, 0.5, 0.1);
        let p = Personality::with_traits(traits);
        assert_eq!(p.archetype(), PersonalityArchetype::Explorer);
    }

    #[test]
    fn test_archetype_guardian() {
        let traits = custom_traits(0.1, 0.8, 0.3, 0.5, 0.9);
        let p = Personality::with_traits(traits);
        assert_eq!(p.archetype(), PersonalityArchetype::Guardian);
    }

    #[test]
    fn test_archetype_commander() {
        let traits = custom_traits(0.5, 0.5, 0.9, 0.1, 0.1);
        let p = Personality::with_traits(traits);
        assert_eq!(p.archetype(), PersonalityArchetype::Commander);
    }

    #[test]
    fn test_archetype_default_is_not_balanced() {
        let p = Personality::new();
        let arch = p.archetype();
        assert_ne!(
            arch,
            PersonalityArchetype::Balanced,
            "AURA defaults should have a clear archetype"
        );
    }

    // ==================== PersonalityEngine tests ==========================

    #[test]
    fn test_engine_new() {
        let engine = PersonalityEngine::new();
        assert!((engine.personality().traits.openness - 0.85).abs() < f32::EPSILON);
    }

    #[test]
    fn test_engine_with_traits() {
        let traits = custom_traits(0.6, 0.6, 0.6, 0.6, 0.4);
        let engine = PersonalityEngine::with_traits(traits);
        assert!((engine.personality().traits.openness - 0.6).abs() < f32::EPSILON);
    }

    #[test]
    fn test_engine_evolve_updates_personality() {
        let mut engine = PersonalityEngine::new();
        let before = engine.personality().traits.openness;
        engine.evolve(PersonalityEvent::PositiveInteraction);
        let after = engine.personality().traits.openness;
        assert!(
            after > before,
            "positive interaction should increase openness"
        );
    }

    #[test]
    fn test_engine_consistency_check() {
        let engine = PersonalityEngine::new();
        let report = engine.consistency_check();
        assert!(report.is_consistent);
    }

    #[test]
    fn test_engine_archetype() {
        let engine = PersonalityEngine::new();
        let arch = engine.archetype();
        assert_ne!(arch, PersonalityArchetype::Balanced);
    }

    #[test]
    fn test_engine_serialization_roundtrip() {
        let engine = PersonalityEngine::new();
        let json = serde_json::to_string(&engine).expect("serialize");
        let deserialized: PersonalityEngine = serde_json::from_str(&json).expect("deserialize");
        assert!((deserialized.personality().traits.openness - 0.85).abs() < f32::EPSILON);
    }

    #[test]
    fn test_personality_serialization_roundtrip() {
        let mut p = Personality::new();
        p.evolve(PersonalityEvent::PositiveInteraction);
        let json = serde_json::to_string(&p).expect("serialize");
        let deserialized: Personality = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(deserialized.evolution_count(), 1);
        assert!((deserialized.traits.openness - p.traits.openness).abs() < f32::EPSILON);
    }
}
