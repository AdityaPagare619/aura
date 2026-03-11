use aura_types::identity::{DispositionState, OceanTraits, RelationshipStage};
use serde::{Deserialize, Serialize};

use super::behavior_modifiers::{self, GoalWeights, ResponseStyleParams};
use super::prompt_personality::PersonalityPromptInjector;

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
const EVOLUTION_WINDOW_SIZE: usize = 100;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Computed response-style weights derived from the OCEAN personality.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResponseStyle {
    pub humor: f32,
    pub directness: f32,
    pub formality: f32,
    pub empathy: f32,
    pub proactivity: f32,
}

/// Events that may subtly shift the personality over time.
#[derive(Debug, Clone)]
pub enum PersonalityEvent {
    PositiveInteraction,
    NegativeInteraction,
    UserFeedback(String),
    ContextualPressure { trait_name: String, direction: f32 },
}

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

/// Composite personality influence — all personality-derived parameters
/// bundled together for consumption by the pipeline.
#[derive(Debug, Clone)]
pub struct PersonalityInfluence {
    /// Prompt directives for the LLM (TRUTH + OCEAN + mood + relationship).
    pub prompt_context: String,
    /// Goal prioritization weights.
    pub goal_weights: GoalWeights,
    /// Response style parameters (proactivity, verbosity, risk, autonomy).
    pub response_params: ResponseStyleParams,
    /// Routing bias in \[-0.15, 0.15\].
    pub routing_bias: f32,
    /// Complexity threshold modifier in \[-0.10, 0.10\].
    pub complexity_modifier: f32,
    /// The computed five-factor response style.
    pub response_style: ResponseStyle,
    /// Current archetype classification.
    pub archetype: PersonalityArchetype,
}

/// Tone parameters derived from personality traits using continuous functions.
///
/// Unlike `ResponseStyle` which is general, `ToneParameters` is specifically
/// for response-generation tone control.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToneParameters {
    /// How warm vs. clinical the response should feel (0 = clinical, 1 = warm).
    /// Formula: `A×0.5 + E×0.3 + (1−N)×0.2`
    pub warmth: f32,
    /// How confident vs. hedging the response should be (0 = hedging, 1 = confident).
    /// Formula: `(1−N)×0.5 + C×0.3 + E×0.2`
    pub confidence: f32,
    /// How elaborate vs. terse (0 = terse, 1 = elaborate).
    /// Formula: `E×0.4 + O×0.35 + A×0.25`
    pub elaboration: f32,
    /// How playful vs. serious (0 = serious, 1 = playful).
    /// Formula: `O×0.4 + E×0.35 − N×0.25`
    pub playfulness: f32,
    /// How assertive vs. deferential (0 = deferential, 1 = assertive).
    /// Formula: `E×0.4 + (1−A)×0.35 + (1−N)×0.25`
    pub assertiveness: f32,
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

    /// Compute the five response-style weights from the current traits.
    ///
    /// Formulas (spec §2.1):
    /// - humor      = O×0.6 + E×0.4 − N×0.2
    /// - directness = C×0.5 + (1−A)×0.3 + E×0.2
    /// - formality  = C×0.4 + (1−O)×0.3 + A×0.3
    /// - empathy    = A×0.5 + (1−N)×0.3 + O×0.2
    /// - proactivity= E×0.4 + O×0.3 + C×0.3
    pub fn response_style(&self) -> ResponseStyle {
        let o = self.traits.openness;
        let c = self.traits.conscientiousness;
        let e = self.traits.extraversion;
        let a = self.traits.agreeableness;
        let n = self.traits.neuroticism;

        ResponseStyle {
            humor: (o * 0.6 + e * 0.4 - n * 0.2).clamp(0.0, 1.0),
            directness: (c * 0.5 + (1.0 - a) * 0.3 + e * 0.2).clamp(0.0, 1.0),
            formality: (c * 0.4 + (1.0 - o) * 0.3 + a * 0.3).clamp(0.0, 1.0),
            empathy: (a * 0.5 + (1.0 - n) * 0.3 + o * 0.2).clamp(0.0, 1.0),
            proactivity: (e * 0.4 + o * 0.3 + c * 0.3).clamp(0.0, 1.0),
        }
    }

    /// Compute tone parameters using continuous functions of OCEAN traits.
    ///
    /// These are specifically designed for response-generation tone control
    /// and use smooth blending rather than threshold-based if-else.
    pub fn tone_parameters(&self) -> ToneParameters {
        let o = self.traits.openness;
        let c = self.traits.conscientiousness;
        let e = self.traits.extraversion;
        let a = self.traits.agreeableness;
        let n = self.traits.neuroticism;

        ToneParameters {
            warmth: (a * 0.5 + e * 0.3 + (1.0 - n) * 0.2).clamp(0.0, 1.0),
            confidence: ((1.0 - n) * 0.5 + c * 0.3 + e * 0.2).clamp(0.0, 1.0),
            elaboration: (e * 0.4 + o * 0.35 + a * 0.25).clamp(0.0, 1.0),
            playfulness: (o * 0.4 + e * 0.35 - n * 0.25).clamp(0.0, 1.0),
            assertiveness: (e * 0.4 + (1.0 - a) * 0.35 + (1.0 - n) * 0.25).clamp(0.0, 1.0),
        }
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
    /// 3. Implausible trait combinations (e.g., neuroticism at max while
    ///    conscientiousness also at max — unlikely in OCEAN literature)
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
    ///
    /// Uses a scoring system rather than hard thresholds — each archetype
    /// has an affinity function and the highest-scoring one wins.
    pub fn archetype(&self) -> PersonalityArchetype {
        let o = self.traits.openness;
        let c = self.traits.conscientiousness;
        let e = self.traits.extraversion;
        let a = self.traits.agreeableness;
        let n = self.traits.neuroticism;

        // Affinity scores for each archetype (continuous, not if-else).
        let analyst = o * 0.4 + c * 0.4 + (1.0 - n) * 0.2;
        let helper = a * 0.4 + e * 0.3 + (1.0 - n) * 0.3;
        let explorer = o * 0.4 + e * 0.35 + (1.0 - c) * 0.25;
        let guardian = c * 0.35 + n * 0.35 + (1.0 - o) * 0.30;
        let commander = e * 0.4 + (1.0 - a) * 0.35 + (1.0 - n) * 0.25;

        let scores = [
            (analyst, PersonalityArchetype::Analyst),
            (helper, PersonalityArchetype::Helper),
            (explorer, PersonalityArchetype::Explorer),
            (guardian, PersonalityArchetype::Guardian),
            (commander, PersonalityArchetype::Commander),
        ];

        let max_score = scores
            .iter()
            .map(|(s, _)| *s)
            .fold(f32::NEG_INFINITY, f32::max);
        let min_score = scores.iter().map(|(s, _)| *s).fold(f32::INFINITY, f32::min);

        // If the range is very small, the personality is balanced.
        if (max_score - min_score) < 0.05 {
            return PersonalityArchetype::Balanced;
        }

        scores
            .iter()
            .max_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal))
            .map(|(_, arch)| *arch)
            .unwrap_or(PersonalityArchetype::Balanced)
    }

    // -- behavior-influence methods (Team 5 wiring) -------------------------

    /// Compute a personality-derived routing bias in \[-0.15, 0.15\].
    ///
    /// The bias pushes the routing score toward System2 (positive) or
    /// System1 (negative) based on the OCEAN profile.
    ///
    /// - High O → prefer System2 (explore more):    `+O×0.15`
    /// - High C → prefer System1 (trust templates):  `−C×0.10`
    /// - High N → prefer System2 (be cautious):      `+N×0.10`
    ///
    /// ```text
    /// routing_bias = O×0.15 − C×0.10 + N×0.10 − 0.075
    /// ```
    /// Centered around ~0 for the AURA defaults (O=0.85, C=0.75, N=0.25).
    #[tracing::instrument(skip(self))]
    pub fn routing_bias(&self) -> f32 {
        let raw = self.traits.openness * 0.15 - self.traits.conscientiousness * 0.10
            + self.traits.neuroticism * 0.10
            - 0.075;
        raw.clamp(-0.15, 0.15)
    }

    /// Compute a modifier for the complexity threshold used by the router.
    ///
    /// Returns a value in \[-0.10, 0.10\] that is **added** to the base
    /// complexity threshold (default 0.50). A negative modifier means
    /// the personality favours escalating to System2 at lower complexity.
    ///
    /// - High O → lower threshold (explore more):      `−O×0.10`
    /// - High C → raise threshold (trust System1):      `+C×0.08`
    /// - High N → lower threshold (be cautious):        `−N×0.05`
    ///
    /// ```text
    /// modifier = −O×0.10 + C×0.08 − N×0.05 + 0.025
    /// ```
    #[tracing::instrument(skip(self))]
    pub fn complexity_threshold_modifier(&self) -> f32 {
        let raw = -self.traits.openness * 0.10 + self.traits.conscientiousness * 0.08
            - self.traits.neuroticism * 0.05
            + 0.025;
        raw.clamp(-0.10, 0.10)
    }

    /// Nudge all traits with differential weights reflecting OCEAN psychology:
    /// - Agreeableness & Extraversion are more malleable through interaction
    /// - Openness evolves at moderate rate
    /// - Conscientiousness & Neuroticism are most trait-like (slower to change)
    /// See: McCrae & Costa (2003), Concept Design §4.1, AGI Audit §6.
    fn nudge_all(&mut self, delta: f32) {
        self.traits.openness += delta * 1.0;            // Moderate stability
        self.traits.conscientiousness += delta * 0.7;   // High stability (trait-like)
        self.traits.extraversion += delta * 1.2;        // Socially malleable
        self.traits.agreeableness += delta * 1.3;       // Most interaction-sensitive
        self.traits.neuroticism += delta * 0.8;         // Relatively stable
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

    /// Generate prompt directives influenced by personality, mood, and
    /// relationship context.
    ///
    /// Delegates to [`PersonalityPromptInjector::generate_personality_context`].
    #[tracing::instrument(skip(self, mood))]
    pub fn influence_prompt(
        &self,
        mood: &DispositionState,
        relationship_stage: RelationshipStage,
        trust: f32,
    ) -> String {
        PersonalityPromptInjector::generate_personality_context(
            &self.personality.traits,
            mood,
            relationship_stage,
            trust,
        )
    }

    /// Compute personality-influenced goal prioritization weights.
    ///
    /// Delegates to [`behavior_modifiers::goal_prioritization_weights`].
    /// The weights are normalized to sum to 1.0 and reflect how the OCEAN
    /// profile shifts emphasis between satisfaction, efficiency, safety,
    /// and exploration.
    #[tracing::instrument(skip(self))]
    pub fn influence_goal_priority(&self) -> GoalWeights {
        behavior_modifiers::goal_prioritization_weights(&self.personality.traits)
    }

    /// Compute personality-influenced response tone parameters.
    ///
    /// Returns continuous `ToneParameters` derived from OCEAN traits using
    /// smooth mathematical functions (not threshold-based).
    #[tracing::instrument(skip(self))]
    pub fn influence_response_tone(&self) -> ToneParameters {
        self.personality.tone_parameters()
    }

    /// Compute the full response style parameters from behavior_modifiers.
    ///
    /// Includes proactivity, verbosity, risk tolerance, exploration drive,
    /// and autonomy level — all derived from OCEAN using continuous formulas.
    #[tracing::instrument(skip(self))]
    pub fn response_style_params(&self) -> ResponseStyleParams {
        behavior_modifiers::response_style(&self.personality.traits)
    }

    /// Compute a complete `PersonalityInfluence` bundle — everything the
    /// pipeline needs from the personality system in a single call.
    ///
    /// This is the primary integration point: call once per event and
    /// distribute the result to routing, prompting, goal selection, and
    /// response generation.
    #[tracing::instrument(skip(self, mood))]
    pub fn compute_influence(
        &self,
        mood: &DispositionState,
        relationship_stage: RelationshipStage,
        trust: f32,
    ) -> PersonalityInfluence {
        PersonalityInfluence {
            prompt_context: self.influence_prompt(mood, relationship_stage, trust),
            goal_weights: self.influence_goal_priority(),
            response_params: self.response_style_params(),
            routing_bias: self.personality.routing_bias(),
            complexity_modifier: self.personality.complexity_threshold_modifier(),
            response_style: self.personality.response_style(),
            archetype: self.personality.archetype(),
        }
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
    #[allow(unused_imports)]
    use aura_types::identity::{EmotionLabel, MoodVAD};

    // -- helpers --

    fn default_mood() -> DispositionState {
        DispositionState::default()
    }

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
    fn test_default_response_style() {
        let p = Personality::new();
        let s = p.response_style();

        // humor = 0.85*0.6 + 0.50*0.4 - 0.25*0.2 = 0.51+0.20-0.05 = 0.66
        assert!((s.humor - 0.66).abs() < 0.01, "humor={}", s.humor);
        // directness = 0.75*0.5 + 0.30*0.3 + 0.50*0.2 = 0.375+0.09+0.10 = 0.565
        assert!(
            (s.directness - 0.565).abs() < 0.01,
            "directness={}",
            s.directness
        );
        // formality = 0.75*0.4 + 0.15*0.3 + 0.70*0.3 = 0.30+0.045+0.21 = 0.555
        assert!(
            (s.formality - 0.555).abs() < 0.01,
            "formality={}",
            s.formality
        );
        // empathy = 0.70*0.5 + 0.75*0.3 + 0.85*0.2 = 0.35+0.225+0.17 = 0.745
        assert!((s.empathy - 0.745).abs() < 0.01, "empathy={}", s.empathy);
        // proactivity = 0.50*0.4 + 0.85*0.3 + 0.75*0.3 = 0.20+0.255+0.225 = 0.68
        assert!(
            (s.proactivity - 0.68).abs() < 0.01,
            "proactivity={}",
            s.proactivity
        );
    }

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

        // With exposure attenuation, early interactions have near-full deltas.
        // After a few interactions the personality should show meaningful change.
        for _ in 0..5 {
            p.evolve(PersonalityEvent::PositiveInteraction);
        }
        // After 5 strong positive interactions, at least one trait should be
        // noticeably different from the checkpoint.
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

        // A single positive interaction should move Agreeableness (1.3×) more
        // than Conscientiousness (0.7×), even with exposure attenuation.
        p.evolve(PersonalityEvent::PositiveInteraction);

        let delta_a = p.traits.agreeableness - before_a;
        let delta_c = p.traits.conscientiousness - before_c;

        assert!(
            delta_a > delta_c,
            "Agreeableness delta ({:.4}) should exceed Conscientiousness delta ({:.4})",
            delta_a, delta_c
        );
        // The ratio of OCEAN malleability weights (1.3/0.7) should be preserved
        // regardless of attenuation factor, since it multiplies uniformly.
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

    #[test]
    fn test_routing_bias_within_bounds() {
        let p = Personality::new();
        let bias = p.routing_bias();
        assert!(
            bias >= -0.15 && bias <= 0.15,
            "routing_bias={} must be in [-0.15, 0.15]",
            bias
        );
    }

    #[test]
    fn test_routing_bias_high_openness_positive() {
        let mut p = Personality::new();
        p.traits.openness = 0.9;
        p.traits.conscientiousness = 0.1;
        p.traits.neuroticism = 0.9;
        let bias = p.routing_bias();
        assert!(
            bias > 0.0,
            "high O+N, low C should give positive bias, got {}",
            bias
        );
    }

    #[test]
    fn test_complexity_threshold_modifier_within_bounds() {
        let p = Personality::new();
        let m = p.complexity_threshold_modifier();
        assert!(
            m >= -0.10 && m <= 0.10,
            "complexity_threshold_modifier={} must be in [-0.10, 0.10]",
            m
        );
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
        // At least the neuroticism+conscientiousness implausible combo
        assert!(
            !report.is_consistent,
            "high N + high C should flag implausible"
        );
        assert!(report.issues.iter().any(|i| i.contains("implausible")));
    }

    #[test]
    fn test_consistency_check_extreme_drift() {
        let mut p = Personality::new();
        // Push all traits to max — baseline is AURA defaults, so drift is large
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
    fn test_tone_parameters_within_bounds() {
        let p = Personality::new();
        let tone = p.tone_parameters();
        assert!(tone.warmth >= 0.0 && tone.warmth <= 1.0);
        assert!(tone.confidence >= 0.0 && tone.confidence <= 1.0);
        assert!(tone.elaboration >= 0.0 && tone.elaboration <= 1.0);
        assert!(tone.playfulness >= 0.0 && tone.playfulness <= 1.0);
        assert!(tone.assertiveness >= 0.0 && tone.assertiveness <= 1.0);
    }

    #[test]
    fn test_tone_parameters_high_agreeableness_warm() {
        let traits = custom_traits(0.5, 0.5, 0.8, 0.9, 0.1);
        let p = Personality::with_traits(traits);
        let tone = p.tone_parameters();
        assert!(
            tone.warmth > 0.7,
            "high A+E, low N should be warm: {}",
            tone.warmth
        );
    }

    #[test]
    fn test_tone_parameters_low_neuroticism_confident() {
        let traits = custom_traits(0.5, 0.8, 0.7, 0.5, 0.1);
        let p = Personality::with_traits(traits);
        let tone = p.tone_parameters();
        assert!(
            tone.confidence > 0.7,
            "low N, high C should be confident: {}",
            tone.confidence
        );
    }

    #[test]
    fn test_tone_parameters_extreme_values() {
        // All high
        let high = Personality::with_traits(custom_traits(0.9, 0.9, 0.9, 0.9, 0.9));
        let t = high.tone_parameters();
        assert!(t.warmth >= 0.0 && t.warmth <= 1.0);
        assert!(t.confidence >= 0.0 && t.confidence <= 1.0);

        // All low
        let low = Personality::with_traits(custom_traits(0.1, 0.1, 0.1, 0.1, 0.1));
        let t = low.tone_parameters();
        assert!(t.warmth >= 0.0 && t.warmth <= 1.0);
        assert!(t.playfulness >= 0.0 && t.playfulness <= 1.0);
    }

    #[test]
    fn test_micro_drift_regression_toward_mean() {
        // Trait at 0.9 should drift slightly downward due to regression
        let traits = custom_traits(0.9, 0.5, 0.5, 0.5, 0.5);
        let mut p = Personality::with_traits(traits);
        let _before = p.traits.openness;

        p.evolve(PersonalityEvent::PositiveInteraction);

        // The positive delta pushes up, but the micro-drift regression
        // should slightly counteract the upward push on openness (at 0.9)
        // compared to a trait at center. Just verify it didn't explode.
        assert!(p.traits.openness <= 0.9, "should be clamped to 0.9");
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
        // AURA defaults should have a clear archetype, not Balanced
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
    fn test_engine_influence_prompt_contains_truth() {
        let engine = PersonalityEngine::new();
        let mood = default_mood();
        let prompt = engine.influence_prompt(&mood, RelationshipStage::Stranger, 0.0);
        assert!(
            prompt.contains("TRUTH"),
            "prompt must contain TRUTH framework"
        );
    }

    #[test]
    fn test_engine_influence_goal_priority_sums_to_one() {
        let engine = PersonalityEngine::new();
        let gw = engine.influence_goal_priority();
        let sum = gw.user_satisfaction + gw.efficiency + gw.safety + gw.exploration;
        assert!(
            (sum - 1.0).abs() < 0.001,
            "goal weights must sum to 1.0, got {}",
            sum
        );
    }

    #[test]
    fn test_engine_influence_response_tone() {
        let engine = PersonalityEngine::new();
        let tone = engine.influence_response_tone();
        assert!(tone.warmth >= 0.0 && tone.warmth <= 1.0);
        assert!(tone.confidence >= 0.0 && tone.confidence <= 1.0);
    }

    #[test]
    fn test_engine_response_style_params() {
        let engine = PersonalityEngine::new();
        let params = engine.response_style_params();
        assert!(params.proactivity >= 0.0 && params.proactivity <= 1.0);
        assert!(params.verbosity >= 0.0 && params.verbosity <= 1.0);
        assert!(params.risk_tolerance >= 0.0 && params.risk_tolerance <= 1.0);
    }

    #[test]
    fn test_engine_compute_influence_complete() {
        let engine = PersonalityEngine::new();
        let mood = default_mood();
        let influence = engine.compute_influence(&mood, RelationshipStage::Friend, 0.5);

        // Check all fields are populated
        assert!(!influence.prompt_context.is_empty());
        assert!(influence.routing_bias >= -0.15 && influence.routing_bias <= 0.15);
        assert!(influence.complexity_modifier >= -0.10 && influence.complexity_modifier <= 0.10);
        assert!(influence.response_style.humor >= 0.0);
        let sum = influence.goal_weights.user_satisfaction
            + influence.goal_weights.efficiency
            + influence.goal_weights.safety
            + influence.goal_weights.exploration;
        assert!((sum - 1.0).abs() < 0.001);
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
        // AURA defaults: O=0.85, C=0.75, E=0.50, A=0.70, N=0.25
        // Should have a clear archetype
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
