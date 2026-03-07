//! Hardware sensor access for AURA v4.
//!
//! Provides access to device sensors (accelerometer, light, proximity, step
//! counter) via JNI on Android, with desktop mock implementations for testing.
//!
//! # Architecture
//!
//! On Android, sensor data is fetched from `AuraDaemonBridge` static methods
//! that read from the Android `SensorManager`. On desktop, mock values are
//! returned for testing.
//!
//! # Usage
//!
//! ```ignore
//! let mut sm = SensorManager::new();
//! sm.poll()?;
//! if sm.is_device_stationary() {
//!     // User hasn't moved — safe for background work
//! }
//! ```

use std::time::{Duration, Instant};

use aura_types::errors::PlatformError;
use serde::{Deserialize, Serialize};

// ─── Constants ──────────────────────────────────────────────────────────────

/// Maximum number of samples in the accelerometer history ring.
const ACCEL_HISTORY_SIZE: usize = 16;

/// Movement threshold (m/s²) — below this, the device is considered stationary.
const STATIONARY_THRESHOLD: f32 = 0.5;

/// Minimum number of stationary samples required before declaring stationary.
const STATIONARY_SAMPLE_COUNT: usize = 4;

/// Light level threshold for "dark" environment (lux).
const DARK_LUX_THRESHOLD: f32 = 10.0;

/// Light level threshold for "bright" environment (lux).
const BRIGHT_LUX_THRESHOLD: f32 = 500.0;

// ─── Sensor Types ───────────────────────────────────────────────────────────

/// 3-axis accelerometer reading in m/s².
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct AccelerometerReading {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

impl AccelerometerReading {
    /// Magnitude of the acceleration vector (should be ~9.8 when stationary).
    pub fn magnitude(&self) -> f32 {
        (self.x * self.x + self.y * self.y + self.z * self.z).sqrt()
    }

    /// Deviation from Earth gravity (|magnitude - 9.81|).
    ///
    /// A low deviation indicates the device is stationary.
    pub fn gravity_deviation(&self) -> f32 {
        (self.magnitude() - 9.81).abs()
    }
}

/// Ambient light level classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AmbientLight {
    /// Very dark — device likely in pocket or screen-off.
    Dark,
    /// Normal indoor lighting.
    Normal,
    /// Bright outdoor or direct light.
    Bright,
}

impl std::fmt::Display for AmbientLight {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Dark => write!(f, "Dark"),
            Self::Normal => write!(f, "Normal"),
            Self::Bright => write!(f, "Bright"),
        }
    }
}

/// Proximity sensor state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProximityState {
    /// Object is near the sensor (e.g., phone to ear, in pocket).
    Near,
    /// No object detected near the sensor.
    Far,
    /// Proximity sensor data unavailable.
    Unknown,
}

impl std::fmt::Display for ProximityState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Near => write!(f, "Near"),
            Self::Far => write!(f, "Far"),
            Self::Unknown => write!(f, "Unknown"),
        }
    }
}

/// Aggregated snapshot of all sensor readings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SensorSnapshot {
    pub accelerometer: AccelerometerReading,
    pub light_lux: f32,
    pub ambient_light: AmbientLight,
    pub proximity: ProximityState,
    pub step_count: u32,
    pub is_stationary: bool,
}

// ─── SensorManager ──────────────────────────────────────────────────────────

/// Manages hardware sensor reads and derived state (stationary detection, etc.).
pub struct SensorManager {
    /// Latest accelerometer reading.
    accelerometer: AccelerometerReading,
    /// Accelerometer history ring for motion detection.
    accel_history: Vec<AccelerometerReading>,
    /// Latest ambient light level (lux).
    light_lux: f32,
    /// Classified ambient light level.
    ambient_light: AmbientLight,
    /// Latest proximity state.
    proximity: ProximityState,
    /// Cumulative step count since last reset.
    step_count: u32,
    /// Step count at last reset (for delta calculation).
    step_count_baseline: u32,
    /// Whether the device is currently stationary.
    is_stationary: bool,
    /// Timestamp of last sensor poll.
    last_poll: Instant,
    /// Number of consecutive stationary samples.
    stationary_streak: usize,
}

impl SensorManager {
    /// Create a new `SensorManager` with default (idle) values.
    pub fn new() -> Self {
        Self {
            accelerometer: AccelerometerReading::default(),
            accel_history: Vec::with_capacity(ACCEL_HISTORY_SIZE),
            light_lux: 100.0,
            ambient_light: AmbientLight::Normal,
            proximity: ProximityState::Unknown,
            step_count: 0,
            step_count_baseline: 0,
            is_stationary: true,
            last_poll: Instant::now(),
            stationary_streak: 0,
        }
    }

    /// Poll all sensors and update internal state.
    ///
    /// On Android, this calls JNI methods to read sensor values.
    /// On desktop, mock values are used.
    pub fn poll(&mut self) -> Result<SensorSnapshot, PlatformError> {
        // Read raw sensor values from platform.
        let accel = read_accelerometer()?;
        let light = read_light_sensor()?;
        let proximity = read_proximity()?;
        let steps = read_step_counter()?;

        // Update accelerometer state.
        self.accelerometer = accel;
        self.record_accel_sample(accel);

        // Update light state.
        self.light_lux = light;
        self.ambient_light = classify_light(light);

        // Update proximity.
        self.proximity = proximity;

        // Update step count.
        self.step_count = steps.saturating_sub(self.step_count_baseline);

        // Update stationary detection.
        self.update_stationary();

        self.last_poll = Instant::now();

        Ok(self.snapshot())
    }

    /// Get a snapshot of the current sensor state without polling.
    pub fn snapshot(&self) -> SensorSnapshot {
        SensorSnapshot {
            accelerometer: self.accelerometer,
            light_lux: self.light_lux,
            ambient_light: self.ambient_light,
            proximity: self.proximity,
            step_count: self.step_count,
            is_stationary: self.is_stationary,
        }
    }

    /// Whether the device appears stationary (low accelerometer variance).
    pub fn is_device_stationary(&self) -> bool {
        self.is_stationary
    }

    /// Whether the device appears to be in a pocket (near + dark).
    pub fn is_likely_in_pocket(&self) -> bool {
        self.proximity == ProximityState::Near && self.ambient_light == AmbientLight::Dark
    }

    /// Current ambient light classification.
    pub fn ambient_light(&self) -> AmbientLight {
        self.ambient_light
    }

    /// Current proximity state.
    pub fn proximity(&self) -> ProximityState {
        self.proximity
    }

    /// Steps since last reset.
    pub fn step_count(&self) -> u32 {
        self.step_count
    }

    /// Reset the step counter baseline to the current platform value.
    pub fn reset_step_count(&mut self) {
        if let Ok(current) = read_step_counter() {
            self.step_count_baseline = current;
            self.step_count = 0;
        }
    }

    /// Time since the last sensor poll.
    pub fn time_since_last_poll(&self) -> Duration {
        self.last_poll.elapsed()
    }

    // ─── Internal ───────────────────────────────────────────────────────

    /// Record an accelerometer sample in the history ring.
    fn record_accel_sample(&mut self, sample: AccelerometerReading) {
        if self.accel_history.len() >= ACCEL_HISTORY_SIZE {
            self.accel_history.remove(0);
        }
        self.accel_history.push(sample);
    }

    /// Update the stationary detection based on recent accelerometer history.
    fn update_stationary(&mut self) {
        if self.accel_history.len() < 2 {
            return;
        }

        // Check if the latest reading shows low gravity deviation.
        if self.accelerometer.gravity_deviation() < STATIONARY_THRESHOLD {
            self.stationary_streak += 1;
        } else {
            self.stationary_streak = 0;
        }

        self.is_stationary = self.stationary_streak >= STATIONARY_SAMPLE_COUNT;
    }
}

impl Default for SensorManager {
    fn default() -> Self {
        Self::new()
    }
}

// ─── Light Classification ───────────────────────────────────────────────────

/// Classify a raw lux reading into an ambient light level.
fn classify_light(lux: f32) -> AmbientLight {
    if lux < DARK_LUX_THRESHOLD {
        AmbientLight::Dark
    } else if lux > BRIGHT_LUX_THRESHOLD {
        AmbientLight::Bright
    } else {
        AmbientLight::Normal
    }
}

// ─── Platform Reads (cfg-gated) ─────────────────────────────────────────────

/// Read accelerometer from the platform.
#[cfg(target_os = "android")]
fn read_accelerometer() -> Result<AccelerometerReading, PlatformError> {
    let (x, y, z) = super::jni_bridge::jni_get_accelerometer()?;
    Ok(AccelerometerReading { x, y, z })
}

#[cfg(not(target_os = "android"))]
fn read_accelerometer() -> Result<AccelerometerReading, PlatformError> {
    // Mock: device sitting flat on a table (gravity along -Z axis).
    Ok(AccelerometerReading {
        x: 0.02,
        y: 0.01,
        z: 9.81,
    })
}

/// Read ambient light sensor (lux).
#[cfg(target_os = "android")]
fn read_light_sensor() -> Result<f32, PlatformError> {
    super::jni_bridge::jni_get_light_level()
}

#[cfg(not(target_os = "android"))]
fn read_light_sensor() -> Result<f32, PlatformError> {
    // Mock: typical indoor lighting.
    Ok(150.0)
}

/// Read proximity sensor state.
#[cfg(target_os = "android")]
fn read_proximity() -> Result<ProximityState, PlatformError> {
    let near = super::jni_bridge::jni_is_proximity_near()?;
    Ok(if near {
        ProximityState::Near
    } else {
        ProximityState::Far
    })
}

#[cfg(not(target_os = "android"))]
fn read_proximity() -> Result<ProximityState, PlatformError> {
    Ok(ProximityState::Far)
}

/// Read step counter (cumulative since boot).
#[cfg(target_os = "android")]
fn read_step_counter() -> Result<u32, PlatformError> {
    super::jni_bridge::jni_get_step_count()
}

#[cfg(not(target_os = "android"))]
fn read_step_counter() -> Result<u32, PlatformError> {
    Ok(0)
}

// ─── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sensor_manager_default() {
        let sm = SensorManager::new();
        assert!(sm.is_device_stationary());
        assert_eq!(sm.step_count(), 0);
        assert_eq!(sm.ambient_light(), AmbientLight::Normal);
        assert_eq!(sm.proximity(), ProximityState::Unknown);
    }

    #[test]
    fn test_poll_returns_snapshot() {
        let mut sm = SensorManager::new();
        let snapshot = sm.poll().expect("poll should succeed on host");
        assert!(snapshot.light_lux > 0.0);
        assert_eq!(snapshot.proximity, ProximityState::Far);
    }

    #[test]
    fn test_accelerometer_magnitude_at_rest() {
        let reading = AccelerometerReading {
            x: 0.0,
            y: 0.0,
            z: 9.81,
        };
        assert!((reading.magnitude() - 9.81).abs() < 0.01);
        assert!(reading.gravity_deviation() < STATIONARY_THRESHOLD);
    }

    #[test]
    fn test_accelerometer_magnitude_in_motion() {
        let reading = AccelerometerReading {
            x: 3.0,
            y: 4.0,
            z: 9.81,
        };
        // magnitude = sqrt(9+16+96.24) ≈ 11.0
        assert!(reading.gravity_deviation() > STATIONARY_THRESHOLD);
    }

    #[test]
    fn test_light_classification_dark() {
        assert_eq!(classify_light(2.0), AmbientLight::Dark);
        assert_eq!(classify_light(0.0), AmbientLight::Dark);
    }

    #[test]
    fn test_light_classification_normal() {
        assert_eq!(classify_light(100.0), AmbientLight::Normal);
        assert_eq!(classify_light(300.0), AmbientLight::Normal);
    }

    #[test]
    fn test_light_classification_bright() {
        assert_eq!(classify_light(600.0), AmbientLight::Bright);
        assert_eq!(classify_light(10000.0), AmbientLight::Bright);
    }

    #[test]
    fn test_stationary_detection_after_multiple_polls() {
        let mut sm = SensorManager::new();
        // Reset the streak since constructor starts with stationary=true
        sm.is_stationary = false;
        sm.stationary_streak = 0;

        // Poll multiple times — mock always returns stationary readings.
        for _ in 0..(STATIONARY_SAMPLE_COUNT + 1) {
            sm.poll().expect("poll");
        }
        assert!(sm.is_device_stationary());
    }

    #[test]
    fn test_is_likely_in_pocket() {
        let mut sm = SensorManager::new();
        sm.proximity = ProximityState::Near;
        sm.ambient_light = AmbientLight::Dark;
        assert!(sm.is_likely_in_pocket());

        sm.proximity = ProximityState::Far;
        assert!(!sm.is_likely_in_pocket());

        sm.proximity = ProximityState::Near;
        sm.ambient_light = AmbientLight::Normal;
        assert!(!sm.is_likely_in_pocket());
    }

    #[test]
    fn test_step_count_reset() {
        let mut sm = SensorManager::new();
        sm.step_count = 100;
        sm.reset_step_count();
        assert_eq!(sm.step_count(), 0);
    }

    #[test]
    fn test_accel_history_bounded() {
        let mut sm = SensorManager::new();
        for i in 0..(ACCEL_HISTORY_SIZE + 5) {
            sm.record_accel_sample(AccelerometerReading {
                x: i as f32,
                y: 0.0,
                z: 9.81,
            });
        }
        assert_eq!(sm.accel_history.len(), ACCEL_HISTORY_SIZE);
    }

    #[test]
    fn test_sensor_snapshot_fields() {
        let mut sm = SensorManager::new();
        let snap = sm.poll().expect("poll");
        // Desktop mock values.
        assert!((snap.light_lux - 150.0).abs() < f32::EPSILON);
        assert_eq!(snap.ambient_light, AmbientLight::Normal);
        assert_eq!(snap.proximity, ProximityState::Far);
        assert_eq!(snap.step_count, 0);
    }

    #[test]
    fn test_proximity_state_display() {
        assert_eq!(ProximityState::Near.to_string(), "Near");
        assert_eq!(ProximityState::Far.to_string(), "Far");
        assert_eq!(ProximityState::Unknown.to_string(), "Unknown");
    }

    #[test]
    fn test_ambient_light_display() {
        assert_eq!(AmbientLight::Dark.to_string(), "Dark");
        assert_eq!(AmbientLight::Normal.to_string(), "Normal");
        assert_eq!(AmbientLight::Bright.to_string(), "Bright");
    }

    #[test]
    fn test_time_since_last_poll() {
        let sm = SensorManager::new();
        let elapsed = sm.time_since_last_poll();
        // Should be very small since we just created it.
        assert!(elapsed < Duration::from_secs(1));
    }

    #[test]
    fn test_desktop_mock_accelerometer() {
        let accel = read_accelerometer().expect("mock");
        assert!((accel.z - 9.81).abs() < 0.01);
    }

    #[test]
    fn test_desktop_mock_light() {
        let lux = read_light_sensor().expect("mock");
        assert!((lux - 150.0).abs() < f32::EPSILON);
    }

    #[test]
    fn test_desktop_mock_proximity() {
        let prox = read_proximity().expect("mock");
        assert_eq!(prox, ProximityState::Far);
    }

    #[test]
    fn test_desktop_mock_steps() {
        let steps = read_step_counter().expect("mock");
        assert_eq!(steps, 0);
    }
}
