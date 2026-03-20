//! Proactive consent management for AURA.
//!
//! This module controls when AURA can proactively speak/make suggestions.
//! User consent is REQUIRED before any proactive behavior occurs.
//!
//! GDPR requires consent to be specific, not all-or-nothing.
//! This implementation supports 6 consent categories for fine-grained control.

use serde::{Deserialize, Serialize};

/// The 6 consent categories as specified in the architecture documentation.
/// Each category can be independently granted or revoked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConsentCategory {
    /// Push notifications from AURA
    Notification,
    /// Proactive background operations
    BackgroundTask,
    /// AURA accessing specific data categories
    DataAccess,
    /// AURA executing device control actions
    DeviceControl,
    /// AURA storing new information about the user
    MemoryWrite,
    /// AURA initiating conversation without user prompt
    ProactiveSuggestion,
}

impl ConsentCategory {
    pub fn all() -> [Self; 6] {
        [
            Self::Notification,
            Self::BackgroundTask,
            Self::DataAccess,
            Self::DeviceControl,
            Self::MemoryWrite,
            Self::ProactiveSuggestion,
        ]
    }
}

/// Individual consent level for a category.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum ConsentLevel {
    /// User has not been asked - NO behavior allowed for this category
    #[default]
    Unasked,
    /// User explicitly declined - NO behavior for this category
    Declined,
    /// User explicitly accepted - behavior allowed for this category
    Accepted,
}

/// Legacy binary consent format for backward compatibility.
/// Old AcceptedAll → all 6 categories = Accepted
/// Old DeclinedAll → all 6 categories = Declined
/// Old Unasked → all 6 categories = Unasked
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum ProactiveConsent {
    #[default]
    Unasked,
    Declined,
    AcceptedAll,
}

impl ProactiveConsent {
    pub fn is_allowed(&self) -> bool {
        matches!(self, Self::AcceptedAll)
    }
}

/// New granular consent storage - per-category consent levels.
/// This is the primary storage format moving forward.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct GranularConsent {
    pub notification: ConsentLevel,
    pub background_task: ConsentLevel,
    pub data_access: ConsentLevel,
    pub device_control: ConsentLevel,
    pub memory_write: ConsentLevel,
    pub proactive_suggestion: ConsentLevel,
}

impl Default for GranularConsent {
    fn default() -> Self {
        Self {
            notification: ConsentLevel::Unasked,
            background_task: ConsentLevel::Unasked,
            data_access: ConsentLevel::Unasked,
            device_control: ConsentLevel::Unasked,
            memory_write: ConsentLevel::Unasked,
            proactive_suggestion: ConsentLevel::Unasked,
        }
    }
}

impl GranularConsent {
    pub fn get(&self, category: ConsentCategory) -> ConsentLevel {
        match category {
            ConsentCategory::Notification => self.notification,
            ConsentCategory::BackgroundTask => self.background_task,
            ConsentCategory::DataAccess => self.data_access,
            ConsentCategory::DeviceControl => self.device_control,
            ConsentCategory::MemoryWrite => self.memory_write,
            ConsentCategory::ProactiveSuggestion => self.proactive_suggestion,
        }
    }

    pub fn set(&mut self, category: ConsentCategory, level: ConsentLevel) {
        match category {
            ConsentCategory::Notification => self.notification = level,
            ConsentCategory::BackgroundTask => self.background_task = level,
            ConsentCategory::DataAccess => self.data_access = level,
            ConsentCategory::DeviceControl => self.device_control = level,
            ConsentCategory::MemoryWrite => self.memory_write = level,
            ConsentCategory::ProactiveSuggestion => self.proactive_suggestion = level,
        }
    }

    pub fn is_allowed(&self, category: ConsentCategory) -> bool {
        self.get(category) == ConsentLevel::Accepted
    }
}

/// Unified consent type that handles both legacy binary and new granular format.
/// This enum is used for serialization to support backward compatibility.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Consent {
    /// Legacy binary format (for backward compatibility during migration)
    Legacy(ProactiveConsent),
    /// New granular format
    Granular(GranularConsent),
}

impl Default for Consent {
    fn default() -> Self {
        Self::Granular(GranularConsent::default())
    }
}

impl Consent {
    /// Convert legacy binary to granular format
    fn from_legacy(legacy: ProactiveConsent) -> GranularConsent {
        let level = match legacy {
            ProactiveConsent::Unasked => ConsentLevel::Unasked,
            ProactiveConsent::Declined => ConsentLevel::Declined,
            ProactiveConsent::AcceptedAll => ConsentLevel::Accepted,
        };
        GranularConsent {
            notification: level,
            background_task: level,
            data_access: level,
            device_control: level,
            memory_write: level,
            proactive_suggestion: level,
        }
    }

    /// Migrate legacy format to new granular format
    pub fn migrate_if_needed(self) -> Self {
        match self {
            Consent::Legacy(legacy) => {
                tracing::info!("Migrating legacy binary consent to 6-category granular consent");
                Self::Granular(Self::from_legacy(legacy))
            }
            already_granular => already_granular,
        }
    }

    /// Check if a specific category is allowed
    pub fn is_category_allowed(&self, category: ConsentCategory) -> bool {
        match self {
            Consent::Legacy(legacy) => legacy.is_allowed(),
            Consent::Granular(granular) => granular.is_allowed(category),
        }
    }

    /// Check if any proactive behavior is allowed (for backward compat)
    pub fn is_allowed(&self) -> bool {
        match self {
            Consent::Legacy(legacy) => legacy.is_allowed(),
            Consent::Granular(granular) => {
                granular.is_allowed(ConsentCategory::ProactiveSuggestion)
            }
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProactiveSettings {
    pub consent: Consent,
    pub quiet_hours_start: Option<u8>,
    pub quiet_hours_end: Option<u8>,
    pub max_proactive_per_hour: u32,
}

impl Default for ProactiveSettings {
    fn default() -> Self {
        Self {
            consent: Consent::default(),
            quiet_hours_start: None,
            quiet_hours_end: None,
            max_proactive_per_hour: 10,
        }
    }
}

impl ProactiveSettings {
    /// Check if proactive behavior is allowed (uses ProactiveSuggestion category)
    pub fn can_proact(&self, hour: u8) -> bool {
        if !self
            .consent
            .is_category_allowed(ConsentCategory::ProactiveSuggestion)
        {
            return false;
        }

        if let (Some(start), Some(end)) = (self.quiet_hours_start, self.quiet_hours_end) {
            if start <= end {
                if hour >= start && hour < end {
                    return false;
                }
            } else {
                if hour >= start || hour < end {
                    return false;
                }
            }
        }

        true
    }

    /// Check if a specific consent category is allowed
    pub fn can_do(&self, category: ConsentCategory, hour: u8) -> bool {
        if !self.consent.is_category_allowed(category) {
            return false;
        }

        if let (Some(start), Some(end)) = (self.quiet_hours_start, self.quiet_hours_end) {
            if start <= end {
                if hour >= start && hour < end {
                    return false;
                }
            } else {
                if hour >= start || hour < end {
                    return false;
                }
            }
        }

        true
    }

    /// Set consent for a specific category
    pub fn set_category_consent(&mut self, category: ConsentCategory, level: ConsentLevel) {
        if let Consent::Granular(ref mut granular) = self.consent {
            granular.set(category, level);
        } else {
            self.consent = Consent::Granular(GranularConsent::default());
            if let Consent::Granular(ref mut granular) = self.consent {
                granular.set(category, level);
            }
        }
    }

    /// Migrate legacy consent format to new granular format if needed
    pub fn migrate(&mut self) {
        self.consent = self.consent.clone().migrate_if_needed();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_unasked_blocks_proactive() {
        let settings = ProactiveSettings::default();
        assert!(!settings.can_proact(10));
    }

    #[test]
    fn test_granular_unasked_blocks_proactive() {
        let settings = ProactiveSettings::default();
        assert!(!settings.can_do(ConsentCategory::ProactiveSuggestion, 10));
    }

    #[test]
    fn test_granular_accepted_allows_proactive() {
        let mut settings = ProactiveSettings::default();
        settings.set_category_consent(ConsentCategory::ProactiveSuggestion, ConsentLevel::Accepted);
        assert!(settings.can_proact(10));
    }

    #[test]
    fn test_per_category_consent() {
        let mut settings = ProactiveSettings::default();

        settings.set_category_consent(ConsentCategory::MemoryWrite, ConsentLevel::Accepted);
        settings.set_category_consent(ConsentCategory::DeviceControl, ConsentLevel::Declined);

        assert!(settings
            .consent
            .is_category_allowed(ConsentCategory::MemoryWrite));
        assert!(!settings
            .consent
            .is_category_allowed(ConsentCategory::DeviceControl));
    }

    #[test]
    fn test_legacy_migration() {
        let legacy = Consent::Legacy(ProactiveConsent::AcceptedAll);
        let migrated = legacy.migrate_if_needed();

        assert!(migrated.is_category_allowed(ConsentCategory::Notification));
        assert!(migrated.is_category_allowed(ConsentCategory::BackgroundTask));
        assert!(migrated.is_category_allowed(ConsentCategory::DataAccess));
        assert!(migrated.is_category_allowed(ConsentCategory::DeviceControl));
        assert!(migrated.is_category_allowed(ConsentCategory::MemoryWrite));
        assert!(migrated.is_category_allowed(ConsentCategory::ProactiveSuggestion));
    }

    #[test]
    fn test_legacy_declined_migration() {
        let legacy = Consent::Legacy(ProactiveConsent::Declined);
        let migrated = legacy.migrate_if_needed();

        assert!(!migrated.is_category_allowed(ConsentCategory::ProactiveSuggestion));
    }

    #[test]
    fn test_quiet_hours_blocks() {
        let mut settings = ProactiveSettings::default();
        settings.set_category_consent(ConsentCategory::ProactiveSuggestion, ConsentLevel::Accepted);
        settings.quiet_hours_start = Some(22);
        settings.quiet_hours_end = Some(7);

        assert!(!settings.can_proact(23));
        assert!(!settings.can_proact(3));
        assert!(settings.can_proact(12));
    }
}
