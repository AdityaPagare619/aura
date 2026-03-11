//! Heartbeat Health System — periodic self-diagnostic for AURA v4.
//!
//! Monitors daemon health across multiple dimensions:
//! - **Neocortex liveness**: is the inference engine responding?
//! - **Resource pressure**: memory, storage, battery, thermal state
//! - **Functional status**: error rates, consecutive healthy checks
//! - **Session tracking**: uptime, session number, last successful action
//!
//! The health monitor runs on a configurable interval (default 30s, 60s in
//! low-power mode) and produces a [`HealthReport`] that can be serialized
//! for checkpoint persistence or formatted for Telegram alerts.
//!
//! # Spec Reference
//!
//! See `03-AURA-MISSING-FOUNDATIONS.md` — Foundation 9: Heartbeat Health System.

pub mod monitor;

pub use monitor::{HealthMonitor, HealthReport, HealthStatus, ThermalState};
