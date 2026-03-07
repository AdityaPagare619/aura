//! Vital signs monitoring with online statistics (spec §3.2).
//!
//! Uses Welford's algorithm for single-pass mean and variance computation,
//! enabling z-score anomaly detection without storing the full time series.
//! An anomaly is flagged when |z| >= 2.5 (configurable).

use serde::{Deserialize, Serialize};
use tracing::warn;

use crate::arc::ArcError;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum number of distinct vital-sign types tracked.
const MAX_VITAL_TYPES: usize = 16;

/// Maximum readings retained per vital type (ring buffer).
const MAX_READINGS_PER_TYPE: usize = 1440; // ~1 per minute for 24h

/// Default z-score threshold for anomaly detection.
const DEFAULT_ANOMALY_Z: f64 = 2.5;

/// Minimum samples before z-score is meaningful.
const MIN_SAMPLES_FOR_ZSCORE: u64 = 10;

// ---------------------------------------------------------------------------
// VitalType
// ---------------------------------------------------------------------------

/// Known vital sign types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum VitalType {
    HeartRate = 0,
    BloodPressureSystolic = 1,
    BloodPressureDiastolic = 2,
    SpO2 = 3,
    Temperature = 4,
    RespiratoryRate = 5,
    BloodGlucose = 6,
    Hrv = 7,
}

impl VitalType {
    /// All known vital types.
    pub const ALL: [VitalType; 8] = [
        VitalType::HeartRate,
        VitalType::BloodPressureSystolic,
        VitalType::BloodPressureDiastolic,
        VitalType::SpO2,
        VitalType::Temperature,
        VitalType::RespiratoryRate,
        VitalType::BloodGlucose,
        VitalType::Hrv,
    ];
}

impl std::fmt::Display for VitalType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self)
    }
}

// ---------------------------------------------------------------------------
// VitalReading
// ---------------------------------------------------------------------------

/// A single vital sign reading.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VitalReading {
    pub vital_type: VitalType,
    pub value: f64,
    /// Unix epoch seconds.
    pub timestamp: i64,
    /// Optional source identifier (e.g., "watch", "manual").
    pub source: String,
}

// ---------------------------------------------------------------------------
// WelfordAccumulator — online mean/variance
// ---------------------------------------------------------------------------

/// Online computation of mean and variance using Welford's algorithm.
///
/// Numerically stable, single-pass. Supports incremental updates.
/// Memory: O(1) per vital type regardless of reading count.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WelfordAccumulator {
    /// Number of samples ingested.
    count: u64,
    /// Running mean.
    mean: f64,
    /// Running sum of squared deviations from the mean (M2).
    m2: f64,
}

impl WelfordAccumulator {
    /// Create a new empty accumulator.
    #[must_use]
    pub fn new() -> Self {
        Self {
            count: 0,
            mean: 0.0,
            m2: 0.0,
        }
    }

    /// Ingest a new sample value.
    pub fn update(&mut self, value: f64) {
        self.count += 1;
        let delta = value - self.mean;
        self.mean += delta / self.count as f64;
        let delta2 = value - self.mean;
        self.m2 += delta * delta2;
    }

    /// Current sample count.
    #[must_use]
    pub fn count(&self) -> u64 {
        self.count
    }

    /// Running mean.
    #[must_use]
    pub fn mean(&self) -> f64 {
        self.mean
    }

    /// Population variance.
    #[must_use]
    pub fn variance(&self) -> f64 {
        if self.count < 2 {
            return 0.0;
        }
        self.m2 / self.count as f64
    }

    /// Sample variance (Bessel's correction).
    #[must_use]
    pub fn sample_variance(&self) -> f64 {
        if self.count < 2 {
            return 0.0;
        }
        self.m2 / (self.count - 1) as f64
    }

    /// Standard deviation (population).
    #[must_use]
    pub fn stddev(&self) -> f64 {
        self.variance().sqrt()
    }

    /// Compute the z-score for a given value.
    ///
    /// Returns `None` if insufficient samples or zero variance.
    #[must_use]
    pub fn z_score(&self, value: f64) -> Option<f64> {
        if self.count < MIN_SAMPLES_FOR_ZSCORE {
            return None;
        }
        let sd = self.stddev();
        if sd < f64::EPSILON {
            return None;
        }
        Some((value - self.mean) / sd)
    }

    /// Whether a value is anomalous (|z| >= threshold).
    #[must_use]
    pub fn is_anomalous(&self, value: f64, threshold: f64) -> bool {
        self.z_score(value)
            .map(|z| z.abs() >= threshold)
            .unwrap_or(false)
    }
}

impl Default for WelfordAccumulator {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// VitalChannel — per-type tracker
// ---------------------------------------------------------------------------

/// Tracks a single vital type's readings and statistics.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct VitalChannel {
    vital_type: VitalType,
    accumulator: WelfordAccumulator,
    /// Ring buffer of recent readings.
    recent: Vec<VitalReading>,
    cursor: usize,
    /// Z-score threshold for anomaly detection.
    anomaly_threshold: f64,
}

impl VitalChannel {
    fn new(vital_type: VitalType) -> Self {
        Self {
            vital_type,
            accumulator: WelfordAccumulator::new(),
            recent: Vec::with_capacity(64),
            cursor: 0,
            anomaly_threshold: DEFAULT_ANOMALY_Z,
        }
    }

    fn ingest(&mut self, reading: VitalReading) -> bool {
        let is_anomaly = self
            .accumulator
            .is_anomalous(reading.value, self.anomaly_threshold);

        self.accumulator.update(reading.value);

        if self.recent.len() < MAX_READINGS_PER_TYPE {
            self.recent.push(reading);
        } else {
            let idx = self.cursor % MAX_READINGS_PER_TYPE;
            self.recent[idx] = reading;
        }
        self.cursor += 1;

        is_anomaly
    }
}

// ---------------------------------------------------------------------------
// AnomalyRecord
// ---------------------------------------------------------------------------

/// A detected vital sign anomaly.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnomalyRecord {
    pub vital_type: VitalType,
    pub value: f64,
    pub z_score: f64,
    pub mean: f64,
    pub stddev: f64,
    pub timestamp: i64,
}

// ---------------------------------------------------------------------------
// VitalsMonitor
// ---------------------------------------------------------------------------

/// Monitors multiple vital-sign channels with online anomaly detection.
#[derive(Debug, Serialize, Deserialize)]
pub struct VitalsMonitor {
    channels: Vec<VitalChannel>,
    /// Recent anomalies (bounded ring buffer).
    anomalies: Vec<AnomalyRecord>,
    anomaly_cursor: usize,
}

/// Maximum stored anomalies.
const MAX_ANOMALIES: usize = 100;

impl VitalsMonitor {
    /// Create a new monitor with no channels.
    #[must_use]
    pub fn new() -> Self {
        Self {
            channels: Vec::with_capacity(MAX_VITAL_TYPES),
            anomalies: Vec::with_capacity(16),
            anomaly_cursor: 0,
        }
    }

    /// Ingest a vital-sign reading.
    ///
    /// Automatically creates a channel for new vital types.
    /// Returns `Some(anomaly)` if the reading is anomalous.
    pub fn ingest(&mut self, reading: VitalReading) -> Result<Option<AnomalyRecord>, ArcError> {
        let channel = self.get_or_create_channel(reading.vital_type)?;

        let value = reading.value;
        let timestamp = reading.timestamp;
        let vtype = reading.vital_type;

        // Get pre-ingest stats for anomaly record
        let mean = channel.accumulator.mean();
        let stddev = channel.accumulator.stddev();
        let z = channel.accumulator.z_score(value);

        let is_anomaly = channel.ingest(reading);

        if is_anomaly {
            let z_val = z.unwrap_or(0.0);
            let anomaly = AnomalyRecord {
                vital_type: vtype,
                value,
                z_score: z_val,
                mean,
                stddev,
                timestamp,
            };

            warn!(
                vital = %vtype,
                value,
                z_score = z_val,
                "vital sign anomaly detected"
            );

            // Store in bounded anomaly buffer
            if self.anomalies.len() < MAX_ANOMALIES {
                self.anomalies.push(anomaly.clone());
            } else {
                let idx = self.anomaly_cursor % MAX_ANOMALIES;
                self.anomalies[idx] = anomaly.clone();
            }
            self.anomaly_cursor += 1;

            Ok(Some(anomaly))
        } else {
            Ok(None)
        }
    }

    /// Get or create a channel for the given vital type.
    fn get_or_create_channel(
        &mut self,
        vital_type: VitalType,
    ) -> Result<&mut VitalChannel, ArcError> {
        // Check if channel already exists
        let idx = self
            .channels
            .iter()
            .position(|c| c.vital_type == vital_type);
        if let Some(i) = idx {
            return Ok(&mut self.channels[i]);
        }

        // Create new channel
        if self.channels.len() >= MAX_VITAL_TYPES {
            return Err(ArcError::CapacityExceeded {
                collection: "vital_channels".into(),
                max: MAX_VITAL_TYPES,
            });
        }

        self.channels.push(VitalChannel::new(vital_type));
        let last = self.channels.len() - 1;
        Ok(&mut self.channels[last])
    }

    /// Composite vitals score (0.0 to 1.0).
    ///
    /// Based on how many channels have sufficient data and how stable
    /// recent readings are (low variance relative to expected ranges).
    #[must_use]
    pub fn composite_score(&self) -> f32 {
        if self.channels.is_empty() {
            return 0.5; // Neutral when no data
        }

        let mut score_sum = 0.0_f32;
        let mut count = 0u32;

        for ch in &self.channels {
            if ch.accumulator.count() < MIN_SAMPLES_FOR_ZSCORE {
                continue;
            }
            // Score per channel: 1.0 if stable, lower if recent readings are anomalous
            let recent_anomaly_rate = self.recent_anomaly_rate(ch.vital_type);
            let stability = 1.0 - recent_anomaly_rate;
            score_sum += stability as f32;
            count += 1;
        }

        if count > 0 {
            (score_sum / count as f32).clamp(0.0, 1.0)
        } else {
            0.5
        }
    }

    /// Anomaly rate in recent readings for a vital type.
    fn recent_anomaly_rate(&self, vital_type: VitalType) -> f64 {
        let recent_anomalies = self
            .anomalies
            .iter()
            .filter(|a| a.vital_type == vital_type)
            .count();
        let total_readings = self
            .channels
            .iter()
            .find(|c| c.vital_type == vital_type)
            .map(|c| c.accumulator.count())
            .unwrap_or(1);

        if total_readings == 0 {
            return 0.0;
        }
        (recent_anomalies as f64 / total_readings as f64).min(1.0)
    }

    /// Total number of readings across all channels.
    #[must_use]
    pub fn reading_count(&self) -> usize {
        self.channels
            .iter()
            .map(|c| c.accumulator.count() as usize)
            .sum()
    }

    /// Access recent anomalies.
    #[must_use]
    pub fn recent_anomalies(&self) -> &[AnomalyRecord] {
        &self.anomalies
    }

    /// Get the accumulator for a specific vital type.
    #[must_use]
    pub fn accumulator_for(&self, vital_type: VitalType) -> Option<&WelfordAccumulator> {
        self.channels
            .iter()
            .find(|c| c.vital_type == vital_type)
            .map(|c| &c.accumulator)
    }
}

impl Default for VitalsMonitor {
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
    fn test_welford_basic() {
        let mut acc = WelfordAccumulator::new();
        // Values: 2, 4, 4, 4, 5, 5, 7, 9
        for &v in &[2.0, 4.0, 4.0, 4.0, 5.0, 5.0, 7.0, 9.0] {
            acc.update(v);
        }
        assert_eq!(acc.count(), 8);
        assert!((acc.mean() - 5.0).abs() < 0.001, "mean = {}", acc.mean());
        // Population variance = 4.0
        assert!(
            (acc.variance() - 4.0).abs() < 0.001,
            "var = {}",
            acc.variance()
        );
        // stddev = 2.0
        assert!((acc.stddev() - 2.0).abs() < 0.001);
    }

    #[test]
    fn test_welford_z_score() {
        let mut acc = WelfordAccumulator::new();
        // Build up enough samples (mean=50, stddev=10 roughly)
        for i in 0..100 {
            acc.update(50.0 + (i as f64 % 20.0) - 10.0);
        }
        let z = acc.z_score(50.0);
        assert!(z.is_some());
        // 50 is near the mean, z should be near 0
        assert!(z.expect("z").abs() < 1.0);
    }

    #[test]
    fn test_welford_insufficient_samples() {
        let mut acc = WelfordAccumulator::new();
        acc.update(5.0);
        acc.update(10.0);
        assert!(
            acc.z_score(7.5).is_none(),
            "need >= {MIN_SAMPLES_FOR_ZSCORE} samples"
        );
    }

    #[test]
    fn test_anomaly_detection() {
        let mut acc = WelfordAccumulator::new();
        // Normal heart rates: 60-100
        for i in 0..50 {
            acc.update(70.0 + (i as f64 % 10.0));
        }
        // 200 BPM should be anomalous
        assert!(acc.is_anomalous(200.0, DEFAULT_ANOMALY_Z));
        // 75 should not be
        assert!(!acc.is_anomalous(75.0, DEFAULT_ANOMALY_Z));
    }

    #[test]
    fn test_vitals_monitor_ingest() {
        let mut monitor = VitalsMonitor::new();
        let reading = VitalReading {
            vital_type: VitalType::HeartRate,
            value: 72.0,
            timestamp: 1000,
            source: "watch".into(),
        };
        let result = monitor.ingest(reading).expect("ingest");
        assert!(result.is_none()); // Not enough data for anomaly
        assert_eq!(monitor.reading_count(), 1);
    }

    #[test]
    fn test_vitals_monitor_detects_anomaly() {
        let mut monitor = VitalsMonitor::new();

        // Feed normal readings
        for i in 0..50 {
            let r = VitalReading {
                vital_type: VitalType::HeartRate,
                value: 72.0 + (i as f64 % 5.0),
                timestamp: 1000 + i * 60,
                source: "watch".into(),
            };
            let _ = monitor.ingest(r);
        }

        // Feed anomalous reading
        let anomalous = VitalReading {
            vital_type: VitalType::HeartRate,
            value: 200.0,
            timestamp: 5000,
            source: "watch".into(),
        };
        let result = monitor.ingest(anomalous).expect("ingest");
        assert!(result.is_some(), "should detect anomaly");
        assert_eq!(monitor.recent_anomalies().len(), 1);
    }

    #[test]
    fn test_composite_score_neutral() {
        let monitor = VitalsMonitor::new();
        assert!((monitor.composite_score() - 0.5).abs() < 0.001);
    }

    #[test]
    fn test_channel_capacity() {
        let mut monitor = VitalsMonitor::new();
        // VitalType only has 8 variants, well under MAX_VITAL_TYPES
        for &vt in &VitalType::ALL {
            let r = VitalReading {
                vital_type: vt,
                value: 50.0,
                timestamp: 1000,
                source: "test".into(),
            };
            assert!(monitor.ingest(r).is_ok());
        }
    }
}
