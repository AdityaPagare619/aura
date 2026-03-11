//! Sleep tracking and PSQI-adapted quality scoring (spec §3.4).
//!
//! Models sleep records with onset/wake times, computes a quality score
//! adapted from the Pittsburgh Sleep Quality Index (PSQI), and detects
//! concerning patterns like late-night device usage.
//!
//! # Quality Formula (adapted PSQI)
//!
//! ```text
//! sleep_quality = (duration_score × 0.35
//!                + efficiency_score × 0.25
//!                + consistency_score × 0.20
//!                + latency_score × 0.10
//!                + disturbance_score × 0.10)
//! ```
//!
//! Each sub-score is in [0.0, 1.0].

use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

use crate::arc::ArcError;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum sleep records retained.
const MAX_SLEEP_RECORDS: usize = 365;

/// Default ideal sleep duration (used until AURA learns the user's pattern).
const DEFAULT_IDEAL_SLEEP_HOURS: f64 = 7.5;

/// Minimum acceptable sleep duration in hours.
const MIN_ACCEPTABLE_HOURS: f64 = 6.0;

/// Maximum useful sleep duration in hours (above this is diminishing returns).
const MAX_USEFUL_HOURS: f64 = 9.0;

/// Default sleep onset time (22:30) — used until AURA learns the user's pattern.
const DEFAULT_ONSET_SECS: i64 = 22 * 3600 + 30 * 60;

/// Tolerance for onset time consistency (1 hour).
const ONSET_CONSISTENCY_TOLERANCE: f64 = 3600.0;

/// Maximum sleep latency in minutes before concern.
const MAX_LATENCY_MINS: f64 = 30.0;

/// Weight constants for quality formula.
const W_DURATION: f32 = 0.35;
const W_EFFICIENCY: f32 = 0.25;
const W_CONSISTENCY: f32 = 0.20;
const W_LATENCY: f32 = 0.10;
const W_DISTURBANCE: f32 = 0.10;

/// Consecutive late-night usage days to flag as concern.
const LATE_NIGHT_CONCERN_DAYS: usize = 3;

/// Default "late night" threshold (23:00) — until user's pattern is learned.
const DEFAULT_LATE_NIGHT_THRESHOLD_SECS: i64 = 23 * 3600;

/// EMA alpha for learning user's sleep parameters.
const SLEEP_EMA_ALPHA: f64 = 0.15;

/// Minimum sleep records before adaptive parameters take effect.
const MIN_RECORDS_FOR_ADAPTATION: usize = 5;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// A single sleep record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SleepRecord {
    /// Sleep onset time (unix epoch seconds).
    pub onset_at: i64,
    /// Wake time (unix epoch seconds).
    pub wake_at: i64,
    /// Sleep latency in minutes (time to fall asleep).
    pub latency_mins: f32,
    /// Number of awakenings during the night.
    pub awakenings: u16,
    /// Total time awake during the night in minutes.
    pub waso_mins: f32,
    /// Subjective quality (0-3, where 0=very bad, 3=very good).
    /// Optional — not always available from sensors.
    pub subjective_quality: Option<u8>,
    /// Source of the data: "sensor", "manual", "inferred".
    pub source: String,
}

impl SleepRecord {
    /// Total time in bed in hours.
    #[must_use]
    pub fn time_in_bed_hours(&self) -> f64 {
        let diff = self.wake_at - self.onset_at;
        if diff <= 0 {
            return 0.0;
        }
        diff as f64 / 3600.0
    }

    /// Actual sleep duration in hours (time in bed minus WASO and latency).
    #[must_use]
    pub fn sleep_duration_hours(&self) -> f64 {
        let tib = self.time_in_bed_hours();
        let waso_hours = self.waso_mins as f64 / 60.0;
        let latency_hours = self.latency_mins as f64 / 60.0;
        (tib - waso_hours - latency_hours).max(0.0)
    }

    /// Sleep efficiency: actual sleep / time in bed.
    #[must_use]
    pub fn efficiency(&self) -> f64 {
        let tib = self.time_in_bed_hours();
        if tib <= 0.0 {
            return 0.0;
        }
        (self.sleep_duration_hours() / tib).clamp(0.0, 1.0)
    }

    /// Extract the onset time-of-day in seconds from midnight.
    #[must_use]
    pub fn onset_time_of_day(&self) -> i64 {
        ((self.onset_at % 86400) + 86400) % 86400
    }
}

// ---------------------------------------------------------------------------
// SleepTracker
// ---------------------------------------------------------------------------

/// Tracks sleep records and computes quality scores.
///
/// The tracker learns the user's personal sleep parameters (ideal duration,
/// onset time) through exponential moving averages of actual sleep data.
/// Until enough data accumulates, defaults are used.
#[derive(Debug, Serialize, Deserialize)]
pub struct SleepTracker {
    /// Ring buffer of sleep records.
    records: Vec<SleepRecord>,
    cursor: usize,
    /// Count of consecutive days with late-night device usage.
    late_night_streak: u16,
    /// Last day index with late-night usage.
    last_late_night_day: i64,
    /// Learned ideal sleep duration (EMA of actual user sleep hours).
    learned_ideal_hours: f64,
    /// Learned onset time of day in seconds from midnight (EMA).
    learned_onset_secs: f64,
}

impl SleepTracker {
    /// Create a new sleep tracker.
    #[must_use]
    pub fn new() -> Self {
        Self {
            records: Vec::with_capacity(32),
            cursor: 0,
            late_night_streak: 0,
            last_late_night_day: 0,
            learned_ideal_hours: DEFAULT_IDEAL_SLEEP_HOURS,
            learned_onset_secs: DEFAULT_ONSET_SECS as f64,
        }
    }

    /// The ideal sleep hours this tracker has learned for the user.
    #[must_use]
    pub fn ideal_hours(&self) -> f64 {
        if self.records.len() < MIN_RECORDS_FOR_ADAPTATION {
            DEFAULT_IDEAL_SLEEP_HOURS
        } else {
            self.learned_ideal_hours
        }
    }

    /// The late-night threshold derived from the user's learned onset time.
    /// 1 hour before the user's learned bedtime = "late night".
    #[must_use]
    fn late_night_threshold_secs(&self) -> i64 {
        if self.records.len() < MIN_RECORDS_FOR_ADAPTATION {
            DEFAULT_LATE_NIGHT_THRESHOLD_SECS
        } else {
            // 1 hour before learned onset: if user sleeps at 01:00, late = 00:00.
            let onset = self.learned_onset_secs as i64;
            (onset - 3600).rem_euclid(86400)
        }
    }

    /// Record a sleep session.
    ///
    /// Also updates the learned ideal sleep hours and onset time via EMA,
    /// so AURA gradually learns the user's personal sleep pattern.
    pub fn record(&mut self, record: SleepRecord) -> Result<(), ArcError> {
        if record.wake_at <= record.onset_at {
            return Err(ArcError::DomainError {
                domain: crate::arc::DomainId::Health,
                detail: "wake time must be after onset time".into(),
            });
        }

        // Update learned parameters via EMA.
        let dur = record.sleep_duration_hours();
        let onset_tod = {
            let raw = record.onset_time_of_day() as f64;
            // Handle wrap-around: if after midnight, treat as late-night.
            if raw < 12.0 * 3600.0 { raw + 86400.0 } else { raw }
        };
        self.learned_ideal_hours =
            SLEEP_EMA_ALPHA * dur + (1.0 - SLEEP_EMA_ALPHA) * self.learned_ideal_hours;
        self.learned_onset_secs =
            SLEEP_EMA_ALPHA * onset_tod + (1.0 - SLEEP_EMA_ALPHA) * self.learned_onset_secs;

        debug!(
            duration_h = dur,
            efficiency = record.efficiency(),
            learned_ideal = self.learned_ideal_hours,
            "sleep recorded"
        );

        if self.records.len() < MAX_SLEEP_RECORDS {
            self.records.push(record);
        } else {
            let idx = self.cursor % MAX_SLEEP_RECORDS;
            self.records[idx] = record;
        }
        self.cursor += 1;
        Ok(())
    }

    /// Record late-night device usage event.
    ///
    /// Uses the user's learned onset time to determine "late night" rather
    /// than a hardcoded 23:00 threshold.
    pub fn record_late_night_usage(&mut self, timestamp: i64) {
        let time_of_day = timestamp.rem_euclid(86400);
        let threshold = self.late_night_threshold_secs();
        if time_of_day >= threshold {
            let day_index = timestamp / 86400;
            if day_index == self.last_late_night_day + 1 || self.last_late_night_day == 0 {
                self.late_night_streak += 1;
            } else if day_index > self.last_late_night_day + 1 {
                self.late_night_streak = 1;
            }
            self.last_late_night_day = day_index;

            if self.late_night_streak >= LATE_NIGHT_CONCERN_DAYS as u16 {
                warn!(
                    streak = self.late_night_streak,
                    "consecutive late-night usage detected"
                );
            }
        }
    }

    /// Whether late-night usage is concerning (3+ consecutive days).
    #[must_use]
    pub fn is_late_night_concerning(&self) -> bool {
        self.late_night_streak >= LATE_NIGHT_CONCERN_DAYS as u16
    }

    /// Compute the composite sleep quality score (0.0 to 1.0).
    ///
    /// Uses an adapted PSQI formula with sub-component weights.
    #[must_use]
    pub fn quality_score(&self) -> f32 {
        if self.records.is_empty() {
            return 0.5; // Neutral when no data
        }

        let recent = self.recent_records(7);
        if recent.is_empty() {
            return 0.5;
        }

        let duration = self.duration_score(&recent);
        let efficiency = self.efficiency_score(&recent);
        let consistency = self.consistency_score(&recent);
        let latency = self.latency_score(&recent);
        let disturbance = self.disturbance_score(&recent);

        let total = W_DURATION * duration
            + W_EFFICIENCY * efficiency
            + W_CONSISTENCY * consistency
            + W_LATENCY * latency
            + W_DISTURBANCE * disturbance;

        total.clamp(0.0, 1.0)
    }

    /// Duration sub-score: how close average sleep is to the user's learned ideal.
    ///
    /// Uses `self.ideal_hours()` which is an EMA of the user's actual sleep.
    /// Until enough data accumulates, falls back to the default (7.5h).
    fn duration_score(&self, records: &[&SleepRecord]) -> f32 {
        let avg_hours: f64 = records
            .iter()
            .map(|r| r.sleep_duration_hours())
            .sum::<f64>()
            / records.len() as f64;

        let ideal = self.ideal_hours();

        if (MIN_ACCEPTABLE_HOURS..=MAX_USEFUL_HOURS).contains(&avg_hours) {
            // Closer to the user's learned ideal = higher score.
            let deviation = (avg_hours - ideal).abs();
            (1.0 - deviation / 3.0).max(0.0) as f32
        } else if avg_hours < MIN_ACCEPTABLE_HOURS {
            // Severely short sleep scored against the user's ideal.
            (avg_hours / ideal).max(0.0) as f32
        } else {
            // Over-sleeping: gentle penalty.
            (1.0 - (avg_hours - MAX_USEFUL_HOURS) / 3.0).max(0.0) as f32
        }
    }

    /// Efficiency sub-score: average sleep efficiency, penalized by insufficient duration.
    ///
    /// Raw efficiency (sleep/TIB ratio) is scaled down when actual sleep duration
    /// is below the minimum acceptable hours, because sleeping 3 hours "efficiently"
    /// is still objectively poor sleep.
    fn efficiency_score(&self, records: &[&SleepRecord]) -> f32 {
        let avg_eff: f64 =
            records.iter().map(|r| r.efficiency()).sum::<f64>() / records.len() as f64;
        let avg_hours: f64 = records
            .iter()
            .map(|r| r.sleep_duration_hours())
            .sum::<f64>()
            / records.len() as f64;

        // Raw efficiency score: 85% efficiency = 1.0
        let raw = (avg_eff / 0.85).min(1.0);

        // Duration penalty: if avg sleep < minimum acceptable, scale down
        let duration_factor = if avg_hours < MIN_ACCEPTABLE_HOURS {
            (avg_hours / MIN_ACCEPTABLE_HOURS).max(0.0)
        } else {
            1.0
        };

        (raw * duration_factor) as f32
    }

    /// Consistency sub-score: how regular is the sleep schedule.
    ///
    /// Uses standard deviation of onset times.
    fn consistency_score(&self, records: &[&SleepRecord]) -> f32 {
        if records.len() < 2 {
            return 0.5;
        }

        let onsets: Vec<f64> = records
            .iter()
            .map(|r| {
                let tod = r.onset_time_of_day() as f64;
                // Handle wrap-around: if after midnight, add 24h
                if tod < 12.0 * 3600.0 {
                    tod + 86400.0
                } else {
                    tod
                }
            })
            .collect();

        let mean: f64 = onsets.iter().sum::<f64>() / onsets.len() as f64;
        let variance: f64 =
            onsets.iter().map(|&t| (t - mean).powi(2)).sum::<f64>() / onsets.len() as f64;
        let stddev = variance.sqrt();

        // Lower stddev = more consistent = higher score
        // Tolerance: 1 hour stddev → score 1.0, above that penalized
        let score = (1.0 - stddev / (ONSET_CONSISTENCY_TOLERANCE * 2.0)).max(0.0);
        score as f32
    }

    /// Latency sub-score: how quickly user falls asleep.
    fn latency_score(&self, records: &[&SleepRecord]) -> f32 {
        let avg_latency: f64 =
            records.iter().map(|r| r.latency_mins as f64).sum::<f64>() / records.len() as f64;

        if avg_latency <= 10.0 {
            1.0
        } else if avg_latency >= MAX_LATENCY_MINS {
            0.0
        } else {
            (1.0 - (avg_latency - 10.0) / (MAX_LATENCY_MINS - 10.0)) as f32
        }
    }

    /// Disturbance sub-score: night-time awakenings.
    fn disturbance_score(&self, records: &[&SleepRecord]) -> f32 {
        let avg_awakenings: f64 =
            records.iter().map(|r| r.awakenings as f64).sum::<f64>() / records.len() as f64;

        // 0 awakenings = 1.0, 5+ = 0.0
        (1.0 - avg_awakenings / 5.0).max(0.0) as f32
    }

    /// Get recent sleep records (last N days).
    fn recent_records(&self, days: usize) -> Vec<&SleepRecord> {
        if self.records.is_empty() {
            return Vec::new();
        }
        // Take the last N records as a proxy for last N days
        let start = if self.records.len() > days {
            self.records.len() - days
        } else {
            0
        };
        self.records[start..].iter().collect()
    }

    /// Number of sleep records stored.
    #[must_use]
    pub fn record_count(&self) -> usize {
        self.records.len()
    }

    /// Access stored records.
    #[must_use]
    pub fn records(&self) -> &[SleepRecord] {
        &self.records
    }

    /// Late-night usage streak count.
    #[must_use]
    pub fn late_night_streak(&self) -> u16 {
        self.late_night_streak
    }

    // -----------------------------------------------------------------------
    // Recommendations engine
    // -----------------------------------------------------------------------

    /// Generate actionable sleep recommendations based on recent data.
    ///
    /// Analyzes quality sub-scores and late-night patterns to produce
    /// personalized advice.  Returns up to 5 recommendations sorted by
    /// priority (High first) then confidence (descending).
    #[must_use]
    pub fn generate_recommendations(&self) -> Vec<SleepRecommendation> {
        let recent = self.recent_records(7);
        if recent.len() < 3 {
            return Vec::new();
        }

        let mut recs = Vec::with_capacity(6);

        let dur = self.duration_score(&recent);
        if dur < 0.6 {
            let priority = if dur < 0.4 {
                SleepRecommendationPriority::High
            } else {
                SleepRecommendationPriority::Medium
            };
            recs.push(SleepRecommendation {
                priority,
                category: "duration",
                message: "Try to get closer to 7-8 hours of sleep".into(),
                confidence: (1.0 - dur).clamp(0.0, 1.0),
            });
        }

        let eff = self.efficiency_score(&recent);
        if eff < 0.6 {
            recs.push(SleepRecommendation {
                priority: SleepRecommendationPriority::Medium,
                category: "efficiency",
                message: "Consider reducing time in bed awake — only go to bed when sleepy".into(),
                confidence: (1.0 - eff).clamp(0.0, 1.0),
            });
        }

        let con = self.consistency_score(&recent);
        if con < 0.6 {
            let priority = if con < 0.3 {
                SleepRecommendationPriority::High
            } else {
                SleepRecommendationPriority::Medium
            };
            recs.push(SleepRecommendation {
                priority,
                category: "consistency",
                message: "Try to go to sleep and wake up at the same time each day".into(),
                confidence: (1.0 - con).clamp(0.0, 1.0),
            });
        }

        let lat = self.latency_score(&recent);
        if lat < 0.6 {
            recs.push(SleepRecommendation {
                priority: SleepRecommendationPriority::Medium,
                category: "latency",
                message: "Consider a wind-down routine 30 minutes before bed".into(),
                confidence: (1.0 - lat).clamp(0.0, 1.0),
            });
        }

        let dis = self.disturbance_score(&recent);
        if dis < 0.6 {
            recs.push(SleepRecommendation {
                priority: SleepRecommendationPriority::Medium,
                category: "disturbance",
                message: "Frequent awakenings detected — consider sleep environment improvements"
                    .into(),
                confidence: (1.0 - dis).clamp(0.0, 1.0),
            });
        }

        if self.is_late_night_concerning() {
            recs.push(SleepRecommendation {
                priority: SleepRecommendationPriority::High,
                category: "screen_time",
                message:
                    "Reduce screen time after 11 PM — late-night usage is affecting your sleep"
                        .into(),
                confidence: 0.9,
            });
        }

        // Sort: High first, then by confidence descending
        recs.sort_by(|a, b| {
            let pa = a.priority.sort_key();
            let pb = b.priority.sort_key();
            pa.cmp(&pb).then(
                b.confidence
                    .partial_cmp(&a.confidence)
                    .unwrap_or(std::cmp::Ordering::Equal),
            )
        });
        recs.truncate(5);
        recs
    }
}

// ---------------------------------------------------------------------------
// Recommendation types
// ---------------------------------------------------------------------------

/// Priority level for a sleep recommendation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SleepRecommendationPriority {
    High,
    Medium,
    Low,
}

impl SleepRecommendationPriority {
    /// Sorting key: lower = higher priority.
    #[must_use]
    fn sort_key(self) -> u8 {
        match self {
            Self::High => 0,
            Self::Medium => 1,
            Self::Low => 2,
        }
    }
}

/// An actionable sleep improvement recommendation.
#[derive(Debug, Clone)]
pub struct SleepRecommendation {
    /// Priority of this recommendation.
    pub priority: SleepRecommendationPriority,
    /// Category: "duration", "consistency", "latency", "disturbance",
    /// "efficiency", "screen_time".
    pub category: &'static str,
    /// Human-readable recommendation text.
    pub message: String,
    /// Confidence that this recommendation is relevant (0.0–1.0).
    pub confidence: f32,
}

impl Default for SleepTracker {
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

    fn make_record(onset: i64, wake: i64, latency: f32, awakenings: u16) -> SleepRecord {
        SleepRecord {
            onset_at: onset,
            wake_at: wake,
            latency_mins: latency,
            awakenings,
            waso_mins: awakenings as f32 * 5.0, // 5 min per awakening
            subjective_quality: None,
            source: "test".into(),
        }
    }

    #[test]
    fn test_sleep_duration() {
        // 8 hours in bed, 10 min latency, 1 awakening (5 min WASO)
        let r = make_record(0, 8 * 3600, 10.0, 1);
        let dur = r.sleep_duration_hours();
        // 8.0 - 5/60 - 10/60 = 8.0 - 0.083 - 0.167 = 7.75
        assert!((dur - 7.75).abs() < 0.01, "got {dur}");
    }

    #[test]
    fn test_sleep_efficiency() {
        let r = make_record(0, 8 * 3600, 0.0, 0);
        assert!((r.efficiency() - 1.0).abs() < 0.001);

        let r2 = make_record(0, 8 * 3600, 60.0, 0);
        // 7h actual / 8h in bed = 0.875
        assert!(
            (r2.efficiency() - 0.875).abs() < 0.01,
            "got {}",
            r2.efficiency()
        );
    }

    #[test]
    fn test_quality_score_good_sleep() {
        let mut tracker = SleepTracker::new();

        // 7 nights of good sleep
        for i in 0..7 {
            let onset = 86400 * (i + 1) + 22 * 3600 + 30 * 60; // 22:30
            let wake = onset + 8 * 3600; // 8 hours later
            let r = make_record(onset, wake, 10.0, 0);
            tracker.record(r).expect("record");
        }

        let score = tracker.quality_score();
        assert!(score > 0.7, "good sleep should score high, got {score}");
    }

    #[test]
    fn test_quality_score_poor_sleep() {
        let mut tracker = SleepTracker::new();

        // 7 nights of poor sleep
        for i in 0..7 {
            let onset = 86400 * (i + 1) + 2 * 3600; // 02:00 (late)
            let wake = onset + 4 * 3600; // Only 4 hours
            let r = make_record(onset, wake, 45.0, 5);
            tracker.record(r).expect("record");
        }

        let score = tracker.quality_score();
        assert!(score < 0.5, "poor sleep should score low, got {score}");
    }

    #[test]
    fn test_quality_score_no_data() {
        let tracker = SleepTracker::new();
        assert!((tracker.quality_score() - 0.5).abs() < 0.001);
    }

    #[test]
    fn test_late_night_detection() {
        let mut tracker = SleepTracker::new();

        // Day 1: usage at 23:30
        tracker.record_late_night_usage(86400 + 23 * 3600 + 30 * 60);
        assert!(!tracker.is_late_night_concerning());

        // Day 2: usage at 23:15
        tracker.record_late_night_usage(86400 * 2 + 23 * 3600 + 15 * 60);
        assert!(!tracker.is_late_night_concerning());

        // Day 3: usage at 23:45
        tracker.record_late_night_usage(86400 * 3 + 23 * 3600 + 45 * 60);
        assert!(tracker.is_late_night_concerning());
    }

    #[test]
    fn test_late_night_streak_resets() {
        let mut tracker = SleepTracker::new();

        // Day 1, 2: late night
        tracker.record_late_night_usage(86400 + 23 * 3600);
        tracker.record_late_night_usage(86400 * 2 + 23 * 3600);
        assert_eq!(tracker.late_night_streak(), 2);

        // Day 5 (skip day 3-4): should reset
        tracker.record_late_night_usage(86400 * 5 + 23 * 3600);
        assert_eq!(tracker.late_night_streak(), 1);
    }

    #[test]
    fn test_record_validation() {
        let mut tracker = SleepTracker::new();
        // wake before onset should fail
        let bad = SleepRecord {
            onset_at: 10000,
            wake_at: 5000,
            latency_mins: 0.0,
            awakenings: 0,
            waso_mins: 0.0,
            subjective_quality: None,
            source: "test".into(),
        };
        assert!(tracker.record(bad).is_err());
    }

    #[test]
    fn test_ring_buffer_capacity() {
        let mut tracker = SleepTracker::new();
        for i in 0..(MAX_SLEEP_RECORDS + 50) {
            let onset = (i as i64 + 1) * 86400 + 22 * 3600;
            let r = make_record(onset, onset + 8 * 3600, 10.0, 0);
            tracker.record(r).expect("record");
        }
        assert_eq!(tracker.record_count(), MAX_SLEEP_RECORDS);
    }

    // -----------------------------------------------------------------------
    // Recommendation tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_recommendations_insufficient_data() {
        let mut tracker = SleepTracker::new();
        // Only 2 records — below the 3 minimum
        for i in 0..2 {
            let onset = 86400 * (i + 1) + 22 * 3600;
            tracker
                .record(make_record(onset, onset + 8 * 3600, 10.0, 0))
                .expect("record");
        }
        let recs = tracker.generate_recommendations();
        assert!(recs.is_empty(), "should return empty with <3 records");
    }

    #[test]
    fn test_recommendations_good_sleep() {
        let mut tracker = SleepTracker::new();
        // 7 nights of excellent sleep: 8h, low latency, no awakenings, consistent time
        for i in 0..7 {
            let onset = 86400 * (i + 1) + 22 * 3600 + 30 * 60; // 22:30
            let r = make_record(onset, onset + 8 * 3600, 5.0, 0);
            tracker.record(r).expect("record");
        }
        let recs = tracker.generate_recommendations();
        // Good sleep should produce few or no recommendations
        assert!(
            recs.len() <= 1,
            "good sleep: expected <=1 recs, got {}",
            recs.len()
        );
    }

    #[test]
    fn test_recommendations_poor_duration() {
        let mut tracker = SleepTracker::new();
        // 7 nights of very short sleep (4 hours)
        for i in 0..7 {
            let onset = 86400 * (i + 1) + 22 * 3600;
            let r = make_record(onset, onset + 4 * 3600, 5.0, 0);
            tracker.record(r).expect("record");
        }
        let recs = tracker.generate_recommendations();
        assert!(!recs.is_empty(), "poor duration should generate recs");
        let has_duration = recs.iter().any(|r| r.category == "duration");
        assert!(has_duration, "should have duration recommendation");
    }

    #[test]
    fn test_recommendations_late_night() {
        let mut tracker = SleepTracker::new();
        // 7 nights of decent sleep
        for i in 0..7 {
            let onset = 86400 * (i + 1) + 22 * 3600;
            let r = make_record(onset, onset + 8 * 3600, 10.0, 0);
            tracker.record(r).expect("record");
        }
        // 3 consecutive late-night usage days
        for i in 1..=3 {
            tracker.record_late_night_usage(86400 * i + 23 * 3600 + 30 * 60);
        }
        let recs = tracker.generate_recommendations();
        let has_screen = recs.iter().any(|r| r.category == "screen_time");
        assert!(has_screen, "should detect late-night screen usage");
    }

    #[test]
    fn test_recommendations_max_five() {
        let mut tracker = SleepTracker::new();
        // Worst possible sleep: short, high latency, many awakenings, inconsistent
        for i in 0..7 {
            // Vary onset wildly for inconsistency
            let base_onset = 86400 * (i + 1);
            let onset = base_onset + (18 + (i % 6) as i64) * 3600;
            let r = make_record(onset, onset + 3 * 3600, 45.0, 6);
            tracker.record(r).expect("record");
        }
        // Also add late night concern
        for i in 1..=3 {
            tracker.record_late_night_usage(86400 * i + 23 * 3600);
        }
        let recs = tracker.generate_recommendations();
        assert!(
            recs.len() <= 5,
            "should never return more than 5, got {}",
            recs.len()
        );
    }
}
