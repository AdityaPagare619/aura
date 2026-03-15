//! Fixed-size metrics ring buffer — zero heap allocation on hot path.
//!
//! `MetricsRing<N>` stores the last N metric entries in a circular buffer.
//! All push and read operations operate on stack-allocated arrays. Labels are
//! stored as fixed-size `[u8; 32]` to avoid any `String` allocation during
//! recording.

use std::fmt;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// The kind of metric being recorded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MetricKind {
    /// Monotonically increasing counter.
    Counter,
    /// Point-in-time gauge value.
    Gauge,
    /// Distribution histogram bucket.
    Histogram,
    /// Duration measurement in microseconds.
    Duration,
}

impl fmt::Display for MetricKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MetricKind::Counter => write!(f, "counter"),
            MetricKind::Gauge => write!(f, "gauge"),
            MetricKind::Histogram => write!(f, "histogram"),
            MetricKind::Duration => write!(f, "duration"),
        }
    }
}

/// A single metric entry — fully stack-allocated, no heap.
#[derive(Clone, Copy)]
pub struct MetricEntry {
    /// Timestamp in milliseconds since daemon start.
    pub timestamp_ms: u64,
    /// What kind of metric this is.
    pub kind: MetricKind,
    /// Numeric value.
    pub value: f64,
    /// Fixed-size label — up to 31 UTF-8 bytes + NUL-padded remainder.
    pub label: [u8; 32],
    /// Whether this slot has been written at least once.
    occupied: bool,
}

impl Default for MetricEntry {
    fn default() -> Self {
        Self {
            timestamp_ms: 0,
            kind: MetricKind::Counter,
            value: 0.0,
            label: [0u8; 32],
            occupied: false,
        }
    }
}

impl fmt::Debug for MetricEntry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MetricEntry")
            .field("timestamp_ms", &self.timestamp_ms)
            .field("kind", &self.kind)
            .field("value", &self.value)
            .field("label", &self.label_str())
            .field("occupied", &self.occupied)
            .finish()
    }
}

impl MetricEntry {
    /// Create a new entry. Label is truncated to 31 bytes if longer.
    pub fn new(timestamp_ms: u64, kind: MetricKind, value: f64, label: &str) -> Self {
        Self {
            timestamp_ms,
            kind,
            value,
            label: encode_label(label),
            occupied: true,
        }
    }

    /// Extract the label as a `&str` (up to the first NUL byte).
    pub fn label_str(&self) -> &str {
        let end = self.label.iter().position(|&b| b == 0).unwrap_or(32);
        // SAFETY: we only ever write valid UTF-8 into the label via `encode_label`.
        // If somehow invalid, fall back to empty to satisfy the no-panic rule.
        std::str::from_utf8(&self.label[..end]).unwrap_or("")
    }

    /// Returns `true` if this slot has been written at least once.
    pub fn is_occupied(&self) -> bool {
        self.occupied
    }
}

/// Encode a `&str` label into a fixed `[u8; 32]` array. Truncates at byte
/// boundaries (respecting UTF-8 char boundaries) if longer than 31 bytes.
fn encode_label(label: &str) -> [u8; 32] {
    let mut buf = [0u8; 32];
    let bytes = label.as_bytes();
    let max_len = 31; // leave at least 1 NUL byte
    let copy_len = if bytes.len() <= max_len {
        bytes.len()
    } else {
        // Find the last valid UTF-8 char boundary at or before max_len.
        let mut end = max_len;
        while end > 0 && !label.is_char_boundary(end) {
            end -= 1;
        }
        end
    };
    buf[..copy_len].copy_from_slice(&bytes[..copy_len]);
    buf
}

// ---------------------------------------------------------------------------
// Ring buffer
// ---------------------------------------------------------------------------

/// Fixed-size circular buffer for metric entries.
///
/// `N` defaults to 4096. Push is O(1), overwrites the oldest entry when full.
/// All storage is inline — no heap allocation for the ring itself.
pub struct MetricsRing<const N: usize = 4096> {
    /// Backing array. Entries are default-initialised and overwritten lazily.
    entries: Box<[MetricEntry; N]>,
    /// Index of the *next* write position.
    head: usize,
    /// Number of entries written (capped at `N`).
    count: usize,
}

impl<const N: usize> MetricsRing<N> {
    /// Create a new, empty metrics ring.
    ///
    /// # Errors
    /// Returns `Err` if `N` is zero.
    pub fn new() -> Result<Self, RingError> {
        if N == 0 {
            return Err(RingError::ZeroCapacity);
        }
        // Use Box to avoid blowing the stack for large N.
        let entries = vec![MetricEntry::default(); N]
            .into_boxed_slice()
            .try_into()
            .map_err(|_| RingError::AllocationFailed)?;
        Ok(Self {
            entries,
            head: 0,
            count: 0,
        })
    }

    /// Capacity of the ring.
    pub const fn capacity(&self) -> usize {
        N
    }

    /// Number of occupied entries (≤ N).
    pub fn len(&self) -> usize {
        self.count
    }

    /// Returns `true` if the ring contains no entries.
    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    // ── Write ───────────────────────────────────────────────────────────

    /// Push a metric entry into the ring. O(1), overwrites oldest on wrap.
    pub fn push(&mut self, entry: MetricEntry) {
        self.entries[self.head] = entry;
        self.head = (self.head + 1) % N;
        if self.count < N {
            self.count += 1;
        }
    }

    /// Record a metric by components (convenience wrapper around `push`).
    pub fn record(&mut self, timestamp_ms: u64, kind: MetricKind, label: &str, value: f64) {
        self.push(MetricEntry::new(timestamp_ms, kind, value, label));
    }

    // ── Read ────────────────────────────────────────────────────────────

    /// Return the last `n` entries in chronological order (oldest first).
    ///
    /// If `n > count`, returns all available entries.
    pub fn query_last(&self, n: usize) -> Vec<&MetricEntry> {
        let take = n.min(self.count);
        if take == 0 {
            return Vec::new();
        }
        let mut result = Vec::with_capacity(take);
        // Start index is `head - take` (modular).
        let start = if self.head >= take {
            self.head - take
        } else {
            N - (take - self.head)
        };
        for i in 0..take {
            let idx = (start + i) % N;
            result.push(&self.entries[idx]);
        }
        result
    }

    /// Return entries whose timestamp falls within `[start_ms, end_ms]`.
    pub fn query_range(&self, start_ms: u64, end_ms: u64) -> Vec<&MetricEntry> {
        self.occupied_iter()
            .filter(|e| e.timestamp_ms >= start_ms && e.timestamp_ms <= end_ms)
            .collect()
    }

    /// Return entries matching the given label.
    pub fn query_by_label<'a>(&'a self, label: &str) -> Vec<&'a MetricEntry> {
        let target = encode_label(label);
        self.occupied_iter().filter(|e| e.label == target).collect()
    }

    /// Compute a summary (min/max/avg/p50/p95/p99) for each distinct label
    /// within a time window ending at `now_ms` and spanning `window_ms` back.
    pub fn summary(&self, now_ms: u64, window_ms: u64) -> Vec<LabelSummary> {
        let start_ms = now_ms.saturating_sub(window_ms);
        let entries: Vec<&MetricEntry> = self.query_range(start_ms, now_ms);

        if entries.is_empty() {
            return Vec::new();
        }

        // Group by label — bounded: we only have at most N entries.
        // Use a simple Vec of (label, values) since the number of distinct
        // labels is expected to be small (< 100).
        let mut groups: Vec<([u8; 32], Vec<f64>)> = Vec::with_capacity(64);

        for entry in &entries {
            if let Some(group) = groups.iter_mut().find(|(l, _)| *l == entry.label) {
                group.1.push(entry.value);
            } else {
                if groups.len() >= 256 {
                    // Cap distinct labels to prevent unbounded growth.
                    continue;
                }
                groups.push((entry.label, vec![entry.value]));
            }
        }

        groups
            .into_iter()
            .filter_map(|(label_bytes, mut values)| {
                if values.is_empty() {
                    return None;
                }
                values.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
                let count = values.len();
                let min = values[0];
                let max = values[count - 1];
                let sum: f64 = values.iter().sum();
                let avg = sum / count as f64;
                let p50 = percentile(&values, 50.0);
                let p95 = percentile(&values, 95.0);
                let p99 = percentile(&values, 99.0);

                let label_end = label_bytes.iter().position(|&b| b == 0).unwrap_or(32);
                let label_str = std::str::from_utf8(&label_bytes[..label_end]).unwrap_or("?");

                Some(LabelSummary {
                    label: label_str.to_string(),
                    count,
                    min,
                    max,
                    avg,
                    p50,
                    p95,
                    p99,
                })
            })
            .collect()
    }

    /// Format all entries in the ring as Prometheus-style text exposition.
    pub fn export_prometheus(&self) -> String {
        let mut out = String::with_capacity(self.count * 80);
        for entry in self.occupied_iter() {
            let label = entry.label_str();
            let kind_str = entry.kind.to_string();
            // aura_<label>{kind="<kind>"} <value> <timestamp>
            out.push_str("aura_");
            out.push_str(label);
            out.push_str("{kind=\"");
            out.push_str(&kind_str);
            out.push_str("\"} ");
            out.push_str(&entry.value.to_string());
            out.push(' ');
            out.push_str(&entry.timestamp_ms.to_string());
            out.push('\n');
        }
        out
    }

    // ── Internal ────────────────────────────────────────────────────────

    /// Iterate over all occupied entries in chronological order.
    fn occupied_iter(&self) -> impl Iterator<Item = &MetricEntry> {
        let start = if self.count < N { 0 } else { self.head };
        let count = self.count;
        let n = N;
        let entries = &*self.entries;
        (0..count).map(move |i| {
            let idx = (start + i) % n;
            &entries[idx]
        })
    }
}

/// Compute the k-th percentile (0–100) from a sorted slice.
fn percentile(sorted: &[f64], pct: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    if sorted.len() == 1 {
        return sorted[0];
    }
    let rank = (pct / 100.0) * (sorted.len() - 1) as f64;
    let lower = rank.floor() as usize;
    let upper = rank.ceil() as usize;
    if lower == upper || upper >= sorted.len() {
        return sorted[lower.min(sorted.len() - 1)];
    }
    let frac = rank - lower as f64;
    sorted[lower] * (1.0 - frac) + sorted[upper] * frac
}

// ---------------------------------------------------------------------------
// Summary types
// ---------------------------------------------------------------------------

/// Statistical summary for a single metric label within a time window.
#[derive(Debug, Clone)]
pub struct LabelSummary {
    pub label: String,
    pub count: usize,
    pub min: f64,
    pub max: f64,
    pub avg: f64,
    pub p50: f64,
    pub p95: f64,
    pub p99: f64,
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Errors that can occur in ring buffer operations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RingError {
    /// Ring capacity must be > 0.
    ZeroCapacity,
    /// Boxed allocation failed (shouldn't happen in practice).
    AllocationFailed,
}

impl fmt::Display for RingError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RingError::ZeroCapacity => write!(f, "ring capacity must be > 0"),
            RingError::AllocationFailed => write!(f, "ring allocation failed"),
        }
    }
}

impl std::error::Error for RingError {}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_push_and_query_last() {
        let mut ring: MetricsRing<8> = MetricsRing::new().expect("ring");
        for i in 0..5 {
            ring.record(i as u64 * 100, MetricKind::Counter, "events", i as f64);
        }
        assert_eq!(ring.len(), 5);

        let last3 = ring.query_last(3);
        assert_eq!(last3.len(), 3);
        assert!((last3[0].value - 2.0).abs() < f64::EPSILON);
        assert!((last3[2].value - 4.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_ring_wraps_around() {
        let mut ring: MetricsRing<4> = MetricsRing::new().expect("ring");
        for i in 0..10 {
            ring.record(i as u64 * 10, MetricKind::Gauge, "temp", i as f64);
        }
        // Capacity is 4, so only last 4 entries should remain.
        assert_eq!(ring.len(), 4);

        let all = ring.query_last(100);
        assert_eq!(all.len(), 4);
        // Values 6, 7, 8, 9 (the last 4 pushed)
        assert!((all[0].value - 6.0).abs() < f64::EPSILON);
        assert!((all[3].value - 9.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_query_range() {
        let mut ring: MetricsRing<16> = MetricsRing::new().expect("ring");
        ring.record(100, MetricKind::Counter, "a", 1.0);
        ring.record(200, MetricKind::Counter, "a", 2.0);
        ring.record(300, MetricKind::Counter, "a", 3.0);
        ring.record(400, MetricKind::Counter, "a", 4.0);

        let range = ring.query_range(150, 350);
        assert_eq!(range.len(), 2);
        assert!((range[0].value - 2.0).abs() < f64::EPSILON);
        assert!((range[1].value - 3.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_query_by_label() {
        let mut ring: MetricsRing<16> = MetricsRing::new().expect("ring");
        ring.record(100, MetricKind::Counter, "alpha", 1.0);
        ring.record(200, MetricKind::Gauge, "beta", 2.0);
        ring.record(300, MetricKind::Counter, "alpha", 3.0);

        let alpha = ring.query_by_label("alpha");
        assert_eq!(alpha.len(), 2);
        let beta = ring.query_by_label("beta");
        assert_eq!(beta.len(), 1);
        let gamma = ring.query_by_label("gamma");
        assert!(gamma.is_empty());
    }

    #[test]
    fn test_summary_statistics() {
        let mut ring: MetricsRing<64> = MetricsRing::new().expect("ring");
        let values = [1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0];
        for (i, &v) in values.iter().enumerate() {
            ring.record(i as u64 * 10, MetricKind::Duration, "latency", v);
        }

        let summaries = ring.summary(100, 200);
        assert_eq!(summaries.len(), 1);

        let s = &summaries[0];
        assert_eq!(s.label, "latency");
        assert_eq!(s.count, 10);
        assert!((s.min - 1.0).abs() < f64::EPSILON);
        assert!((s.max - 10.0).abs() < f64::EPSILON);
        assert!((s.avg - 5.5).abs() < f64::EPSILON);
        // p50 of 1..10 ≈ 5.5
        assert!((s.p50 - 5.5).abs() < 0.5);
    }

    #[test]
    fn test_label_truncation() {
        let long_label = "this_is_a_very_long_label_name_that_exceeds_32_bytes_easily";
        let entry = MetricEntry::new(0, MetricKind::Counter, 1.0, long_label);
        let recovered = entry.label_str();
        assert!(recovered.len() <= 31);
        assert!(long_label.starts_with(recovered));
    }

    #[test]
    fn test_empty_ring_queries() {
        let ring: MetricsRing<8> = MetricsRing::new().expect("ring");
        assert!(ring.is_empty());
        assert!(ring.query_last(10).is_empty());
        assert!(ring.query_range(0, 1000).is_empty());
        assert!(ring.query_by_label("x").is_empty());
        assert!(ring.summary(1000, 1000).is_empty());
    }

    #[test]
    fn test_export_prometheus_format() {
        let mut ring: MetricsRing<8> = MetricsRing::new().expect("ring");
        ring.record(1000, MetricKind::Counter, "events", 42.0);
        ring.record(2000, MetricKind::Gauge, "temp", 72.5);

        let output = ring.export_prometheus();
        assert!(output.contains("aura_events{kind=\"counter\"} 42"));
        assert!(output.contains("aura_temp{kind=\"gauge\"} 72.5"));
    }

    #[test]
    fn test_metric_kind_display() {
        assert_eq!(MetricKind::Counter.to_string(), "counter");
        assert_eq!(MetricKind::Gauge.to_string(), "gauge");
        assert_eq!(MetricKind::Histogram.to_string(), "histogram");
        assert_eq!(MetricKind::Duration.to_string(), "duration");
    }
}
