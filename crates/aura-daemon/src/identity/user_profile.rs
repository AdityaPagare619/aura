//! User profile — persistent representation of the user AURA is serving.
//!
//! Stores name, preferences, interests, daily patterns, privacy settings,
//! and provides persistence to/from SQLite. The profile evolves over time
//! as AURA learns more about its user through interactions.

use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

use aura_types::errors::OnboardingError;
use aura_types::identity::OceanTraits;

use crate::identity::proactive_consent::ProactiveSettings;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum length for the user's display name.
const MAX_NAME_LENGTH: usize = 128;

/// Maximum number of interests tracked.
const MAX_INTERESTS: usize = 50;

/// Maximum number of daily patterns tracked.
const MAX_DAILY_PATTERNS: usize = 20;

/// Schema version for profile serialization migration.
const PROFILE_SCHEMA_VERSION: u32 = 1;

// ---------------------------------------------------------------------------
// Core types
// ---------------------------------------------------------------------------

/// Privacy level controlling how much data AURA retains.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PrivacyLevel {
    /// Minimal data retention — only what's needed for core function.
    Minimal,
    /// Standard — retain interaction patterns, preferences, no raw messages.
    Standard,
    /// Full — retain everything for maximum personalisation.
    Full,
}

impl Default for PrivacyLevel {
    fn default() -> Self {
        Self::Standard
    }
}

/// Notification preference for proactive suggestions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NotificationPreference {
    /// All proactive notifications enabled.
    All,
    /// Only important notifications.
    ImportantOnly,
    /// No proactive notifications.
    None,
}

impl Default for NotificationPreference {
    fn default() -> Self {
        Self::All
    }
}

/// Communication style preference as expressed by the user.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CommunicationStyle {
    /// Brief, to-the-point responses.
    Concise,
    /// Balanced responses.
    Balanced,
    /// Detailed, thorough responses.
    Detailed,
}

impl Default for CommunicationStyle {
    fn default() -> Self {
        Self::Balanced
    }
}

/// A recurring daily pattern detected or stated by the user.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DailyPattern {
    /// Human-readable description (e.g. "Wake up around 7am").
    pub description: String,
    /// Hour of day this pattern centres on (0–23).
    pub hour: u8,
    /// Confidence that this pattern is accurate [0.0, 1.0].
    pub confidence: f32,
    /// Whether the user explicitly stated this vs AURA inferred it.
    pub user_stated: bool,
}

/// User privacy settings — what data AURA may collect and retain.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PrivacySettings {
    /// Overall privacy level.
    pub level: PrivacyLevel,
    /// Allow AURA to read notification content.
    pub allow_notification_reading: bool,
    /// Allow AURA to observe screen content.
    pub allow_screen_observation: bool,
    /// Allow AURA to track app usage patterns.
    pub allow_app_tracking: bool,
    /// Allow AURA to store conversation history.
    pub allow_conversation_history: bool,
    /// Allow AURA to learn from interactions.
    pub allow_learning: bool,
}

impl Default for PrivacySettings {
    fn default() -> Self {
        Self {
            level: PrivacyLevel::Standard,
            allow_notification_reading: true,
            allow_screen_observation: false,
            allow_app_tracking: true,
            allow_conversation_history: true,
            allow_learning: true,
        }
    }
}

/// User preferences for AURA's behaviour.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct UserPreferences {
    /// Preferred communication style.
    pub communication_style: CommunicationStyle,
    /// Preferred notification level.
    pub notification_preference: NotificationPreference,
    /// Preferred morning briefing hour (0–23).
    pub morning_briefing_hour: u8,
    /// Whether the user prefers humour in responses.
    pub likes_humor: bool,
    /// Whether the user prefers proactive suggestions.
    pub likes_proactive: bool,
    /// Preferred language/locale (BCP 47 tag, e.g. "en-US").
    pub locale: String,
}

impl Default for UserPreferences {
    fn default() -> Self {
        Self {
            communication_style: CommunicationStyle::Balanced,
            notification_preference: NotificationPreference::All,
            morning_briefing_hour: 7,
            likes_humor: true,
            likes_proactive: true,
            locale: "en-US".to_string(),
        }
    }
}

// ---------------------------------------------------------------------------
// UserProfile
// ---------------------------------------------------------------------------

/// Complete user profile — the persistent model of who AURA is serving.
///
/// Created during onboarding (with minimal data) and enriched over time.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserProfile {
    /// Schema version for migration support.
    pub schema_version: u32,
    /// User's chosen display name (can be empty if not provided).
    pub name: String,
    /// User's interests / topics of interest.
    pub interests: Vec<String>,
    /// Detected or stated daily patterns.
    pub daily_patterns: Vec<DailyPattern>,
    /// User preferences for AURA behaviour.
    pub preferences: UserPreferences,
    /// Privacy settings.
    pub privacy: PrivacySettings,
    /// Proactive behavior consent settings.
    pub proactive_settings: ProactiveSettings,
    /// Initial OCEAN adjustments from personality calibration.
    /// These are *deltas* from the default OCEAN values.
    pub ocean_adjustments: OceanTraits,
    /// Timestamp (ms) when the profile was first created.
    pub created_at_ms: u64,
    /// Timestamp (ms) of the last profile update.
    pub updated_at_ms: u64,
    /// Whether onboarding has been completed.
    pub onboarding_completed: bool,
    /// Number of days since onboarding (tracked by welcome system).
    pub days_since_onboarding: u32,
}

impl Default for UserProfile {
    fn default() -> Self {
        Self {
            schema_version: PROFILE_SCHEMA_VERSION,
            name: String::new(),
            interests: Vec::new(),
            daily_patterns: Vec::new(),
            preferences: UserPreferences::default(),
            privacy: PrivacySettings::default(),
            proactive_settings: ProactiveSettings::default(),
            ocean_adjustments: OceanTraits::DEFAULT,
            created_at_ms: 0,
            updated_at_ms: 0,
            onboarding_completed: false,
            days_since_onboarding: 0,
        }
    }
}

impl UserProfile {
    /// Create a new profile with the given name and current timestamp.
    pub fn new(name: &str, now_ms: u64) -> Result<Self, OnboardingError> {
        let trimmed = name.trim();
        if trimmed.len() > MAX_NAME_LENGTH {
            return Err(OnboardingError::ProfileError(format!(
                "name too long: {} chars (max {})",
                trimmed.len(),
                MAX_NAME_LENGTH
            )));
        }

        Ok(Self {
            name: trimmed.to_string(),
            created_at_ms: now_ms,
            updated_at_ms: now_ms,
            ..Default::default()
        })
    }

    /// Update the user's display name.
    pub fn set_name(&mut self, name: &str, now_ms: u64) -> Result<(), OnboardingError> {
        let trimmed = name.trim();
        if trimmed.len() > MAX_NAME_LENGTH {
            return Err(OnboardingError::ProfileError(format!(
                "name too long: {} chars (max {})",
                trimmed.len(),
                MAX_NAME_LENGTH
            )));
        }
        self.name = trimmed.to_string();
        self.updated_at_ms = now_ms;
        debug!(name = %self.name, "user name updated");
        Ok(())
    }

    /// Add an interest, deduplicating and respecting the cap.
    pub fn add_interest(&mut self, interest: &str, now_ms: u64) -> bool {
        let trimmed = interest.trim().to_lowercase();
        if trimmed.is_empty() {
            return false;
        }
        if self.interests.iter().any(|i| i.to_lowercase() == trimmed) {
            return false; // duplicate
        }
        if self.interests.len() >= MAX_INTERESTS {
            warn!(max = MAX_INTERESTS, "interest cap reached, dropping oldest");
            self.interests.remove(0);
        }
        self.interests.push(trimmed);
        self.updated_at_ms = now_ms;
        true
    }

    /// Remove an interest by exact match (case-insensitive).
    pub fn remove_interest(&mut self, interest: &str, now_ms: u64) -> bool {
        let trimmed = interest.trim().to_lowercase();
        let before = self.interests.len();
        self.interests.retain(|i| i.to_lowercase() != trimmed);
        if self.interests.len() != before {
            self.updated_at_ms = now_ms;
            true
        } else {
            false
        }
    }

    /// Add a daily pattern.
    pub fn add_daily_pattern(&mut self, pattern: DailyPattern, now_ms: u64) -> bool {
        if pattern.hour > 23 {
            return false;
        }
        if self.daily_patterns.len() >= MAX_DAILY_PATTERNS {
            // Remove the lowest-confidence inferred pattern.
            if let Some(idx) = self
                .daily_patterns
                .iter()
                .enumerate()
                .filter(|(_, p)| !p.user_stated)
                .min_by(|(_, a), (_, b)| a.confidence.partial_cmp(&b.confidence).unwrap())
                .map(|(i, _)| i)
            {
                self.daily_patterns.remove(idx);
            } else {
                return false; // All user-stated, can't evict.
            }
        }
        self.daily_patterns.push(pattern);
        self.updated_at_ms = now_ms;
        true
    }

    /// Apply OCEAN adjustments from personality calibration.
    ///
    /// The adjustments are deltas added to the default OCEAN values,
    /// then clamped to [0.1, 0.9].
    pub fn apply_ocean_calibration(&mut self, adjustments: OceanTraits, now_ms: u64) {
        self.ocean_adjustments = adjustments;
        self.ocean_adjustments.clamp_all();
        self.updated_at_ms = now_ms;
        info!(
            o = self.ocean_adjustments.openness,
            c = self.ocean_adjustments.conscientiousness,
            e = self.ocean_adjustments.extraversion,
            a = self.ocean_adjustments.agreeableness,
            n = self.ocean_adjustments.neuroticism,
            "OCEAN calibration applied"
        );
    }

    /// Compute the effective OCEAN traits (defaults + calibration adjustments).
    pub fn effective_ocean(&self) -> OceanTraits {
        let mut traits = OceanTraits {
            openness: OceanTraits::DEFAULT.openness
                + (self.ocean_adjustments.openness - OceanTraits::DEFAULT.openness),
            conscientiousness: OceanTraits::DEFAULT.conscientiousness
                + (self.ocean_adjustments.conscientiousness
                    - OceanTraits::DEFAULT.conscientiousness),
            extraversion: OceanTraits::DEFAULT.extraversion
                + (self.ocean_adjustments.extraversion - OceanTraits::DEFAULT.extraversion),
            agreeableness: OceanTraits::DEFAULT.agreeableness
                + (self.ocean_adjustments.agreeableness - OceanTraits::DEFAULT.agreeableness),
            neuroticism: OceanTraits::DEFAULT.neuroticism
                + (self.ocean_adjustments.neuroticism - OceanTraits::DEFAULT.neuroticism),
        };
        traits.clamp_all();
        traits
    }

    /// Mark onboarding as completed.
    pub fn complete_onboarding(&mut self, now_ms: u64) {
        self.onboarding_completed = true;
        self.updated_at_ms = now_ms;
        info!("onboarding marked as completed in user profile");
    }

    /// Increment days since onboarding (called by welcome system).
    pub fn tick_day(&mut self, now_ms: u64) {
        self.days_since_onboarding = self.days_since_onboarding.saturating_add(1);
        self.updated_at_ms = now_ms;
    }

    /// Check if proactive behavior is allowed for the current hour.
    /// Defaults to false for safety if no profile exists.
    pub fn is_proactive_allowed(&self, hour: u8) -> bool {
        self.proactive_settings.can_proact(hour)
    }

    /// Get a reference to the proactive settings.
    pub fn proactive_settings(&self) -> &ProactiveSettings {
        &self.proactive_settings
    }

    /// Get a mutable reference to the proactive settings.
    pub fn proactive_settings_mut(&mut self) -> &mut ProactiveSettings {
        &mut self.proactive_settings
    }

    // -----------------------------------------------------------------------
    // Persistence
    // -----------------------------------------------------------------------

    /// Serialize the profile to JSON bytes for storage.
    pub fn to_json(&self) -> Result<Vec<u8>, OnboardingError> {
        serde_json::to_vec(self)
            .map_err(|e| OnboardingError::PersistenceFailed(format!("serialize profile: {e}")))
    }

    /// Deserialize a profile from JSON bytes.
    pub fn from_json(data: &[u8]) -> Result<Self, OnboardingError> {
        serde_json::from_slice(data)
            .map_err(|e| OnboardingError::PersistenceFailed(format!("deserialize profile: {e}")))
    }

    /// Save the profile to a SQLite database.
    ///
    /// Creates the `user_profile` table if it doesn't exist, then upserts.
    pub fn save_to_db(&self, db: &rusqlite::Connection) -> Result<(), OnboardingError> {
        db.execute_batch(
            "CREATE TABLE IF NOT EXISTS user_profile (
                id INTEGER PRIMARY KEY CHECK (id = 1),
                data BLOB NOT NULL,
                updated_at_ms INTEGER NOT NULL
            );",
        )
        .map_err(|e| OnboardingError::PersistenceFailed(format!("create table: {e}")))?;

        let json = self.to_json()?;
        db.execute(
            "INSERT INTO user_profile (id, data, updated_at_ms)
             VALUES (1, ?1, ?2)
             ON CONFLICT(id) DO UPDATE SET data = ?1, updated_at_ms = ?2;",
            rusqlite::params![json, self.updated_at_ms as i64],
        )
        .map_err(|e| OnboardingError::PersistenceFailed(format!("upsert profile: {e}")))?;

        debug!(size_bytes = json.len(), "user profile saved to DB");
        Ok(())
    }

    /// Load the profile from a SQLite database.
    ///
    /// Returns `None` if no profile exists (first run).
    pub fn load_from_db(db: &rusqlite::Connection) -> Result<Option<Self>, OnboardingError> {
        // Check if table exists first.
        let table_exists: bool = db
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='user_profile';",
                [],
                |row| row.get::<_, i64>(0),
            )
            .map(|c| c > 0)
            .map_err(|e| OnboardingError::PersistenceFailed(format!("check table: {e}")))?;

        if !table_exists {
            return Ok(None);
        }

        let result: Result<Vec<u8>, _> =
            db.query_row("SELECT data FROM user_profile WHERE id = 1;", [], |row| {
                row.get(0)
            });

        match result {
            Ok(data) => {
                let profile = Self::from_json(&data)?;
                debug!("user profile loaded from DB");
                Ok(Some(profile))
            }
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(OnboardingError::PersistenceFailed(format!(
                "load profile: {e}"
            ))),
        }
    }

    // -----------------------------------------------------------------------
    // Export / Import (privacy compliance)
    // -----------------------------------------------------------------------

    /// Export the full profile as a human-readable JSON string.
    pub fn export_json(&self) -> Result<String, OnboardingError> {
        serde_json::to_string_pretty(self)
            .map_err(|e| OnboardingError::ProfileError(format!("export failed: {e}")))
    }

    /// Delete all profile data from the database (right to erasure).
    pub fn delete_from_db(db: &rusqlite::Connection) -> Result<(), OnboardingError> {
        db.execute_batch("DELETE FROM user_profile WHERE id = 1;")
            .map_err(|e| OnboardingError::PersistenceFailed(format!("delete profile: {e}")))?;

        info!("user profile deleted from DB (right to erasure)");
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_profile() {
        let p = UserProfile::new("Alice", 1000).expect("should succeed");
        assert_eq!(p.name, "Alice");
        assert_eq!(p.created_at_ms, 1000);
        assert_eq!(p.updated_at_ms, 1000);
        assert!(!p.onboarding_completed);
    }

    #[test]
    fn test_new_profile_trims_name() {
        let p = UserProfile::new("  Bob  ", 0).expect("should succeed");
        assert_eq!(p.name, "Bob");
    }

    #[test]
    fn test_name_too_long() {
        let long_name = "A".repeat(MAX_NAME_LENGTH + 1);
        let err = UserProfile::new(&long_name, 0).unwrap_err();
        assert!(matches!(err, OnboardingError::ProfileError(_)));
    }

    #[test]
    fn test_set_name() {
        let mut p = UserProfile::default();
        p.set_name("Charlie", 500).expect("should succeed");
        assert_eq!(p.name, "Charlie");
        assert_eq!(p.updated_at_ms, 500);
    }

    #[test]
    fn test_add_interest() {
        let mut p = UserProfile::default();
        assert!(p.add_interest("Rust", 100));
        assert!(p.add_interest("Music", 200));
        assert_eq!(p.interests.len(), 2);
        assert_eq!(p.updated_at_ms, 200);
    }

    #[test]
    fn test_add_interest_dedup() {
        let mut p = UserProfile::default();
        assert!(p.add_interest("Rust", 100));
        assert!(!p.add_interest("rust", 200)); // case-insensitive dup
        assert_eq!(p.interests.len(), 1);
    }

    #[test]
    fn test_add_interest_empty_rejected() {
        let mut p = UserProfile::default();
        assert!(!p.add_interest("", 100));
        assert!(!p.add_interest("  ", 100));
    }

    #[test]
    fn test_add_interest_cap() {
        let mut p = UserProfile::default();
        for i in 0..MAX_INTERESTS {
            assert!(p.add_interest(&format!("topic_{i}"), i as u64));
        }
        assert_eq!(p.interests.len(), MAX_INTERESTS);
        // Adding one more should evict the oldest.
        assert!(p.add_interest("overflow", 9999));
        assert_eq!(p.interests.len(), MAX_INTERESTS);
        assert!(p.interests.contains(&"overflow".to_string()));
        assert!(!p.interests.contains(&"topic_0".to_string()));
    }

    #[test]
    fn test_remove_interest() {
        let mut p = UserProfile::default();
        p.add_interest("Rust", 100);
        p.add_interest("Music", 200);
        assert!(p.remove_interest("RUST", 300)); // case-insensitive
        assert_eq!(p.interests.len(), 1);
        assert_eq!(p.updated_at_ms, 300);
    }

    #[test]
    fn test_remove_interest_not_found() {
        let mut p = UserProfile::default();
        assert!(!p.remove_interest("Nope", 100));
    }

    #[test]
    fn test_add_daily_pattern() {
        let mut p = UserProfile::default();
        let pattern = DailyPattern {
            description: "Wake up".into(),
            hour: 7,
            confidence: 0.9,
            user_stated: true,
        };
        assert!(p.add_daily_pattern(pattern, 100));
        assert_eq!(p.daily_patterns.len(), 1);
    }

    #[test]
    fn test_add_daily_pattern_invalid_hour() {
        let mut p = UserProfile::default();
        let pattern = DailyPattern {
            description: "Bad hour".into(),
            hour: 25,
            confidence: 0.5,
            user_stated: false,
        };
        assert!(!p.add_daily_pattern(pattern, 100));
    }

    #[test]
    fn test_ocean_calibration() {
        let mut p = UserProfile::default();
        let adj = OceanTraits {
            openness: 0.9,
            conscientiousness: 0.6,
            extraversion: 0.7,
            agreeableness: 0.8,
            neuroticism: 0.3,
        };
        p.apply_ocean_calibration(adj, 500);
        assert!((p.ocean_adjustments.openness - 0.9).abs() < f32::EPSILON);
        assert_eq!(p.updated_at_ms, 500);
    }

    #[test]
    fn test_effective_ocean() {
        let p = UserProfile::default();
        let effective = p.effective_ocean();
        // With default adjustments (same as DEFAULT), effective should equal DEFAULT.
        assert!((effective.openness - OceanTraits::DEFAULT.openness).abs() < f32::EPSILON);
    }

    #[test]
    fn test_complete_onboarding() {
        let mut p = UserProfile::default();
        assert!(!p.onboarding_completed);
        p.complete_onboarding(1000);
        assert!(p.onboarding_completed);
    }

    #[test]
    fn test_tick_day() {
        let mut p = UserProfile::default();
        assert_eq!(p.days_since_onboarding, 0);
        p.tick_day(1000);
        assert_eq!(p.days_since_onboarding, 1);
        p.tick_day(2000);
        assert_eq!(p.days_since_onboarding, 2);
    }

    #[test]
    fn test_json_roundtrip() {
        let mut p = UserProfile::new("Test", 1000).expect("ok");
        p.add_interest("Coding", 2000);
        let json = p.to_json().expect("serialize");
        let restored = UserProfile::from_json(&json).expect("deserialize");
        assert_eq!(restored.name, "Test");
        assert_eq!(restored.interests.len(), 1);
    }

    #[test]
    fn test_export_json() {
        let p = UserProfile::new("Export", 100).expect("ok");
        let json_str = p.export_json().expect("export");
        assert!(json_str.contains("Export"));
    }

    #[test]
    fn test_db_save_load_roundtrip() {
        let db = rusqlite::Connection::open_in_memory().expect("open db");
        let mut p = UserProfile::new("DB-Test", 5000).expect("ok");
        p.add_interest("Testing", 6000);
        p.save_to_db(&db).expect("save");

        let loaded = UserProfile::load_from_db(&db)
            .expect("load")
            .expect("should exist");
        assert_eq!(loaded.name, "DB-Test");
        assert_eq!(loaded.interests, vec!["testing"]);
    }

    #[test]
    fn test_db_load_no_table() {
        let db = rusqlite::Connection::open_in_memory().expect("open db");
        let loaded = UserProfile::load_from_db(&db).expect("load");
        assert!(loaded.is_none());
    }

    #[test]
    fn test_db_delete() {
        let db = rusqlite::Connection::open_in_memory().expect("open db");
        let p = UserProfile::new("ToDelete", 100).expect("ok");
        p.save_to_db(&db).expect("save");
        assert!(UserProfile::load_from_db(&db).expect("load").is_some());

        UserProfile::delete_from_db(&db).expect("delete");
        assert!(UserProfile::load_from_db(&db).expect("load").is_none());
    }

    #[test]
    fn test_privacy_settings_default() {
        let ps = PrivacySettings::default();
        assert_eq!(ps.level, PrivacyLevel::Standard);
        assert!(ps.allow_notification_reading);
        assert!(!ps.allow_screen_observation);
        assert!(ps.allow_learning);
    }

    #[test]
    fn test_preferences_default() {
        let prefs = UserPreferences::default();
        assert_eq!(prefs.communication_style, CommunicationStyle::Balanced);
        assert_eq!(prefs.morning_briefing_hour, 7);
        assert!(prefs.likes_humor);
        assert_eq!(prefs.locale, "en-US");
    }
}
