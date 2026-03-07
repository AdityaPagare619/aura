use std::collections::VecDeque;

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const RING_SIZE: usize = 20;
const BLOCK_THRESHOLD: f32 = 0.40;
const WARN_THRESHOLD: f32 = 0.25;
const MAX_REGENERATIONS: u8 = 3;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Record of a single response's sycophantic indicators.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct ResponseRecord {
    /// Did AURA agree with the user?
    pub agreed: bool,
    /// Did AURA use hedging language ("maybe", "I think", "perhaps")?
    pub hedged: bool,
    /// Did AURA reverse a previously-stated opinion?
    pub reversed_opinion: bool,
    /// Did AURA offer unprompted praise?
    pub praised: bool,
    /// Did AURA challenge/push back on the user?
    pub challenged: bool,
}

/// Breakdown of the five sycophancy sub-scores.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SycophancyScore {
    pub agreement_ratio: f32,
    pub hedging_frequency: f32,
    pub opinion_reversal: f32,
    pub praise_density: f32,
    pub challenge_avoidance: f32,
    /// Arithmetic mean of the five components.
    pub composite: f32,
}

/// The guard's verdict on the current sycophancy level.
#[derive(Debug, Clone, PartialEq)]
pub enum SycophancyVerdict {
    /// Composite ≤ 0.25 — safe.
    Ok,
    /// 0.25 < composite ≤ 0.40 — concerning.
    Warn(f32),
    /// Composite > 0.40 — block and request regeneration.
    Block(f32),
}

/// Pipeline-facing gate result that wraps the internal verdict into
/// actionable directives for the response pipeline.
#[derive(Debug, Clone, PartialEq)]
pub enum GateResult {
    /// Response passes the sycophancy check.
    Pass,
    /// Response is concerning — add an honesty nudge directive.
    Nudge { reason: String },
    /// Response is blocked — must regenerate with honesty directive.
    Block { reason: String },
}

/// Guard that tracks recent response patterns and detects sycophantic
/// behaviour over a sliding window of the last 20 responses.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SycophancyGuard {
    ring: VecDeque<ResponseRecord>,
    /// How many times the guard has requested regeneration for the
    /// *current* conversation turn. Resets when a non-blocked response
    /// is recorded.
    regeneration_count: u8,
}

// ---------------------------------------------------------------------------
// Implementation
// ---------------------------------------------------------------------------

impl SycophancyGuard {
    pub fn new() -> Self {
        Self {
            ring: VecDeque::with_capacity(RING_SIZE),
            regeneration_count: 0,
        }
    }

    /// Push a new response record into the sliding window.
    pub fn record_response(&mut self, record: ResponseRecord) {
        if self.ring.len() >= RING_SIZE {
            self.ring.pop_front();
        }
        self.ring.push_back(record);
        // Reset regeneration counter on any accepted record.
        self.regeneration_count = 0;
    }

    /// Evaluate the current sycophancy score across all five dimensions.
    pub fn evaluate(&self) -> SycophancyVerdict {
        let score = self.score();

        if score.composite > BLOCK_THRESHOLD {
            SycophancyVerdict::Block(score.composite)
        } else if score.composite > WARN_THRESHOLD {
            SycophancyVerdict::Warn(score.composite)
        } else {
            SycophancyVerdict::Ok
        }
    }

    /// Compute the detailed [`SycophancyScore`].
    pub fn score(&self) -> SycophancyScore {
        let total = self.ring.len() as f32;
        if total == 0.0 {
            return SycophancyScore {
                agreement_ratio: 0.0,
                hedging_frequency: 0.0,
                opinion_reversal: 0.0,
                praise_density: 0.0,
                challenge_avoidance: 0.0,
                composite: 0.0,
            };
        }

        let agreed = self.ring.iter().filter(|r| r.agreed).count() as f32;
        let hedged = self.ring.iter().filter(|r| r.hedged).count() as f32;
        let reversed = self.ring.iter().filter(|r| r.reversed_opinion).count() as f32;
        let praised = self.ring.iter().filter(|r| r.praised).count() as f32;
        let challenged = self.ring.iter().filter(|r| r.challenged).count() as f32;

        let agreement_ratio = agreed / total;
        let hedging_frequency = hedged / total;
        let opinion_reversal = reversed / total;
        let praise_density = praised / total;
        let challenge_avoidance = 1.0 - (challenged / total);

        let composite = (agreement_ratio
            + hedging_frequency
            + opinion_reversal
            + praise_density
            + challenge_avoidance)
            / 5.0;

        SycophancyScore {
            agreement_ratio,
            hedging_frequency,
            opinion_reversal,
            praise_density,
            challenge_avoidance,
            composite,
        }
    }

    /// Ask whether the response should be regenerated.
    ///
    /// Returns `true` if regeneration count < MAX (3). Increments the
    /// internal counter. After 3 failed regenerations the guard gives up
    /// and force-allows the response (with a warning log).
    pub fn should_regenerate(&mut self) -> bool {
        if self.regeneration_count < MAX_REGENERATIONS {
            self.regeneration_count += 1;
            tracing::warn!(
                attempt = self.regeneration_count,
                max = MAX_REGENERATIONS,
                "sycophancy guard requesting regeneration"
            );
            true
        } else {
            tracing::warn!("sycophancy guard: max regenerations reached — force-allowing");
            false
        }
    }

    /// Current regeneration attempt count.
    pub fn regeneration_count(&self) -> u8 {
        self.regeneration_count
    }

    /// Pipeline-facing gate that combines `evaluate()` and `should_regenerate()`
    /// into a single actionable result.
    ///
    /// - `Ok` verdict → `GateResult::Pass`
    /// - `Warn` verdict → `GateResult::Nudge`
    /// - `Block` verdict + can regenerate → `GateResult::Block`
    /// - `Block` verdict + max retries exhausted → `GateResult::Nudge` (downgrade)
    #[tracing::instrument(skip(self))]
    pub fn gate(&mut self) -> GateResult {
        let verdict = self.evaluate();
        match verdict {
            SycophancyVerdict::Ok => GateResult::Pass,
            SycophancyVerdict::Warn(score) => GateResult::Nudge {
                reason: format!("sycophancy score {:.2} exceeds warn threshold", score),
            },
            SycophancyVerdict::Block(score) => {
                if self.should_regenerate() {
                    GateResult::Block {
                        reason: format!(
                            "sycophancy score {:.2} exceeds block threshold (attempt {}/{})",
                            score, self.regeneration_count, MAX_REGENERATIONS
                        ),
                    }
                } else {
                    // Max retries exhausted — downgrade to nudge
                    tracing::warn!(
                        score,
                        "sycophancy block downgraded to nudge — max regenerations exhausted"
                    );
                    GateResult::Nudge {
                        reason: format!(
                            "sycophancy score {:.2} — regeneration limit reached, adding honesty nudge",
                            score
                        ),
                    }
                }
            }
        }
    }
}

impl Default for SycophancyGuard {
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

    /// Helper: build a "clean" (non-sycophantic) response record.
    fn clean_record() -> ResponseRecord {
        ResponseRecord {
            agreed: false,
            hedged: false,
            reversed_opinion: false,
            praised: false,
            challenged: true,
        }
    }

    /// Helper: build a maximally sycophantic response record.
    fn sycophantic_record() -> ResponseRecord {
        ResponseRecord {
            agreed: true,
            hedged: true,
            reversed_opinion: true,
            praised: true,
            challenged: false,
        }
    }

    #[test]
    fn test_empty_guard_is_ok() {
        let guard = SycophancyGuard::new();
        assert_eq!(guard.evaluate(), SycophancyVerdict::Ok);
    }

    #[test]
    fn test_clean_responses_pass() {
        let mut guard = SycophancyGuard::new();
        for _ in 0..20 {
            guard.record_response(clean_record());
        }
        let v = guard.evaluate();
        assert_eq!(v, SycophancyVerdict::Ok);
    }

    #[test]
    fn test_sycophantic_responses_blocked() {
        let mut guard = SycophancyGuard::new();
        for _ in 0..20 {
            guard.record_response(sycophantic_record());
        }
        let v = guard.evaluate();
        match v {
            SycophancyVerdict::Block(score) => {
                // All 5 components maxed: (1+1+1+1+1)/5 = 1.0
                assert!((score - 1.0).abs() < f32::EPSILON, "score={}", score);
            }
            other => panic!("expected Block, got {:?}", other),
        }
    }

    #[test]
    fn test_regeneration_limit() {
        let mut guard = SycophancyGuard::new();
        assert!(guard.should_regenerate()); // 1
        assert!(guard.should_regenerate()); // 2
        assert!(guard.should_regenerate()); // 3
        assert!(!guard.should_regenerate()); // force-allow
    }

    #[test]
    fn test_recording_resets_regeneration_counter() {
        let mut guard = SycophancyGuard::new();
        guard.should_regenerate();
        guard.should_regenerate();
        assert_eq!(guard.regeneration_count(), 2);

        guard.record_response(clean_record());
        assert_eq!(guard.regeneration_count(), 0);
    }

    #[test]
    fn test_ring_buffer_evicts_oldest() {
        let mut guard = SycophancyGuard::new();

        // Fill with sycophantic records.
        for _ in 0..20 {
            guard.record_response(sycophantic_record());
        }

        // Now replace all with clean records.
        for _ in 0..20 {
            guard.record_response(clean_record());
        }

        assert_eq!(guard.evaluate(), SycophancyVerdict::Ok);
    }

    #[test]
    fn test_mixed_responses_warn() {
        let mut guard = SycophancyGuard::new();

        // 10 clean + 10 sycophantic → ~50% sycophantic
        for _ in 0..10 {
            guard.record_response(clean_record());
        }
        for _ in 0..10 {
            guard.record_response(sycophantic_record());
        }

        let v = guard.evaluate();
        // Should be Warn or Block depending on exact composite
        assert!(
            matches!(v, SycophancyVerdict::Warn(_) | SycophancyVerdict::Block(_)),
            "expected Warn or Block, got {:?}",
            v
        );
    }

    #[test]
    fn test_gate_clean_passes() {
        let mut guard = SycophancyGuard::new();
        for _ in 0..10 {
            guard.record_response(clean_record());
        }
        assert_eq!(guard.gate(), GateResult::Pass);
    }

    #[test]
    fn test_gate_sycophantic_blocks_then_downgrades() {
        let mut guard = SycophancyGuard::new();
        for _ in 0..20 {
            guard.record_response(sycophantic_record());
        }

        // First 3 calls should Block (requesting regeneration)
        for i in 1..=3 {
            let result = guard.gate();
            assert!(
                matches!(result, GateResult::Block { .. }),
                "attempt {} should block",
                i
            );
        }

        // 4th call — max retries exhausted, downgrade to Nudge
        let result = guard.gate();
        assert!(
            matches!(result, GateResult::Nudge { .. }),
            "should downgrade to nudge after max retries"
        );
    }
}
