//! User profile — persistent representation of the user AURA is serving.
//!
//! Stores name, preferences, interests, daily patterns, privacy settings,
//! and provides persistence to/from SQLite and a two-tier file-backed system.
//!
//! # Two-tier persistence
//!
//! - **Public tier** (`user_profile.json`): human-readable/editable preferences (name, locale,
//!   timezone, behavior modifiers). Written atomically via a temp-file rename so partial writes
//!   never corrupt the file.
//! - **Private tier** (vault, AES-256-GCM): trust tier, relationship history, personal revelations.
//!   Stored under the `user_profile_sensitive` vault key. Gracefully skipped if the vault is
//!   unavailable.

use std::path::Path;

use aura_types::{errors::OnboardingError, identity::OceanTraits};
use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum PrivacyLevel {
    /// Minimal data retention — only what's needed for core function.
    Minimal,
    /// Standard — retain interaction patterns, preferences, no raw messages.
    #[default]
    Standard,
    /// Full — retain everything for maximum personalisation.
    Full,
}

/// Notification preference for proactive suggestions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum NotificationPreference {
    /// All proactive notifications enabled.
    #[default]
    All,
    /// Only important notifications.
    ImportantOnly,
    /// No proactive notifications.
    None,
}

/// Communication style preference as expressed by the user.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum CommunicationStyle {
    /// Brief, to-the-point responses.
    Concise,
    /// Balanced responses.
    #[default]
    Balanced,
    /// Detailed, thorough responses.
    Detailed,
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

/// Comprehensive GDPR data export struct — Article 15 (Right to Access) & Article 20 (Data Portability).
///
/// Includes: profile data, memory tier counts + samples, vault entries, consent records.
/// This is what gets returned to users who request their data.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FullGdprExport {
    /// Timestamp of export (milliseconds since epoch).
    pub exported_at_ms: i64,
    /// The full user profile as JSON.
    pub profile: serde_json::Value,
    /// Working memory slot count.
    pub working_memory: usize,
    /// Episodic memory episode count.
    pub episodic_count: u64,
    /// Semantic memory entry count.
    pub semantic_count: u64,
    /// Archive memory blob count.
    pub archive_count: u64,
    /// All non-Critical vault entries with decrypted values.
    pub vault_entries: Vec<crate::persistence::vault::VaultEntryExport>,
    /// All consent records.
    pub consent_records: Vec<crate::identity::ConsentRecord>,
}

/// Result of a GDPR erasure operation — Article 17 (Right to Erasure / "Right to be Forgotten").
///
/// This confirms what was deleted from each tier.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GdprErasureResult {
    /// Working memory slots cleared.
    pub working_slots: usize,
    /// Episodic episodes deleted.
    pub episodic_episodes: u64,
    /// Semantic entries deleted.
    pub semantic_entries: u64,
    /// Archive blobs deleted.
    pub archive_blobs: u64,
    /// Vault entries destroyed (cryptographic key erased).
    pub vault_entries: usize,
    /// Whether consent records were cleared.
    pub consent_records_cleared: bool,
    /// Whether the profile row was deleted from the database.
    pub profile_deleted: bool,
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
            ocean_adjustments: OceanTraits {
                openness: 0.0,
                conscientiousness: 0.0,
                extraversion: 0.0,
                agreeableness: 0.0,
                neuroticism: 0.0,
            },
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
                .min_by(|(_, a), (_, b)| {
                    a.confidence
                        .partial_cmp(&b.confidence)
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
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
    ///
    /// `ocean_adjustments` stores *deltas* from `OceanTraits::DEFAULT`.
    /// A zero-delta profile returns DEFAULT exactly.
    pub fn effective_ocean(&self) -> OceanTraits {
        let mut traits = OceanTraits {
            openness: OceanTraits::DEFAULT.openness + self.ocean_adjustments.openness,
            conscientiousness: OceanTraits::DEFAULT.conscientiousness
                + self.ocean_adjustments.conscientiousness,
            extraversion: OceanTraits::DEFAULT.extraversion + self.ocean_adjustments.extraversion,
            agreeableness: OceanTraits::DEFAULT.agreeableness
                + self.ocean_adjustments.agreeableness,
            neuroticism: OceanTraits::DEFAULT.neuroticism + self.ocean_adjustments.neuroticism,
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
            },
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(OnboardingError::PersistenceFailed(format!(
                "load profile: {e}"
            ))),
        }
    }

    // -----------------------------------------------------------------------
    // GDPR: Right to Access (Article 15) & Data Portability (Article 20)
    // -----------------------------------------------------------------------

    /// Comprehensive GDPR data export — includes profile, all memory tiers,
    /// vault entries (non-Critical), and consent records.
    ///
    /// This is the method that should be called for GDPR Right to Access requests.
    /// Returns a struct that can be serialized to JSON for the user.
    pub async fn export_comprehensive(
        &self,
        memory: &crate::memory::AuraMemory,
        vault: &mut crate::persistence::vault::CriticalVault,
        consent_tracker: &crate::identity::ConsentTracker,
    ) -> Result<FullGdprExport, OnboardingError> {
        let profile = serde_json::to_value(self)
            .map_err(|e| OnboardingError::ProfileError(format!("profile serialization: {e}")))?;

        let working = memory.export_working();
        let episodic = memory
            .export_episodic()
            .await
            .map_err(|e| OnboardingError::ProfileError(format!("episodic export: {e}")))?;
        let semantic = memory
            .export_semantic()
            .await
            .map_err(|e| OnboardingError::ProfileError(format!("semantic export: {e}")))?;
        let archive = memory
            .export_archive()
            .await
            .map_err(|e| OnboardingError::ProfileError(format!("archive export: {e}")))?;
        let vault_entries = vault.export_all();
        let consent_records = consent_tracker
            .get_all_consents()
            .into_iter()
            .cloned()
            .collect::<Vec<_>>();

        Ok(FullGdprExport {
            exported_at_ms: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as i64,
            profile,
            working_memory: working.len(),
            episodic_count: episodic.len() as u64,
            semantic_count: semantic.len() as u64,
            archive_count: archive.len() as u64,
            vault_entries,
            consent_records,
        })
    }

    /// Export the profile as a human-readable JSON string (backwards-compatible).
    ///
    /// NOTE: This only exports the UserProfile struct. For full GDPR Right to Access,
    /// use [`Self::export_comprehensive()`] which includes all memory tiers, vault,
    /// and consent records.
    pub fn export_json(&self) -> Result<String, OnboardingError> {
        serde_json::to_string_pretty(self)
            .map_err(|e| OnboardingError::ProfileError(format!("export failed: {e}")))
    }

    // -----------------------------------------------------------------------
    // GDPR: Right to Erasure (Article 17)
    // -----------------------------------------------------------------------

    /// Delete ALL user data — complete GDPR "right to be forgotten" erasure.
    ///
    /// This erases data from every tier:
    /// 1. **Memory**: Working, episodic, semantic, and archive tiers
    /// 2. **Vault**: Cryptographic key erasure + data zeroing (nuclear option)
    /// 3. **Consent**: All consent records cleared
    /// 4. **Profile DB**: user_profile table entry deleted
    ///
    /// After this operation, the user is completely unrecoverable in AURA.
    pub async fn delete_with_gdpr(
        db: &rusqlite::Connection,
        memory: &mut crate::memory::AuraMemory,
        vault: &mut crate::persistence::vault::CriticalVault,
        consent_tracker: &mut crate::identity::ConsentTracker,
    ) -> Result<GdprErasureResult, OnboardingError> {
        info!("GDPR erasure: initiating complete user data deletion");

        // Step 1: Erase all memory tiers
        let memory_report = memory
            .erase_all()
            .await
            .map_err(|e| OnboardingError::ProfileError(format!("memory erasure: {e}")))?;

        // Step 2: Cryptographic key erasure + data zeroing (vault nuclear option)
        let vault_deleted = vault.clear();

        // Step 3: Clear all consent records
        consent_tracker.clear();

        // Step 4: Delete profile from database
        db.execute_batch("DELETE FROM user_profile WHERE id = 1;")
            .map_err(|e| OnboardingError::PersistenceFailed(format!("delete profile: {e}")))?;

        let result = GdprErasureResult {
            working_slots: memory_report.working_slots_cleared,
            episodic_episodes: memory_report.episodic_episodes_deleted,
            semantic_entries: memory_report.semantic_entries_deleted,
            archive_blobs: memory_report.archive_blobs_deleted,
            vault_entries: vault_deleted,
            consent_records_cleared: true,
            profile_deleted: true,
        };

        info!(
            "GDPR erasure complete: {} working, {} episodic, {} semantic, {} archive, {} vault entries",
            result.working_slots,
            result.episodic_episodes,
            result.semantic_entries,
            result.archive_blobs,
            result.vault_entries
        );

        Ok(result)
    }

    /// Delete only the profile from the database (no memory/vault/consent erasure).
    ///
    /// WARNING: This is NOT a complete GDPR erasure. Use [`Self::delete_with_gdpr()`]
    /// for actual GDPR Article 17 compliance.
    pub fn erase_profile_only(db: &rusqlite::Connection) -> Result<(), OnboardingError> {
        db.execute_batch("DELETE FROM user_profile WHERE id = 1;")
            .map_err(|e| OnboardingError::PersistenceFailed(format!("delete profile: {e}")))?;

        info!("profile erased from DB (profile-only — NOT a complete GDPR erasure)");
        Ok(())
    }

    /// @deprecated Use [`Self::erase_profile_only()`] instead. The old `delete_from_db`
    /// name was misleading — it only deleted the profile, not all user data.
    #[deprecated(
        since = "0.4.0",
        note = "use erase_profile_only() or delete_with_gdpr()"
    )]
    pub fn delete_from_db(db: &rusqlite::Connection) -> Result<(), OnboardingError> {
        Self::erase_profile_only(db)
    }

    // -----------------------------------------------------------------------
    // Two-tier file-backed persistence
    // -----------------------------------------------------------------------

    /// Load the profile from the two-tier system, or create a default if absent.
    ///
    /// - Public tier: `{config_dir}/user_profile.json` (plain JSON)
    /// - Private tier: vault key `user_profile_sensitive` (if vault available)
    ///
    /// Returns a fresh default profile on first run (neither file nor vault entry
    /// exists). Vault errors are logged as warnings and do not prevent loading.
    pub fn load_or_create(
        config_dir: &Path,
        vault: Option<&mut crate::persistence::vault::CriticalVault>,
    ) -> Result<Self, OnboardingError> {
        let profile_path = config_dir.join("user_profile.json");

        let mut profile = if profile_path.exists() {
            let raw = std::fs::read(&profile_path).map_err(|e| {
                OnboardingError::PersistenceFailed(format!("read user_profile.json: {e}"))
            })?;
            let p: ProfilePreferences = serde_json::from_slice(&raw).map_err(|e| {
                OnboardingError::PersistenceFailed(format!("parse user_profile.json: {e}"))
            })?;
            debug!("user profile preferences loaded from file");
            let mut profile = UserProfile::default();
            profile.name = p.name;
            profile.preferences.locale = p.preferred_language;
            profile.preferences.communication_style = match p.behavior_modifiers.verbosity.as_str()
            {
                "concise" => CommunicationStyle::Concise,
                "detailed" => CommunicationStyle::Detailed,
                _ => CommunicationStyle::Balanced,
            };
            if let Some(ts) = p.created_at_ms {
                profile.created_at_ms = ts;
            }
            if let Some(ts) = p.last_seen_ms {
                profile.updated_at_ms = ts;
            }
            profile
        } else {
            debug!("no user_profile.json found — using default profile");
            let now_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64;
            let mut p = UserProfile::default();
            p.created_at_ms = now_ms;
            p.updated_at_ms = now_ms;
            p
        };

        // Load sensitive tier from vault (non-fatal).
        if let Some(vault) = vault {
            match vault.retrieve("user_profile_sensitive", "user_profile", true) {
                Ok(bytes) => {
                    match serde_json::from_slice::<ProfileSensitive>(&bytes) {
                        Ok(sensitive) => {
                            // Merge sensitive fields back into profile.
                            profile.interests = sensitive.interests;
                            debug!("user profile sensitive data loaded from vault");
                        },
                        Err(e) => {
                            warn!("failed to parse sensitive profile from vault: {e}");
                        },
                    }
                },
                Err(e) => {
                    // Not found or vault not configured — not an error on first run.
                    debug!("sensitive profile not in vault (may be first run): {e:?}");
                },
            }
        }

        Ok(profile)
    }

    /// Save the public (non-sensitive) preferences tier to `{config_dir}/user_profile.json`.
    ///
    /// Uses an atomic temp-file rename to prevent partial writes.
    pub fn save_preferences(&self, config_dir: &Path) -> Result<(), OnboardingError> {
        std::fs::create_dir_all(config_dir)
            .map_err(|e| OnboardingError::PersistenceFailed(format!("create config dir: {e}")))?;

        let prefs = ProfilePreferences {
            version: PROFILE_SCHEMA_VERSION,
            name: self.name.clone(),
            preferred_language: self.preferences.locale.clone(),
            timezone: String::new(), // Reserved for future use.
            behavior_modifiers: BehaviorModifiers {
                verbosity: match self.preferences.communication_style {
                    CommunicationStyle::Concise => "concise".to_string(),
                    CommunicationStyle::Balanced => "balanced".to_string(),
                    CommunicationStyle::Detailed => "detailed".to_string(),
                },
                formality: "neutral".to_string(),
                proactivity: if self.proactive_settings.consent.is_allowed() {
                    "high".to_string()
                } else {
                    "low".to_string()
                },
            },
            created_at_ms: Some(self.created_at_ms),
            last_seen_ms: Some(self.updated_at_ms),
        };

        let json = serde_json::to_vec_pretty(&prefs).map_err(|e| {
            OnboardingError::PersistenceFailed(format!("serialize preferences: {e}"))
        })?;

        // Atomic write via temp file.
        let profile_path = config_dir.join("user_profile.json");
        let tmp_path = config_dir.join("user_profile.json.tmp");
        std::fs::write(&tmp_path, &json).map_err(|e| {
            OnboardingError::PersistenceFailed(format!("write tmp preferences: {e}"))
        })?;
        std::fs::rename(&tmp_path, &profile_path).map_err(|e| {
            OnboardingError::PersistenceFailed(format!("rename preferences file: {e}"))
        })?;

        debug!(
            size_bytes = json.len(),
            "user profile preferences saved to file"
        );
        Ok(())
    }

    /// Save sensitive profile data to the vault.
    ///
    /// Stored under key `user_profile_sensitive` at `DataTier::Personal`.
    /// If the vault is `None` or not configured, this is a no-op with a warning.
    pub fn save_sensitive(
        &self,
        vault: Option<&mut crate::persistence::vault::CriticalVault>,
    ) -> Result<(), OnboardingError> {
        let vault = match vault {
            Some(v) => v,
            None => {
                warn!("vault not available — skipping sensitive profile save");
                return Ok(());
            },
        };

        let sensitive = ProfileSensitive {
            interests: self.interests.clone(),
        };

        let bytes = serde_json::to_vec(&sensitive).map_err(|e| {
            OnboardingError::PersistenceFailed(format!("serialize sensitive profile: {e}"))
        })?;

        vault
            .store(
                "user_profile_sensitive",
                &bytes,
                crate::persistence::vault::DataTier::Personal,
                crate::persistence::vault::EntryMetadata {
                    description: "User profile sensitive data (interests, personal context)"
                        .to_string(),
                    category: crate::persistence::vault::DataCategory::Personal,
                    auto_classified: false,
                    expiry_ms: None,
                },
            )
            .map_err(|e| {
                OnboardingError::PersistenceFailed(format!("vault store sensitive profile: {e:?}"))
            })?;

        debug!("user profile sensitive data saved to vault");
        Ok(())
    }

    /// Save both tiers: preferences to JSON file and sensitive data to vault.
    ///
    /// Equivalent to calling `save_preferences` then `save_sensitive`.
    pub fn save(
        &self,
        config_dir: &Path,
        vault: Option<&mut crate::persistence::vault::CriticalVault>,
    ) -> Result<(), OnboardingError> {
        self.save_preferences(config_dir)?;
        self.save_sensitive(vault)?;
        info!("user profile saved (both tiers)");
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Fixed-path convenience persistence (no-arg variants)
    // -----------------------------------------------------------------------

    /// Return the canonical config path: `~/.config/aura/user_profile.json`.
    pub fn config_path() -> std::path::PathBuf {
        let home = if let Ok(h) = std::env::var("HOME") {
            std::path::PathBuf::from(h)
        } else {
            #[cfg(target_os = "windows")]
            {
                if let Ok(up) = std::env::var("USERPROFILE") {
                    std::path::PathBuf::from(up)
                } else {
                    let drive = std::env::var("HOMEDRIVE").unwrap_or_else(|_| "C:".to_string());
                    let homepath =
                        std::env::var("HOMEPATH").unwrap_or_else(|_| "\\Users\\user".to_string());
                    std::path::PathBuf::from(format!("{}{}", drive, homepath))
                }
            }
            #[cfg(not(target_os = "windows"))]
            std::path::PathBuf::from("/tmp")
        };
        home.join(".config").join("aura").join("user_profile.json")
    }

    /// Serialize non-sensitive fields to `~/.config/aura/user_profile.json`.
    ///
    /// Uses an atomic temp-file rename to prevent partial writes.
    /// This is a convenience wrapper around [`save_preferences`] using the
    /// fixed path returned by [`config_path`].
    pub fn save_preferences_default(&self) -> Result<(), Box<dyn std::error::Error>> {
        let path = Self::config_path();
        let config_dir = path.parent().ok_or("config path has no parent directory")?;
        self.save_preferences(config_dir)
            .map_err(|e| Box::new(e) as Box<dyn std::error::Error>)
    }

    /// Deserialize from `~/.config/aura/user_profile.json`, returning a
    /// fresh default profile if the file is absent or cannot be parsed.
    ///
    /// This is a convenience wrapper around [`load_or_create`] that uses the
    /// fixed path returned by [`config_path`] and no vault.
    pub fn load_preferences_default() -> Result<Self, Box<dyn std::error::Error>> {
        let path = Self::config_path();
        let config_dir = path.parent().ok_or("config path has no parent directory")?;
        Self::load_or_create(config_dir, None)
            .map_err(|e| Box::new(e) as Box<dyn std::error::Error>)
    }
}

// ---------------------------------------------------------------------------
// File-backed persistence helper types
// ---------------------------------------------------------------------------

/// Public (human-readable) tier stored in `user_profile.json`.
#[derive(Debug, Serialize, Deserialize)]
struct ProfilePreferences {
    version: u32,
    name: String,
    preferred_language: String,
    timezone: String,
    behavior_modifiers: BehaviorModifiers,
    #[serde(skip_serializing_if = "Option::is_none")]
    created_at_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_seen_ms: Option<u64>,
}

/// Behavior modifier fields within the public preferences JSON.
#[derive(Debug, Serialize, Deserialize)]
struct BehaviorModifiers {
    /// verbosity: "concise" | "balanced" | "detailed"
    verbosity: String,
    /// formality: "casual" | "neutral" | "formal"
    formality: String,
    /// proactivity: "low" | "medium" | "high"
    proactivity: String,
}

/// Private (vault) tier — sensitive interests and personal data.
#[derive(Debug, Serialize, Deserialize)]
struct ProfileSensitive {
    interests: Vec<String>,
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
        // With zero adjustments (no calibration), effective should equal DEFAULT.
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

        UserProfile::erase_profile_only(&db).expect("delete");
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
