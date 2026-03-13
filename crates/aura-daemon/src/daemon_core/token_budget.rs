//! Daemon-side token budget management for LLM context window overflow prevention.
//!
//! The neocortex (LLM process) has no visibility into how many tokens it has consumed
//! across a session. The [`TokenBudgetManager`] fills this gap: it tracks per-session
//! token usage in the daemon, signals when compaction is needed, and terminates sessions
//! that have exceeded their budget.
//!
//! Architecture law: **Rust tracks numbers. LLM reasons about meaning.**
//! The daemon never interprets content — it only counts tokens using the
//! `estimate_tokens()` heuristic and fires threshold callbacks.

use aura_types::config::TokenBudgetConfig;

// ─── Token estimation ────────────────────────────────────────────────────────

/// Estimate token count for a text string using a 3.5 chars-per-token heuristic.
///
/// This is a fast, on-device approximation — not a BPE tokenizer.
/// Suitable for budget tracking; not suitable for exact context window sizing.
#[must_use]
pub fn estimate_tokens(text: &str) -> u32 {
    (text.len() as f32 / 3.5).ceil() as u32
}

// ─── BudgetStatus ────────────────────────────────────────────────────────────

/// Current health of the token budget.
#[derive(Debug, Clone, PartialEq)]
pub enum BudgetStatus {
    /// Budget is healthy — plenty of tokens remaining.
    Healthy {
        /// Tokens available for the next call.
        available: u32,
        /// Fraction of session limit consumed (0.0–1.0).
        used_pct: f32,
    },
    /// Budget is approaching the compaction threshold. Summarization is advisable
    /// but not yet mandatory.
    Warning {
        available: u32,
        used_pct: f32,
        message: &'static str,
    },
    /// Budget has crossed the force-compaction threshold. The session MUST
    /// summarize before the next LLM call.
    Critical { available: u32, used_pct: f32 },
    /// Budget is fully exhausted. The session must be reset or abandoned.
    Exhausted,
}

// ─── TokenBudgetSnapshot ─────────────────────────────────────────────────────

/// A point-in-time view of the budget state — useful for logging and tests.
#[derive(Debug, Clone)]
pub struct TokenBudgetSnapshot {
    /// Tokens consumed in the current session.
    pub session_used: u32,
    /// Configured session token limit.
    pub session_limit: u32,
    /// Fraction consumed (0.0–1.0).
    pub used_pct: f32,
    /// Number of LLM calls recorded in this session.
    pub calls_in_session: u32,
    /// Average tokens per call this session (0 if no calls yet).
    pub avg_tokens_per_call: u32,
}

// ─── TokenBudgetManager ──────────────────────────────────────────────────────

/// Tracks daemon-side per-session token consumption.
///
/// One manager per agentic session. Created at session start, consulted
/// before every LLM IPC call, updated after every LLM response.
#[derive(Debug, Clone)]
pub struct TokenBudgetManager {
    // ── Per-session state ──
    /// Tokens consumed so far in this session.
    session_tokens_used: u32,
    /// Maximum tokens allowed in one session (from config).
    session_tokens_limit: u32,
    /// Tokens reserved for the LLM response — excluded from planning budget.
    response_reserve: u32,
    /// Token count from the most recent call.
    last_call_tokens: u32,
    /// Peak tokens-used in any single call this session.
    high_watermark: u32,
    /// Number of LLM calls in this session.
    calls_in_session: u32,

    // ── Thresholds (from config) ──
    /// Fraction at which `BudgetStatus::Warning` fires and `should_summarize()` returns true.
    compaction_threshold: f32,
    /// Fraction at which `BudgetStatus::Critical` fires and `must_summarize()` returns true.
    force_compaction_threshold: f32,

    // ── Lifetime counters (survive `reset_session`) ──
    /// Total tokens consumed across all sessions since daemon start.
    pub total_tokens_all_time: u64,
    /// Total LLM calls across all sessions since daemon start.
    pub total_calls: u64,
}

impl TokenBudgetManager {
    /// Create a new manager from configuration.
    pub fn new(config: TokenBudgetConfig) -> Self {
        Self {
            session_tokens_used: 0,
            session_tokens_limit: config.session_limit,
            response_reserve: config.response_reserve,
            last_call_tokens: 0,
            high_watermark: 0,
            calls_in_session: 0,
            compaction_threshold: config.compaction_threshold,
            force_compaction_threshold: config.force_compaction_threshold,
            total_tokens_all_time: 0,
            total_calls: 0,
        }
    }

    // ── Budget queries ────────────────────────────────────────────────────────

    /// Returns the number of tokens available for planning (excluding response reserve).
    ///
    /// Returns 0 if consumption already exceeds the planning budget.
    #[must_use]
    pub fn available_tokens(&self) -> u32 {
        let planning_budget = self.session_tokens_limit.saturating_sub(self.response_reserve);
        planning_budget.saturating_sub(self.session_tokens_used)
    }

    /// Current fraction of the session limit consumed (0.0–1.0, capped at 1.0).
    #[must_use]
    fn used_pct(&self) -> f32 {
        if self.session_tokens_limit == 0 {
            return 1.0;
        }
        (self.session_tokens_used as f32 / self.session_tokens_limit as f32).min(1.0)
    }

    /// Evaluate current budget health and return the appropriate status.
    #[must_use]
    pub fn check_budget(&self) -> BudgetStatus {
        let pct = self.used_pct();
        let available = self.available_tokens();

        if self.session_tokens_used >= self.session_tokens_limit {
            return BudgetStatus::Exhausted;
        }

        if pct >= self.force_compaction_threshold {
            return BudgetStatus::Critical { available, used_pct: pct };
        }

        if pct >= self.compaction_threshold {
            return BudgetStatus::Warning {
                available,
                used_pct: pct,
                message: "approaching context limit — summarization recommended",
            };
        }

        BudgetStatus::Healthy { available, used_pct: pct }
    }

    /// Returns `true` if the budget has crossed the compaction threshold.
    ///
    /// Summarization is advisable but not yet mandatory.
    #[must_use]
    pub fn should_summarize(&self) -> bool {
        let pct = self.used_pct();
        pct >= self.compaction_threshold
    }

    /// Returns `true` if the budget has crossed the force-compaction threshold.
    ///
    /// The session MUST summarize before the next LLM call.
    #[must_use]
    pub fn must_summarize(&self) -> bool {
        let pct = self.used_pct();
        pct >= self.force_compaction_threshold
    }

    // ── Mutation ──────────────────────────────────────────────────────────────

    /// Record token usage from a completed LLM call.
    ///
    /// If `tokens_used` is 0 (i.e. the IPC response does not carry a count),
    /// a no-op is performed — the budget is not updated.
    pub fn record_usage(&mut self, tokens_used: u32) {
        if tokens_used == 0 {
            return;
        }
        self.session_tokens_used = self.session_tokens_used.saturating_add(tokens_used);
        self.last_call_tokens = tokens_used;
        if tokens_used > self.high_watermark {
            self.high_watermark = tokens_used;
        }
        self.calls_in_session += 1;
        self.total_tokens_all_time += tokens_used as u64;
        self.total_calls += 1;
    }

    /// Reset per-session counters while preserving lifetime statistics.
    ///
    /// Called after a successful context compaction (Summarize → Summary round-trip).
    pub fn reset_session(&mut self) {
        self.session_tokens_used = 0;
        self.last_call_tokens = 0;
        self.high_watermark = 0;
        self.calls_in_session = 0;
        // total_tokens_all_time and total_calls are preserved.
    }

    // ── Observability ─────────────────────────────────────────────────────────

    /// Capture a point-in-time snapshot of the budget state.
    #[must_use]
    pub fn snapshot(&self) -> TokenBudgetSnapshot {
        let avg = if self.calls_in_session == 0 {
            0
        } else {
            self.session_tokens_used / self.calls_in_session
        };
        TokenBudgetSnapshot {
            session_used: self.session_tokens_used,
            session_limit: self.session_tokens_limit,
            used_pct: self.used_pct(),
            calls_in_session: self.calls_in_session,
            avg_tokens_per_call: avg,
        }
    }
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use aura_types::config::TokenBudgetConfig;

    fn make_manager() -> TokenBudgetManager {
        TokenBudgetManager::new(TokenBudgetConfig::default())
    }

    #[test]
    fn test_budget_healthy() {
        let mgr = make_manager();
        let status = mgr.check_budget();
        assert!(
            matches!(status, BudgetStatus::Healthy { .. }),
            "fresh manager must be Healthy, got {status:?}"
        );
        assert!(!mgr.should_summarize());
        assert!(!mgr.must_summarize());
    }

    #[test]
    fn test_budget_warning() {
        let mut mgr = make_manager();
        // Fill to 80% (past 0.75 compaction threshold, before 0.90 force threshold).
        // Default session_limit = 2048, response_reserve = 512 → limit = 2048
        // 80% of 2048 = 1638 tokens
        mgr.record_usage(1638);
        let status = mgr.check_budget();
        assert!(
            matches!(status, BudgetStatus::Warning { .. }),
            "80% usage must yield Warning, got {status:?}"
        );
        assert!(mgr.should_summarize(), "should_summarize must be true at 80%");
        assert!(!mgr.must_summarize(), "must_summarize must be false at 80%");
    }

    #[test]
    fn test_budget_critical() {
        let mut mgr = make_manager();
        // Fill to 95% (past 0.90 force_compaction threshold).
        // 95% of 2048 ≈ 1945 tokens
        mgr.record_usage(1945);
        let status = mgr.check_budget();
        assert!(
            matches!(status, BudgetStatus::Critical { .. }),
            "95% usage must yield Critical, got {status:?}"
        );
        assert!(mgr.should_summarize());
        assert!(mgr.must_summarize(), "must_summarize must be true at 95%");
    }

    #[test]
    fn test_budget_exhausted() {
        let mut mgr = make_manager();
        // Exceed the session limit entirely.
        mgr.record_usage(2049);
        let status = mgr.check_budget();
        assert!(
            matches!(status, BudgetStatus::Exhausted),
            "over-limit usage must yield Exhausted, got {status:?}"
        );
    }

    #[test]
    fn test_reset_session_preserves_lifetime_stats() {
        let mut mgr = make_manager();
        mgr.record_usage(500);
        mgr.record_usage(300);
        assert_eq!(mgr.total_tokens_all_time, 800);
        assert_eq!(mgr.total_calls, 2);

        mgr.reset_session();
        assert_eq!(mgr.session_tokens_used, 0, "session_used must reset");
        assert_eq!(mgr.calls_in_session, 0, "calls_in_session must reset");
        // Lifetime counters survive the reset.
        assert_eq!(mgr.total_tokens_all_time, 800);
        assert_eq!(mgr.total_calls, 2);

        let status = mgr.check_budget();
        assert!(matches!(status, BudgetStatus::Healthy { .. }));
    }

    #[test]
    fn test_estimate_tokens() {
        // Empty string → 0 tokens.
        assert_eq!(estimate_tokens(""), 0);

        // 35-char string → ceil(35 / 3.5) = 10.
        assert_eq!(estimate_tokens(&"a".repeat(35)), 10);

        // 1 char → ceil(1 / 3.5) = 1.
        assert_eq!(estimate_tokens("x"), 1);

        // 7 chars → ceil(7 / 3.5) = 2.
        assert_eq!(estimate_tokens("abcdefg"), 2);
    }

    #[test]
    fn test_available_tokens_decreases_with_usage() {
        let mut mgr = make_manager();
        // planning budget = session_limit - response_reserve = 2048 - 512 = 1536
        let initial = mgr.available_tokens();
        assert_eq!(initial, 1536);

        mgr.record_usage(100);
        assert_eq!(mgr.available_tokens(), 1436);
    }

    #[test]
    fn test_snapshot_avg_tokens() {
        let mut mgr = make_manager();
        mgr.record_usage(200);
        mgr.record_usage(400);
        let snap = mgr.snapshot();
        assert_eq!(snap.calls_in_session, 2);
        assert_eq!(snap.session_used, 600);
        assert_eq!(snap.avg_tokens_per_call, 300);
    }
}
