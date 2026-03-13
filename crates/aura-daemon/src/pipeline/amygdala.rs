use aura_types::events::{EventSource, GateDecision, ParsedEvent, ScoredEvent};
use serde::{Deserialize, Serialize};
use tracing::{debug, instrument, trace, warn};

// ---------------------------------------------------------------------------
// Decision Sanctuary (Cognitive Load Routing)
// ---------------------------------------------------------------------------

/// The routing tier for a decision, based on calculated cognitive load.
/// Implementations of Concept 2: "Decision Sanctuary".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DecisionTier {
    /// Tier 1: AURA decides (micro-decisions) to preserve human energy.
    AutoHandle,
    /// Tier 2: AURA narrows options (e.g. presents top 3 instead of 50).
    NarrowOptions,
    /// Tier 3: Human decides (critical/important decisions).
    RequireHuman,
}

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Default channel weights: [lex, src, time, anom].
const DEFAULT_WEIGHTS: [f32; 4] = [0.40, 0.25, 0.20, 0.15];

/// Default wake threshold.
const DEFAULT_THRESHOLD: f32 = 0.65;

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

/// Default cognitive load decay half-life in hours (used for serde backward compat).
const DEFAULT_DECAY_HALF_LIFE_HOURS: f32 = 24.0;

/// Minimum adaptive half-life (4 hours — fast recovery).
#[allow(dead_code)]
const MIN_DECAY_HALF_LIFE: f32 = 4.0;

/// Maximum adaptive half-life (168 hours = 7 days — very slow recovery).
#[allow(dead_code)]
const MAX_DECAY_HALF_LIFE: f32 = 168.0;

/// Serde default function for `decay_half_life_hours`.
fn default_half_life() -> f32 {
    DEFAULT_DECAY_HALF_LIFE_HOURS
}

// ---------------------------------------------------------------------------
// Static signal weight table — maps known urgency signal words to weights.
//
// Architecture note — Theater AGI guard:
// This table produces a METRIC (lexical signal strength), NOT a routing
// decision. Rust does not classify intent from these keywords. The float
// output is passed as a channel score to the LLM context so the LLM can
// reason about urgency. No routing branch may be driven solely by this score.
// ---------------------------------------------------------------------------

static LEXICAL_SIGNAL_WEIGHTS: &[(&str, f32)] = &[
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
    /// Current estimated cognitive load of the user [0.0, 1.0].
    pub cognitive_load: f32,
    /// Timestamp (ms) of the last cognitive-load update, used for
    /// exponential decay.  `#[serde(default)]` ensures backward compat
    /// with persisted state that lacks this field.
    #[serde(default)]
    last_load_update_ms: u64,
    /// Adaptive cognitive load decay half-life in hours.
    /// Learned from observed outcomes: if AURA acts during "low load"
    /// and the user was overwhelmed, the half-life increases (slower decay).
    /// Default 24.0 preserves original behavior until learning kicks in.
    #[serde(default = "default_half_life")]
    pub(crate) decay_half_life_hours: f32,
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
            cognitive_load: 0.3, // warm start with low/medium load
            last_load_update_ms: 0,
            decay_half_life_hours: DEFAULT_DECAY_HALF_LIFE_HOURS,
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

        // ── Decay cognitive load before scoring ───────────────────────
        // Without this, cognitive_load accumulates monotonically whenever
        // record_human_decision / route_decision are not called frequently.
        // The existing decay infrastructure (adaptive half-life, numerical
        // stability guards, clamping) handles all edge cases.
        self.decay_cognitive_load(event.timestamp_ms);

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

    /// Record a decision made by the human to increase estimated cognitive load.
    pub fn record_human_decision(&mut self) {
        let now = Self::current_time_ms();
        self.decay_cognitive_load(now);
        // Increase load, capping at 1.0.
        self.cognitive_load = (self.cognitive_load + 0.05).min(1.0);
        trace!(cognitive_load = self.cognitive_load, "Human decision recorded, cognitive load increased");
    }

    /// Produce routing metrics for a pending decision.
    ///
    /// # Architecture note — Theater AGI guard
    /// Rust does NOT decide which tier to use. The LLM reasons about routing.
    /// This method returns raw metrics (importance + cognitive_load) so the
    /// LLM context builder can include them in the prompt. The final
    /// `DecisionTier` is determined by the LLM, not by a formula here.
    ///
    /// Callers that previously branched on the returned `DecisionTier` must
    /// instead pass both metrics to the LLM and let it choose.
    pub fn route_decision_metrics(&mut self, decision_importance: f32) -> (f32, f32) {
        self.decay_cognitive_load(Self::current_time_ms());
        // Returns (decision_importance, current_cognitive_load) — both in [0.0, 1.0].
        // LLM interprets these and decides the appropriate response tier.
        (decision_importance.clamp(0.0, 1.0), self.cognitive_load)
    }

    /// Deprecated stub — kept for call-site compatibility during migration.
    ///
    /// # Architecture note — Theater AGI guard
    /// This always returns `DecisionTier::RequireHuman` so the LLM is always
    /// consulted. Replace all call sites with `route_decision_metrics()`.
    #[deprecated(note = "Theater AGI: formula drove tier routing. Use route_decision_metrics() and let the LLM decide.")]
    pub fn route_decision(&mut self, decision_importance: f32) -> DecisionTier {
        self.route_decision_metrics(decision_importance);
        // Always escalate to LLM — do not auto-route in Rust.
        DecisionTier::RequireHuman
    }

    /// Apply exponential decay to `cognitive_load` based on elapsed time.
    ///
    /// Half-life is adaptive (default ~24 hours, learned via `adjust_decay_rate`):
    /// `load *= exp(-ln2 * elapsed_h / decay_half_life_hours)`.
    fn decay_cognitive_load(&mut self, now_ms: u64) {
        if self.last_load_update_ms == 0 || now_ms <= self.last_load_update_ms {
            self.last_load_update_ms = now_ms;
            return;
        }
        let elapsed_ms = now_ms - self.last_load_update_ms;
        let elapsed_hours = elapsed_ms as f64 / 3_600_000.0;
        let half_life = self.decay_half_life_hours as f64;
        // Guard against non-finite or zero half-life values.
        let safe_half_life = if half_life.is_finite() && half_life > 0.0 {
            half_life
        } else {
            DEFAULT_DECAY_HALF_LIFE_HOURS as f64
        };
        let decay = (-std::f64::consts::LN_2 * elapsed_hours / safe_half_life).exp() as f32;
        self.cognitive_load *= decay;
        // Clamp tiny residuals to zero.
        if self.cognitive_load < 0.001 {
            self.cognitive_load = 0.0;
        }
        self.last_load_update_ms = now_ms;
        trace!(
            cognitive_load = self.cognitive_load,
            decay,
            elapsed_hours,
            half_life = self.decay_half_life_hours,
            "cognitive load decayed"
        );
    }

    /// Adjust cognitive load decay rate based on observed outcomes.
    /// Called by the learning engine when it observes that autonomous actions
    /// during low cognitive load periods had good/bad outcomes.
    ///
    /// - `good_outcome = true`: AURA acted during low load and user was satisfied
    ///   → current decay rate is good, reinforce (small regression toward 24h mean)
    /// - `good_outcome = false`: AURA acted during low load but user was overwhelmed
    ///   → decay is too fast, increase half-life (slow down decay)
    // Phase 8 wire point: called by outcome_bus learning subscriber.
    #[allow(dead_code)]
    pub(crate) fn adjust_decay_rate(&mut self, good_outcome: bool) {
        const LEARN_RATE: f32 = 0.05;

        if good_outcome {
            // Reinforce current rate — slight regression toward mean (24h).
            let adjustment = LEARN_RATE * (DEFAULT_DECAY_HALF_LIFE_HOURS - self.decay_half_life_hours);
            let new_half_life = self.decay_half_life_hours + adjustment;
            // Guard against NaN/Inf from arithmetic.
            if new_half_life.is_finite() {
                self.decay_half_life_hours = new_half_life;
            }
        } else {
            // Bad outcome during "low load" — we decayed too fast, slow it down.
            let new_half_life = self.decay_half_life_hours * (1.0 + LEARN_RATE);
            if new_half_life.is_finite() {
                self.decay_half_life_hours = new_half_life;
            }
        }

        self.decay_half_life_hours = self.decay_half_life_hours.clamp(MIN_DECAY_HALF_LIFE, MAX_DECAY_HALF_LIFE);

        tracing::debug!(
            half_life = self.decay_half_life_hours,
            good_outcome,
            "adjusted cognitive load decay rate"
        );
    }

    /// Current wall-clock time in milliseconds (monotonic-safe).
    fn current_time_ms() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64
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
    ///
    /// Architecture note — Theater AGI guard:
    /// Gating is a POLICY decision (timing, thresholds, storm protection) —
    /// NOT a content classification. The lexical score feeds into s_total as
    /// a weighted channel; only the composite score drives gate decisions here.
    /// Rust does not classify intent from s_lex directly.
    fn decide_gate(&mut self, _s_lex: f32, s_total: f32, now_ms: u64) -> GateDecision {
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

    /// Measure lexical signal strength: scan content for urgency signal words,
    /// return maximum weight as a float metric in [0.0, 1.0].
    ///
    /// Architecture note — Theater AGI guard:
    /// This is a METRIC, not a routing decision. The returned float is one
    /// channel in the composite importance score passed to the LLM. Rust does
    /// not route or classify intent from this value.
    fn score_lexical(content: &str) -> f32 {
        let lower = content.to_ascii_lowercase();
        let mut max_weight: f32 = 0.0;
        for &(keyword, weight) in LEXICAL_SIGNAL_WEIGHTS {
            if lower.contains(keyword) {
                if weight > max_weight {
                    max_weight = weight;
                }
                // Early exit: no higher weight possible above 0.95.
                if max_weight >= 0.95 {
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
        // Architecture note: EmergencyBypass no longer triggered by keyword scan alone.
        // High-lex events now go through composite scoring → InstantWake (or SlowAccumulate
        // during cold-start). The lexical metric is preserved in score_lex.
        let mut amygdala = Amygdala::new();
        let scored = amygdala.score(&make_event("emergency alert!", 1_000));
        assert!(scored.score_lex >= 0.90);
        // Gate decision is now driven by composite score, not keyword shortcut.
        assert!(
            matches!(
                scored.gate_decision,
                GateDecision::InstantWake | GateDecision::SlowAccumulate
            ),
            "gate={:?}",
            scored.gate_decision
        );
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
        // High composite score → InstantWake (EmergencyBypass no longer triggered from keyword scan).
        assert!(
            matches!(
                s1.gate_decision,
                GateDecision::InstantWake | GateDecision::SlowAccumulate
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

    #[test]
    fn test_decision_sanctuary_routing() {
        let mut amygdala = Amygdala::new();

        // Architecture note: route_decision() is now a deprecated stub that always
        // returns RequireHuman so the LLM is consulted. The real metrics are
        // returned by route_decision_metrics(). Test the metrics instead.
        let (imp, load) = amygdala.route_decision_metrics(0.2);
        assert!((imp - 0.2).abs() < 0.01);
        assert!(load >= 0.0 && load <= 1.0);

        let (imp2, _) = amygdala.route_decision_metrics(0.8);
        assert!((imp2 - 0.8).abs() < 0.01);

        // Deprecated stub always returns RequireHuman.
        #[allow(deprecated)]
        {
            assert_eq!(amygdala.route_decision(0.2), DecisionTier::RequireHuman);
            assert_eq!(amygdala.route_decision(0.8), DecisionTier::RequireHuman);
        }
    }

    #[test]
    fn test_cognitive_load_decay() {
        let mut amygdala = Amygdala::new();
        amygdala.cognitive_load = 0.8;

        // Simulate 24 hours passing (should roughly halve).
        let base_ms: u64 = 1_700_000_000_000;
        amygdala.last_load_update_ms = base_ms;
        let after_24h = base_ms + 24 * 3_600_000;
        amygdala.decay_cognitive_load(after_24h);
        assert!(
            (amygdala.cognitive_load - 0.4).abs() < 0.02,
            "load should halve after 24h, got {}",
            amygdala.cognitive_load,
        );

        // Simulate another 24 hours — halves again.
        let after_48h = after_24h + 24 * 3_600_000;
        amygdala.decay_cognitive_load(after_48h);
        assert!(
            (amygdala.cognitive_load - 0.2).abs() < 0.02,
            "load should halve again, got {}",
            amygdala.cognitive_load,
        );
    }

    #[test]
    fn test_cognitive_load_decay_zero_elapsed() {
        let mut amygdala = Amygdala::new();
        amygdala.cognitive_load = 0.5;
        let t = 1_700_000_000_000u64;
        amygdala.last_load_update_ms = t;

        // Same timestamp → no decay.
        amygdala.decay_cognitive_load(t);
        assert!((amygdala.cognitive_load - 0.5).abs() < f32::EPSILON);
    }

    #[test]
    fn test_decay_half_life_default() {
        let amygdala = Amygdala::new();
        assert!(
            (amygdala.decay_half_life_hours - 24.0).abs() < f32::EPSILON,
            "default half-life should be 24.0, got {}",
            amygdala.decay_half_life_hours,
        );
    }

    #[test]
    fn test_adaptive_half_life_used_in_decay() {
        // With a 12-hour half-life, load should halve in 12 hours (not 24).
        let mut amygdala = Amygdala::new();
        amygdala.decay_half_life_hours = 12.0;
        amygdala.cognitive_load = 0.8;

        let base_ms: u64 = 1_700_000_000_000;
        amygdala.last_load_update_ms = base_ms;
        let after_12h = base_ms + 12 * 3_600_000;
        amygdala.decay_cognitive_load(after_12h);
        assert!(
            (amygdala.cognitive_load - 0.4).abs() < 0.02,
            "load should halve after 12h with 12h half-life, got {}",
            amygdala.cognitive_load,
        );
    }

    #[test]
    fn test_adjust_decay_rate_bad_outcome_increases_half_life() {
        let mut amygdala = Amygdala::new();
        let before = amygdala.decay_half_life_hours;
        amygdala.adjust_decay_rate(false);
        assert!(
            amygdala.decay_half_life_hours > before,
            "bad outcome should increase half-life: {} -> {}",
            before,
            amygdala.decay_half_life_hours,
        );
    }

    #[test]
    fn test_adjust_decay_rate_good_outcome_regresses_toward_mean() {
        let mut amygdala = Amygdala::new();
        // Start with a high half-life.
        amygdala.decay_half_life_hours = 48.0;
        amygdala.adjust_decay_rate(true);
        // Good outcome regresses toward 24h mean, so it should decrease.
        assert!(
            amygdala.decay_half_life_hours < 48.0,
            "good outcome should regress high half-life toward 24h: {}",
            amygdala.decay_half_life_hours,
        );

        // Start with a low half-life.
        amygdala.decay_half_life_hours = 8.0;
        amygdala.adjust_decay_rate(true);
        // Good outcome regresses toward 24h mean, so it should increase.
        assert!(
            amygdala.decay_half_life_hours > 8.0,
            "good outcome should regress low half-life toward 24h: {}",
            amygdala.decay_half_life_hours,
        );
    }

    #[test]
    fn test_adjust_decay_rate_clamped_to_range() {
        let mut amygdala = Amygdala::new();

        // Many bad outcomes should not exceed MAX (168h).
        for _ in 0..1000 {
            amygdala.adjust_decay_rate(false);
        }
        assert!(
            amygdala.decay_half_life_hours <= 168.0,
            "half-life should be clamped to max: {}",
            amygdala.decay_half_life_hours,
        );

        // Force to minimum and verify clamping.
        amygdala.decay_half_life_hours = 1.0; // below MIN
        amygdala.adjust_decay_rate(true); // triggers clamp
        assert!(
            amygdala.decay_half_life_hours >= 4.0,
            "half-life should be clamped to min: {}",
            amygdala.decay_half_life_hours,
        );
    }

    #[test]
    fn test_decay_half_life_serde_backward_compat() {
        // Simulate deserializing old persisted state without decay_half_life_hours.
        // The serde default function should provide 24.0.
        let json = r#"{
            "source_ema": [0.5,0.5,0.5,0.5,0.5],
            "time_histogram": [0.5,0.5,0.5,0.5,0.5,0.5,0.5,0.5,0.5,0.5,0.5,0.5,0.5,0.5,0.5,0.5,0.5,0.5,0.5,0.5,0.5,0.5,0.5,0.5],
            "welford": {"count":0,"mean":0.0,"m2":0.0},
            "dedup_ring": [],
            "dedup_cursor": 0,
            "event_count": 0,
            "first_event_ms": 0,
            "last_wake_ms": 0,
            "channel_weights": [0.4,0.25,0.2,0.15],
            "threshold": 0.65,
            "cognitive_load": 0.3,
            "last_load_update_ms": 0
        }"#;
        let amygdala: Amygdala = serde_json::from_str(json).expect("deserialize old state");
        assert!(
            (amygdala.decay_half_life_hours - 24.0).abs() < f32::EPSILON,
            "missing field should default to 24.0, got {}",
            amygdala.decay_half_life_hours,
        );
    }
}
