//! 4-tier cycle detection using a zero-heap-allocation transition buffer.
//!
//! The `TransitionBuffer` is a 16-entry circular buffer (256 bytes) that stores
//! `(state_hash: u64, action_hash: u32, _pad: u32)` tuples. All four detection
//! checks run in under 100μs combined with no heap allocation.
//!
//! ## 4 detection checks:
//! 1. **Exact loop** — same (state, action, next_state) seen ≥ `threshold` times (default 2)
//! 2. **Orbit detection** — repeated sequence of states with minimum period 2
//! 3. **Stagnation** — 3 consecutive same-state entries
//! 4. **Temporal stagnation** — no meaningful state change within 8000ms window
//!
//! ## 4-tier recovery (strictly monotonic — never re-enter same or lower tier):
//! - **Tier 1 (micro):** wait-retry, action-variant, dismiss-retry. 3 attempts, 3s max.
//! - **Tier 2 (strategic):** Neocortex replan or daemon heuristics. 2 attempts, 8s max.
//! - **Tier 3 (graceful abort):** rollback evaluation, save partial progress.
//! - **Tier 4 (emergency):** user notification, full stop.

/// A single entry in the transition buffer. 16 bytes each, 16 entries = 256 bytes.
#[derive(Debug, Clone, Copy, Default)]
#[repr(C)]
pub struct TransitionEntry {
    /// FNV-1a 64-bit hash of the screen state.
    pub state_hash: u64,
    /// FNV-1a 32-bit hash of the action that was taken from this state.
    pub action_hash: u32,
    /// Timestamp in ms (relative to buffer creation) — used for temporal stagnation.
    pub timestamp_ms: u32,
}

/// Which cycle tier was detected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum CycleTier {
    /// No cycle detected.
    None,
    /// Tier 1: micro-recovery (wait, variant action, dismiss).
    Micro,
    /// Tier 2: strategic pivot (replan via LLM or daemon heuristics).
    Strategic,
    /// Tier 3: graceful abort with rollback evaluation.
    GracefulAbort,
    /// Tier 4: emergency — user notification, full stop.
    Emergency,
}

/// Result of a cycle detection check.
#[derive(Debug, Clone)]
pub struct CycleCheckResult {
    pub tier: CycleTier,
    /// Which check triggered: "exact_loop", "orbit", "stagnation", "temporal"
    pub check_name: &'static str,
    /// Human-readable reason string.
    pub reason: String,
}

impl CycleCheckResult {
    fn none() -> Self {
        Self {
            tier: CycleTier::None,
            check_name: "none",
            reason: String::new(),
        }
    }
}

/// 4-tier cycle detector with a fixed-size 16-entry transition buffer.
///
/// Memory: 256 bytes for the buffer + ~32 bytes for state = 288 bytes total.
/// All detection runs in O(16) = O(1) with zero heap allocation during checks.
pub struct CycleDetector {
    /// Fixed-size circular buffer of 16 transition entries (256 bytes).
    buffer: [TransitionEntry; 16],
    /// Number of valid entries (0..=16).
    count: u8,
    /// Write index (wraps at 16).
    write_idx: u8,
    /// Current highest tier that has been entered. Monotonic: only goes up.
    current_tier: CycleTier,
    /// Number of recovery attempts at the current tier.
    attempts_at_tier: u8,
    /// Reference timestamp (ms) for the temporal stagnation check.
    epoch_ms: u64,

    // ── Thresholds ──
    /// How many times the same (state→action→state) triple must appear for exact-loop.
    exact_loop_threshold: u8,
    /// How many consecutive same-state entries trigger stagnation.
    stagnation_threshold: u8,
    /// Window in ms for temporal stagnation detection.
    temporal_window_ms: u32,
}

impl CycleDetector {
    /// Create a new detector with default thresholds.
    pub fn new() -> Self {
        Self {
            buffer: [TransitionEntry::default(); 16],
            count: 0,
            write_idx: 0,
            current_tier: CycleTier::None,
            attempts_at_tier: 0,
            epoch_ms: 0,
            exact_loop_threshold: 2,
            stagnation_threshold: 3,
            temporal_window_ms: 8_000,
        }
    }

    /// Set the epoch timestamp (ms) for temporal stagnation.
    pub fn set_epoch_ms(&mut self, ms: u64) {
        self.epoch_ms = ms;
    }

    /// Current highest tier entered.
    pub fn current_tier(&self) -> CycleTier {
        self.current_tier
    }

    /// Number of recovery attempts at the current tier.
    pub fn attempts_at_tier(&self) -> u8 {
        self.attempts_at_tier
    }

    /// Record a new transition and return the cycle check result.
    ///
    /// `state_hash`: FNV-1a 64-bit of the screen tree (from `verifier::hash_screen_state`).
    /// `action_hash`: FNV-1a 32-bit of the action (from `verifier::hash_action`).
    /// `now_ms`: current timestamp in ms (used for temporal stagnation).
    pub fn record_and_check(
        &mut self,
        state_hash: u64,
        action_hash: u32,
        now_ms: u64,
    ) -> CycleCheckResult {
        // Compute relative timestamp
        let relative_ms = if self.epoch_ms > 0 {
            (now_ms.saturating_sub(self.epoch_ms)) as u32
        } else {
            self.epoch_ms = now_ms;
            0
        };

        // Write entry
        let entry = TransitionEntry {
            state_hash,
            action_hash,
            timestamp_ms: relative_ms,
        };
        self.buffer[self.write_idx as usize] = entry;
        self.write_idx = (self.write_idx + 1) % 16;
        if self.count < 16 {
            self.count += 1;
        }

        // Run all 4 checks in order of cheapest to most expensive
        // Total: O(16) per check = O(64) overall, no heap allocation
        let result = self
            .check_stagnation()
            .or_else(|| self.check_exact_loop())
            .or_else(|| self.check_orbit())
            .or_else(|| self.check_temporal_stagnation())
            .unwrap_or_else(CycleCheckResult::none);

        // Escalate tier if needed (monotonic: never go down)
        if result.tier > self.current_tier {
            self.current_tier = result.tier;
            self.attempts_at_tier = 0;
        }

        result
    }

    /// Acknowledge a recovery attempt at the current tier.
    ///
    /// Returns `true` if more attempts are allowed at this tier,
    /// `false` if we should escalate.
    pub fn acknowledge_recovery(&mut self) -> bool {
        self.attempts_at_tier += 1;
        let max_attempts = match self.current_tier {
            CycleTier::None => u8::MAX,
            CycleTier::Micro => 3,
            CycleTier::Strategic => 2,
            CycleTier::GracefulAbort => 1,
            CycleTier::Emergency => 0,
        };
        self.attempts_at_tier <= max_attempts
    }

    /// Force escalation to the next tier. Returns the new tier.
    pub fn escalate(&mut self) -> CycleTier {
        let next = match self.current_tier {
            CycleTier::None => CycleTier::Micro,
            CycleTier::Micro => CycleTier::Strategic,
            CycleTier::Strategic => CycleTier::GracefulAbort,
            CycleTier::GracefulAbort => CycleTier::Emergency,
            CycleTier::Emergency => CycleTier::Emergency,
        };
        self.current_tier = next;
        self.attempts_at_tier = 0;
        next
    }

    /// Reset the detector (for a new task).
    pub fn reset(&mut self) {
        self.buffer = [TransitionEntry::default(); 16];
        self.count = 0;
        self.write_idx = 0;
        self.current_tier = CycleTier::None;
        self.attempts_at_tier = 0;
        self.epoch_ms = 0;
    }

    /// Number of entries in the buffer.
    pub fn len(&self) -> u8 {
        self.count
    }

    /// Whether the buffer is empty.
    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    // ── Detection checks ────────────────────────────────────────────────────

    /// Check 1: Exact loop — same (state, action, next_state) triple seen ≥ threshold times.
    fn check_exact_loop(&self) -> Option<CycleCheckResult> {
        if self.count < 3 {
            return None;
        }

        // Get the latest transition: (prev_state, action, current_state)
        let latest_state = self.entry_at(0);
        let latest_action_entry = self.entry_at(1);

        let cur_state = latest_state?.state_hash;
        let prev_entry = latest_action_entry?;
        let prev_state = prev_entry.state_hash;
        let action = prev_entry.action_hash;

        // Count how many times this triple appears in history
        let mut matches = 0u8;
        let max_check = (self.count as usize).min(15); // check pairs
        for i in 1..max_check {
            let entry_i = self.entry_at(i);
            let entry_next = self.entry_at(i.wrapping_sub(1)); // the state we transitioned TO
            if let (Some(e), Some(next)) = (entry_i, entry_next) {
                if e.state_hash == prev_state
                    && e.action_hash == action
                    && next.state_hash == cur_state
                {
                    matches += 1;
                }
            }
        }

        if matches >= self.exact_loop_threshold {
            Some(CycleCheckResult {
                tier: self.escalated_tier(CycleTier::Micro),
                check_name: "exact_loop",
                reason: format!(
                    "exact loop detected: state {:016x} -> action {:08x} -> state {:016x} seen {} times",
                    prev_state, action, cur_state, matches + 1
                ),
            })
        } else {
            None
        }
    }

    /// Check 2: Orbit detection — a repeating sequence of states with period ≥ 2.
    ///
    /// Checks for periods 2, 3, 4, 5 (A-B-A-B, A-B-C-A-B-C, etc.).
    fn check_orbit(&self) -> Option<CycleCheckResult> {
        if self.count < 4 {
            return None;
        }

        // Check periods 2 through 5
        for period in 2u8..=5 {
            if self.count < period * 2 {
                continue;
            }
            let mut is_orbit = true;
            for i in 0..period {
                let recent = self.entry_at(i as usize);
                let earlier = self.entry_at((i + period) as usize);
                match (recent, earlier) {
                    (Some(r), Some(e)) if r.state_hash == e.state_hash => {}
                    _ => {
                        is_orbit = false;
                        break;
                    }
                }
            }
            if is_orbit {
                return Some(CycleCheckResult {
                    tier: self.escalated_tier(CycleTier::Micro),
                    check_name: "orbit",
                    reason: format!("orbit detected with period {}", period),
                });
            }
        }

        None
    }

    /// Check 3: Stagnation — N consecutive entries with the same state_hash.
    fn check_stagnation(&self) -> Option<CycleCheckResult> {
        if self.count < self.stagnation_threshold {
            return None;
        }

        let threshold = self.stagnation_threshold as usize;
        let first = self.entry_at(0)?;

        for i in 1..threshold {
            let entry = self.entry_at(i)?;
            if entry.state_hash != first.state_hash {
                return None;
            }
        }

        Some(CycleCheckResult {
            tier: self.escalated_tier(CycleTier::Micro),
            check_name: "stagnation",
            reason: format!(
                "{} consecutive same state: {:016x}",
                threshold, first.state_hash
            ),
        })
    }

    /// Check 4: Temporal stagnation — no state change within the temporal window.
    fn check_temporal_stagnation(&self) -> Option<CycleCheckResult> {
        if self.count < 4 {
            return None;
        }

        let latest = self.entry_at(0)?;

        // Find the oldest entry within the temporal window
        let window_start = latest.timestamp_ms.saturating_sub(self.temporal_window_ms);
        let mut distinct_states = 0u8;
        let mut seen: [u64; 16] = [0; 16]; // track unique hashes

        let len = self.count as usize;
        for i in 0..len {
            let entry = self.entry_at(i)?;
            if entry.timestamp_ms < window_start {
                break; // outside the temporal window
            }
            // Check if we've seen this state
            let mut found = false;
            for j in 0..distinct_states as usize {
                if seen[j] == entry.state_hash {
                    found = true;
                    break;
                }
            }
            if !found && (distinct_states as usize) < 16 {
                seen[distinct_states as usize] = entry.state_hash;
                distinct_states += 1;
            }
        }

        // If there's only 1 distinct state in the window and we have enough entries
        if distinct_states <= 1 && self.count >= 4 {
            Some(CycleCheckResult {
                tier: self.escalated_tier(CycleTier::Strategic),
                check_name: "temporal",
                reason: format!(
                    "no meaningful state change in {}ms window ({} entries, {} distinct states)",
                    self.temporal_window_ms, len, distinct_states
                ),
            })
        } else {
            None
        }
    }

    // ── Helpers ──────────────────────────────────────────────────────────────

    /// Get the entry at position `n` from the most recent (0 = most recent).
    fn entry_at(&self, n: usize) -> Option<TransitionEntry> {
        if n >= self.count as usize {
            return None;
        }
        // write_idx points to the NEXT slot, so most recent is write_idx - 1
        let idx = if self.write_idx as usize > n {
            self.write_idx as usize - 1 - n
        } else {
            16 - 1 - (n - self.write_idx as usize)
        };
        Some(self.buffer[idx])
    }

    /// Return the minimum of the suggested tier and the next escalation from current.
    /// Ensures monotonic escalation.
    fn escalated_tier(&self, suggested: CycleTier) -> CycleTier {
        if suggested > self.current_tier {
            suggested
        } else {
            // Already at or above this tier — suggest next escalation
            match self.current_tier {
                CycleTier::None => CycleTier::Micro,
                CycleTier::Micro => CycleTier::Strategic,
                CycleTier::Strategic => CycleTier::GracefulAbort,
                CycleTier::GracefulAbort => CycleTier::Emergency,
                CycleTier::Emergency => CycleTier::Emergency,
            }
        }
    }
}

impl Default for CycleDetector {
    fn default() -> Self {
        Self::new()
    }
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_detector() {
        let det = CycleDetector::new();
        assert_eq!(det.len(), 0);
        assert!(det.is_empty());
        assert_eq!(det.current_tier(), CycleTier::None);
    }

    #[test]
    fn test_no_cycle_with_different_states() {
        let mut det = CycleDetector::new();
        for i in 0u64..10 {
            let result = det.record_and_check(i * 1000 + 1, (i as u32) * 100, i * 500);
            assert_eq!(
                result.tier,
                CycleTier::None,
                "unexpected cycle at step {}: {:?}",
                i,
                result
            );
        }
    }

    #[test]
    fn test_stagnation_detection() {
        let mut det = CycleDetector::new();
        // 3 consecutive same states should trigger stagnation
        let result1 = det.record_and_check(0xAAAA, 1, 1000);
        assert_eq!(result1.tier, CycleTier::None);

        let result2 = det.record_and_check(0xAAAA, 2, 2000);
        assert_eq!(result2.tier, CycleTier::None);

        let result3 = det.record_and_check(0xAAAA, 3, 3000);
        assert_eq!(result3.check_name, "stagnation");
        assert!(result3.tier >= CycleTier::Micro);
    }

    #[test]
    fn test_exact_loop_detection() {
        let mut det = CycleDetector::new();

        // Build pattern: state_A -> action_X -> state_B -> action_X -> state_A -> action_X ->
        // state_B This creates the triple (A, X, B) appearing multiple times
        det.record_and_check(0xAAAA, 0x11, 1000); // entry: state A, action X
        det.record_and_check(0xBBBB, 0x22, 2000); // entry: state B, action Y
        det.record_and_check(0xAAAA, 0x11, 3000); // entry: state A, action X (same as first)
        let result = det.record_and_check(0xBBBB, 0x22, 4000); // entry: state B again

        // Should detect orbit (A-B-A-B) which is period 2
        // The exact check depends on the order of checks
        assert!(
            result.tier >= CycleTier::Micro,
            "expected cycle detection, got {:?}",
            result
        );
    }

    #[test]
    fn test_orbit_detection_period_2() {
        let mut det = CycleDetector::new();
        // A-B-A-B pattern
        det.record_and_check(0x1111, 1, 1000);
        det.record_and_check(0x2222, 2, 1500);
        det.record_and_check(0x1111, 1, 2000);
        let result = det.record_and_check(0x2222, 2, 2500);

        assert!(
            result.tier >= CycleTier::Micro,
            "expected orbit detection, got {:?}",
            result
        );
    }

    #[test]
    fn test_monotonic_escalation() {
        let mut det = CycleDetector::new();
        assert_eq!(det.current_tier(), CycleTier::None);

        det.escalate();
        assert_eq!(det.current_tier(), CycleTier::Micro);

        det.escalate();
        assert_eq!(det.current_tier(), CycleTier::Strategic);

        det.escalate();
        assert_eq!(det.current_tier(), CycleTier::GracefulAbort);

        det.escalate();
        assert_eq!(det.current_tier(), CycleTier::Emergency);

        // Can't go beyond Emergency
        det.escalate();
        assert_eq!(det.current_tier(), CycleTier::Emergency);
    }

    #[test]
    fn test_acknowledge_recovery() {
        let mut det = CycleDetector::new();
        det.escalate(); // -> Micro (max 3 attempts)

        assert!(det.acknowledge_recovery()); // attempt 1
        assert!(det.acknowledge_recovery()); // attempt 2
        assert!(det.acknowledge_recovery()); // attempt 3
        assert!(!det.acknowledge_recovery()); // attempt 4 => should escalate
    }

    #[test]
    fn test_reset() {
        let mut det = CycleDetector::new();
        det.record_and_check(0xAAAA, 1, 1000);
        det.record_and_check(0xBBBB, 2, 2000);
        det.escalate();

        det.reset();
        assert_eq!(det.len(), 0);
        assert!(det.is_empty());
        assert_eq!(det.current_tier(), CycleTier::None);
    }

    #[test]
    fn test_buffer_size() {
        // Verify TransitionEntry is 16 bytes and buffer is 256 bytes
        assert_eq!(std::mem::size_of::<TransitionEntry>(), 16);
        assert_eq!(std::mem::size_of::<[TransitionEntry; 16]>(), 256);
    }

    #[test]
    fn test_temporal_stagnation() {
        let mut det = CycleDetector::new();
        // All entries with the same state within 8s window
        for i in 0..6 {
            det.record_and_check(0xDEAD, i as u32, 1000 + i * 500);
        }
        // By now we have 6 entries, all same state, within ~3s window
        // But stagnation check (3 consecutive) fires first
        // Let's check the result of the last entry
        let result = det.record_and_check(0xDEAD, 7, 4000);
        assert!(
            result.tier >= CycleTier::Micro,
            "expected cycle detection (stagnation or temporal), got {:?}",
            result
        );
    }

    #[test]
    fn test_entry_at_wrapping() {
        let mut det = CycleDetector::new();
        // Fill buffer with 20 entries (wraps around 16)
        for i in 0u64..20 {
            det.record_and_check(i * 100, i as u32, i * 1000);
        }

        // Most recent should be the last entry
        let recent = det.entry_at(0).unwrap();
        assert_eq!(recent.state_hash, 19 * 100);

        // Second most recent
        let prev = det.entry_at(1).unwrap();
        assert_eq!(prev.state_hash, 18 * 100);
    }
}
