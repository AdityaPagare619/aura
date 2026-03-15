//! Named atomic counters for daemon self-monitoring.
//!
//! `CounterSet` holds up to 64 named counters. Each counter uses `AtomicU64`
//! so it can be safely incremented from any thread without a mutex.

use std::{
    fmt,
    sync::atomic::{AtomicU64, Ordering},
};

// ---------------------------------------------------------------------------
// Constants — pre-defined counter names
// ---------------------------------------------------------------------------

/// Pre-defined counter names. The daemon registers these at startup.
pub const PREDEFINED_COUNTERS: &[&str] = &[
    "events_received",
    "events_processed",
    "events_dropped",
    "inference_calls",
    "inference_tokens",
    "inference_latency_us",
    "actions_executed",
    "actions_succeeded",
    "actions_failed",
    "memory_queries",
    "memory_stores",
    "memory_evictions",
    "goals_created",
    "goals_completed",
    "goals_failed",
    "ipc_messages_sent",
    "ipc_messages_received",
    "ipc_errors",
    "cycle_detections",
    "safety_violations",
    "checkpoint_saves",
    "checkpoint_load_ms",
];

/// Maximum number of named counters.
const MAX_COUNTERS: usize = 64;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// A single named counter backed by an atomic u64.
pub struct NamedCounter {
    /// Name stored as fixed-size byte array (no heap alloc on hot path).
    name: [u8; 32],
    /// Current counter value.
    value: AtomicU64,
    /// Timestamp (ms) when this counter was last reset.
    last_reset_ms: AtomicU64,
}

impl NamedCounter {
    /// Create a new counter with the given name and reset timestamp.
    fn new(name: &str, now_ms: u64) -> Self {
        Self {
            name: encode_counter_name(name),
            value: AtomicU64::new(0),
            last_reset_ms: AtomicU64::new(now_ms),
        }
    }

    /// Extract the name as `&str`.
    pub fn name_str(&self) -> &str {
        let end = self.name.iter().position(|&b| b == 0).unwrap_or(32);
        std::str::from_utf8(&self.name[..end]).unwrap_or("")
    }

    /// Current value (relaxed load — suitable for monitoring, not for synchronization).
    pub fn get(&self) -> u64 {
        self.value.load(Ordering::Relaxed)
    }

    /// Increment by 1. Lock-free, can be called from any thread.
    pub fn increment(&self) {
        self.value.fetch_add(1, Ordering::Relaxed);
    }

    /// Increment by an arbitrary delta.
    pub fn increment_by(&self, delta: u64) {
        self.value.fetch_add(delta, Ordering::Relaxed);
    }

    /// Reset the counter to 0 and update the reset timestamp.
    pub fn reset(&self, now_ms: u64) {
        self.value.store(0, Ordering::Relaxed);
        self.last_reset_ms.store(now_ms, Ordering::Relaxed);
    }

    /// Timestamp of last reset.
    pub fn last_reset_ms(&self) -> u64 {
        self.last_reset_ms.load(Ordering::Relaxed)
    }
}

impl fmt::Debug for NamedCounter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("NamedCounter")
            .field("name", &self.name_str())
            .field("value", &self.get())
            .field("last_reset_ms", &self.last_reset_ms())
            .finish()
    }
}

/// Encode a counter name into a `[u8; 32]` array.
fn encode_counter_name(name: &str) -> [u8; 32] {
    let mut buf = [0u8; 32];
    let bytes = name.as_bytes();
    let copy_len = bytes.len().min(31);
    buf[..copy_len].copy_from_slice(&bytes[..copy_len]);
    buf
}

// ---------------------------------------------------------------------------
// CounterSet
// ---------------------------------------------------------------------------

/// A bounded set of named counters. Maximum 64 counters.
///
/// Counters are stored in a fixed-size array. Registration is fallible;
/// once 64 counters are registered, new registrations return an error.
///
/// Increment operations are lock-free (`AtomicU64::fetch_add`).
pub struct CounterSet {
    /// Fixed-capacity backing storage. We use `Option` to allow moving
    /// `NamedCounter` out for initialization without `Default` on atomics.
    counters: Vec<NamedCounter>,
}

impl CounterSet {
    /// Create a new, empty counter set.
    pub fn new() -> Self {
        Self {
            counters: Vec::with_capacity(MAX_COUNTERS),
        }
    }

    /// Create a counter set pre-populated with all predefined counters.
    pub fn with_defaults(now_ms: u64) -> Self {
        let mut set = Self::new();
        for name in PREDEFINED_COUNTERS {
            // Predefined list is < MAX_COUNTERS, so this always succeeds.
            let _ = set.register(name, now_ms);
        }
        set
    }

    /// Number of registered counters.
    pub fn count(&self) -> usize {
        self.counters.len()
    }

    /// Register a new named counter.
    ///
    /// # Errors
    /// Returns `CounterError::CapacityExceeded` if 64 counters are already
    /// registered, or `CounterError::AlreadyExists` if the name is taken.
    pub fn register(&mut self, name: &str, now_ms: u64) -> Result<(), CounterError> {
        if self.counters.len() >= MAX_COUNTERS {
            return Err(CounterError::CapacityExceeded { max: MAX_COUNTERS });
        }
        let encoded = encode_counter_name(name);
        if self.counters.iter().any(|c| c.name == encoded) {
            return Err(CounterError::AlreadyExists(name.to_string()));
        }
        self.counters.push(NamedCounter::new(name, now_ms));
        Ok(())
    }

    /// Look up a counter by name.
    pub fn get(&self, name: &str) -> Option<&NamedCounter> {
        let encoded = encode_counter_name(name);
        self.counters.iter().find(|c| c.name == encoded)
    }

    /// Increment a named counter by 1.
    ///
    /// # Errors
    /// Returns `CounterError::NotFound` if no counter with that name exists.
    pub fn increment(&self, name: &str) -> Result<(), CounterError> {
        match self.get(name) {
            Some(c) => {
                c.increment();
                Ok(())
            },
            None => Err(CounterError::NotFound(name.to_string())),
        }
    }

    /// Increment a named counter by a delta.
    ///
    /// # Errors
    /// Returns `CounterError::NotFound` if no counter with that name exists.
    pub fn increment_by(&self, name: &str, delta: u64) -> Result<(), CounterError> {
        match self.get(name) {
            Some(c) => {
                c.increment_by(delta);
                Ok(())
            },
            None => Err(CounterError::NotFound(name.to_string())),
        }
    }

    /// Reset all counters to zero.
    pub fn reset_all(&self, now_ms: u64) {
        for c in &self.counters {
            c.reset(now_ms);
        }
    }

    /// Snapshot of all counter values as `(name, value)` pairs.
    pub fn snapshot(&self) -> Vec<CounterSnapshot> {
        self.counters
            .iter()
            .map(|c| CounterSnapshot {
                name: c.name_str().to_string(),
                value: c.get(),
                last_reset_ms: c.last_reset_ms(),
            })
            .collect()
    }
}

impl fmt::Debug for CounterSet {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CounterSet")
            .field("count", &self.counters.len())
            .finish()
    }
}

// ---------------------------------------------------------------------------
// Snapshot / error types
// ---------------------------------------------------------------------------

/// Point-in-time snapshot of a counter.
#[derive(Debug, Clone)]
pub struct CounterSnapshot {
    pub name: String,
    pub value: u64,
    pub last_reset_ms: u64,
}

/// Errors from counter operations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CounterError {
    /// Counter name not found.
    NotFound(String),
    /// Maximum counter capacity reached.
    CapacityExceeded { max: usize },
    /// A counter with this name already exists.
    AlreadyExists(String),
}

impl fmt::Display for CounterError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CounterError::NotFound(n) => write!(f, "counter not found: {n}"),
            CounterError::CapacityExceeded { max } => {
                write!(f, "counter capacity exceeded: max {max}")
            },
            CounterError::AlreadyExists(n) => write!(f, "counter already exists: {n}"),
        }
    }
}

impl std::error::Error for CounterError {}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_register_and_increment() {
        let mut set = CounterSet::new();
        set.register("test_counter", 1000).expect("register");
        set.increment("test_counter").expect("increment");
        set.increment("test_counter").expect("increment");

        let c = set.get("test_counter").expect("get");
        assert_eq!(c.get(), 2);
    }

    #[test]
    fn test_with_defaults_registers_all_predefined() {
        let set = CounterSet::with_defaults(0);
        assert_eq!(set.count(), PREDEFINED_COUNTERS.len());

        for name in PREDEFINED_COUNTERS {
            assert!(
                set.get(name).is_some(),
                "missing predefined counter: {name}"
            );
        }
    }

    #[test]
    fn test_capacity_exceeded() {
        let mut set = CounterSet::new();
        for i in 0..MAX_COUNTERS {
            let name = format!("c{i:02}");
            set.register(&name, 0).expect("register");
        }
        let result = set.register("overflow", 0);
        assert!(matches!(result, Err(CounterError::CapacityExceeded { .. })));
    }

    #[test]
    fn test_duplicate_registration() {
        let mut set = CounterSet::new();
        set.register("dup", 0).expect("register");
        let result = set.register("dup", 0);
        assert!(matches!(result, Err(CounterError::AlreadyExists(_))));
    }

    #[test]
    fn test_increment_not_found() {
        let set = CounterSet::new();
        let result = set.increment("nonexistent");
        assert!(matches!(result, Err(CounterError::NotFound(_))));
    }

    #[test]
    fn test_reset_all() {
        let mut set = CounterSet::new();
        set.register("a", 0).expect("register");
        set.register("b", 0).expect("register");
        set.increment("a").expect("inc a");
        set.increment("a").expect("inc a");
        set.increment("b").expect("inc b");

        set.reset_all(5000);

        assert_eq!(set.get("a").expect("get a").get(), 0);
        assert_eq!(set.get("b").expect("get b").get(), 0);
        assert_eq!(set.get("a").expect("get a").last_reset_ms(), 5000);
    }

    #[test]
    fn test_snapshot() {
        let mut set = CounterSet::new();
        set.register("x", 0).expect("register");
        set.increment_by("x", 42).expect("inc");

        let snap = set.snapshot();
        assert_eq!(snap.len(), 1);
        assert_eq!(snap[0].name, "x");
        assert_eq!(snap[0].value, 42);
    }

    #[test]
    fn test_increment_by_delta() {
        let mut set = CounterSet::new();
        set.register("tokens", 0).expect("register");
        set.increment_by("tokens", 100).expect("inc");
        set.increment_by("tokens", 50).expect("inc");
        assert_eq!(set.get("tokens").expect("get").get(), 150);
    }

    #[test]
    fn test_counter_name_truncation() {
        let long_name = "this_counter_name_is_way_too_long_for_the_buffer";
        let mut set = CounterSet::new();
        set.register(long_name, 0).expect("register");
        let c = set.get(long_name);
        // get() encodes the same way, so truncated names still match
        assert!(c.is_some());
    }
}
