//! Contact management — contact profiles, categories, platform tracking (spec §4.1).
//!
//! Each contact has a unique ID, one or more platform aliases, a category,
//! and metadata used by the importance scorer and relationship health engine.

use serde::{Deserialize, Serialize};
use tracing::debug;

use crate::arc::ArcError;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum contacts stored.
const MAX_CONTACTS: usize = 500;

/// Maximum platform aliases per contact.
const MAX_ALIASES_PER_CONTACT: usize = 8;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Contact importance tier (§4.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum ContactTier {
    /// Contact within last 7 days, high importance.
    Close,
    /// Contact within last 14 days.
    Friend,
    /// Contact within last 30 days.
    Acquaintance,
    /// Contact within last 90 days.
    Contact,
    /// No contact in 90+ days.
    Dormant,
}

impl ContactTier {
    /// Expected contact interval in seconds.
    #[must_use]
    pub fn expected_interval_secs(self) -> i64 {
        match self {
            ContactTier::Close => 7 * 86400,
            ContactTier::Friend => 14 * 86400,
            ContactTier::Acquaintance => 30 * 86400,
            ContactTier::Contact => 90 * 86400,
            ContactTier::Dormant => i64::MAX,
        }
    }
}

/// Communication platform.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Platform {
    Sms,
    WhatsApp,
    Telegram,
    Signal,
    Email,
    Phone,
    Instagram,
    Twitter,
    Other,
}

impl std::fmt::Display for Platform {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self)
    }
}

/// Relationship category.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ContactCategory {
    Family,
    Partner,
    CloseFriend,
    Friend,
    Colleague,
    Professional,
    Acquaintance,
    Service,
    Other,
}

/// A platform alias for a contact.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlatformAlias {
    pub platform: Platform,
    /// Platform-specific identifier (phone number, handle, etc).
    pub identifier: String,
    /// Display name on this platform.
    pub display_name: Option<String>,
}

/// A contact profile.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Contact {
    /// Unique internal identifier.
    pub id: u64,
    /// Primary display name.
    pub name: String,
    /// Relationship category.
    pub category: ContactCategory,
    /// Current importance tier (computed by ImportanceScorer).
    pub tier: ContactTier,
    /// Platform aliases for this contact.
    pub aliases: Vec<PlatformAlias>,
    /// Explicit importance override (0.0 to 1.0, None = auto).
    pub explicit_importance: Option<f32>,
    /// Unix epoch seconds of last interaction.
    pub last_interaction_at: i64,
    /// Total interaction count.
    pub interaction_count: u64,
    /// Average message depth (words per message, EMA).
    pub avg_message_depth: f32,
    /// Created at (unix epoch seconds).
    pub created_at: i64,
}

impl Contact {
    /// Create a new contact with minimal information.
    #[must_use]
    pub fn new(id: u64, name: String, category: ContactCategory) -> Self {
        Self {
            id,
            name,
            category,
            tier: ContactTier::Contact,
            aliases: Vec::new(),
            explicit_importance: None,
            last_interaction_at: 0,
            interaction_count: 0,
            avg_message_depth: 0.0,
            created_at: 0,
        }
    }

    /// Add a platform alias.
    pub fn add_alias(&mut self, alias: PlatformAlias) -> Result<(), ArcError> {
        if self.aliases.len() >= MAX_ALIASES_PER_CONTACT {
            return Err(ArcError::CapacityExceeded {
                collection: "contact_aliases".into(),
                max: MAX_ALIASES_PER_CONTACT,
            });
        }
        self.aliases.push(alias);
        Ok(())
    }

    /// Record an interaction with this contact.
    pub fn record_interaction(&mut self, timestamp: i64, message_depth: f32) {
        self.interaction_count = self.interaction_count.saturating_add(1);
        if timestamp > self.last_interaction_at {
            self.last_interaction_at = timestamp;
        }
        // EMA for message depth (alpha = 0.2)
        if self.avg_message_depth <= 0.0 {
            self.avg_message_depth = message_depth;
        } else {
            self.avg_message_depth = self.avg_message_depth * 0.8 + message_depth * 0.2;
        }
    }
}

// ---------------------------------------------------------------------------
// ContactStore
// ---------------------------------------------------------------------------

/// Bounded store of contacts.
#[derive(Debug, Serialize, Deserialize)]
pub struct ContactStore {
    contacts: Vec<Contact>,
}

impl ContactStore {
    /// Create an empty contact store.
    #[must_use]
    pub fn new() -> Self {
        Self {
            contacts: Vec::with_capacity(64),
        }
    }

    /// Add a contact.
    pub fn add(&mut self, contact: Contact) -> Result<(), ArcError> {
        if self.contacts.len() >= MAX_CONTACTS {
            return Err(ArcError::CapacityExceeded {
                collection: "contacts".into(),
                max: MAX_CONTACTS,
            });
        }
        // Check for duplicate ID
        if self.contacts.iter().any(|c| c.id == contact.id) {
            return Err(ArcError::DomainError {
                domain: crate::arc::DomainId::Social,
                detail: format!("contact with id {} already exists", contact.id),
            });
        }
        debug!(id = contact.id, name = %contact.name, "contact added");
        self.contacts.push(contact);
        Ok(())
    }

    /// Remove a contact by ID.
    pub fn remove(&mut self, id: u64) -> Result<Contact, ArcError> {
        let idx = self
            .contacts
            .iter()
            .position(|c| c.id == id)
            .ok_or(ArcError::NotFound {
                entity: "contact".into(),
                id,
            })?;
        Ok(self.contacts.swap_remove(idx))
    }

    /// Get a contact by ID.
    #[must_use]
    pub fn get(&self, id: u64) -> Option<&Contact> {
        self.contacts.iter().find(|c| c.id == id)
    }

    /// Get a mutable reference to a contact by ID.
    pub fn get_mut(&mut self, id: u64) -> Option<&mut Contact> {
        self.contacts.iter_mut().find(|c| c.id == id)
    }

    /// Find a contact by platform alias.
    #[must_use]
    pub fn find_by_alias(&self, platform: Platform, identifier: &str) -> Option<&Contact> {
        self.contacts.iter().find(|c| {
            c.aliases
                .iter()
                .any(|a| a.platform == platform && a.identifier == identifier)
        })
    }

    /// Get all contacts in a given tier.
    #[must_use]
    pub fn by_tier(&self, tier: ContactTier) -> Vec<&Contact> {
        self.contacts.iter().filter(|c| c.tier == tier).collect()
    }

    /// Get all contacts in a given category.
    #[must_use]
    pub fn by_category(&self, category: ContactCategory) -> Vec<&Contact> {
        self.contacts
            .iter()
            .filter(|c| c.category == category)
            .collect()
    }

    /// Total number of contacts.
    #[must_use]
    pub fn total_contacts(&self) -> usize {
        self.contacts.len()
    }

    /// Read-only access to all contacts.
    #[must_use]
    pub fn all(&self) -> &[Contact] {
        &self.contacts
    }

    /// Mutable access to all contacts (for batch updates).
    pub fn all_mut(&mut self) -> &mut [Contact] {
        &mut self.contacts
    }
}

impl Default for ContactStore {
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

    fn make_contact(id: u64, name: &str) -> Contact {
        Contact::new(id, name.into(), ContactCategory::Friend)
    }

    #[test]
    fn test_add_and_get() {
        let mut store = ContactStore::new();
        store.add(make_contact(1, "Alice")).expect("add");
        assert_eq!(store.total_contacts(), 1);
        assert!(store.get(1).is_some());
        assert_eq!(store.get(1).map(|c| c.name.as_str()), Some("Alice"));
    }

    #[test]
    fn test_duplicate_id_rejected() {
        let mut store = ContactStore::new();
        store.add(make_contact(1, "Alice")).expect("add");
        assert!(store.add(make_contact(1, "Bob")).is_err());
    }

    #[test]
    fn test_remove() {
        let mut store = ContactStore::new();
        store.add(make_contact(1, "Alice")).expect("add");
        let removed = store.remove(1).expect("remove");
        assert_eq!(removed.name, "Alice");
        assert_eq!(store.total_contacts(), 0);
    }

    #[test]
    fn test_find_by_alias() {
        let mut store = ContactStore::new();
        let mut contact = make_contact(1, "Alice");
        contact
            .add_alias(PlatformAlias {
                platform: Platform::WhatsApp,
                identifier: "+1234567890".into(),
                display_name: Some("Alice WA".into()),
            })
            .expect("alias");
        store.add(contact).expect("add");

        let found = store.find_by_alias(Platform::WhatsApp, "+1234567890");
        assert!(found.is_some());
        assert_eq!(found.map(|c| c.name.as_str()), Some("Alice"));

        let not_found = store.find_by_alias(Platform::Telegram, "+1234567890");
        assert!(not_found.is_none());
    }

    #[test]
    fn test_by_tier() {
        let mut store = ContactStore::new();
        let mut c1 = make_contact(1, "Close");
        c1.tier = ContactTier::Close;
        let mut c2 = make_contact(2, "Friend");
        c2.tier = ContactTier::Friend;
        store.add(c1).expect("add");
        store.add(c2).expect("add");

        assert_eq!(store.by_tier(ContactTier::Close).len(), 1);
        assert_eq!(store.by_tier(ContactTier::Friend).len(), 1);
        assert_eq!(store.by_tier(ContactTier::Dormant).len(), 0);
    }

    #[test]
    fn test_record_interaction() {
        let mut contact = make_contact(1, "Alice");
        contact.record_interaction(1000, 15.0);
        assert_eq!(contact.interaction_count, 1);
        assert_eq!(contact.last_interaction_at, 1000);
        assert!((contact.avg_message_depth - 15.0).abs() < 0.001);

        contact.record_interaction(2000, 25.0);
        assert_eq!(contact.interaction_count, 2);
        // EMA: 15.0 * 0.8 + 25.0 * 0.2 = 17.0
        assert!((contact.avg_message_depth - 17.0).abs() < 0.001);
    }

    #[test]
    fn test_capacity_limit() {
        let mut store = ContactStore::new();
        for i in 0..MAX_CONTACTS {
            store
                .add(make_contact(i as u64, &format!("C{i}")))
                .expect("add");
        }
        assert!(store
            .add(make_contact(MAX_CONTACTS as u64, "Overflow"))
            .is_err());
    }

    #[test]
    fn test_tier_intervals() {
        assert!(
            ContactTier::Close.expected_interval_secs()
                < ContactTier::Friend.expected_interval_secs()
        );
        assert!(
            ContactTier::Friend.expected_interval_secs()
                < ContactTier::Acquaintance.expected_interval_secs()
        );
    }
}
