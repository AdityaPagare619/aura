//! Financial life arc — tracks financial behavioral patterns.
//!
//! # What this tracks (FACTS, not advice)
//!
//! - Savings actions (deposits, transfers to savings)
//! - Spending events by category
//! - Debt payments
//! - Income received
//! - User-set monthly savings targets
//!
//! # What this does NOT do
//!
//! - Give financial advice (LLM does that)
//! - Store actual balances (privacy — we track BEHAVIOURS, not amounts)
//! - Make investment decisions
//!
//! # Scoring formula
//!
//! `score = savings_consistency * 0.40 + spending_discipline * 0.40 + goal_progress * 0.20`
//!
//! All weights are constants but exposed for documentation clarity.
//! Day-zero: with no data, score defaults to 0.5 (Stable), no triggers.

use serde::{Deserialize, Serialize};

use super::primitives::{ArcHealth, ArcType, ProactiveTrigger, ONE_DAY_MS};

// ---------------------------------------------------------------------------
// Weights
// ---------------------------------------------------------------------------

/// Weight of savings consistency in the financial score.
const W_SAVINGS_CONSISTENCY: f32 = 0.40;
/// Weight of spending discipline in the financial score.
const W_SPENDING_DISCIPLINE: f32 = 0.40;
/// Weight of goal progress in the financial score.
const W_GOAL_PROGRESS: f32 = 0.20;

/// Days in the rolling window for scoring.
const ROLLING_WINDOW_DAYS: u64 = 30;
const ROLLING_WINDOW_MS: u64 = ROLLING_WINDOW_DAYS * ONE_DAY_MS;

/// Minimum savings actions in 30 days to score as fully consistent.
const TARGET_SAVINGS_ACTIONS_30D: u32 = 8;

/// Maximum spend actions per month before discipline score degrades.
/// This is intentionally generous — frequent small transactions are fine.
const SPEND_CONCERN_THRESHOLD_30D: u32 = 60;

/// Minimum events needed before producing a proactive trigger (day-zero guard).
const MIN_EVENTS_FOR_TRIGGER: u32 = 3;

// ---------------------------------------------------------------------------
// FinancialEvent
// ---------------------------------------------------------------------------

/// Events that the daemon records into the financial arc.
///
/// All amounts are in minor currency units (cents / pence / etc.) to avoid
/// floating-point representation issues. Sign convention: positive = inflow
/// for SavingsAction (amount_cents can be negative for withdrawals).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum FinancialEvent {
    /// A savings or investment action. Positive = deposit, negative = withdrawal.
    SavingsAction {
        amount_cents: i64,
        description: String,
    },
    /// A spending event. Always positive (amount spent).
    SpendingAction { amount_cents: u64, category: String },
    /// A debt payment (reduces outstanding debt).
    DebtPayment { amount_cents: u64 },
    /// Income received.
    IncomeReceived { amount_cents: u64 },
    /// User set a monthly savings goal.
    BudgetGoalSet { monthly_savings_target_cents: u64 },
}

// ---------------------------------------------------------------------------
// FinancialArc
// ---------------------------------------------------------------------------

/// Tracks financial behavioral patterns over rolling windows.
///
/// Privacy-preserving: tracks COUNTS and aggregate amounts — not transaction
/// history or account details. The user controls what events are recorded.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FinancialArc {
    /// Current computed health level.
    pub health: ArcHealth,

    // Rolling 30-day counts (event count, not amounts — privacy).
    /// Number of savings actions in the rolling 30-day window.
    pub recent_savings_actions: u32,
    /// Number of spending actions in the rolling 30-day window.
    pub recent_spending_actions: u32,
    /// Number of debt payments in the rolling 30-day window.
    pub recent_debt_payments: u32,

    /// Consecutive days (approximate) where at least one savings action occurred.
    /// Approximated from event timestamps — not exact calendar tracking.
    pub savings_streak_days: u32,

    /// Unix millisecond timestamp of the most recent event.
    pub last_event_ms: u64,

    /// Unix millisecond timestamp of the last proactive trigger (0 = never).
    /// Used to enforce max-1-per-day deduplication.
    pub last_trigger_ms: u64,

    /// User-set monthly savings target (None if not configured).
    pub monthly_savings_target_cents: Option<u64>,

    /// Running total of net savings actions this month (cents, signed).
    pub total_savings_this_month_cents: i64,

    /// Total events recorded (lifetime, for day-zero guard).
    pub total_events: u32,

    // Internal: timestamps of recent savings actions for streak/window calculation.
    // Bounded to 90 entries (3 months of daily actions).
    recent_savings_timestamps: Vec<u64>,

    // Internal: timestamps of recent spending actions for window calculation.
    // Bounded to 180 entries.
    recent_spending_timestamps: Vec<u64>,
}

impl FinancialArc {
    /// Capacity bounds for timestamp ring buffers.
    const MAX_SAVINGS_TS: usize = 90;
    const MAX_SPENDING_TS: usize = 180;

    /// Create a new financial arc in the default day-zero state.
    ///
    /// Day-zero safe: health defaults to `Stable`, no triggers.
    #[must_use]
    pub fn new() -> Self {
        Self {
            health: ArcHealth::Stable,
            recent_savings_actions: 0,
            recent_spending_actions: 0,
            recent_debt_payments: 0,
            savings_streak_days: 0,
            last_event_ms: 0,
            last_trigger_ms: 0,
            monthly_savings_target_cents: None,
            total_savings_this_month_cents: 0,
            total_events: 0,
            recent_savings_timestamps: Vec::with_capacity(16),
            recent_spending_timestamps: Vec::with_capacity(32),
        }
    }

    /// Record a financial event at the given timestamp.
    pub fn record_event(&mut self, event: FinancialEvent, now_ms: u64) {
        self.last_event_ms = now_ms;
        self.total_events = self.total_events.saturating_add(1);

        match event {
            FinancialEvent::SavingsAction { amount_cents, .. } => {
                self.total_savings_this_month_cents = self
                    .total_savings_this_month_cents
                    .saturating_add(amount_cents);
                self.push_savings_timestamp(now_ms);
            }
            FinancialEvent::SpendingAction { .. } => {
                self.push_spending_timestamp(now_ms);
            }
            FinancialEvent::DebtPayment { .. } => {
                self.recent_debt_payments = self.recent_debt_payments.saturating_add(1);
            }
            FinancialEvent::IncomeReceived { .. } => {
                // Income is a positive signal but we don't track amounts.
                // Just noted for potential future scoring.
            }
            FinancialEvent::BudgetGoalSet {
                monthly_savings_target_cents,
            } => {
                self.monthly_savings_target_cents = Some(monthly_savings_target_cents);
            }
        }

        // Recompute rolling window counts from timestamps.
        self.refresh_window_counts(now_ms);
        // Recompute streak.
        self.refresh_streak(now_ms);
    }

    /// Push a savings timestamp into the bounded ring buffer.
    fn push_savings_timestamp(&mut self, ts: u64) {
        if self.recent_savings_timestamps.len() >= Self::MAX_SAVINGS_TS {
            self.recent_savings_timestamps.remove(0);
        }
        self.recent_savings_timestamps.push(ts);
    }

    /// Push a spending timestamp into the bounded ring buffer.
    fn push_spending_timestamp(&mut self, ts: u64) {
        if self.recent_spending_timestamps.len() >= Self::MAX_SPENDING_TS {
            self.recent_spending_timestamps.remove(0);
        }
        self.recent_spending_timestamps.push(ts);
    }

    /// Recompute 30-day window event counts from stored timestamps.
    fn refresh_window_counts(&mut self, now_ms: u64) {
        let window_start = now_ms.saturating_sub(ROLLING_WINDOW_MS);
        self.recent_savings_actions = self
            .recent_savings_timestamps
            .iter()
            .filter(|&&ts| ts >= window_start)
            .count() as u32;
        self.recent_spending_actions = self
            .recent_spending_timestamps
            .iter()
            .filter(|&&ts| ts >= window_start)
            .count() as u32;
    }

    /// Approximate savings streak: count how many of the last N days
    /// (looking back from now) had at least one savings action.
    fn refresh_streak(&mut self, now_ms: u64) {
        if self.recent_savings_timestamps.is_empty() {
            self.savings_streak_days = 0;
            return;
        }

        // Walk back day by day (max 30 days) checking for coverage.
        let mut streak: u32 = 0;
        let one_day = ONE_DAY_MS;

        // We check each day slot from the most recent backwards.
        // A day "has coverage" if any timestamp falls within that 24h window.
        let mut day_end = now_ms;
        for _ in 0..30u32 {
            let day_start = day_end.saturating_sub(one_day);
            let has_action = self
                .recent_savings_timestamps
                .iter()
                .any(|&ts| ts >= day_start && ts < day_end);
            if has_action {
                streak += 1;
                day_end = day_start;
            } else {
                // Break on first gap
                break;
            }
        }
        self.savings_streak_days = streak;
    }

    /// Compute the financial arc score in `[0.0, 1.0]`.
    ///
    /// Day-zero: returns 0.5 when no events recorded.
    #[must_use]
    pub fn score(&self, now_ms: u64) -> f32 {
        if self.total_events == 0 {
            return 0.5; // Day-zero neutral
        }

        let window_start = now_ms.saturating_sub(ROLLING_WINDOW_MS);

        // --- Savings consistency score ---
        // How regularly is the user making savings actions?
        let savings_in_window = self
            .recent_savings_timestamps
            .iter()
            .filter(|&&ts| ts >= window_start)
            .count() as f32;
        let savings_consistency = (savings_in_window / TARGET_SAVINGS_ACTIONS_30D as f32).min(1.0);

        // Streak bonus: >7-day streak adds a small multiplier.
        let streak_bonus = if self.savings_streak_days >= 7 {
            0.1_f32
        } else {
            0.0
        };
        let savings_score = (savings_consistency + streak_bonus).min(1.0);

        // --- Spending discipline score ---
        // Low discipline = very high uncontrolled spending relative to baseline.
        let spending_in_window = self
            .recent_spending_timestamps
            .iter()
            .filter(|&&ts| ts >= window_start)
            .count() as f32;
        let spending_discipline = if spending_in_window <= SPEND_CONCERN_THRESHOLD_30D as f32 {
            1.0
        } else {
            // Linear decay above threshold. At 2x threshold → 0.5.
            let excess_ratio = spending_in_window / SPEND_CONCERN_THRESHOLD_30D as f32;
            (1.0 / excess_ratio).clamp(0.0, 1.0)
        };

        // --- Goal progress score ---
        let goal_progress = match self.monthly_savings_target_cents {
            None => 0.5, // No goal set — neutral, not penalised
            Some(0) => 0.5,
            Some(target) => {
                let actual = self.total_savings_this_month_cents.max(0) as f32;
                (actual / target as f32).clamp(0.0, 1.0)
            }
        };

        // --- Weighted composite ---
        let raw = W_SAVINGS_CONSISTENCY * savings_score
            + W_SPENDING_DISCIPLINE * spending_discipline
            + W_GOAL_PROGRESS * goal_progress;

        raw.clamp(0.0, 1.0)
    }

    /// Recompute and update the health level from the current score.
    pub fn update_health(&mut self, now_ms: u64) {
        let s = self.score(now_ms);
        self.health = ArcHealth::from_score(s);
    }

    /// Check whether a proactive trigger should fire for this arc.
    ///
    /// Conditions:
    /// 1. Enough data has accumulated (day-zero guard).
    /// 2. Health warrants attention (AtRisk or NeedsAttention).
    /// 3. At least 24 hours since the last trigger.
    #[must_use]
    pub fn check_proactive_trigger(&self, now_ms: u64) -> Option<ProactiveTrigger> {
        // Day-zero guard: don't trigger until we have meaningful data.
        if self.total_events < MIN_EVENTS_FOR_TRIGGER {
            return None;
        }

        // Health must warrant attention.
        if !self.health.warrants_proactive() {
            return None;
        }

        // Enforce 24-hour cooldown.
        if self.last_trigger_ms > 0 && now_ms.saturating_sub(self.last_trigger_ms) < ONE_DAY_MS {
            return None;
        }

        Some(ProactiveTrigger {
            arc_type: ArcType::Financial,
            health: self.health.clone(),
            triggered_at_ms: now_ms,
            context_for_llm: self.to_llm_context(),
        })
    }

    /// Acknowledge that a proactive trigger was fired (update deduplication timestamp).
    pub fn mark_trigger_fired(&mut self, now_ms: u64) {
        self.last_trigger_ms = now_ms;
    }

    /// Build a structured factual context string for LLM injection.
    ///
    /// Contains ONLY facts — no pre-packaged advice. The LLM reasons about this.
    #[must_use]
    pub fn to_llm_context(&self) -> String {
        let goal_str = match self.monthly_savings_target_cents {
            None => "no monthly savings goal set".to_string(),
            Some(t) => format!(
                "monthly savings target: {} cents; saved this month: {} cents",
                t, self.total_savings_this_month_cents
            ),
        };

        format!(
            "[financial_arc] health={health} \
             savings_actions_30d={savings} \
             spending_actions_30d={spending} \
             debt_payments_30d={debt} \
             savings_streak_days={streak} \
             {goal} \
             total_events_lifetime={total}",
            health = self.health.label(),
            savings = self.recent_savings_actions,
            spending = self.recent_spending_actions,
            debt = self.recent_debt_payments,
            streak = self.savings_streak_days,
            goal = goal_str,
            total = self.total_events,
        )
    }
}

impl Default for FinancialArc {
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

    const T0: u64 = 1_000_000_000; // arbitrary baseline timestamp (ms)
    const ONE_HOUR_MS: u64 = 3_600_000;

    #[test]
    fn test_day_zero_stable() {
        let arc = FinancialArc::new();
        assert_eq!(arc.health, ArcHealth::Stable);
        assert_eq!(arc.score(T0), 0.5);
        assert_eq!(arc.total_events, 0);
    }

    #[test]
    fn test_no_trigger_day_zero() {
        let arc = FinancialArc::new();
        assert!(arc.check_proactive_trigger(T0).is_none());
    }

    #[test]
    fn test_record_savings_action() {
        let mut arc = FinancialArc::new();
        arc.record_event(
            FinancialEvent::SavingsAction {
                amount_cents: 5000,
                description: "monthly transfer".into(),
            },
            T0,
        );
        assert_eq!(arc.total_events, 1);
        assert_eq!(arc.total_savings_this_month_cents, 5000);
        assert_eq!(arc.recent_savings_actions, 1);
    }

    #[test]
    fn test_score_increases_with_savings() {
        let mut arc = FinancialArc::new();
        // Record enough savings to push score above day-zero 0.5
        for i in 0..TARGET_SAVINGS_ACTIONS_30D {
            arc.record_event(
                FinancialEvent::SavingsAction {
                    amount_cents: 100,
                    description: format!("save_{i}"),
                },
                T0 + (i as u64) * ONE_HOUR_MS,
            );
        }
        let score = arc.score(T0 + TARGET_SAVINGS_ACTIONS_30D as u64 * ONE_HOUR_MS);
        assert!(
            score > 0.5,
            "score should exceed neutral with full savings activity, got {score}"
        );
    }

    #[test]
    fn test_budget_goal_improves_score() {
        let mut arc = FinancialArc::new();
        arc.record_event(
            FinancialEvent::BudgetGoalSet {
                monthly_savings_target_cents: 10_000,
            },
            T0,
        );
        arc.record_event(
            FinancialEvent::SavingsAction {
                amount_cents: 10_000,
                description: "goal met".into(),
            },
            T0 + ONE_HOUR_MS,
        );
        arc.update_health(T0 + ONE_HOUR_MS);
        // With goal fully met and one savings action: score should be above 0.5
        let s = arc.score(T0 + ONE_HOUR_MS);
        assert!(s > 0.5, "goal-met score should exceed neutral, got {s}");
    }

    #[test]
    fn test_trigger_cooldown_respected() {
        let mut arc = FinancialArc::new();
        // Force low health state with many events.
        for i in 0..MIN_EVENTS_FOR_TRIGGER {
            arc.record_event(
                FinancialEvent::SpendingAction {
                    amount_cents: 100,
                    category: "misc".into(),
                },
                T0 + i as u64 * ONE_HOUR_MS,
            );
        }
        // Manually set health to NeedsAttention.
        arc.health = ArcHealth::NeedsAttention;

        // First trigger should fire.
        let t1 = arc.check_proactive_trigger(T0 + 10 * ONE_HOUR_MS);
        assert!(t1.is_some());

        // Mark fired, then check within 24h — should not fire again.
        arc.mark_trigger_fired(T0 + 10 * ONE_HOUR_MS);
        let t2 = arc.check_proactive_trigger(T0 + 15 * ONE_HOUR_MS);
        assert!(t2.is_none(), "trigger should be on cooldown");

        // After 24h, should fire again.
        let t3 = arc.check_proactive_trigger(T0 + 10 * ONE_HOUR_MS + ONE_DAY_MS);
        assert!(t3.is_some(), "trigger should fire after 24h cooldown");
    }

    #[test]
    fn test_to_llm_context_contains_key_fields() {
        let mut arc = FinancialArc::new();
        arc.record_event(
            FinancialEvent::SavingsAction {
                amount_cents: 500,
                description: "test".into(),
            },
            T0,
        );
        let ctx = arc.to_llm_context();
        assert!(ctx.contains("[financial_arc]"), "must have arc tag");
        assert!(
            ctx.contains("savings_actions_30d="),
            "must have savings count"
        );
        assert!(ctx.contains("health="), "must have health label");
    }

    #[test]
    fn test_savings_negative_withdrawal() {
        let mut arc = FinancialArc::new();
        arc.record_event(
            FinancialEvent::SavingsAction {
                amount_cents: 10_000,
                description: "deposit".into(),
            },
            T0,
        );
        arc.record_event(
            FinancialEvent::SavingsAction {
                amount_cents: -3_000,
                description: "partial withdrawal".into(),
            },
            T0 + ONE_HOUR_MS,
        );
        assert_eq!(arc.total_savings_this_month_cents, 7_000);
    }
}
