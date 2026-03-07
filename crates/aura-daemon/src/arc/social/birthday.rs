//! Birthday tracking and reminder scheduling (spec §4.5).
//!
//! Tracks birthdays for contacts, scans for upcoming birthdays within a
//! configurable lookahead window, and manages reminder-sent flags with
//! yearly reset.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use tracing::instrument;

use crate::arc::ArcError;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum number of tracked birthdays.
const MAX_TRACKED: usize = 500;

/// Maximum entries in the upcoming-birthday cache.
const MAX_UPCOMING_CACHE: usize = 30;

/// Maximum length for a contact name.
const MAX_NAME_LEN: usize = 64;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// A single birthday entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BirthdayEntry {
    /// Contact identifier.
    pub contact_id: u64,
    /// Birth month (1–12).
    pub month: u8,
    /// Birth day of month (1–31).
    pub day: u8,
    /// Birth year (optional — not everyone shares their year).
    pub year: Option<u16>,
    /// Whether a reminder has already been sent this cycle.
    pub reminder_sent: bool,
    /// Display name (truncated to [`MAX_NAME_LEN`]).
    pub name: String,
}

// ---------------------------------------------------------------------------
// BirthdayTracker
// ---------------------------------------------------------------------------

/// Bounded birthday tracker with upcoming-birthday scanning.
#[derive(Debug, Serialize, Deserialize)]
pub struct BirthdayTracker {
    /// Map from contact_id to birthday entry.
    birthdays: HashMap<u64, BirthdayEntry>,
    /// Cached upcoming birthdays: (contact_id, days_until).
    upcoming_cache: Vec<(u64, u16)>,
    /// Day-of-year when the last scan was performed (1–366).
    last_scan_day: u32,
}

impl BirthdayTracker {
    /// Create a new, empty tracker.
    #[must_use]
    pub fn new() -> Self {
        Self {
            birthdays: HashMap::with_capacity(64),
            upcoming_cache: Vec::with_capacity(MAX_UPCOMING_CACHE),
            last_scan_day: 0,
        }
    }

    /// Register or update a birthday.
    ///
    /// `name` is truncated to 64 characters.  Month must be 1–12, day 1–31.
    #[instrument(skip_all)]
    pub fn add_birthday(
        &mut self,
        contact_id: u64,
        name: &str,
        month: u8,
        day: u8,
        year: Option<u16>,
    ) -> Result<(), ArcError> {
        if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
            return Err(ArcError::DomainError {
                domain: crate::arc::DomainId::Social,
                detail: format!("invalid date: month={month}, day={day}"),
            });
        }

        // Enforce capacity (only for new entries, updates are always OK).
        if !self.birthdays.contains_key(&contact_id) && self.birthdays.len() >= MAX_TRACKED {
            return Err(ArcError::CapacityExceeded {
                collection: "birthdays".into(),
                max: MAX_TRACKED,
            });
        }

        let truncated_name: String = name.chars().take(MAX_NAME_LEN).collect();

        self.birthdays.insert(
            contact_id,
            BirthdayEntry {
                contact_id,
                month,
                day,
                year,
                reminder_sent: false,
                name: truncated_name,
            },
        );

        Ok(())
    }

    /// Remove a tracked birthday.
    #[instrument(skip_all)]
    pub fn remove_birthday(&mut self, contact_id: u64) -> Result<(), ArcError> {
        self.birthdays
            .remove(&contact_id)
            .ok_or(ArcError::NotFound {
                entity: "birthday".into(),
                id: contact_id,
            })?;
        Ok(())
    }

    /// Scan for birthdays within `days_ahead` of the given date.
    ///
    /// Returns references sorted by days-until ascending.  The internal
    /// upcoming cache is refreshed as a side-effect.
    #[instrument(skip_all)]
    pub fn scan_upcoming(
        &mut self,
        today_month: u8,
        today_day: u8,
        days_ahead: u16,
    ) -> Result<Vec<&BirthdayEntry>, ArcError> {
        if !(1..=12).contains(&today_month) || !(1..=31).contains(&today_day) {
            return Err(ArcError::DomainError {
                domain: crate::arc::DomainId::Social,
                detail: format!("invalid today date: month={today_month}, day={today_day}"),
            });
        }

        let today_ordinal = month_day_to_ordinal(today_month, today_day);
        let mut upcoming: Vec<(u64, u16)> = Vec::new();

        for entry in self.birthdays.values() {
            let bday_ordinal = month_day_to_ordinal(entry.month, entry.day);
            let days_until = ordinal_distance(today_ordinal, bday_ordinal);

            if days_until <= days_ahead {
                upcoming.push((entry.contact_id, days_until));
            }
        }

        upcoming.sort_by_key(|&(_, d)| d);

        // Truncate cache to bounded size.
        upcoming.truncate(MAX_UPCOMING_CACHE);
        self.upcoming_cache = upcoming;
        self.last_scan_day = today_ordinal as u32;

        let ids: Vec<u64> = self.upcoming_cache.iter().map(|(id, _)| *id).collect();
        let mut result: Vec<&BirthdayEntry> = Vec::new();
        for id in &ids {
            if let Some(entry) = self.birthdays.get(id) {
                result.push(entry);
            }
        }
        Ok(result)
    }

    /// Mark a contact's birthday reminder as sent.
    #[instrument(skip_all)]
    pub fn mark_reminded(&mut self, contact_id: u64) -> Result<(), ArcError> {
        let entry = self
            .birthdays
            .get_mut(&contact_id)
            .ok_or(ArcError::NotFound {
                entity: "birthday".into(),
                id: contact_id,
            })?;
        entry.reminder_sent = true;
        Ok(())
    }

    /// Reset all `reminder_sent` flags (call at start of each year cycle).
    #[instrument(skip_all)]
    pub fn reset_yearly(&mut self) {
        for entry in self.birthdays.values_mut() {
            entry.reminder_sent = false;
        }
        self.upcoming_cache.clear();
        self.last_scan_day = 0;
    }

    /// Number of tracked birthdays.
    #[must_use]
    pub fn tracked_count(&self) -> usize {
        self.birthdays.len()
    }

    /// Read-only access to a birthday entry.
    #[must_use]
    pub fn get(&self, contact_id: u64) -> Option<&BirthdayEntry> {
        self.birthdays.get(&contact_id)
    }
}

impl Default for BirthdayTracker {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Approximate ordinal day-of-year from month/day (non-leap).
/// Good enough for birthday distance; exact leap-year handling is overkill.
#[must_use]
fn month_day_to_ordinal(month: u8, day: u8) -> u16 {
    const CUMULATIVE: [u16; 13] = [0, 0, 31, 59, 90, 120, 151, 181, 212, 243, 273, 304, 334];
    let m = (month as usize).min(12);
    CUMULATIVE[m] + day as u16
}

/// Circular distance in days (handles year wrap).
#[must_use]
fn ordinal_distance(from: u16, to: u16) -> u16 {
    if to >= from {
        to - from
    } else {
        365 - from + to
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_add_and_get() {
        let mut tracker = BirthdayTracker::new();
        tracker
            .add_birthday(1, "Alice", 3, 15, Some(1990))
            .expect("add");
        assert_eq!(tracker.tracked_count(), 1);
        let entry = tracker.get(1).expect("get");
        assert_eq!(entry.month, 3);
        assert_eq!(entry.day, 15);
        assert_eq!(entry.year, Some(1990));
        assert_eq!(entry.name, "Alice");
        assert!(!entry.reminder_sent);
    }

    #[test]
    fn test_invalid_date_rejected() {
        let mut tracker = BirthdayTracker::new();
        assert!(tracker.add_birthday(1, "Bad", 0, 15, None).is_err());
        assert!(tracker.add_birthday(1, "Bad", 13, 15, None).is_err());
        assert!(tracker.add_birthday(1, "Bad", 3, 0, None).is_err());
        assert!(tracker.add_birthday(1, "Bad", 3, 32, None).is_err());
    }

    #[test]
    fn test_name_truncation() {
        let mut tracker = BirthdayTracker::new();
        let long_name = "A".repeat(100);
        tracker
            .add_birthday(1, &long_name, 6, 1, None)
            .expect("add");
        let entry = tracker.get(1).expect("get");
        assert_eq!(entry.name.len(), MAX_NAME_LEN);
    }

    #[test]
    fn test_remove() {
        let mut tracker = BirthdayTracker::new();
        tracker.add_birthday(1, "Alice", 3, 15, None).expect("add");
        tracker.remove_birthday(1).expect("remove");
        assert_eq!(tracker.tracked_count(), 0);
        assert!(tracker.remove_birthday(1).is_err());
    }

    #[test]
    fn test_scan_upcoming() {
        let mut tracker = BirthdayTracker::new();
        // Today: March 10. Birthdays on Mar 12 (2 days) and Apr 20 (41 days).
        tracker.add_birthday(1, "Alice", 3, 12, None).expect("add");
        tracker.add_birthday(2, "Bob", 4, 20, None).expect("add");

        let upcoming = tracker.scan_upcoming(3, 10, 7).expect("scan");
        assert_eq!(upcoming.len(), 1);
        assert_eq!(upcoming[0].contact_id, 1);

        let upcoming_wide = tracker.scan_upcoming(3, 10, 45).expect("scan");
        assert_eq!(upcoming_wide.len(), 2);
    }

    #[test]
    fn test_scan_year_wrap() {
        let mut tracker = BirthdayTracker::new();
        // Today: Dec 28. Birthday on Jan 3 (6 days away, wrapping year).
        tracker.add_birthday(1, "Eve", 1, 3, None).expect("add");

        let upcoming = tracker.scan_upcoming(12, 28, 10).expect("scan");
        assert_eq!(upcoming.len(), 1);
        assert_eq!(upcoming[0].contact_id, 1);
    }

    #[test]
    fn test_mark_reminded_and_reset() {
        let mut tracker = BirthdayTracker::new();
        tracker.add_birthday(1, "Alice", 3, 15, None).expect("add");

        tracker.mark_reminded(1).expect("mark");
        assert!(tracker.get(1).expect("get").reminder_sent);

        tracker.reset_yearly();
        assert!(!tracker.get(1).expect("get").reminder_sent);
    }

    #[test]
    fn test_capacity_limit() {
        let mut tracker = BirthdayTracker::new();
        for i in 0..MAX_TRACKED as u64 {
            tracker
                .add_birthday(i, &format!("C{i}"), 1, 1, None)
                .expect("add");
        }
        assert!(tracker
            .add_birthday(MAX_TRACKED as u64, "Overflow", 1, 1, None)
            .is_err());
    }

    #[test]
    fn test_update_existing_birthday() {
        let mut tracker = BirthdayTracker::new();
        tracker.add_birthday(1, "Alice", 3, 15, None).expect("add");
        tracker
            .add_birthday(1, "Alice Updated", 4, 20, Some(1991))
            .expect("update");
        assert_eq!(tracker.tracked_count(), 1);
        let entry = tracker.get(1).expect("get");
        assert_eq!(entry.month, 4);
        assert_eq!(entry.day, 20);
        assert_eq!(entry.name, "Alice Updated");
    }
}
