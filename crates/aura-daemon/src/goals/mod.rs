//! Goal management subsystem — AURA's ability to pursue MANY goals simultaneously.
//!
//! This module implements a full BDI (Belief-Desire-Intention) inspired goal
//! management system with:
//!
//! - **Registry**: What AURA can do (capabilities, app actions, confidence tracking)
//! - **Decomposer**: Breaking high-level goals into sub-goal DAGs via HTN templates
//! - **Scheduler**: Priority-based scheduling with preemption, aging, and power awareness
//! - **Tracker**: Goal lifecycle state machine with progress, retries, and history
//!
//! # Bounded Collections
//!
//! ALL collections in this module are bounded to prevent unbounded memory growth
//! on resource-constrained Android devices. [`BoundedVec`] and [`BoundedMap`]
//! enforce compile-time or runtime capacity limits.

pub mod conflicts;
pub mod decomposer;
pub mod registry;
pub mod scheduler;
pub mod tracker;

// Re-export primary types for convenient access.
pub use conflicts::ConflictResolver;
pub use decomposer::{GoalDecomposer, HtnDecomposer};
pub use registry::GoalRegistry;
pub use scheduler::{BdiScheduler, GoalScheduler};
pub use tracker::GoalTracker;

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// ---------------------------------------------------------------------------
// Bounded collection primitives
// ---------------------------------------------------------------------------

/// A `Vec` with a hard capacity limit. Insertions beyond `CAP` are rejected.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BoundedVec<T, const CAP: usize> {
    inner: Vec<T>,
}

impl<T, const CAP: usize> BoundedVec<T, CAP> {
    /// Create an empty bounded vec.
    pub fn new() -> Self {
        Self {
            inner: Vec::with_capacity(CAP.min(64)), // don't pre-alloc huge
        }
    }

    /// Try to push an item. Returns `Err(item)` if at capacity.
    pub fn try_push(&mut self, item: T) -> Result<(), T> {
        if self.inner.len() >= CAP {
            return Err(item);
        }
        self.inner.push(item);
        Ok(())
    }

    /// Current number of items.
    #[must_use]
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    /// Whether the collection is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// Maximum capacity.
    #[must_use]
    pub fn capacity(&self) -> usize {
        CAP
    }

    /// Whether the collection is full.
    #[must_use]
    pub fn is_full(&self) -> bool {
        self.inner.len() >= CAP
    }

    /// Immutable slice access.
    pub fn as_slice(&self) -> &[T] {
        &self.inner
    }

    /// Mutable slice access.
    pub fn as_mut_slice(&mut self) -> &mut [T] {
        &mut self.inner
    }

    /// Iterate over items.
    pub fn iter(&self) -> std::slice::Iter<'_, T> {
        self.inner.iter()
    }

    /// Mutable iteration.
    pub fn iter_mut(&mut self) -> std::slice::IterMut<'_, T> {
        self.inner.iter_mut()
    }

    /// Remove an element by index, preserving order.
    pub fn remove(&mut self, index: usize) -> T {
        self.inner.remove(index)
    }

    /// Retain only elements matching the predicate.
    pub fn retain<F: FnMut(&T) -> bool>(&mut self, f: F) {
        self.inner.retain(f);
    }
}

impl<T, const CAP: usize> Default for BoundedVec<T, CAP> {
    fn default() -> Self {
        Self::new()
    }
}

/// A `HashMap` with a hard capacity limit. Insertions beyond `CAP` are rejected.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BoundedMap<K: Eq + std::hash::Hash, V, const CAP: usize> {
    inner: HashMap<K, V>,
}

impl<K: Eq + std::hash::Hash, V, const CAP: usize> BoundedMap<K, V, CAP> {
    /// Create an empty bounded map.
    pub fn new() -> Self {
        Self {
            inner: HashMap::with_capacity(CAP.min(64)),
        }
    }

    /// Try to insert a key-value pair. Returns `Err((key, value))` if at
    /// capacity and the key doesn't already exist.
    pub fn try_insert(&mut self, key: K, value: V) -> Result<Option<V>, (K, V)> {
        if self.inner.contains_key(&key) {
            // Replacing existing key — no capacity issue.
            Ok(self.inner.insert(key, value))
        } else if self.inner.len() >= CAP {
            Err((key, value))
        } else {
            Ok(self.inner.insert(key, value))
        }
    }

    /// Get a reference to a value by key.
    pub fn get(&self, key: &K) -> Option<&V> {
        self.inner.get(key)
    }

    /// Get a mutable reference to a value by key.
    pub fn get_mut(&mut self, key: &K) -> Option<&mut V> {
        self.inner.get_mut(key)
    }

    /// Remove a key-value pair.
    pub fn remove(&mut self, key: &K) -> Option<V> {
        self.inner.remove(key)
    }

    /// Check if a key exists.
    pub fn contains_key(&self, key: &K) -> bool {
        self.inner.contains_key(key)
    }

    /// Current number of entries.
    #[must_use]
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    /// Whether the map is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// Maximum capacity.
    #[must_use]
    pub fn capacity(&self) -> usize {
        CAP
    }

    /// Whether the map is at capacity.
    #[must_use]
    pub fn is_full(&self) -> bool {
        self.inner.len() >= CAP
    }

    /// Iterate over entries.
    pub fn iter(&self) -> std::collections::hash_map::Iter<'_, K, V> {
        self.inner.iter()
    }

    /// Mutable iteration.
    pub fn iter_mut(&mut self) -> std::collections::hash_map::IterMut<'_, K, V> {
        self.inner.iter_mut()
    }

    /// Get all values.
    pub fn values(&self) -> std::collections::hash_map::Values<'_, K, V> {
        self.inner.values()
    }

    /// Retain only entries matching the predicate.
    pub fn retain<F: FnMut(&K, &mut V) -> bool>(&mut self, f: F) {
        self.inner.retain(f);
    }
}

impl<K: Eq + std::hash::Hash, V, const CAP: usize> Default for BoundedMap<K, V, CAP> {
    fn default() -> Self {
        Self::new()
    }
}

/// A fixed-size circular buffer that overwrites the oldest entry when full.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CircularBuffer<T, const CAP: usize> {
    buf: Vec<T>,
    head: usize,
    count: usize,
}

impl<T, const CAP: usize> CircularBuffer<T, CAP> {
    /// Create an empty circular buffer.
    pub fn new() -> Self {
        Self {
            buf: Vec::with_capacity(CAP.min(64)),
            head: 0,
            count: 0,
        }
    }

    /// Push an item, overwriting the oldest if full.
    pub fn push(&mut self, item: T) {
        if self.buf.len() < CAP {
            self.buf.push(item);
            self.count = self.buf.len();
        } else {
            self.buf[self.head] = item;
            self.head = (self.head + 1) % CAP;
            // count stays at CAP
        }
    }

    /// Current number of items.
    #[must_use]
    pub fn len(&self) -> usize {
        self.count
    }

    /// Whether the buffer is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    /// Iterate over items from oldest to newest.
    pub fn iter(&self) -> impl Iterator<Item = &T> {
        let (a, b) = if self.buf.len() < CAP {
            (self.buf.as_slice(), &[][..])
        } else {
            let (second, first) = self.buf.split_at(self.head);
            (first, second)
        };
        a.iter().chain(b.iter())
    }

    /// Get the most recently pushed item.
    pub fn last(&self) -> Option<&T> {
        if self.count == 0 {
            return None;
        }
        if self.buf.len() < CAP {
            self.buf.last()
        } else {
            let idx = if self.head == 0 {
                CAP - 1
            } else {
                self.head - 1
            };
            Some(&self.buf[idx])
        }
    }
}

impl<T, const CAP: usize> Default for CircularBuffer<T, CAP> {
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
    fn test_bounded_vec_capacity_enforced() {
        let mut v: BoundedVec<u32, 3> = BoundedVec::new();
        assert!(v.try_push(1).is_ok());
        assert!(v.try_push(2).is_ok());
        assert!(v.try_push(3).is_ok());
        assert_eq!(v.try_push(4), Err(4));
        assert_eq!(v.len(), 3);
        assert!(v.is_full());
    }

    #[test]
    fn test_bounded_vec_remove_and_push() {
        let mut v: BoundedVec<u32, 2> = BoundedVec::new();
        assert!(v.try_push(10).is_ok());
        assert!(v.try_push(20).is_ok());
        assert!(v.is_full());
        v.remove(0);
        assert!(!v.is_full());
        assert!(v.try_push(30).is_ok());
        assert_eq!(v.as_slice(), &[20, 30]);
    }

    #[test]
    fn test_bounded_map_capacity_enforced() {
        let mut m: BoundedMap<u64, String, 2> = BoundedMap::new();
        assert!(m.try_insert(1, "a".into()).is_ok());
        assert!(m.try_insert(2, "b".into()).is_ok());
        assert!(m.try_insert(3, "c".into()).is_err());
        // Replacing existing key should work even at capacity.
        assert!(m.try_insert(1, "a2".into()).is_ok());
        assert_eq!(m.get(&1), Some(&"a2".to_string()));
    }

    #[test]
    fn test_bounded_map_remove_then_insert() {
        let mut m: BoundedMap<u64, u32, 2> = BoundedMap::new();
        assert!(m.try_insert(1, 10).is_ok());
        assert!(m.try_insert(2, 20).is_ok());
        m.remove(&1);
        assert!(m.try_insert(3, 30).is_ok());
        assert_eq!(m.len(), 2);
        assert!(!m.contains_key(&1));
    }

    #[test]
    fn test_circular_buffer_wraps() {
        let mut buf: CircularBuffer<u32, 3> = CircularBuffer::new();
        buf.push(1);
        buf.push(2);
        buf.push(3);
        assert_eq!(buf.len(), 3);

        // Overwrite oldest (1).
        buf.push(4);
        assert_eq!(buf.len(), 3);

        let items: Vec<_> = buf.iter().copied().collect();
        assert_eq!(items, vec![2, 3, 4]);
        assert_eq!(buf.last(), Some(&4));
    }

    #[test]
    fn test_circular_buffer_empty_and_single() {
        let mut buf: CircularBuffer<u32, 4> = CircularBuffer::new();
        assert!(buf.is_empty());
        assert_eq!(buf.last(), None);

        buf.push(42);
        assert_eq!(buf.len(), 1);
        assert_eq!(buf.last(), Some(&42));
    }
}
