//! Proactive engine — AURA's "initiative center" (SPEC-ARC section 8.3).
//!
//! Generates suggestions, detects threats, manages morning briefings, and
//! coordinates routine automation. Operates on a budget system where each
//! proactive action costs "initiative points" that regenerate over time.

pub mod attention;
pub mod morning;
pub mod routines;
pub mod suggestions;
pub mod welcome;

use std::collections::VecDeque;

pub use attention::{AttentionState, ForestGuardian};
use aura_types::power::PowerTier;
pub use morning::{BriefingSection, MorningBriefing};
pub use routines::{Automation, DetectedRoutine, RoutineManager};
use serde::{Deserialize, Serialize};
pub use suggestions::{Suggestion, SuggestionEngine, SuggestionTrigger};
use tracing::{debug, info, instrument, warn};

use super::{ArcError, ContextMode, DomainId};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum proactive suggestions surfaced per day.
const MAX_DAILY_SUGGESTIONS: u16 = 50;

/// Initiative budget regeneration rate (per second).
const INITIATIVE_REGEN_RATE: f32 = 0.001;

/// Minimum initiative cost for any proactive action.
const MIN_INITIATIVE_COST: f32 = 0.05;

/// Maximum initiative budget (cap).
const MAX_INITIATIVE_BUDGET: f32 = 1.0;

/// Battery penalty threshold (20% = 0.20).
const BATTERY_PENALTY_THRESHOLD: f32 = 0.20;

/// Thermal penalty threshold (45°C).
const THERMAL_PENALTY_THRESHOLD: f32 = 45.0;

/// Initiative regen rate when battery is below threshold (per second).
const LOW_BATTERY_REGEN: f32 = 0.0005;

/// Initiative regen rate when temperature exceeds threshold (per second).
const THERMAL_REGEN: f32 = 0.0005;

/// Maximum proactive actions returned per tick.
const MAX_ACTIONS_PER_TICK: usize = 8;

/// Default elapsed-seconds value used for the daily budget reset (1 day = 86400s).
pub(crate) const DEFAULT_DAILY_BUDGET_RESET_SECS: f32 = 86400.0;

// ---------------------------------------------------------------------------
// Serde default helpers
// ---------------------------------------------------------------------------

fn default_daily_budget_reset_secs() -> f32 {
    DEFAULT_DAILY_BUDGET_RESET_SECS
}

// ---------------------------------------------------------------------------
// ProactiveAction — output type
// ---------------------------------------------------------------------------

/// An action the proactive engine wants to surface to the user or execute.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ProactiveAction {
    /// A proactive suggestion to show the user.
    Suggest(Suggestion),
    /// A morning briefing to present.
    Briefing(Vec<BriefingSection>),
    /// Run a detected automation routine.
    RunAutomation {
        routine_id: u64,
        actions: Vec<String>,
    },
    /// An urgent alert from a specific domain.
    Alert {
        domain: DomainId,
        message: String,
        urgency: f32,
    },
}

// ---------------------------------------------------------------------------
// ProactiveEngine
// ---------------------------------------------------------------------------

/// The proactive engine aggregate — owns briefing, suggestion, and routine
/// sub-engines plus the initiative budget system.
#[derive(Debug, Serialize, Deserialize)]
pub struct ProactiveEngine {
    /// Morning briefing sub-engine.
    pub morning: MorningBriefing,
    /// Suggestion generation sub-engine.
    pub suggestions: SuggestionEngine,
    /// Routine detection and automation sub-engine.
    pub routines: RoutineManager,
    /// Monitor to protect user from Attention Lock-in / Doomscrolling
    pub forest_guardian: ForestGuardian,
    /// Initiative budget: [0.0, 1.0]. Each action costs points; regenerates over time.
    initiative_budget: f32,
    /// Number of suggestions surfaced today.
    daily_suggestions_count: u16,
    /// Day-of-year (1..=366) when daily counters were last reset.
    last_reset_day: u32,
    /// Timestamp (ms) of last tick for delta-time calculation.
    last_tick_ms: u64,
    /// Queue of pending proactive actions awaiting drain.
    pending_actions: VecDeque<ProactiveAction>,
    /// Accumulated threat score [0.0, 1.0], decays exponentially each tick.
    threat_score: f32,
    /// Count of negative signals (rejections, dismissals) since last reset.
    negative_signal_count: u32,
    /// Elapsed seconds used for the daily initiative budget reset.
    #[serde(default = "default_daily_budget_reset_secs")]
    daily_budget_reset_secs: f32,
}

impl ProactiveEngine {
    /// Create a new proactive engine with full budget and empty sub-engines.
    #[must_use]
    pub fn new() -> Self {
        Self {
            morning: MorningBriefing::new(),
            suggestions: SuggestionEngine::new(),
            routines: RoutineManager::new(),
            forest_guardian: ForestGuardian::new(),
            initiative_budget: MAX_INITIATIVE_BUDGET,
            daily_suggestions_count: 0,
            last_reset_day: 0,
            last_tick_ms: 0,
            pending_actions: VecDeque::new(),
            threat_score: 0.0,
            negative_signal_count: 0,
            daily_budget_reset_secs: DEFAULT_DAILY_BUDGET_RESET_SECS,
        }
    }

    /// Configured elapsed seconds for the daily initiative budget reset.
    #[must_use]
    pub(crate) fn daily_budget_reset_secs(&self) -> f32 {
        self.daily_budget_reset_secs
    }

    /// Current initiative budget.
    #[must_use]
    pub fn budget(&self) -> f32 {
        self.initiative_budget
    }

    /// Number of suggestions surfaced today.
    #[must_use]
    pub fn daily_suggestions(&self) -> u16 {
        self.daily_suggestions_count
    }

    /// Regenerate initiative budget based on elapsed seconds, battery level, and temperature.
    pub fn regenerate_initiative(
        &mut self,
        elapsed_secs: f32,
        battery_percent: u8,
        temperature_c: f32,
    ) {
        if elapsed_secs <= 0.0 {
            return;
        }

        let battery_level = battery_percent as f32 / 100.0;
        let regen = if battery_level < BATTERY_PENALTY_THRESHOLD {
            LOW_BATTERY_REGEN
        } else if temperature_c > THERMAL_PENALTY_THRESHOLD {
            THERMAL_REGEN
        } else {
            INITIATIVE_REGEN_RATE
        };

        let gain = elapsed_secs * regen;
        self.initiative_budget = (self.initiative_budget + gain).min(MAX_INITIATIVE_BUDGET);
        debug!(
            budget = self.initiative_budget,
            gain,
            regen_rate = regen,
            battery_pct = battery_percent,
            temp_c = temperature_c,
            "initiative regenerated"
        );
    }

    /// Cap the initiative budget to a maximum value.
    ///
    /// Used by the health monitor to throttle proactive activity under battery
    /// or thermal pressure. Pass `0.0` to immediately stop all proactive actions.
    pub fn cap_budget(&mut self, max: f32) {
        self.initiative_budget = self
            .initiative_budget
            .min(max.clamp(0.0, MAX_INITIATIVE_BUDGET));
        debug!(
            budget = self.initiative_budget,
            cap = max,
            "initiative budget capped by health monitor"
        );
    }

    /// Spend initiative budget for an action. Returns `false` if insufficient.
    fn spend_initiative(&mut self, cost: f32) -> bool {
        let actual_cost = cost.max(MIN_INITIATIVE_COST);
        if self.initiative_budget >= actual_cost {
            self.initiative_budget -= actual_cost;
            true
        } else {
            false
        }
    }

    /// Reset daily counters if the day has changed.
    fn maybe_reset_daily(&mut self, current_day: u32) {
        if current_day != self.last_reset_day {
            debug!(
                old_day = self.last_reset_day,
                new_day = current_day,
                suggestions = self.daily_suggestions_count,
                "daily proactive counters reset"
            );
            self.daily_suggestions_count = 0;
            self.last_reset_day = current_day;
        }
    }

    // -----------------------------------------------------------------------
    // Cron-facing methods (called from main_loop cron handlers)
    // -----------------------------------------------------------------------

    /// Detect proactive opportunities by evaluating suggestion triggers and
    /// routine patterns. Discovered actions are pushed onto the internal
    /// `pending_actions` queue for later draining.
    ///
    /// Called by the `opportunity_detect` cron job (every 15 min, P2Normal).
    /// Returns the number of newly enqueued actions.
    #[instrument(name = "opportunity_detect", skip_all)]
    pub(crate) fn detect_opportunities(&mut self, now_ms: u64) -> Result<usize, ArcError> {
        let mut enqueued: usize = 0;

        // 1) Evaluate suggestion triggers for new opportunities.
        match self.suggestions.evaluate_triggers(now_ms) {
            Ok(suggestions) => {
                for suggestion in suggestions {
                    self.pending_actions
                        .push_back(ProactiveAction::Suggest(suggestion));
                    enqueued += 1;
                }
            }
            Err(e) => {
                warn!(error = %e, "opportunity detection: suggestion trigger evaluation failed");
            }
        }

        // 2) Check routine automations for the current time window.
        let total_hours = now_ms / 3_600_000;
        let hour = (total_hours % 24) as u8;
        let day = (now_ms / 86_400_000) as u32;
        let day_of_week = (day % 7) as u8;

        let due_automations: Vec<Automation> = self
            .routines
            .check_automations(hour, day_of_week)
            .into_iter()
            .cloned()
            .collect();
        for automation in &due_automations {
            self.pending_actions
                .push_back(ProactiveAction::RunAutomation {
                    routine_id: automation.routine_id,
                    actions: automation.actions.clone(),
                });
            enqueued += 1;
        }

        // 3) Cross-domain telemetry: high acceptance for a domain is a positive signal. Reduce
        //    threat score slightly so the LLM sees a healthier metric. This is MEASUREMENT only —
        //    it does not alter what is surfaced.
        let stats = self.suggestions.category_stats_snapshot();
        for (_domain, acceptance_rate, total) in &stats {
            if *total >= 5 && *acceptance_rate > 0.7 {
                self.threat_score = (self.threat_score - 0.02).max(0.0);
            }
        }

        info!(
            enqueued,
            pending = self.pending_actions.len(),
            "opportunity detection complete"
        );
        Ok(enqueued)
    }

    /// Accumulate threat signals by applying exponential decay to the current
    /// threat score and then inspecting suggestion rejection rates across
    /// domains.
    ///
    /// # Architecture contract — telemetry only
    /// `threat_score` is a **reporting metric exposed to the LLM** so it can
    /// reason about whether the user is finding AURA's suggestions useful.
    /// It does NOT gate which actions are surfaced, does NOT alter routing,
    /// and does NOT suppress suggestions in Rust. Any behavioural response to
    /// a high threat score is the LLM's decision.
    ///
    /// Called by the `threat_accumulate` cron job (every 2 min, P0Always).
    /// Returns the current threat score after accumulation.
    #[instrument(name = "threat_accumulate", skip_all)]
    pub(crate) fn accumulate_threats(&mut self) -> f32 {
        // 1) Exponential decay — threat_score halves roughly every 30 minutes. At 2-min cadence
        //    (120s), decay factor ≈ 0.955  (0.5^(120/1800)).
        const DECAY_FACTOR: f32 = 0.955;
        self.threat_score *= DECAY_FACTOR;

        // 2) Scan category stats for high-rejection domains → update telemetry. This is a
        //    MEASUREMENT step only. It does not alter what gets surfaced.
        let stats = self.suggestions.category_stats_snapshot();
        for (_domain, acceptance_rate, total) in &stats {
            // Only consider domains with meaningful sample sizes.
            if *total >= 3 && *acceptance_rate < 0.3 {
                // Low acceptance is a signal the LLM can act on.
                // Rust records the fact; it does not make the decision.
                self.threat_score = (self.threat_score + 0.05).min(1.0);
                self.negative_signal_count = self.negative_signal_count.saturating_add(1);
            }
        }

        // 3) Clamp floor.
        if self.threat_score < 0.001 {
            self.threat_score = 0.0;
        }

        info!(
            threat_score = self.threat_score,
            negative_signals = self.negative_signal_count,
            "threat accumulation complete"
        );
        self.threat_score
    }

    /// Current accumulated threat score [0.0, 1.0].
    #[must_use]
    pub fn threat_score(&self) -> f32 {
        self.threat_score
    }

    /// Number of pending actions in the drain queue.
    #[must_use]
    pub fn pending_action_count(&self) -> usize {
        self.pending_actions.len()
    }

    /// Drain pending proactive actions, spending initiative budget for each.
    /// Returns the actions that were successfully drained (budget permitting).
    ///
    /// Called by the `action_drain` cron job (every 60s, P0Always).
    #[instrument(name = "action_drain", skip_all)]
    pub(crate) fn drain_pending_actions(&mut self) -> Vec<ProactiveAction> {
        let mut drained: Vec<ProactiveAction> = Vec::with_capacity(MAX_ACTIONS_PER_TICK);

        while let Some(action) = self.pending_actions.front() {
            if drained.len() >= MAX_ACTIONS_PER_TICK {
                break;
            }

            // Determine cost based on action type.
            let cost = match action {
                ProactiveAction::Suggest(s) => 0.1 * (1.0 - s.confidence).max(MIN_INITIATIVE_COST),
                ProactiveAction::RunAutomation { .. } => 0.1,
                ProactiveAction::Briefing(_) => 0.15,
                ProactiveAction::Alert { urgency, .. } => {
                    // Urgent alerts are cheap — we want them to go through.
                    0.05 * (1.0 - urgency).max(MIN_INITIATIVE_COST)
                }
            };

            if !self.spend_initiative(cost) {
                // Budget exhausted — stop draining, leave remaining in queue.
                debug!(
                    remaining = self.pending_actions.len(),
                    budget = self.initiative_budget,
                    "drain stopped: insufficient initiative budget"
                );
                break;
            }

            // Budget spent — pop and collect.
            if let Some(action) = self.pending_actions.pop_front() {
                // Count suggestions against daily limit.
                if matches!(&action, ProactiveAction::Suggest(_)) {
                    self.daily_suggestions_count = self.daily_suggestions_count.saturating_add(1);
                }
                drained.push(action);
            }
        }

        info!(
            drained = drained.len(),
            remaining = self.pending_actions.len(),
            budget = self.initiative_budget,
            "action drain complete"
        );
        drained
    }

    /// Main evaluation cycle — call once per tick from the cron scheduler.
    ///
    /// Checks morning briefing, evaluates suggestion triggers, and checks
    /// routine automations. All outputs are gated by initiative budget and
    /// power tier.
    #[instrument(name = "proactive_tick", skip_all)]
    pub fn tick(
        &mut self,
        now_ms: u64,
        power: PowerTier,
        mode: ContextMode,
        proactive_allowed: bool,
        battery_percent: u8,
        temperature_c: f32,
    ) -> Result<Vec<ProactiveAction>, ArcError> {
        // CRITICAL: If user hasn't consented, do NOT produce any proactive actions
        if !proactive_allowed {
            debug!("proactive blocked - user consent required");
            return Ok(Vec::new());
        }

        // Time-delta for budget regeneration.
        let elapsed_ms = now_ms.saturating_sub(self.last_tick_ms);
        let elapsed_secs = elapsed_ms as f32 / 1000.0;
        self.last_tick_ms = now_ms;

        self.regenerate_initiative(elapsed_secs, battery_percent, temperature_c);

        // Derive hour and day-of-year from millisecond timestamp.
        // Simple approximation: we divide by ms-per-hour / ms-per-day.
        let total_hours = now_ms / 3_600_000;
        let hour = (total_hours % 24) as u8;
        let day = (now_ms / 86_400_000) as u32;
        let day_of_week = (day % 7) as u8; // 0=epoch-day-of-week

        self.maybe_reset_daily(day);

        let mut actions: Vec<ProactiveAction> = Vec::with_capacity(MAX_ACTIONS_PER_TICK);

        // Skip non-critical proactive work in low-power modes.
        let skip_suggestions = matches!(mode, ContextMode::Sleeping | ContextMode::DoNotDisturb);

        // --- Morning briefing ---
        // Only in P2Normal or better, and not in Sleeping/DnD.
        if !skip_suggestions
            && matches!(
                power,
                PowerTier::P0Always | PowerTier::P1IdlePlus | PowerTier::P2Normal
            )
            && self.morning.should_trigger(hour, day)
            && self.spend_initiative(0.15)
        {
            match self.morning.generate(day) {
                Ok(sections) => {
                    if !sections.is_empty() {
                        info!(sections = sections.len(), "morning briefing generated");
                        actions.push(ProactiveAction::Briefing(sections));
                    }
                }
                Err(e) => {
                    warn!(error = %e, "morning briefing generation failed");
                }
            }
        }

        // --- Suggestions ---
        if !skip_suggestions
            && self.daily_suggestions_count < MAX_DAILY_SUGGESTIONS
            && matches!(
                power,
                PowerTier::P0Always | PowerTier::P1IdlePlus | PowerTier::P2Normal
            )
        {
            match self.suggestions.evaluate_triggers(now_ms) {
                Ok(new_suggestions) => {
                    for suggestion in new_suggestions {
                        if actions.len() >= MAX_ACTIONS_PER_TICK {
                            break;
                        }
                        if self.daily_suggestions_count >= MAX_DAILY_SUGGESTIONS {
                            break;
                        }
                        let cost = 0.1 * (1.0 - suggestion.confidence).max(MIN_INITIATIVE_COST);
                        if self.spend_initiative(cost) {
                            self.daily_suggestions_count =
                                self.daily_suggestions_count.saturating_add(1);
                            actions.push(ProactiveAction::Suggest(suggestion));
                        }
                    }
                }
                Err(e) => {
                    warn!(error = %e, "suggestion evaluation failed");
                }
            }
        }

        // --- Routine automations ---
        // Automations can run in any mode except Sleeping, at P1 or better.
        if !matches!(mode, ContextMode::Sleeping)
            && matches!(
                power,
                PowerTier::P0Always | PowerTier::P1IdlePlus | PowerTier::P2Normal
            )
        {
            let due_automations: Vec<Automation> = self
                .routines
                .check_automations(hour, day_of_week)
                .into_iter()
                .cloned()
                .collect();
            for automation in &due_automations {
                if actions.len() >= MAX_ACTIONS_PER_TICK {
                    break;
                }
                if self.spend_initiative(0.1) {
                    actions.push(ProactiveAction::RunAutomation {
                        routine_id: automation.routine_id,
                        actions: automation.actions.clone(),
                    });
                }
            }
        }

        debug!(
            action_count = actions.len(),
            budget = self.initiative_budget,
            daily = self.daily_suggestions_count,
            "proactive tick complete"
        );

        Ok(actions)
    }
}

impl Default for ProactiveEngine {
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

    #[test]
    fn test_new_engine() {
        let e = ProactiveEngine::new();
        assert!((e.budget() - 1.0).abs() < f32::EPSILON);
        assert_eq!(e.daily_suggestions(), 0);
        assert_eq!(e.last_reset_day, 0);
    }

    #[test]
    fn test_regenerate_initiative() {
        let mut e = ProactiveEngine::new();
        e.initiative_budget = 0.5;
        e.regenerate_initiative(100.0, 100, 30.0); // 100s * 0.001 = 0.1 (normal conditions)
        assert!((e.budget() - 0.6).abs() < 0.001, "got {}", e.budget());
    }

    #[test]
    fn test_regenerate_capped_at_max() {
        let mut e = ProactiveEngine::new();
        e.initiative_budget = 0.99;
        e.regenerate_initiative(1000.0, 100, 30.0); // would add 1.0
        assert!(
            (e.budget() - 1.0).abs() < f32::EPSILON,
            "got {}",
            e.budget()
        );
    }

    #[test]
    fn test_regenerate_negative_elapsed() {
        let mut e = ProactiveEngine::new();
        e.initiative_budget = 0.5;
        e.regenerate_initiative(-10.0, 100, 30.0);
        assert!((e.budget() - 0.5).abs() < f32::EPSILON);
    }

    #[test]
    fn test_regenerate_low_battery_penalty() {
        let mut e = ProactiveEngine::new();
        e.initiative_budget = 0.5;
        e.regenerate_initiative(100.0, 15, 30.0); // 15% battery (< 20% threshold)
                                                  // Should use LOW_BATTERY_REGEN = 0.0005, so gain = 100 * 0.0005 = 0.05
        assert!((e.budget() - 0.55).abs() < 0.001, "got {}", e.budget());
    }

    #[test]
    fn test_regenerate_thermal_penalty() {
        let mut e = ProactiveEngine::new();
        e.initiative_budget = 0.5;
        e.regenerate_initiative(100.0, 100, 50.0); // 50°C > 45°C threshold
                                                   // Should use THERMAL_REGEN = 0.0005, so gain = 100 * 0.0005 = 0.05
        assert!((e.budget() - 0.55).abs() < 0.001, "got {}", e.budget());
    }

    #[test]
    fn test_regenerate_battery_threshold_edge() {
        let mut e = ProactiveEngine::new();
        e.initiative_budget = 0.5;
        // At exactly 20%, should use base rate (not penalized)
        e.regenerate_initiative(100.0, 20, 30.0);
        // Base rate = 0.001, gain = 100 * 0.001 = 0.1
        assert!((e.budget() - 0.6).abs() < 0.001, "got {}", e.budget());
    }

    #[test]
    fn test_regenerate_thermal_threshold_edge() {
        let mut e = ProactiveEngine::new();
        e.initiative_budget = 0.5;
        // At exactly 45°C, should use base rate (not penalized)
        e.regenerate_initiative(100.0, 100, 45.0);
        // Base rate = 0.001, gain = 100 * 0.001 = 0.1
        assert!((e.budget() - 0.6).abs() < 0.001, "got {}", e.budget());
    }

    #[test]
    fn test_spend_initiative() {
        let mut e = ProactiveEngine::new();
        assert!(e.spend_initiative(0.3));
        assert!((e.budget() - 0.7).abs() < 0.001);
    }

    #[test]
    fn test_spend_initiative_min_cost() {
        let mut e = ProactiveEngine::new();
        // Spending 0.01 should actually cost MIN_INITIATIVE_COST (0.05)
        assert!(e.spend_initiative(0.01));
        assert!((e.budget() - 0.95).abs() < 0.001, "got {}", e.budget());
    }

    #[test]
    fn test_spend_initiative_insufficient() {
        let mut e = ProactiveEngine::new();
        e.initiative_budget = 0.02;
        assert!(!e.spend_initiative(0.1));
        assert!((e.budget() - 0.02).abs() < f32::EPSILON);
    }

    #[test]
    fn test_daily_reset() {
        let mut e = ProactiveEngine::new();
        e.last_reset_day = 100;
        e.daily_suggestions_count = 42;
        e.maybe_reset_daily(101);
        assert_eq!(e.daily_suggestions_count, 0);
        assert_eq!(e.last_reset_day, 101);
    }

    #[test]
    fn test_daily_no_reset_same_day() {
        let mut e = ProactiveEngine::new();
        e.last_reset_day = 100;
        e.daily_suggestions_count = 10;
        e.maybe_reset_daily(100);
        assert_eq!(e.daily_suggestions_count, 10);
    }

    #[test]
    fn test_tick_returns_ok_empty() {
        let mut e = ProactiveEngine::new();
        let result = e.tick(
            1_000,
            PowerTier::P2Normal,
            ContextMode::Default,
            true,
            100,
            30.0,
        );
        assert!(result.is_ok());
        // No triggers registered, so no actions.
        let actions = result.expect("should be ok");
        // Morning won't trigger at hour 0 (default hour is 7), and no triggers registered.
        assert!(
            actions.is_empty(),
            "expected no actions at hour 0 with no triggers, got {actions:?}"
        );
    }

    #[test]
    fn test_tick_sleeping_suppresses_suggestions() {
        let mut e = ProactiveEngine::new();
        let actions = e
            .tick(
                1_000,
                PowerTier::P0Always,
                ContextMode::Sleeping,
                true,
                100,
                30.0,
            )
            .expect("tick ok");
        // In sleeping mode, suggestions and briefings are suppressed.
        for a in &actions {
            match a {
                ProactiveAction::Suggest(_) | ProactiveAction::Briefing(_) => {
                    panic!("should not get suggestions/briefings in sleeping mode");
                }
                _ => {}
            }
        }
    }

    #[test]
    fn test_proactive_action_variants() {
        // Verify all ProactiveAction variants can be constructed.
        let s = Suggestion {
            id: 1,
            category: DomainId::Health,
            text: "Drink water".into(),
            confidence: 0.8,
            created_ms: 1000,
            priority: 5,
        };
        let _a1 = ProactiveAction::Suggest(s);
        let _a2 = ProactiveAction::Briefing(vec![BriefingSection::Weather]);
        let _a3 = ProactiveAction::RunAutomation {
            routine_id: 42,
            actions: vec!["open_app".into()],
        };
        let _a4 = ProactiveAction::Alert {
            domain: DomainId::Health,
            message: "High heart rate".into(),
            urgency: 0.9,
        };
    }
}
