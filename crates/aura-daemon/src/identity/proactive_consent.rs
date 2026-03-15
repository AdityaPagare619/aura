//! Proactive consent management for AURA.
//!
//! This module controls when AURA can proactively speak/make suggestions.
//! User consent is REQUIRED before any proactive behavior occurs.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ProactiveConsent {
    /// User has not been asked - NO proactive behavior allowed
    #[default]
    Unasked,
    /// User explicitly declined - NO proactive behavior
    Declined,
    /// User accepted all proactive suggestions
    AcceptedAll,
}

impl ProactiveConsent {
    /// Check if proactive behavior is allowed for this user
    pub fn is_allowed(&self) -> bool {
        match self {
            Self::Unasked => false,
            Self::Declined => false,
            Self::AcceptedAll => true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProactiveSettings {
    pub consent: ProactiveConsent,
    pub quiet_hours_start: Option<u8>,
    pub quiet_hours_end: Option<u8>,
    pub max_proactive_per_hour: u32,
}

impl Default for ProactiveSettings {
    fn default() -> Self {
        Self {
            consent: ProactiveConsent::default(),
            quiet_hours_start: None,
            quiet_hours_end: None,
            max_proactive_per_hour: 10,
        }
    }
}

impl ProactiveSettings {
    pub fn can_proact(&self, hour: u8) -> bool {
        // First check consent
        if !self.consent.is_allowed() {
            return false;
        }

        // Check quiet hours
        if let (Some(start), Some(end)) = (self.quiet_hours_start, self.quiet_hours_end) {
            if start <= end {
                if hour >= start && hour < end {
                    return false;
                }
            } else {
                // Quiet hours span midnight
                if hour >= start || hour < end {
                    return false;
                }
            }
        }

        true
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
    fn test_declined_blocks_proactive() {
        let mut settings = ProactiveSettings::default();
        settings.consent = ProactiveConsent::Declined;
        assert!(!settings.can_proact(10));
    }

    #[test]
    fn test_accepted_allows_proactive() {
        let mut settings = ProactiveSettings::default();
        settings.consent = ProactiveConsent::AcceptedAll;
        assert!(settings.can_proact(10));
    }

    #[test]
    fn test_quiet_hours_blocks() {
        let mut settings = ProactiveSettings::default();
        settings.consent = ProactiveConsent::AcceptedAll;
        settings.quiet_hours_start = Some(22);
        settings.quiet_hours_end = Some(7);

        // 23:00 should be blocked
        assert!(!settings.can_proact(23));
        // 3:00 should be blocked
        assert!(!settings.can_proact(3));
        // 12:00 should be allowed
        assert!(settings.can_proact(12));
    }
}
