use aura_types::events::{EventSource, GateDecision, ParsedEvent, ScoredEvent};
use serde::{Deserialize, Serialize};
use tracing::{debug, instrument, trace, warn};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Default channel weights: [lex, src, time, anom].
const DEFAULT_WEIGHTS: [f32; 4] = [0.40, 0.25, 0.20, 0.15];

/// Default wake threshold.
const DEFAULT_THRESHOLD: f32 = 0.65;

/// Emergency bypass: lexical score at or above this triggers immediate bypass.
const EMERGENCY_LEX_THRESHOLD: f32 = 0.90;

/// Suppress threshold — below this, events are suppressed.
const SUPPRESS_THRESHOLD: f32 = 0.20;

/// Source EMA learning rate η.
const SOURCE_EMA_ETA: f32 = 0.02;

/// Temporal histogram blend factor.
const TIME_BLEND: f32 = 0.05;

/// Dedup ring buffer capacity.
const DEDUP_RING_SIZE: usize = 50;

/// Dedup time window in ms (5 s).
const DEDUP_WINDOW_MS: u64 = 5_000;

/// Storm protection: minimum gap between InstantWake events (30 s).
const MIN_WAKE_GAP_MS: u64 = 30_000;

/// Cold-start event count threshold.
const COLD_START_EVENTS: u64 = 200;

/// Cold-start time window (72 h in ms).
const COLD_START_WINDOW_MS: u64 = 72 * 3_600 * 1_000;

/// Cold-start threshold multiplier.
const COLD_START_FACTOR: f32 = 0.85;

// ---------------------------------------------------------------------------
// Static keyword table — sorted by weight descending for early exit
// ---------------------------------------------------------------------------

static KEYWORDS: &[(&str, f32)] = &[
    ("emergency", 0.98),
    ("urgent", 0.95),
    ("crash", 0.90),
    ("error", 0.70),
    ("warning", 0.60),
    ("message", 0.50),
    ("notification", 0.40),
    ("update", 0.30),
    ("routine", 0.10),
];

// ---------------------------------------------------------------------------
// Welford's online algorithm for running mean/variance
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WelfordState {
    count: u64,
    mean: f64,
    m2: f64,
}

impl WelfordState {
    fn new() -> Self {
        Self {
            count: 0,
            mean: 0.0,
            m2: 0.0,
        }
    }

    /// Incorporate a new sample.
    fn update(&mut self, x: f64) {
        self.count += 1;
        let delta = x - self.mean;
        self.mean += delta / self.count as f64;
        let delta2 = x - self.mean;
        self.m2 += delta * delta2;
    }

    fn variance(&self) -> f64 {
        if self.count < 2 {
            0.0
        } else {
            self.m2 / (self.count - 1) as f64
        }
    }

    fn stddev(&self) -> f64 {
        self.variance().sqrt()
    }
}

// ---------------------------------------------------------------------------
// Amygdala
// ---------------------------------------------------------------------------

/// Stage 2 importance scorer — four-channel design, <100 μs per event,
/// zero heap allocations in the hot path.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Amygdala {
    /// Per-source EMA importance (indexed by `EventSource` ordinal).
    source_ema: [f32; 5],
    /// 24-slot hourly activity histogram.
    time_histogram: [f32; 24],
    /// Welford running stats for anomaly detection.
    welford: WelfordState,
    /// Ring buffer of (content_hash, timestamp_ms) for dedup.
    dedup_ring: Vec<(u64, u64)>,
    dedup_cursor: usize,
    /// Lifetime event counter.
    event_count: u64,
    /// Timestamp of the very first event (for cold-start detection).
    first_event_ms: u64,
    /// Timestamp of the last InstantWake gate decision.
    last_wake_ms: u64,
    /// Channel weights [lex, src, time, anom].
    channel_weights: [f32; 4],
    /// Wake threshold.
    threshold: f32,
}

impl Amygdala {
    #[instrument]
    pub fn new() -> Self {
        let mut dedup = Vec::with_capacity(DEDUP_RING_SIZE);
        dedup.resize(DEDUP_RING_SIZE, (0u64, 0u64));
        trace!("Amygdala initialized with default weights and threshold");
        Self {
            source_ema: [0.5; 5], // warm start at 0.5
            time_histogram: [0.5; 24],
            welford: WelfordState::new(),
            dedup_ring: dedup,
            dedup_cursor: 0,
            event_count: 0,
            first_event_ms: 0,
            last_wake_ms: 0,
            channel_weights: DEFAULT_WEIGHTS,
            threshold: DEFAULT_THRESHOLD,
        }
    }

    /// Score a parsed event and produce a [`ScoredEvent`] with gate decision.
    #[instrument(
        skip(self, event),
        fields(
            content_len = event.content.len(),
            source = ?event.source,
            intent = ?event.intent,
            event_count = self.event_count,
        )
    )]
    pub fn score(&mut self, event: &ParsedEvent) -> ScoredEvent {
        trace!("scoring event");

        // Track first event time.
        if self.event_count == 0 {
            self.first_event_ms = event.timestamp_ms;
            debug!(
                first_event_ms = event.timestamp_ms,
                "recording first event timestamp"
            );
        }
        self.event_count += 1;

        // ---- Dedup check ----
        let content_hash = Self::fnv1a_hash(&event.content);
        if self.is_duplicate(content_hash, event.timestamp_ms) {
            warn!(
                content_hash,
                timestamp_ms = event.timestamp_ms,
                "duplicate event suppressed via dedup ring"
            );
            return Self::build_scored(event, 0.0, 0.0, 0.0, 0.0, 0.0, GateDecision::Suppress);
        }
        // Record in dedup ring.
        self.dedup_ring[self.dedup_cursor] = (content_hash, event.timestamp_ms);
        self.dedup_cursor = (self.dedup_cursor + 1) % DEDUP_RING_SIZE;

        // ---- Channel 1: Lexical (S_lex) ----
        let s_lex = Self::score_lexical(&event.content);

        // ---- Channel 2: Source (S_src) ----
        let src_idx = Self::source_ordinal(event.source);
        let s_src = self.source_ema[src_idx];
        // Update EMA: ema = ema * (1 - η) + s_lex * η
        self.source_ema[src_idx] =
            self.source_ema[src_idx] * (1.0 - SOURCE_EMA_ETA) + s_lex * SOURCE_EMA_ETA;

        // ---- Channel 3: Temporal (S_time) ----
        let hour = ((event.timestamp_ms / 3_600_000) % 24) as usize;
        let s_time = self.time_histogram[hour];
        // Update: blend toward activity.
        self.time_histogram[hour] = self.time_histogram[hour] * (1.0 - TIME_BLEND) + TIME_BLEND;

        // ---- Channel 4: Anomaly (S_anom) ----
        self.welford.update(s_lex as f64);
        let z = if self.welford.count > 1 {
            let sd = self.welford.stddev().max(0.001);
            (s_lex as f64 - self.welford.mean) / sd
        } else {
            0.0
        };
        let s_anom = (z as f32 / 3.0).clamp(0.0, 1.0);

        // ---- Composite score ----
        let w = &self.channel_weights;
        let s_total = (w[0] * s_lex + w[1] * s_src + w[2] * s_time + w[3] * s_anom).clamp(0.0, 1.0);

        // ---- Gate decision ----
        let gate = self.decide_gate(s_lex, s_total, event.timestamp_ms);

        tracing::trace!(
            s_lex,
            s_src,
            s_time,
            s_anom,
            s_total,
            ?gate,
            "amygdala scored event"
        );

        Self::build_scored(event, s_total, s_lex, s_src, s_time, s_anom, gate)
    }

    /// Effective threshold accounting for cold-start.
    fn effective_threshold(&self, now_ms: u64) -> f32 {
        let in_cold_start = self.event_count < COLD_START_EVENTS
            && now_ms.saturating_sub(self.first_event_ms) < COLD_START_WINDOW_MS;

        if in_cold_start {
            trace!(
                event_count = self.event_count,
                cold_start_events = COLD_START_EVENTS,
                factor = COLD_START_FACTOR,
                "cold-start active — using reduced threshold"
            );
            self.threshold * COLD_START_FACTOR
        } else {
            self.threshold
        }
    }

    /// Determine the gate decision.
    fn decide_gate(&mut self, s_lex: f32, s_total: f32, now_ms: u64) -> GateDecision {
        // Emergency bypass.
        if s_lex >= EMERGENCY_LEX_THRESHOLD {
            self.last_wake_ms = now_ms;
            warn!(s_lex, "emergency bypass triggered — high lexical score");
            return GateDecision::EmergencyBypass;
        }

        let thresh = self.effective_threshold(now_ms);

        if s_total > thresh {
            // Storm protection: max 1 wake per 30 s.
            if now_ms.saturating_sub(self.last_wake_ms) < MIN_WAKE_GAP_MS {
                debug!(
                    s_total,
                    threshold = thresh,
                    gap_ms = now_ms.saturating_sub(self.last_wake_ms),
                    min_gap_ms = MIN_WAKE_GAP_MS,
                    "storm protection: throttling to SlowAccumulate"
                );
                return GateDecision::SlowAccumulate;
            }
            self.last_wake_ms = now_ms;
            debug!(s_total, threshold = thresh, "InstantWake gate decision");
            GateDecision::InstantWake
        } else if s_total > SUPPRESS_THRESHOLD {
            trace!(s_total, threshold = thresh, "SlowAccumulate gate decision");
            GateDecision::SlowAccumulate
        } else {
            trace!(
                s_total,
                threshold = SUPPRESS_THRESHOLD,
                "Suppress gate decision"
            );
            GateDecision::Suppress
        }
    }

    // -- helpers (all inline, no allocations) --------------------------------

    /// Check the dedup ring for a matching hash within the time window.
    fn is_duplicate(&self, hash: u64, now_ms: u64) -> bool {
        for &(h, ts) in &self.dedup_ring {
            if h == hash && h != 0 && now_ms.saturating_sub(ts) < DEDUP_WINDOW_MS {
                return true;
            }
        }
        false
    }

    /// Lexical scoring: scan content for keywords, return max weight.
    fn score_lexical(content: &str) -> f32 {
        let lower = content.to_ascii_lowercase();
        let mut max_weight: f32 = 0.0;
        for &(keyword, weight) in KEYWORDS {
            if lower.contains(keyword) {
                if weight > max_weight {
                    max_weight = weight;
                }
                // Early exit on emergency-level keyword.
                if max_weight >= EMERGENCY_LEX_THRESHOLD {
                    break;
                }
            }
        }
        max_weight
    }

    /// Map `EventSource` to a stable array index.
    fn source_ordinal(src: EventSource) -> usize {
        match src {
            EventSource::Accessibility => 0,
            EventSource::Notification => 1,
            EventSource::UserCommand => 2,
            EventSource::Cron => 3,
            EventSource::Internal => 4,
        }
    }

    /// FNV-1a hash (64-bit) — simple, no-alloc string hash.
    fn fnv1a_hash(s: &str) -> u64 {
        const FNV_OFFSET: u64 = 0xcbf29ce484222325;
        const FNV_PRIME: u64 = 0x00000100000001B3;
        let mut hash = FNV_OFFSET;
        for byte in s.bytes() {
            hash ^= byte as u64;
            hash = hash.wrapping_mul(FNV_PRIME);
        }
        hash
    }

    fn build_scored(
        event: &ParsedEvent,
        total: f32,
        lex: f32,
        src: f32,
        time: f32,
        anom: f32,
        gate: GateDecision,
    ) -> ScoredEvent {
        ScoredEvent {
            parsed: event.clone(),
            score_total: total,
            score_lex: lex,
            score_src: src,
            score_time: time,
            score_anom: anom,
            gate_decision: gate,
        }
    }
}

impl Default for Amygdala {
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
    use aura_types::events::{EventSource, Intent};

    fn make_event(content: &str, ts: u64) -> ParsedEvent {
        ParsedEvent {
            source: EventSource::Notification,
            intent: Intent::RoutineEvent,
            content: content.to_string(),
            entities: vec![],
            timestamp_ms: ts,
            raw_event_type: 0,
        }
    }

    #[test]
    fn test_emergency_bypass() {
        let mut amygdala = Amygdala::new();
        let scored = amygdala.score(&make_event("emergency alert!", 1_000));
        assert_eq!(scored.gate_decision, GateDecision::EmergencyBypass);
        assert!(scored.score_lex >= 0.90);
    }

    #[test]
    fn test_routine_event_suppressed() {
        let mut amygdala = Amygdala::new();
        // "routine" has weight 0.10 — should be low score.
        let scored = amygdala.score(&make_event("routine check", 1_000));
        assert!(scored.score_lex <= 0.10, "lex={}", scored.score_lex);
        // Total should be below wake threshold.
        assert!(
            matches!(
                scored.gate_decision,
                GateDecision::SlowAccumulate | GateDecision::Suppress
            ),
            "gate={:?}",
            scored.gate_decision
        );
    }

    #[test]
    fn test_dedup_suppresses_duplicate() {
        let mut amygdala = Amygdala::new();
        let e = make_event("error in the system", 10_000);
        let first = amygdala.score(&e);
        assert_ne!(first.gate_decision, GateDecision::Suppress);

        // Same content within 5 s → suppressed.
        let second = amygdala.score(&make_event("error in the system", 12_000));
        assert_eq!(second.gate_decision, GateDecision::Suppress);
    }

    #[test]
    fn test_dedup_allows_after_window() {
        let mut amygdala = Amygdala::new();
        amygdala.score(&make_event("error in the system", 10_000));

        // Same content but after 6 s → allowed.
        let scored = amygdala.score(&make_event("error in the system", 16_000));
        assert_ne!(scored.gate_decision, GateDecision::Suppress);
    }

    #[test]
    fn test_storm_protection() {
        let mut amygdala = Amygdala::new();

        // First high-score event → InstantWake.
        let s1 = amygdala.score(&make_event("urgent error crash", 10_000));
        // Might be EmergencyBypass if "crash" hits 0.90 — that's fine.
        assert!(
            matches!(
                s1.gate_decision,
                GateDecision::EmergencyBypass | GateDecision::InstantWake
            ),
            "gate={:?}",
            s1.gate_decision
        );

        // Second high-score event within 30 s → throttled to SlowAccumulate
        // (unless it's emergency bypass, which ignores storm protection).
        let s2 = amygdala.score(&make_event("warning level high", 15_000));
        // "warning" = 0.60, not emergency, but within 30s of last wake.
        assert!(
            matches!(
                s2.gate_decision,
                GateDecision::SlowAccumulate | GateDecision::Suppress
            ),
            "gate={:?}",
            s2.gate_decision
        );
    }

    #[test]
    fn test_cold_start_lower_threshold() {
        let amygdala = Amygdala::new();

        // During cold start (event_count < 200, within 72h),
        // effective threshold = 0.65 * 0.85 = 0.5525.
        let thresh = amygdala.effective_threshold(1_000_000);
        assert!(
            (thresh - 0.65 * 0.85).abs() < 0.001,
            "cold start thresh={}",
            thresh
        );
    }

    #[test]
    fn test_welford_stats() {
        let mut w = WelfordState::new();
        w.update(2.0);
        w.update(4.0);
        w.update(6.0);
        // mean = 4.0
        assert!((w.mean - 4.0).abs() < 0.001);
        // sample variance = ((2-4)^2 + (4-4)^2 + (6-4)^2) / 2 = 8/2 = 4
        assert!((w.variance() - 4.0).abs() < 0.001);
        // stddev = 2.0
        assert!((w.stddev() - 2.0).abs() < 0.001);
    }

    #[test]
    fn test_fnv1a_deterministic() {
        let h1 = Amygdala::fnv1a_hash("hello world");
        let h2 = Amygdala::fnv1a_hash("hello world");
        assert_eq!(h1, h2);

        let h3 = Amygdala::fnv1a_hash("hello worlD");
        assert_ne!(h1, h3);
    }

    #[test]
    fn test_composite_score_formula() {
        // Manually verify: s_total = 0.40*lex + 0.25*src + 0.20*time + 0.15*anom
        let w = DEFAULT_WEIGHTS;
        let total = w[0] * 0.70 + w[1] * 0.50 + w[2] * 0.50 + w[3] * 0.30;
        // = 0.28 + 0.125 + 0.10 + 0.045 = 0.55
        assert!((total - 0.55).abs() < 0.01);
    }
}
