//! Bi-Cameral Semantic React System
//!
//! # System Design Philosophy
//! Following first principles, this module manages the Essential Complexity of deciding
//! *how hard* AURA should think. It acts as the gatekeeper between System 1 (Fast, ETG-driven)
//! and System 2 (Slow, LLM-driven).
//!
//! # Precise System Modeling
//! - **Points (States)**: `System1` (Fast), `System2` (Slow)
//! - **Events**: `ObservationReceived`, `ExecutionFailed`, `AmbiguityDetected`
//! - **Transitions**: `Escalate`, `Deescalate`
//! - **Invariants**: Escalation must be mathematically justified by a cost-benefit threshold.

use tracing::{debug, info, warn};

/// Represents the current cognitive engagement level of AURA.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CognitiveState {
    /// Fast-path execution. Relies on ETG, Cache, and heuristics. Minimal resources.
    System1,
    /// Slow-path execution. Relies on LLM reasoning. High power/data consumption.
    System2,
}

/// A rigorous, atomic model of current environmental constraints and internal state
/// to determine if we should mathematically cross the threshold into System 2.
#[derive(Debug, Clone)]
pub struct EscalationContext {
    /// 0.0 to 1.0 - How confident is System 1 in its current path?
    pub system1_confidence: f32,
    /// 0.0 to 1.0 - How urgent/critical is the current goal (from Amygdala arousal)?
    pub amygdala_arousal: f32,
    /// Number of consecutive execution failures on the current step.
    pub consecutive_failures: u32,
    /// 0.0 to 1.0 - Current battery level (1.0 = 100%).
    pub battery_level: f32,
    /// Is the device currently in a thermal throttling state?
    pub is_thermal_throttling: bool,
}

/// The Semantic React engine evaluates system transitions based on dynamic thresholds.
#[derive(Debug, Clone)]
pub struct SemanticReact {
    /// Base confidence required to stay in System 1.
    pub base_confidence_threshold: f32,
    /// Number of consecutive failures before forcing escalation to System 2.
    /// Default: 3, but can be raised or lowered based on user tolerance.
    pub consecutive_failure_threshold: u32,
    /// Critical battery level that restricts System 2 unless highly urgent.
    /// Default: 0.15 (15%), but adjusts based on device aging or user behavior.
    pub critical_battery_threshold: f32,
}

impl Default for SemanticReact {
    fn default() -> Self {
        Self {
            base_confidence_threshold: 0.70, // Start demanding 70% confidence to stay in System 1
            consecutive_failure_threshold: 3,
            critical_battery_threshold: 0.15,
        }
    }
}

impl SemanticReact {
    pub fn new() -> Self {
        Self::default()
    }

    /// Allows the Learning Engine to organically adjust escalation thresholds based on 
    /// historical success/failure rates, rather than being permanently hardcoded.
    pub fn adapt_thresholds(&mut self, feedback_delta: f32, successful_runs: u32) {
        // Precise System Modeling: State mutation via learning event.
        self.base_confidence_threshold = (self.base_confidence_threshold + feedback_delta).clamp(0.40, 0.95);
        
        if successful_runs > 50 {
            // User is highly tolerant, or system is highly stable. Expand failure tolerance.
            self.consecutive_failure_threshold = 4;
        }
    }

    /// Determines the optimal cognitive state given the current context.
    ///
    /// # Mathematical Escalation Model
    /// We dynamically adjust the required confidence threshold based on resources and urgency.
    /// If actual confidence < adjusted threshold, we escalate to System 2.
    pub fn evaluate_escalation(&self, ctx: &EscalationContext) -> CognitiveState {
        // 1. HARD Constraints (Precise System Modeling: Invariants)
        if ctx.is_thermal_throttling {
            warn!("Thermal throttling active: FORCING System 1 to save device health.");
            return CognitiveState::System1;
        }

        if ctx.battery_level < self.critical_battery_threshold && ctx.amygdala_arousal < 0.8 {
            warn!("Low battery (<{}%) and non-critical task: FORCING System 1.", self.critical_battery_threshold * 100.0);
            return CognitiveState::System1;
        }

        if ctx.consecutive_failures >= self.consecutive_failure_threshold {
            info!("Consecutive failure threshold reached ({}): ESCALATING to System 2.", self.consecutive_failure_threshold);
            return CognitiveState::System2;
        }

        // 2. Dynamic Threshold Calculation (System Design Philosophy: Balance Trade-offs)
        let mut required_confidence = self.base_confidence_threshold;

        // If highly critical, we demand higher confidence to trust System 1 (else we escalate).
        let urgency_penalty = if ctx.amygdala_arousal > 0.6 {
            (ctx.amygdala_arousal - 0.6) * 0.5 // up to +0.20 required confidence
        } else {
            0.0
        };
        required_confidence += urgency_penalty;

        // If battery is somewhat low, we lower the threshold to resist escalation.
        let resource_bonus = if ctx.battery_level < 0.4 {
            (0.4 - ctx.battery_level) * 0.5 // up to -0.20 required confidence
        } else {
            0.0
        };
        required_confidence -= resource_bonus;

        // Clamp the threshold to logical extremes.
        required_confidence = required_confidence.clamp(0.4, 0.95);

        debug!(
            system1_conf = ctx.system1_confidence,
            adjusted_threshold = required_confidence,
            arousal = ctx.amygdala_arousal,
            battery = ctx.battery_level,
            "Evaluated adaptive escalation threshold"
        );

        // 3. The Core Transition Transition
        if ctx.system1_confidence >= required_confidence {
            CognitiveState::System1
        } else {
            info!(
                confidence = ctx.system1_confidence,
                threshold = required_confidence,
                "Confidence below dynamic threshold: ESCALATING to System 2"
            );
            CognitiveState::System2
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_thermal_throttling_forces_system1() {
        let react = SemanticReact::new();
        let ctx = EscalationContext {
            system1_confidence: 0.1, // Terribly low confidence
            amygdala_arousal: 0.9,   // High urgency
            consecutive_failures: 5, // Lots of failures
            battery_level: 0.5,
            is_thermal_throttling: true, // BUT device is burning
        };
        
        // Exact boundary check: MUST protect device over task success.
        assert_eq!(react.evaluate_escalation(&ctx), CognitiveState::System1);
    }
}
