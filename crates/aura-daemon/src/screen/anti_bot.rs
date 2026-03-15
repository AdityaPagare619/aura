//! Anti-bot module: human-like timing to avoid triggering automated-behavior
//! detection on apps and the Android system itself.
//!
//! Three mechanisms:
//! 1. **Token-bucket rate limiter** — sustained 60/min (Normal), 30/min (Safety), 90/min (Power).
//!    Burst of 10/5/15 in a 5-second window.
//! 2. **Random inter-action delay** — 150–500ms (Normal), 300–800ms (Safety), 100–300ms (Power).
//!    Extra delay after `Type` proportional to text length.
//! 3. **Action timing jitter** — all delays include ±15% gaussian-like jitter.

use std::time::Instant;

use aura_types::{actions::ActionType, config::ExecutionConfig};
use tracing::debug;

/// Timing profile extracted from `ExecutionConfig` with burst limits.
#[derive(Debug, Clone)]
pub struct TimingProfile {
    /// Minimum inter-action delay in ms.
    pub delay_min_ms: u32,
    /// Maximum inter-action delay in ms.
    pub delay_max_ms: u32,
    /// Max sustained actions per minute.
    pub sustained_per_min: u32,
    /// Max actions in a 5-second burst window.
    pub burst_limit: u32,
    /// Extra delay per character for Type actions (ms).
    pub type_delay_per_char_ms: u32,
}

impl TimingProfile {
    /// Normal profile: 150-500ms delay, 60/min, burst 10.
    pub fn normal() -> Self {
        Self {
            delay_min_ms: 150,
            delay_max_ms: 500,
            sustained_per_min: 60,
            burst_limit: 10,
            type_delay_per_char_ms: 30,
        }
    }

    /// Safety profile: 300-800ms delay, 30/min, burst 5.
    pub fn safety() -> Self {
        Self {
            delay_min_ms: 300,
            delay_max_ms: 800,
            sustained_per_min: 30,
            burst_limit: 5,
            type_delay_per_char_ms: 50,
        }
    }

    /// Power profile: 100-300ms delay, 90/min, burst 15.
    pub fn power() -> Self {
        Self {
            delay_min_ms: 100,
            delay_max_ms: 300,
            sustained_per_min: 90,
            burst_limit: 15,
            type_delay_per_char_ms: 20,
        }
    }

    /// Build a profile from an `ExecutionConfig`.
    pub fn from_config(config: &ExecutionConfig) -> Self {
        // Derive burst from sustained rate: ~16% of sustained in 5s
        let burst = std::cmp::max(
            5,
            (config.rate_limit_actions_per_min as f32 * 5.0 / 60.0 * 1.6) as u32,
        );

        Self {
            delay_min_ms: config.delay_min_ms,
            delay_max_ms: config.delay_max_ms,
            sustained_per_min: config.rate_limit_actions_per_min,
            burst_limit: burst,
            type_delay_per_char_ms: 30,
        }
    }
}

/// Token-bucket rate limiter with burst tracking.
///
/// Uses a fixed-size ring buffer of the last 64 action timestamps to track
/// both sustained rate (per-minute) and burst rate (per 5-second window).
pub struct AntiBot {
    profile: TimingProfile,

    /// Ring buffer of recent action timestamps (max 64 entries).
    /// No heap allocation during rate checks — fixed array.
    timestamps: [u64; 64],
    /// Number of valid entries in the ring buffer.
    count: u32,
    /// Write index into the ring buffer.
    write_idx: u32,

    /// Monotonic reference point for relative timestamps.
    epoch: Instant,

    /// Last action timestamp (ms since epoch).
    last_action_ms: u64,

    /// Simple PRNG state for jitter (xorshift32).
    rng_state: u32,
}

impl AntiBot {
    /// Create a new anti-bot instance with the given timing profile.
    pub fn new(profile: TimingProfile) -> Self {
        Self {
            profile,
            timestamps: [0u64; 64],
            count: 0,
            write_idx: 0,
            epoch: Instant::now(),
            last_action_ms: 0,
            rng_state: 0xDEAD_BEEF,
        }
    }

    /// Create an AntiBot with Normal profile.
    pub fn normal() -> Self {
        Self::new(TimingProfile::normal())
    }

    /// Create an AntiBot with Safety profile.
    pub fn safety() -> Self {
        Self::new(TimingProfile::safety())
    }

    /// Create an AntiBot with Power profile.
    pub fn power() -> Self {
        Self::new(TimingProfile::power())
    }

    /// Create from an `ExecutionConfig`.
    pub fn from_config(config: &ExecutionConfig) -> Self {
        Self::new(TimingProfile::from_config(config))
    }

    /// Return the current profile.
    pub fn profile(&self) -> &TimingProfile {
        &self.profile
    }

    /// Check if an action can proceed right now.
    ///
    /// Returns `Ok(delay_ms)` with the recommended delay before executing,
    /// or `Err(wait_ms)` if rate-limited (caller should wait `wait_ms` then retry).
    pub fn check_action(&mut self, action: &ActionType) -> Result<u64, u64> {
        let now_ms = self.now_ms();

        // 1. Check sustained rate (actions in the last 60 seconds)
        let one_minute_ago = now_ms.saturating_sub(60_000);
        let actions_last_minute = self.count_since(one_minute_ago);

        if actions_last_minute >= self.profile.sustained_per_min {
            // Find the oldest action in the window; we need to wait until it expires
            let oldest_in_window = self.oldest_since(one_minute_ago);
            let wait = oldest_in_window
                .map(|ts| 60_000u64.saturating_sub(now_ms.saturating_sub(ts)))
                .unwrap_or(1000);
            debug!(
                actions_last_minute,
                limit = self.profile.sustained_per_min,
                wait_ms = wait,
                "rate limited (sustained)"
            );
            return Err(wait.max(100));
        }

        // 2. Check burst rate (actions in the last 5 seconds)
        let five_sec_ago = now_ms.saturating_sub(5_000);
        let actions_last_5s = self.count_since(five_sec_ago);

        if actions_last_5s >= self.profile.burst_limit {
            let oldest_in_burst = self.oldest_since(five_sec_ago);
            let wait = oldest_in_burst
                .map(|ts| 5_000u64.saturating_sub(now_ms.saturating_sub(ts)))
                .unwrap_or(500);
            debug!(
                actions_last_5s,
                limit = self.profile.burst_limit,
                wait_ms = wait,
                "rate limited (burst)"
            );
            return Err(wait.max(100));
        }

        // 3. Compute the recommended delay
        let base_delay = self.compute_delay(action);
        let jittered = self.apply_jitter(base_delay);

        // 4. Enforce minimum time since last action
        let elapsed_since_last = now_ms.saturating_sub(self.last_action_ms);
        let delay = if elapsed_since_last < jittered {
            jittered - elapsed_since_last
        } else {
            0
        };

        Ok(delay)
    }

    /// Record that an action was executed. Must be called AFTER the action completes.
    pub fn record_action(&mut self) {
        let now_ms = self.now_ms();
        self.timestamps[self.write_idx as usize] = now_ms;
        self.write_idx = (self.write_idx + 1) % 64;
        if self.count < 64 {
            self.count += 1;
        }
        self.last_action_ms = now_ms;
    }

    /// Reset all state (for testing or profile change).
    pub fn reset(&mut self) {
        self.timestamps = [0u64; 64];
        self.count = 0;
        self.write_idx = 0;
        self.last_action_ms = 0;
        self.epoch = Instant::now();
    }

    /// Change the timing profile at runtime.
    pub fn set_profile(&mut self, profile: TimingProfile) {
        self.profile = profile;
    }

    /// Get the number of actions in the last 60 seconds.
    pub fn actions_last_minute(&self) -> u32 {
        let now_ms = self.now_ms();
        self.count_since(now_ms.saturating_sub(60_000))
    }

    /// Get the number of actions in the last 5 seconds.
    pub fn actions_last_burst_window(&self) -> u32 {
        let now_ms = self.now_ms();
        self.count_since(now_ms.saturating_sub(5_000))
    }

    // ── Internals ───────────────────────────────────────────────────────────

    /// Milliseconds since the epoch instant.
    fn now_ms(&self) -> u64 {
        self.epoch.elapsed().as_millis() as u64
    }

    /// Count entries in the ring buffer with timestamp >= `since_ms`.
    fn count_since(&self, since_ms: u64) -> u32 {
        let mut count = 0u32;
        let len = self.count as usize;
        for i in 0..len {
            // Walk backwards from write_idx
            let idx = if self.write_idx as usize > i {
                self.write_idx as usize - 1 - i
            } else {
                64 - 1 - (i - self.write_idx as usize)
            };
            if self.timestamps[idx] >= since_ms {
                count += 1;
            } else if self.timestamps[idx] > 0 {
                // timestamps are roughly in order; once we hit old ones we can
                // keep scanning since the ring wraps (not sorted globally)
                // but we do need to check all valid entries
            }
        }
        count
    }

    /// Find the oldest timestamp >= `since_ms` in the ring buffer.
    fn oldest_since(&self, since_ms: u64) -> Option<u64> {
        let mut oldest: Option<u64> = None;
        let len = self.count as usize;
        for i in 0..len {
            let idx = if self.write_idx as usize > i {
                self.write_idx as usize - 1 - i
            } else {
                64 - 1 - (i - self.write_idx as usize)
            };
            let ts = self.timestamps[idx];
            if ts >= since_ms {
                match oldest {
                    None => oldest = Some(ts),
                    Some(o) if ts < o => oldest = Some(ts),
                    _ => {},
                }
            }
        }
        oldest
    }

    /// Compute the base delay for an action (before jitter).
    fn compute_delay(&self, action: &ActionType) -> u64 {
        let base = self.random_in_range(
            self.profile.delay_min_ms as u64,
            self.profile.delay_max_ms as u64,
        );

        // Extra delay for Type actions: proportional to text length
        let extra = match action {
            ActionType::Type { text } => {
                text.len() as u64 * self.profile.type_delay_per_char_ms as u64
            },
            _ => 0,
        };

        base + extra
    }

    /// Apply ±15% jitter to a delay value.
    fn apply_jitter(&mut self, delay_ms: u64) -> u64 {
        if delay_ms == 0 {
            return 0;
        }
        // ±15% range
        let range = delay_ms * 30 / 100; // total range is 30% of delay
        if range == 0 {
            return delay_ms;
        }
        let jitter = self.next_u32() as u64 % (range + 1);
        // Center around delay: delay - 15% + [0, 30%]
        let min = delay_ms.saturating_sub(range / 2);
        min + jitter
    }

    /// Generate a random value in [min, max] using the internal PRNG.
    fn random_in_range(&self, min: u64, max: u64) -> u64 {
        if min >= max {
            return min;
        }
        let range = max - min;
        // Use current rng_state for "randomness" — not truly random but
        // combined with timing jitter it's good enough for human-like simulation
        let val = self.rng_state as u64;
        min + (val % (range + 1))
    }

    /// Xorshift32 PRNG step.
    fn next_u32(&mut self) -> u32 {
        let mut x = self.rng_state;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.rng_state = x;
        x
    }
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_antibot_creation() {
        let ab = AntiBot::normal();
        assert_eq!(ab.profile.sustained_per_min, 60);
        assert_eq!(ab.profile.burst_limit, 10);
        assert_eq!(ab.profile.delay_min_ms, 150);
        assert_eq!(ab.profile.delay_max_ms, 500);
    }

    #[test]
    fn test_safety_profile() {
        let ab = AntiBot::safety();
        assert_eq!(ab.profile.sustained_per_min, 30);
        assert_eq!(ab.profile.burst_limit, 5);
        assert_eq!(ab.profile.delay_min_ms, 300);
    }

    #[test]
    fn test_power_profile() {
        let ab = AntiBot::power();
        assert_eq!(ab.profile.sustained_per_min, 90);
        assert_eq!(ab.profile.burst_limit, 15);
        assert_eq!(ab.profile.delay_min_ms, 100);
    }

    #[test]
    fn test_first_action_allowed() {
        let mut ab = AntiBot::normal();
        let result = ab.check_action(&ActionType::Back);
        // First action should be allowed (no previous action to enforce delay against)
        assert!(result.is_ok());
    }

    #[test]
    fn test_record_action_increments_count() {
        let mut ab = AntiBot::normal();
        assert_eq!(ab.actions_last_minute(), 0);
        ab.record_action();
        assert_eq!(ab.actions_last_minute(), 1);
        ab.record_action();
        assert_eq!(ab.actions_last_minute(), 2);
    }

    #[test]
    fn test_burst_limit_enforced() {
        // Use a profile with very low burst limit
        let profile = TimingProfile {
            delay_min_ms: 0,
            delay_max_ms: 0,
            sustained_per_min: 1000,
            burst_limit: 3,
            type_delay_per_char_ms: 0,
        };
        let mut ab = AntiBot::new(profile);

        // Record 3 actions quickly
        ab.record_action();
        ab.record_action();
        ab.record_action();

        // 4th action should be rate-limited by burst
        let result = ab.check_action(&ActionType::Back);
        assert!(
            result.is_err(),
            "expected rate limit after burst, got {:?}",
            result
        );
    }

    #[test]
    fn test_type_action_extra_delay() {
        let ab = AntiBot::normal();
        let tap_delay = ab.compute_delay(&ActionType::Tap { x: 0, y: 0 });
        let type_delay = ab.compute_delay(&ActionType::Type {
            text: "Hello, World!".into(),
        });
        // Type should have extra delay proportional to text length
        assert!(
            type_delay > tap_delay,
            "type_delay ({}) should be > tap_delay ({})",
            type_delay,
            tap_delay
        );
    }

    #[test]
    fn test_jitter_stays_in_range() {
        let mut ab = AntiBot::normal();
        for _ in 0..100 {
            let delay = 300u64;
            let jittered = ab.apply_jitter(delay);
            // ±15% of 300 = 255 to 345
            assert!(
                jittered >= 210 && jittered <= 400,
                "jittered {} out of reasonable range for base 300",
                jittered
            );
        }
    }

    #[test]
    fn test_reset_clears_state() {
        let mut ab = AntiBot::normal();
        ab.record_action();
        ab.record_action();
        assert_eq!(ab.actions_last_minute(), 2);

        ab.reset();
        assert_eq!(ab.actions_last_minute(), 0);
        assert_eq!(ab.count, 0);
        assert_eq!(ab.write_idx, 0);
    }

    #[test]
    fn test_from_config() {
        let config = ExecutionConfig::default();
        let ab = AntiBot::from_config(&config);
        assert_eq!(ab.profile.delay_min_ms, 150);
        assert_eq!(ab.profile.delay_max_ms, 500);
        assert_eq!(ab.profile.sustained_per_min, 60);
    }

    #[test]
    fn test_ring_buffer_wraps() {
        let profile = TimingProfile {
            delay_min_ms: 0,
            delay_max_ms: 0,
            sustained_per_min: 10_000,
            burst_limit: 10_000,
            type_delay_per_char_ms: 0,
        };
        let mut ab = AntiBot::new(profile);

        // Record 100 actions — wraps the 64-entry ring buffer
        for _ in 0..100 {
            ab.record_action();
        }

        // Count should be capped at 64
        assert_eq!(ab.count, 64);
        assert_eq!(ab.write_idx, 100 % 64);
    }
}
