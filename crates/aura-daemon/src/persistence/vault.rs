//! Critical Data Vault — 4-tier data classification with encryption tiers.
//!
//! AURA's memory IS identity ("I Remember Therefore I Become"), but not all
//! memories are equal. The vault enforces strict data classification to protect
//! the user's most sensitive information while keeping everyday data accessible.
//!
//! # Tier Architecture
//!
//! ```text
//! ┌──────────────────────────────────────────────────────────┐
//! │ Tier 0: EPHEMERAL — RAM only, never persisted           │
//! ├──────────────────────────────────────────────────────────┤
//! │ Tier 1: PERSONAL  — App-level encryption, SQLite        │
//! ├──────────────────────────────────────────────────────────┤
//! │ Tier 2: SENSITIVE — Keystore + encrypted DB, consent    │
//! ├──────────────────────────────────────────────────────────┤
//! │ Tier 3: CRITICAL  — Keystore + biometric + PIN, per-    │
//! │                     access auth, NEVER in search/LLM    │
//! └──────────────────────────────────────────────────────────┘
//! ```
//!
//! # Anti-Cloud Manifesto
//!
//! All data stays on-device. The vault NEVER transmits data externally.
//! Export manifests list keys and metadata, NEVER values.

use std::collections::{HashMap, VecDeque};

use aes_gcm::{
    aead::{Aead, KeyInit, OsRng},
    Aes256Gcm, Nonce,
};
use argon2::{Algorithm, Argon2, Params, Version};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use zeroize::{Zeroize, ZeroizeOnDrop};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum number of vault entries (mobile-conscious: ~1KB avg = ~1MB total).
const MAX_VAULT_ENTRIES: usize = 1024;

/// Maximum number of access log entries retained.
const MAX_ACCESS_LOG_ENTRIES: usize = 2048;

/// Retention limit for sensitive data (days).
const SENSITIVE_RETENTION_DAYS: u32 = 365;

/// Retention limit for critical data (days).
const CRITICAL_RETENTION_DAYS: u32 = 90;

// NOTE: No confidence scoring for classification — Rust detects structural
// data patterns (digit counts, keyword presence) only. The LLM reasons about
// user intent; Rust only determines encryption tier from data structure.

// ---------------------------------------------------------------------------
// BoundedVec<T>
// ---------------------------------------------------------------------------

/// A vector with a hard upper bound on capacity.
///
/// When the bound is reached, the oldest element (index 0) is evicted to
/// make room for the new one. This guarantees bounded memory usage on
/// mobile devices (4–8 GB RAM).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BoundedVec<T> {
    inner: VecDeque<T>,
    capacity: usize,
}

impl<T> BoundedVec<T> {
    /// Create a new bounded vector with the given maximum capacity.
    pub fn new(capacity: usize) -> Self {
        Self {
            inner: VecDeque::with_capacity(capacity.min(256)), // don't pre-alloc huge
            capacity,
        }
    }

    /// Push an element, evicting the oldest if at capacity. O(1) amortized.
    pub fn push(&mut self, item: T) {
        if self.inner.len() >= self.capacity {
            self.inner.pop_front();
        }
        self.inner.push_back(item);
    }

    /// Number of elements currently stored.
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
        self.capacity
    }

    /// Iterate over all elements.
    pub fn iter(&self) -> std::collections::vec_deque::Iter<'_, T> {
        self.inner.iter()
    }

    /// Retain only elements matching a predicate.
    pub fn retain<F: FnMut(&T) -> bool>(&mut self, f: F) {
        self.inner.retain(f);
    }

    /// Clear all elements.
    pub fn clear(&mut self) {
        self.inner.clear();
    }

    /// Collect the contents as a `Vec<T>` (for testing/serialization).
    #[must_use]
    pub fn to_vec(&self) -> Vec<T>
    where
        T: Clone, {
        self.inner.iter().cloned().collect()
    }
}

// ---------------------------------------------------------------------------
// DataTier
// ---------------------------------------------------------------------------

/// 4-tier data classification system.
///
/// Higher tiers require progressively stronger authentication and impose
/// stricter access controls. The tier determines encryption, retention,
/// search visibility, and LLM exposure policies.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum DataTier {
    /// Tier 0: Working memory / conversation context. RAM only, never saved.
    Ephemeral = 0,
    /// Tier 1: Preferences, habits, routines. App-level encryption. Persists.
    Personal = 1,
    /// Tier 2: Health, contacts, calendar, location. Keystore + encrypted DB.
    Sensitive = 2,
    /// Tier 3: Passwords, bank accounts, IDs, legal. Keystore + biometric + PIN.
    Critical = 3,
}

impl DataTier {
    /// Whether this tier requires encryption at rest.
    #[must_use]
    pub fn requires_encryption(self) -> bool {
        matches!(self, Self::Personal | Self::Sensitive | Self::Critical)
    }

    /// Whether this tier requires biometric authentication for access.
    #[must_use]
    pub fn requires_biometric(self) -> bool {
        matches!(self, Self::Critical)
    }

    /// Whether accesses to this tier must be logged.
    #[must_use]
    pub fn requires_access_logging(self) -> bool {
        matches!(self, Self::Sensitive | Self::Critical)
    }

    /// Maximum retention period in days. `None` means unlimited (Ephemeral
    /// data is never persisted, so retention is not applicable).
    #[must_use]
    pub fn max_retention_days(self) -> Option<u32> {
        match self {
            Self::Ephemeral => None,
            Self::Personal => None, // unlimited
            Self::Sensitive => Some(SENSITIVE_RETENTION_DAYS),
            Self::Critical => Some(CRITICAL_RETENTION_DAYS),
        }
    }

    /// Convert from a numeric tier value.
    #[must_use]
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(Self::Ephemeral),
            1 => Some(Self::Personal),
            2 => Some(Self::Sensitive),
            3 => Some(Self::Critical),
            _ => None,
        }
    }

    /// Numeric tier value.
    #[must_use]
    pub fn as_u8(self) -> u8 {
        self as u8
    }
}

impl std::fmt::Display for DataTier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Ephemeral => write!(f, "Tier0:Ephemeral"),
            Self::Personal => write!(f, "Tier1:Personal"),
            Self::Sensitive => write!(f, "Tier2:Sensitive"),
            Self::Critical => write!(f, "Tier3:Critical"),
        }
    }
}

// ---------------------------------------------------------------------------
// DataCategory
// ---------------------------------------------------------------------------

/// Semantic category of stored data, used for classification and export
/// manifests. Helps the user understand what AURA remembers (transparency).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DataCategory {
    /// Bank accounts, card numbers, financial transactions.
    Financial,
    /// PAN, Aadhaar, passport, driver's license.
    Identity,
    /// Medical records, prescriptions, health metrics.
    Health,
    /// Passwords, API keys, tokens, PINs, OTPs.
    Credential,
    /// Phone numbers, addresses, email addresses.
    Contact,
    /// Legal documents, contracts, agreements.
    Legal,
    /// Preferences, habits, routines — everyday data.
    Personal,
    /// Application-defined category.
    Other(String),
}

impl std::fmt::Display for DataCategory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Financial => write!(f, "Financial"),
            Self::Identity => write!(f, "Identity"),
            Self::Health => write!(f, "Health"),
            Self::Credential => write!(f, "Credential"),
            Self::Contact => write!(f, "Contact"),
            Self::Legal => write!(f, "Legal"),
            Self::Personal => write!(f, "Personal"),
            Self::Other(s) => write!(f, "Other({s})"),
        }
    }
}

// ---------------------------------------------------------------------------
// EntryMetadata
// ---------------------------------------------------------------------------

/// Human-readable metadata attached to each vault entry. Supports
/// transparency: the user can inspect what AURA stores and why.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntryMetadata {
    /// Human-readable description (e.g., "Bank account number").
    pub description: String,
    /// Semantic category of the data.
    pub category: DataCategory,
    /// Whether this tier/category was auto-detected or user-specified.
    pub auto_classified: bool,
    /// Optional absolute expiry timestamp (epoch ms). Entries past this
    /// time are purged by `purge_expired`.
    pub expiry_ms: Option<u64>,
}

// ---------------------------------------------------------------------------
// VaultEntry
// ---------------------------------------------------------------------------

/// A single entry in the Critical Data Vault.
///
/// The `encrypted_value` field holds the ciphertext; the actual encryption
/// is performed by the platform-specific keystore layer above this module.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VaultEntry {
    /// Unique key identifying this entry (e.g., "bank_account_hdfc").
    pub key: String,
    /// Data classification tier.
    pub tier: DataTier,
    /// Encrypted payload. For Tier 0 (Ephemeral), this is cleartext in RAM.
    pub encrypted_value: Vec<u8>,
    /// Creation timestamp (epoch ms).
    pub created_ms: u64,
    /// Last access timestamp (epoch ms).
    pub last_accessed_ms: u64,
    /// Number of times this entry has been accessed.
    pub access_count: u32,
    /// Human-readable metadata for transparency.
    pub metadata: EntryMetadata,
}

// ---------------------------------------------------------------------------
// AccessOperation
// ---------------------------------------------------------------------------

/// Operations that can be performed on a vault entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AccessOperation {
    /// Reading an entry's value.
    Read,
    /// Writing / updating an entry.
    Write,
    /// Deleting an entry.
    Delete,
    /// Searching across entries.
    Search,
    /// Exporting manifest (keys + tiers, never values).
    Export,
}

impl std::fmt::Display for AccessOperation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Read => write!(f, "Read"),
            Self::Write => write!(f, "Write"),
            Self::Delete => write!(f, "Delete"),
            Self::Search => write!(f, "Search"),
            Self::Export => write!(f, "Export"),
        }
    }
}

// ---------------------------------------------------------------------------
// AccessLogEntry
// ---------------------------------------------------------------------------

/// Audit trail entry for vault accesses. Tier 2+ accesses are always logged.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccessLogEntry {
    /// When the access occurred (epoch ms).
    pub timestamp_ms: u64,
    /// Key of the entry accessed.
    pub key: String,
    /// Tier of the entry accessed.
    pub tier: DataTier,
    /// What operation was performed.
    pub operation: AccessOperation,
    /// Whether the caller was authenticated (biometric/PIN).
    pub authenticated: bool,
    /// Which subsystem performed the access (e.g., "memory_search", "export").
    pub caller: String,
}

// ---------------------------------------------------------------------------
// VaultError
// ---------------------------------------------------------------------------

/// Errors returned by the Critical Data Vault.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum VaultError {
    /// Tier 2+ access without authentication.
    AuthenticationRequired,
    /// Tier 3 access without biometric verification.
    BiometricRequired,
    /// Requested key does not exist.
    EntryNotFound,
    /// Vault has reached `MAX_VAULT_ENTRIES`.
    VaultFull,
    /// Attempted to store data at a lower tier than expected.
    TierMismatch { expected: DataTier, got: DataTier },
    /// Encryption/decryption failure from the platform keystore.
    EncryptionFailed(String),
    /// Decryption failure — ciphertext corrupted or wrong key.
    DecryptionFailed(String),
    /// PIN hashing or verification failure.
    HashFailed(String),
    /// Key string is empty or invalid.
    InvalidKey,
    /// Entry has passed its expiry timestamp.
    Expired,
}

impl std::fmt::Display for VaultError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AuthenticationRequired => write!(f, "authentication required for this tier"),
            Self::BiometricRequired => write!(f, "biometric verification required (Tier 3)"),
            Self::EntryNotFound => write!(f, "vault entry not found"),
            Self::VaultFull => write!(f, "vault is full ({MAX_VAULT_ENTRIES} entries)"),
            Self::TierMismatch { expected, got } => {
                write!(f, "tier mismatch: expected {expected}, got {got}")
            },
            Self::EncryptionFailed(msg) => write!(f, "encryption failed: {msg}"),
            Self::DecryptionFailed(msg) => write!(f, "decryption failed: {msg}"),
            Self::HashFailed(msg) => write!(f, "hash operation failed: {msg}"),
            Self::InvalidKey => write!(f, "invalid key (empty or malformed)"),
            Self::Expired => write!(f, "entry has expired"),
        }
    }
}

impl std::error::Error for VaultError {}

// ---------------------------------------------------------------------------
// VaultStats
// ---------------------------------------------------------------------------

/// Aggregate statistics about the vault contents (no values exposed).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VaultStats {
    /// Total number of entries.
    pub total_entries: usize,
    /// Entries per tier: [ephemeral, personal, sensitive, critical].
    pub tier_counts: [u32; 4],
    /// Total access log entries retained.
    pub access_log_size: usize,
    /// Number of expired entries pending purge.
    pub expired_count: usize,
}

// ---------------------------------------------------------------------------
// ExportManifestEntry
// ---------------------------------------------------------------------------

/// A single entry in an export manifest — key + tier + category, NEVER value.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportManifestEntry {
    /// The entry key.
    pub key: String,
    /// Data classification tier.
    pub tier: DataTier,
    /// Semantic category.
    pub category: DataCategory,
    /// Human-readable description.
    pub description: String,
    /// Whether auto-classified.
    pub auto_classified: bool,
    /// Creation timestamp (epoch ms).
    pub created_ms: u64,
}

// ---------------------------------------------------------------------------
// ExportManifest
// ---------------------------------------------------------------------------

/// Complete export manifest listing what the vault stores, for user
/// transparency. Values are NEVER included.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportManifest {
    /// When this manifest was generated (epoch ms).
    pub generated_ms: u64,
    /// All entries (keys + metadata only).
    pub entries: Vec<ExportManifestEntry>,
    /// Aggregate stats.
    pub stats: VaultStats,
}

// ---------------------------------------------------------------------------
// DataClassifier
// ---------------------------------------------------------------------------

/// Intelligent auto-classifier that detects data sensitivity from content
/// patterns. Uses structural heuristics (digit counts, character patterns)
/// for encryption-tier assignment — no confidence scoring, no routing.
/// The LLM reasons about user intent; this only asks "what tier encrypts this?"
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DataClassifier {}

impl DataClassifier {
    /// Create a new classifier with default confidence threshold.
    pub fn new() -> Self {
        Self::default()
    }

    /// Classify a string value into a (DataTier, DataCategory) pair.
    ///
    /// Uses pattern heuristics — no regex for mobile performance.
    /// Returns the most restrictive match found.
    #[must_use]
    pub fn classify(&self, value: &str) -> (DataTier, DataCategory) {
        let lower = value.to_ascii_lowercase();

        // Tier 3: CRITICAL patterns
        if self.looks_like_credit_card(value) {
            return (DataTier::Critical, DataCategory::Financial);
        }
        if self.looks_like_aadhaar(value) {
            return (DataTier::Critical, DataCategory::Identity);
        }
        if self.looks_like_password(&lower) {
            return (DataTier::Critical, DataCategory::Credential);
        }
        if self.looks_like_bank_account(&lower) {
            return (DataTier::Critical, DataCategory::Financial);
        }
        if self.looks_like_api_key(&lower) {
            return (DataTier::Critical, DataCategory::Credential);
        }
        if self.looks_like_pan(value) {
            return (DataTier::Critical, DataCategory::Identity);
        }

        // Tier 2: SENSITIVE patterns
        if self.looks_like_phone_number(value) {
            return (DataTier::Sensitive, DataCategory::Contact);
        }
        if self.looks_like_email(&lower) {
            return (DataTier::Sensitive, DataCategory::Contact);
        }
        if self.looks_like_health_data(&lower) {
            return (DataTier::Sensitive, DataCategory::Health);
        }
        if self.looks_like_address(&lower) {
            return (DataTier::Sensitive, DataCategory::Contact);
        }
        if self.looks_like_legal(&lower) {
            return (DataTier::Sensitive, DataCategory::Legal);
        }

        // Tier 1: PERSONAL — default for non-trivial data
        if value.len() > 2 {
            return (DataTier::Personal, DataCategory::Personal);
        }

        // Tier 0: EPHEMERAL — very short or empty
        (DataTier::Ephemeral, DataCategory::Personal)
    }

    // --- Tier 3 heuristics ---

    /// Credit card: 4 groups of 4 digits (with optional separators).
    fn looks_like_credit_card(&self, value: &str) -> bool {
        let digits: String = value.chars().filter(|c| c.is_ascii_digit()).collect();
        if digits.len() < 13 || digits.len() > 19 {
            return false;
        }
        // Luhn check (basic): credit cards pass the Luhn algorithm
        self.passes_luhn(&digits)
    }

    /// Basic Luhn algorithm check.
    fn passes_luhn(&self, digits: &str) -> bool {
        let mut sum: u32 = 0;
        let mut double = false;
        for ch in digits.chars().rev() {
            let Some(d) = ch.to_digit(10) else {
                return false;
            };
            let val = if double {
                let doubled = d * 2;
                if doubled > 9 {
                    doubled - 9
                } else {
                    doubled
                }
            } else {
                d
            };
            sum += val;
            double = !double;
        }
        sum.is_multiple_of(10) && sum > 0
    }

    /// Aadhaar number: exactly 12 digits (possibly with spaces).
    fn looks_like_aadhaar(&self, value: &str) -> bool {
        let digits: String = value.chars().filter(|c| c.is_ascii_digit()).collect();
        let non_digit_non_space = value
            .chars()
            .any(|c| !c.is_ascii_digit() && !c.is_whitespace() && c != '-');
        digits.len() == 12 && !non_digit_non_space
    }

    /// Indian PAN card: 5 letters + 4 digits + 1 letter (AAAAA9999A).
    fn looks_like_pan(&self, value: &str) -> bool {
        let trimmed = value.trim();
        if trimmed.len() != 10 {
            return false;
        }
        let chars: Vec<char> = trimmed.chars().collect();
        chars[..5].iter().all(|c| c.is_ascii_alphabetic())
            && chars[5..9].iter().all(|c| c.is_ascii_digit())
            && chars[9].is_ascii_alphabetic()
    }

    /// Password/PIN/OTP patterns.
    fn looks_like_password(&self, lower: &str) -> bool {
        lower.contains("password")
            || lower.contains("passwd")
            || lower.contains("pin:")
            || lower.contains("otp:")
            || lower.contains("secret:")
            || lower.contains("token:")
            || lower.starts_with("pin ")
            || lower.starts_with("otp ")
    }

    /// Bank account pattern: "account" near a sequence of digits.
    fn looks_like_bank_account(&self, lower: &str) -> bool {
        (lower.contains("account") || lower.contains("acct") || lower.contains("ifsc"))
            && lower.chars().filter(|c| c.is_ascii_digit()).count() >= 6
    }

    /// API key pattern: long alphanumeric strings with common prefixes.
    fn looks_like_api_key(&self, lower: &str) -> bool {
        lower.contains("api_key")
            || lower.contains("apikey")
            || lower.contains("sk_live")
            || lower.contains("sk_test")
            || lower.contains("bearer ")
            || lower.contains("access_token")
    }

    // --- Tier 2 heuristics ---

    /// Phone number: country code + 10+ digits.
    fn looks_like_phone_number(&self, value: &str) -> bool {
        let digits: String = value.chars().filter(|c| c.is_ascii_digit()).collect();
        let has_plus = value.contains('+');
        (has_plus && digits.len() >= 10 && digits.len() <= 15)
            || (digits.len() == 10
                && value.chars().all(|c| {
                    c.is_ascii_digit() || c.is_whitespace() || c == '-' || c == '(' || c == ')'
                }))
    }

    /// Email: contains @ with text on both sides.
    fn looks_like_email(&self, lower: &str) -> bool {
        let parts: Vec<&str> = lower.split('@').collect();
        parts.len() == 2 && !parts[0].is_empty() && parts[1].contains('.')
    }

    /// Health-related keywords.
    fn looks_like_health_data(&self, lower: &str) -> bool {
        lower.contains("prescription")
            || lower.contains("diagnosis")
            || lower.contains("blood type")
            || lower.contains("medical")
            || lower.contains("hospital")
            || lower.contains("medicine")
            || lower.contains("allergy")
            || lower.contains("doctor")
            || lower.contains("health record")
    }

    /// Address-like patterns.
    fn looks_like_address(&self, lower: &str) -> bool {
        let address_words = [
            "street",
            "road",
            "lane",
            "avenue",
            "blvd",
            "apartment",
            "flat",
            "house no",
            "pin code",
            "postal",
            "zip code",
        ];
        let matches = address_words.iter().filter(|w| lower.contains(**w)).count();
        matches >= 2
    }

    /// Legal document keywords.
    fn looks_like_legal(&self, lower: &str) -> bool {
        lower.contains("contract")
            || lower.contains("agreement")
            || lower.contains("affidavit")
            || lower.contains("legal notice")
            || lower.contains("court order")
            || lower.contains("power of attorney")
    }
}

// ---------------------------------------------------------------------------
// SecretKey — zeroize-on-drop wrapper for encryption key material
// ---------------------------------------------------------------------------

/// A 32-byte secret that is zeroed from memory on drop.
///
/// The `ZeroizeOnDrop` derive ensures the compiler cannot optimise away
/// the zeroing write, preventing the key from lingering in RAM after the
/// owning struct is dropped.
#[derive(Clone, Zeroize, ZeroizeOnDrop)]
struct SecretKey([u8; 32]);

impl SecretKey {
    /// View the raw key bytes (borrows; does NOT move out of the wrapper).
    fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

// Intentionally omit Debug to avoid accidental key logging.
impl std::fmt::Debug for SecretKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SecretKey(***)")
    }
}

// ---------------------------------------------------------------------------
// CriticalVault
// ---------------------------------------------------------------------------

/// The Critical Data Vault — on-device, tiered, encrypted data store.
///
/// Enforces the Anti-Cloud Manifesto: all data stays local. Provides
/// 4-tier classification, per-access authentication for critical data,
/// and a full audit trail for transparency.
///
/// # Capacity
///
/// Bounded to [`MAX_VAULT_ENTRIES`] entries and [`MAX_ACCESS_LOG_ENTRIES`]
/// log entries to stay within mobile memory constraints.
pub struct CriticalVault {
    /// All stored entries, keyed by their unique string key.
    entries: HashMap<String, VaultEntry>,
    /// Access audit log (bounded, oldest evicted first).
    access_log: BoundedVec<AccessLogEntry>,
    /// Quick count per tier: [ephemeral, personal, sensitive, critical].
    tier_counts: [u32; 4],
    /// Auto-classification engine.
    auto_classifier: DataClassifier,
    /// AES-256-GCM encryption key for Tier 1+ entries.
    ///
    /// `None` means the vault is unsealed — encryption is a no-op and the
    /// caller must call [`CriticalVault::set_encryption_key`] before storing
    /// sensitive data.  In production, this is derived from the Android
    /// Keystore or from the user's PIN via Argon2id.
    ///
    /// Wrapped in [`SecretKey`] so the raw bytes are guaranteed to be
    /// zeroed from memory when the vault (or the key) is dropped.
    encryption_key: Option<SecretKey>,
}

// ---------------------------------------------------------------------------
// AES-256-GCM Encryption Helpers
// ---------------------------------------------------------------------------

/// Encrypt `plaintext` with AES-256-GCM using the provided 256-bit `key`.
///
/// Returns `nonce || ciphertext` (12-byte nonce prepended).
/// A fresh random nonce is generated for every call via CSPRNG (OsRng).
fn vault_encrypt(key: &[u8; 32], plaintext: &[u8]) -> Result<Vec<u8>, VaultError> {
    let cipher = Aes256Gcm::new_from_slice(key)
        .map_err(|e| VaultError::EncryptionFailed(format!("key init: {e}")))?;

    let mut nonce_bytes = [0u8; 12];
    OsRng.fill_bytes(&mut nonce_bytes);
    let nonce = Nonce::from_slice(&nonce_bytes);

    let ciphertext = cipher
        .encrypt(nonce, plaintext)
        .map_err(|e| VaultError::EncryptionFailed(format!("encrypt: {e}")))?;

    // Prepend nonce to ciphertext: [nonce(12) || ciphertext(..)]
    let mut out = Vec::with_capacity(12 + ciphertext.len());
    out.extend_from_slice(&nonce_bytes);
    out.extend_from_slice(&ciphertext);
    Ok(out)
}

/// Decrypt `nonce || ciphertext` produced by [`vault_encrypt`].
fn vault_decrypt(key: &[u8; 32], data: &[u8]) -> Result<Vec<u8>, VaultError> {
    if data.len() < 12 {
        return Err(VaultError::DecryptionFailed(
            "ciphertext too short (missing nonce)".into(),
        ));
    }

    let (nonce_bytes, ciphertext) = data.split_at(12);
    let cipher = Aes256Gcm::new_from_slice(key)
        .map_err(|e| VaultError::DecryptionFailed(format!("key init: {e}")))?;
    let nonce = Nonce::from_slice(nonce_bytes);

    cipher
        .decrypt(nonce, ciphertext)
        .map_err(|e| VaultError::DecryptionFailed(format!("decrypt: {e}")))
}

// ---------------------------------------------------------------------------
// Argon2id PIN Hashing
// ---------------------------------------------------------------------------
//
// ## Migration note (PIN hash format change)
//
// Previous versions used a weak `wrapping_mul(31) + XOR` hash with no salt.
// This version uses Argon2id with a 16-byte CSPRNG salt (48-byte output).
//
// On upgrade, stored PIN hashes will fail `verify_pin()` because the old
// format is a different length/structure. Callers should handle this by:
//   1. Detecting a `HashFailed("stored PIN hash too short")` error.
//   2. Falling back to the legacy verification **once** for migration.
//   3. On successful legacy verify, immediately re-hash with `hash_pin()` and persist the new
//      48-byte blob.
//   4. After migration, delete any legacy hash material.
//
// This ensures a seamless one-time upgrade without forcing users to reset
// their PIN, while eliminating the weak hash from storage.
// ---------------------------------------------------------------------------

/// OWASP-recommended Argon2id parameters (2023 guidance, interactive login tier).
///
/// - m_cost:  65536 KiB (64 MiB) — memory hardness
/// - t_cost:  3 iterations        — time hardness
/// - p_cost:  4 lanes             — parallelism
/// - output:  32 bytes            — 256-bit derived key
///
/// These are compile-time constants; `Params::new` is a `const fn` that only
/// fails for out-of-range values, so the `expect` is safe for these literals.
fn argon2_params() -> Params {
    Params::new(65_536, 3, 4, Some(32)).expect("hardcoded Argon2id params are valid")
}

/// Hash a PIN using Argon2id with a random 16-byte CSPRNG salt.
///
/// Returns `salt(16) || argon2id_hash(32)` — 48 bytes total.
/// The salt is generated fresh each time via OsRng.
fn hash_pin(pin: &[u8]) -> Result<Vec<u8>, VaultError> {
    let mut salt = [0u8; 16];
    OsRng.fill_bytes(&mut salt);

    let mut hash_output = [0u8; 32];
    Argon2::new(Algorithm::Argon2id, Version::V0x13, argon2_params())
        .hash_password_into(pin, &salt, &mut hash_output)
        .map_err(|e| VaultError::HashFailed(format!("argon2id hash: {e}")))?;

    // Layout: [salt(16) || hash(32)]
    let mut out = Vec::with_capacity(48);
    out.extend_from_slice(&salt);
    out.extend_from_slice(&hash_output);
    Ok(out)
}

/// Constant-time byte comparison to prevent timing attacks.
/// XOR-accumulates differences; the final `== 0` check happens once.
fn constant_time_eq_bytes(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

/// Verify a PIN against a stored `salt || hash` blob produced by [`hash_pin`].
fn verify_pin(pin: &[u8], stored: &[u8]) -> Result<bool, VaultError> {
    if stored.len() < 48 {
        return Err(VaultError::HashFailed(
            "stored PIN hash too short (expected 48 bytes)".into(),
        ));
    }

    let (salt, expected_hash) = stored.split_at(16);

    let mut hash_output = [0u8; 32];
    Argon2::new(Algorithm::Argon2id, Version::V0x13, argon2_params())
        .hash_password_into(pin, salt, &mut hash_output)
        .map_err(|e| VaultError::HashFailed(format!("argon2id verify: {e}")))?;

    // Constant-time comparison to prevent timing attacks.
    Ok(constant_time_eq_bytes(&hash_output, &expected_hash[..32]))
}

// ---------------------------------------------------------------------------
// Legacy PIN Hash Migration (SEC-MED-7 / SEC-CRIT-004)
// ---------------------------------------------------------------------------
//
// The install.sh creates an initial PIN hash in one of two legacy formats:
//
//   1. Unsalted:  "sha256:<64 hex chars>"                  (71 bytes)
//   2. Salted:    "sha256:<32 hex salt>:<64 hex hash>"     (104 bytes) Hash = SHA-256(salt_hex ||
//      pin_plaintext)
//
// On daemon first start, we detect these legacy formats and transparently
// upgrade to Argon2id. The legacy hash is verified ONCE, then replaced.
//
// Legacy unsalted: UTF-8 string "sha256:<64 hex chars>" (71 bytes)
// Legacy salted:   UTF-8 string "sha256:<32 hex>:<64 hex>" (104 bytes)
// New format:      48 raw bytes [salt(16) || argon2id_hash(32)]
// ---------------------------------------------------------------------------

/// Check if a stored PIN blob is in the legacy unsalted `sha256:<hex>` format.
fn is_legacy_sha256_format(stored: &[u8]) -> bool {
    stored.len() == 71
        && stored.starts_with(b"sha256:")
        && stored[7..].iter().all(|b| b.is_ascii_hexdigit())
}

/// Check if a stored PIN blob is in the salted `sha256:<salt>:<hash>` format
/// written by install.sh (salt = 32 hex chars, hash = 64 hex chars → 104 bytes).
fn is_salted_sha256_format(stored: &[u8]) -> bool {
    // "sha256:" (7) + salt_hex (32) + ":" (1) + hash_hex (64) = 104
    if stored.len() != 104 || !stored.starts_with(b"sha256:") {
        return false;
    }
    // salt_hex: bytes 7..39, separator: byte 39 == ':', hash_hex: bytes 40..104
    stored[7..39].iter().all(|b| b.is_ascii_hexdigit())
        && stored[39] == b':'
        && stored[40..].iter().all(|b| b.is_ascii_hexdigit())
}

/// Verify a PIN against the legacy unsalted SHA-256 format.
///
/// SECURITY: This function exists ONLY for one-time migration. After
/// successful verification, callers MUST immediately re-hash with
/// `hash_pin()` and persist the new Argon2id blob. The legacy hash
/// is constant-time compared to prevent timing side-channels even
/// during migration.
fn verify_legacy_sha256_pin(pin: &[u8], stored: &[u8]) -> bool {
    use sha2::{Digest, Sha256};

    if !is_legacy_sha256_format(stored) {
        return false;
    }

    let expected_hex = std::str::from_utf8(&stored[7..]).unwrap_or("");
    let mut hasher = Sha256::new();
    hasher.update(pin);
    let computed = hasher.finalize();
    // Encode to hex without pulling in `hex` crate.
    let computed_hex: String = computed.iter().map(|b| format!("{:02x}", b)).collect();

    // Constant-time comparison of hex strings.
    constant_time_eq_bytes(computed_hex.as_bytes(), expected_hex.as_bytes())
}

/// Verify a PIN against the salted SHA-256 format `sha256:<salt_hex>:<hash_hex>`.
///
/// install.sh computes: `echo -n "${salt_hex}${pin}" | sha256sum`
/// So we replicate: SHA-256(salt_hex_string || pin_bytes).
///
/// SECURITY: Same one-time migration semantics as `verify_legacy_sha256_pin`.
fn verify_salted_sha256_pin(pin: &[u8], stored: &[u8]) -> bool {
    use sha2::{Digest, Sha256};

    if !is_salted_sha256_format(stored) {
        return false;
    }

    let salt_hex = match std::str::from_utf8(&stored[7..39]) {
        Ok(s) => s,
        Err(_) => return false,
    };
    let expected_hex = match std::str::from_utf8(&stored[40..104]) {
        Ok(s) => s,
        Err(_) => return false,
    };

    let mut hasher = Sha256::new();
    // install.sh does: echo -n "${salt_hex}${pin}" — salt is the hex STRING, not raw bytes
    hasher.update(salt_hex.as_bytes());
    hasher.update(pin);
    let computed = hasher.finalize();
    let computed_hex: String = computed.iter().map(|b| format!("{:02x}", b)).collect();

    constant_time_eq_bytes(computed_hex.as_bytes(), expected_hex.as_bytes())
}

/// Attempt to verify a PIN, transparently migrating legacy formats.
///
/// Returns `Ok((true, Some(new_blob)))` if legacy PIN verified and needs
/// re-persisting with the new Argon2id blob. Returns `Ok((true, None))`
/// if the PIN verified against an already-modern hash. Returns `Ok((false, None))`
/// if the PIN is incorrect.
///
/// SECURITY [SEC-MED-7]: Implements the migration plan from the comment block
/// at line 775. Legacy verification happens AT MOST ONCE per stored hash — the
/// caller must persist the returned `new_blob` to complete the upgrade.
///
/// Supported formats (checked in order):
///   1. Argon2id  — 48 raw bytes `[salt(16) || hash(32)]`
///   2. Salted    — `sha256:<32-hex-salt>:<64-hex-hash>` (104 bytes, install.sh v2)
///   3. Unsalted  — `sha256:<64-hex-hash>` (71 bytes, install.sh v1)
pub fn verify_pin_with_migration(
    pin: &[u8],
    stored: &[u8],
) -> Result<(bool, Option<Vec<u8>>), VaultError> {
    // Try modern Argon2id format first (48-byte blob).
    if stored.len() == 48 {
        let valid = verify_pin(pin, stored)?;
        return Ok((valid, None));
    }

    // Try salted SHA-256 format ("sha256:<salt>:<hex>") — install.sh v2.
    if is_salted_sha256_format(stored) {
        if verify_salted_sha256_pin(pin, stored) {
            tracing::warn!(
                target: "SECURITY",
                "Migrating salted SHA-256 PIN hash to Argon2id"
            );
            let new_blob = hash_pin(pin)?;
            return Ok((true, Some(new_blob)));
        } else {
            return Ok((false, None));
        }
    }

    // Try legacy unsalted SHA-256 format ("sha256:<hex>") — install.sh v1.
    if is_legacy_sha256_format(stored) {
        if verify_legacy_sha256_pin(pin, stored) {
            // PIN correct under legacy hash — upgrade to Argon2id immediately.
            tracing::warn!(
                target: "SECURITY",
                "Migrating legacy unsalted SHA-256 PIN hash to Argon2id"
            );
            let new_blob = hash_pin(pin)?;
            return Ok((true, Some(new_blob)));
        } else {
            return Ok((false, None));
        }
    }

    // Unknown format — reject.
    Err(VaultError::HashFailed(
        format!(
            "unrecognized PIN hash format (len={}, expected 48/Argon2id, 104/salted-SHA256, or 71/unsalted-SHA256)",
            stored.len()
        ),
    ))
}

impl CriticalVault {
    /// Create a new, empty vault.
    pub fn new() -> Self {
        Self {
            entries: HashMap::new(),
            access_log: BoundedVec::new(MAX_ACCESS_LOG_ENTRIES),
            tier_counts: [0; 4],
            auto_classifier: DataClassifier::new(),
            encryption_key: None,
        }
    }

    /// Set the AES-256-GCM encryption key used for Tier 1+ entries.
    ///
    /// In production, derive this from the Android Keystore or from the
    /// user PIN via Argon2id key derivation.  Must be called before
    /// storing or retrieving encrypted data.
    pub fn set_encryption_key(&mut self, key: [u8; 32]) {
        self.encryption_key = Some(SecretKey(key));
    }

    /// Hash a user PIN for secure storage (Argon2id + CSPRNG salt).
    ///
    /// Returns a 48-byte blob: `salt(16) || hash(32)`.
    pub fn hash_user_pin(pin: &[u8]) -> Result<Vec<u8>, VaultError> {
        hash_pin(pin)
    }

    /// Verify a user PIN against a previously stored hash blob.
    pub fn verify_user_pin(pin: &[u8], stored_hash: &[u8]) -> Result<bool, VaultError> {
        verify_pin(pin, stored_hash)
    }

    /// Store a new entry or update an existing one.
    ///
    /// # Errors
    ///
    /// - [`VaultError::InvalidKey`] if `key` is empty.
    /// - [`VaultError::VaultFull`] if the vault has reached capacity and the key is new.
    pub fn store(
        &mut self,
        key: &str,
        value: &[u8],
        tier: DataTier,
        metadata: EntryMetadata,
    ) -> Result<(), VaultError> {
        if key.is_empty() {
            return Err(VaultError::InvalidKey);
        }

        let now_ms = current_timestamp_ms();

        // If updating an existing entry, adjust tier counts.
        if let Some(existing) = self.entries.get(key) {
            let old_tier_idx = existing.tier.as_u8() as usize;
            if old_tier_idx < 4 {
                self.tier_counts[old_tier_idx] = self.tier_counts[old_tier_idx].saturating_sub(1);
            }
        } else if self.entries.len() >= MAX_VAULT_ENTRIES {
            return Err(VaultError::VaultFull);
        }

        // Encrypt value for Tier 1+ entries (Tier 0 = ephemeral/RAM only).
        let encrypted_value = if tier.as_u8() >= 1 {
            match &self.encryption_key {
                Some(sk) => vault_encrypt(sk.as_bytes(), value)?,
                None => {
                    tracing::warn!(
                        target: "VAULT",
                        key = key,
                        tier = %tier,
                        "no encryption key set — storing encrypted data requires \
                         set_encryption_key() first"
                    );
                    return Err(VaultError::EncryptionFailed(
                        "vault encryption key not configured".into(),
                    ));
                },
            }
        } else {
            // Tier 0 (Ephemeral): RAM-only, no encryption needed.
            value.to_vec()
        };

        let entry = VaultEntry {
            key: key.to_string(),
            tier,
            encrypted_value,
            created_ms: now_ms,
            last_accessed_ms: now_ms,
            access_count: 0,
            metadata,
        };

        let tier_idx = tier.as_u8() as usize;
        if tier_idx < 4 {
            self.tier_counts[tier_idx] = self.tier_counts[tier_idx].saturating_add(1);
        }

        self.log_access(key, tier, AccessOperation::Write, true, "vault_store");

        tracing::info!(
            target: "VAULT",
            key = key,
            tier = %tier,
            "stored vault entry"
        );

        self.entries.insert(key.to_string(), entry);
        Ok(())
    }

    /// Retrieve an entry's value, decrypting if necessary.
    ///
    /// # Authentication
    ///
    /// - Tier 2 (Sensitive): `authenticated` must be `true`.
    /// - Tier 3 (Critical): both `authenticated` must be `true` (represents biometric + PIN
    ///   verification performed by the platform layer).
    ///
    /// # Errors
    ///
    /// - [`VaultError::EntryNotFound`] if the key doesn't exist.
    /// - [`VaultError::AuthenticationRequired`] for unauthenticated Tier 2+ access.
    /// - [`VaultError::BiometricRequired`] for unauthenticated Tier 3 access.
    /// - [`VaultError::Expired`] if the entry has passed its expiry.
    /// - [`VaultError::DecryptionFailed`] if the ciphertext is corrupted or the key is wrong.
    pub fn retrieve(
        &mut self,
        key: &str,
        caller: &str,
        authenticated: bool,
    ) -> Result<Vec<u8>, VaultError> {
        // Check existence first (without borrowing mutably).
        if !self.entries.contains_key(key) {
            return Err(VaultError::EntryNotFound);
        }

        // Read tier and expiry before mutable borrow.
        let tier = self.entries[key].tier;
        let expiry = self.entries[key].metadata.expiry_ms;

        // Enforce authentication.
        if tier.requires_biometric() && !authenticated {
            self.log_access(key, tier, AccessOperation::Read, false, caller);
            tracing::warn!(
                target: "VAULT",
                key = key,
                tier = %tier,
                caller = caller,
                "biometric-required access denied"
            );
            return Err(VaultError::BiometricRequired);
        }

        if tier.requires_access_logging() && !authenticated {
            self.log_access(key, tier, AccessOperation::Read, false, caller);
            tracing::warn!(
                target: "VAULT",
                key = key,
                tier = %tier,
                caller = caller,
                "authenticated access denied"
            );
            return Err(VaultError::AuthenticationRequired);
        }

        // Check expiry.
        if let Some(exp) = expiry {
            let now = current_timestamp_ms();
            if now > exp {
                return Err(VaultError::Expired);
            }
        }

        // Log access.
        self.log_access(key, tier, AccessOperation::Read, authenticated, caller);

        // Update access metadata.
        if let Some(entry) = self.entries.get_mut(key) {
            entry.last_accessed_ms = current_timestamp_ms();
            entry.access_count = entry.access_count.saturating_add(1);
        }

        // Decrypt and return the value.
        let raw = &self.entries[key].encrypted_value;
        let tier = self.entries[key].tier;
        if tier.as_u8() >= 1 {
            match &self.encryption_key {
                Some(sk) => vault_decrypt(sk.as_bytes(), raw),
                None => Err(VaultError::DecryptionFailed(
                    "vault encryption key not configured".into(),
                )),
            }
        } else {
            // Tier 0 (Ephemeral): stored as plaintext.
            Ok(raw.clone())
        }
    }

    /// Delete an entry from the vault.
    ///
    /// If `secure` is `true`, the memory backing the value is overwritten
    /// with zeros before deallocation (defense against memory forensics).
    pub fn delete(&mut self, key: &str, secure: bool) -> Result<(), VaultError> {
        let entry = self.entries.get_mut(key).ok_or(VaultError::EntryNotFound)?;
        let tier = entry.tier;

        if secure {
            // Overwrite encrypted_value with zeros before dropping.
            for byte in entry.encrypted_value.iter_mut() {
                *byte = 0;
            }
            tracing::debug!(
                target: "VAULT",
                key = key,
                "secure delete: memory overwritten"
            );
        }

        let tier_idx = tier.as_u8() as usize;
        if tier_idx < 4 {
            self.tier_counts[tier_idx] = self.tier_counts[tier_idx].saturating_sub(1);
        }

        self.entries.remove(key);
        self.log_access(key, tier, AccessOperation::Delete, true, "vault_delete");

        tracing::info!(
            target: "VAULT",
            key = key,
            tier = %tier,
            secure = secure,
            "deleted vault entry"
        );

        Ok(())
    }

    /// Search vault entries by query string (substring match on key and
    /// description).
    ///
    /// **CRITICAL**: Tier 3 entries are NEVER returned in search results.
    /// This prevents accidental exposure of passwords, bank details, etc.
    /// to memory search, LLM context, or proactive suggestions.
    #[must_use]
    pub fn search(&self, query: &str, max_tier: DataTier) -> Vec<&VaultEntry> {
        let lower_query = query.to_ascii_lowercase();
        self.entries
            .values()
            .filter(|e| {
                // NEVER return Tier 3 in search, regardless of max_tier.
                if e.tier == DataTier::Critical {
                    return false;
                }
                if e.tier > max_tier {
                    return false;
                }
                let key_match = e.key.to_ascii_lowercase().contains(&lower_query);
                let desc_match = e
                    .metadata
                    .description
                    .to_ascii_lowercase()
                    .contains(&lower_query);
                key_match || desc_match
            })
            .collect()
    }

    /// Auto-classify a data value into an appropriate tier.
    ///
    /// Uses the built-in [`DataClassifier`] heuristics. Callers should
    /// present the result to the user for confirmation when confidence
    /// is uncertain.
    #[must_use]
    pub fn classify_data(value: &str) -> (DataTier, DataCategory) {
        let classifier = DataClassifier::new();
        classifier.classify(value)
    }

    /// Returns `false` for Tier 2+ entries — they must NEVER be sent to
    /// an LLM for processing. Protects user privacy per Anti-Cloud Manifesto.
    #[must_use]
    pub fn is_safe_for_llm(&self, key: &str) -> bool {
        match self.entries.get(key) {
            Some(entry) => entry.tier < DataTier::Sensitive,
            None => true, // non-existent key is "safe" (nothing to leak)
        }
    }

    /// Returns `false` for Tier 3 entries — they must NEVER appear in
    /// search results. Passwords, bank details, etc. are invisible to search.
    #[must_use]
    pub fn is_safe_for_search(&self, key: &str) -> bool {
        match self.entries.get(key) {
            Some(entry) => entry.tier < DataTier::Critical,
            None => true,
        }
    }

    /// Purge all entries whose expiry timestamp has passed.
    ///
    /// Returns the number of entries removed.
    pub fn purge_expired(&mut self, now_ms: u64) -> u32 {
        let mut purged: u32 = 0;
        let expired_keys: Vec<String> = self
            .entries
            .values()
            .filter(|e| {
                if let Some(exp) = e.metadata.expiry_ms {
                    now_ms > exp
                } else {
                    false
                }
            })
            .map(|e| e.key.clone())
            .collect();

        for key in &expired_keys {
            if let Some(entry) = self.entries.remove(key) {
                let tier_idx = entry.tier.as_u8() as usize;
                if tier_idx < 4 {
                    self.tier_counts[tier_idx] = self.tier_counts[tier_idx].saturating_sub(1);
                }
                purged = purged.saturating_add(1);

                tracing::info!(
                    target: "VAULT",
                    key = key.as_str(),
                    tier = %entry.tier,
                    "purged expired entry"
                );
            }
        }

        if purged > 0 {
            tracing::info!(
                target: "VAULT",
                purged = purged,
                "expired entries purged"
            );
        }

        purged
    }

    /// Get all access log entries for a specific key.
    #[must_use]
    pub fn access_log_for_key(&self, key: &str) -> Vec<&AccessLogEntry> {
        self.access_log.iter().filter(|e| e.key == key).collect()
    }

    /// Aggregate vault statistics (no values exposed).
    #[must_use]
    pub fn vault_stats(&self) -> VaultStats {
        let now_ms = current_timestamp_ms();
        let expired_count = self
            .entries
            .values()
            .filter(|e| {
                if let Some(exp) = e.metadata.expiry_ms {
                    now_ms > exp
                } else {
                    false
                }
            })
            .count();

        VaultStats {
            total_entries: self.entries.len(),
            tier_counts: self.tier_counts,
            access_log_size: self.access_log.len(),
            expired_count,
        }
    }

    /// Generate an export manifest listing all stored keys, tiers, and
    /// categories. Values are NEVER included — this is for user
    /// transparency and data portability.
    #[must_use]
    pub fn export_manifest(&self) -> ExportManifest {
        let now_ms = current_timestamp_ms();
        let mut entries: Vec<ExportManifestEntry> = self
            .entries
            .values()
            .map(|e| ExportManifestEntry {
                key: e.key.clone(),
                tier: e.tier,
                category: e.metadata.category.clone(),
                description: e.metadata.description.clone(),
                auto_classified: e.metadata.auto_classified,
                created_ms: e.created_ms,
            })
            .collect();

        // Sort by tier (most sensitive first), then key.
        entries.sort_by(|a, b| b.tier.cmp(&a.tier).then_with(|| a.key.cmp(&b.key)));

        tracing::debug!(
            target: "VAULT",
            entries = entries.len(),
            "export manifest generated"
        );

        ExportManifest {
            generated_ms: now_ms,
            entries,
            stats: self.vault_stats(),
        }
    }

    /// Get a reference to the auto-classifier.
    #[must_use]
    pub fn classifier(&self) -> &DataClassifier {
        &self.auto_classifier
    }

    /// Total number of entries in the vault.
    #[must_use]
    pub fn entry_count(&self) -> usize {
        self.entries.len()
    }

    /// Check if a key exists in the vault.
    #[must_use]
    pub fn contains_key(&self, key: &str) -> bool {
        self.entries.contains_key(key)
    }

    // --- Private helpers ---

    /// Record an access in the audit log.
    fn log_access(
        &mut self,
        key: &str,
        tier: DataTier,
        operation: AccessOperation,
        authenticated: bool,
        caller: &str,
    ) {
        if tier.requires_access_logging() || operation == AccessOperation::Delete {
            self.access_log.push(AccessLogEntry {
                timestamp_ms: current_timestamp_ms(),
                key: key.to_string(),
                tier,
                operation,
                authenticated,
                caller: caller.to_string(),
            });
        }
    }
}

impl Default for CriticalVault {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for CriticalVault {
    fn drop(&mut self) {
        // `encryption_key` is `Option<SecretKey>` — `SecretKey` derives
        // `ZeroizeOnDrop`, so the 32-byte key material is securely zeroed
        // when this struct is dropped.  Nothing extra needed here, but the
        // explicit `Drop` impl serves as documentation and a hook for
        // future sensitive fields.
    }
}

// ---------------------------------------------------------------------------
// Timestamp helper
// ---------------------------------------------------------------------------

/// Returns the current timestamp in milliseconds since Unix epoch.
///
/// Uses `std::time::SystemTime` — on Android this maps to the monotonic
/// clock via JNI in production.
fn current_timestamp_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn make_metadata(desc: &str, category: DataCategory) -> EntryMetadata {
        EntryMetadata {
            description: desc.to_string(),
            category,
            auto_classified: false,
            expiry_ms: None,
        }
    }

    // --- DataTier tests ---

    #[test]
    fn test_tier_encryption_requirements() {
        assert!(!DataTier::Ephemeral.requires_encryption());
        assert!(DataTier::Personal.requires_encryption());
        assert!(DataTier::Sensitive.requires_encryption());
        assert!(DataTier::Critical.requires_encryption());
    }

    #[test]
    fn test_tier_biometric_requirements() {
        assert!(!DataTier::Ephemeral.requires_biometric());
        assert!(!DataTier::Personal.requires_biometric());
        assert!(!DataTier::Sensitive.requires_biometric());
        assert!(DataTier::Critical.requires_biometric());
    }

    #[test]
    fn test_tier_access_logging() {
        assert!(!DataTier::Ephemeral.requires_access_logging());
        assert!(!DataTier::Personal.requires_access_logging());
        assert!(DataTier::Sensitive.requires_access_logging());
        assert!(DataTier::Critical.requires_access_logging());
    }

    #[test]
    fn test_tier_retention() {
        assert_eq!(DataTier::Ephemeral.max_retention_days(), None);
        assert_eq!(DataTier::Personal.max_retention_days(), None);
        assert_eq!(DataTier::Sensitive.max_retention_days(), Some(365));
        assert_eq!(DataTier::Critical.max_retention_days(), Some(90));
    }

    #[test]
    fn test_tier_ordering() {
        assert!(DataTier::Ephemeral < DataTier::Personal);
        assert!(DataTier::Personal < DataTier::Sensitive);
        assert!(DataTier::Sensitive < DataTier::Critical);
    }

    #[test]
    fn test_tier_roundtrip() {
        for tier in [
            DataTier::Ephemeral,
            DataTier::Personal,
            DataTier::Sensitive,
            DataTier::Critical,
        ] {
            assert_eq!(DataTier::from_u8(tier.as_u8()), Some(tier));
        }
        assert_eq!(DataTier::from_u8(4), None);
    }

    #[test]
    fn test_tier_display() {
        assert_eq!(DataTier::Ephemeral.to_string(), "Tier0:Ephemeral");
        assert_eq!(DataTier::Critical.to_string(), "Tier3:Critical");
    }

    // --- BoundedVec tests ---

    #[test]
    fn test_bounded_vec_eviction() {
        let mut bv: BoundedVec<u32> = BoundedVec::new(3);
        bv.push(1);
        bv.push(2);
        bv.push(3);
        assert_eq!(bv.len(), 3);

        bv.push(4); // should evict 1
        assert_eq!(bv.len(), 3);
        assert_eq!(bv.to_vec(), vec![2, 3, 4]);
    }

    #[test]
    fn test_bounded_vec_retain() {
        let mut bv: BoundedVec<u32> = BoundedVec::new(10);
        bv.push(1);
        bv.push(2);
        bv.push(3);
        bv.push(4);
        bv.retain(|x| x % 2 == 0);
        assert_eq!(bv.to_vec(), vec![2, 4]);
    }

    // --- CriticalVault tests ---

    /// Create a vault pre-loaded with a test encryption key.
    fn make_test_vault() -> CriticalVault {
        let mut vault = CriticalVault::new();
        // Deterministic test key — NEVER use in production.
        vault.set_encryption_key([0xAA; 32]);
        vault
    }

    #[test]
    fn test_store_and_retrieve_personal() {
        let mut vault = make_test_vault();
        let meta = make_metadata("User's name", DataCategory::Personal);
        vault
            .store("name", b"Aditya", DataTier::Personal, meta)
            .unwrap();

        let val = vault.retrieve("name", "test", false).unwrap();
        assert_eq!(val, b"Aditya");
    }

    #[test]
    fn test_sensitive_requires_auth() {
        let mut vault = make_test_vault();
        let meta = make_metadata("Phone number", DataCategory::Contact);
        vault
            .store("phone", b"+919876543210", DataTier::Sensitive, meta)
            .unwrap();

        // Unauthenticated access should fail.
        let result = vault.retrieve("phone", "test", false);
        assert!(matches!(result, Err(VaultError::AuthenticationRequired)));

        // Authenticated access should succeed.
        let val = vault.retrieve("phone", "test", true).unwrap();
        assert_eq!(val, b"+919876543210");
    }

    #[test]
    fn test_critical_requires_biometric() {
        let mut vault = make_test_vault();
        let meta = make_metadata("Bank account", DataCategory::Financial);
        vault
            .store("bank", b"1234567890", DataTier::Critical, meta)
            .unwrap();

        let result = vault.retrieve("bank", "test", false);
        assert!(matches!(result, Err(VaultError::BiometricRequired)));

        let val = vault.retrieve("bank", "test", true).unwrap();
        assert_eq!(val, b"1234567890");
    }

    #[test]
    fn test_search_never_returns_critical() {
        let mut vault = make_test_vault();

        let meta_p = make_metadata("Username", DataCategory::Personal);
        vault
            .store("username", b"aditya", DataTier::Personal, meta_p)
            .unwrap();

        let meta_c = make_metadata("Password", DataCategory::Credential);
        vault
            .store("password", b"s3cret", DataTier::Critical, meta_c)
            .unwrap();

        // Search with max tier = Critical should still not return Tier 3.
        let results = vault.search("", DataTier::Critical);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].key, "username");
    }

    #[test]
    fn test_search_respects_max_tier() {
        let mut vault = make_test_vault();

        let meta_p = make_metadata("Name", DataCategory::Personal);
        vault
            .store("name", b"Aditya", DataTier::Personal, meta_p)
            .unwrap();

        let meta_s = make_metadata("Email", DataCategory::Contact);
        vault
            .store("email", b"a@b.com", DataTier::Sensitive, meta_s)
            .unwrap();

        let results = vault.search("", DataTier::Personal);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].key, "name");
    }

    #[test]
    fn test_is_safe_for_llm() {
        let mut vault = make_test_vault();

        let meta_p = make_metadata("Name", DataCategory::Personal);
        vault
            .store("name", b"Aditya", DataTier::Personal, meta_p)
            .unwrap();

        let meta_s = make_metadata("Email", DataCategory::Contact);
        vault
            .store("email", b"a@b.com", DataTier::Sensitive, meta_s)
            .unwrap();

        assert!(vault.is_safe_for_llm("name"));
        assert!(!vault.is_safe_for_llm("email"));
        assert!(vault.is_safe_for_llm("nonexistent")); // safe by default
    }

    #[test]
    fn test_is_safe_for_search() {
        let mut vault = make_test_vault();

        let meta_s = make_metadata("Email", DataCategory::Contact);
        vault
            .store("email", b"a@b.com", DataTier::Sensitive, meta_s)
            .unwrap();

        let meta_c = make_metadata("Password", DataCategory::Credential);
        vault
            .store("pass", b"secret", DataTier::Critical, meta_c)
            .unwrap();

        assert!(vault.is_safe_for_search("email"));
        assert!(!vault.is_safe_for_search("pass"));
    }

    #[test]
    fn test_delete_entry() {
        let mut vault = make_test_vault();
        let meta = make_metadata("Temp data", DataCategory::Personal);
        vault
            .store("temp", b"data", DataTier::Personal, meta)
            .unwrap();

        assert!(vault.contains_key("temp"));
        vault.delete("temp", false).unwrap();
        assert!(!vault.contains_key("temp"));
    }

    #[test]
    fn test_secure_delete() {
        let mut vault = make_test_vault();
        let meta = make_metadata("Secret", DataCategory::Credential);
        vault
            .store("secret", b"important", DataTier::Critical, meta)
            .unwrap();

        vault.delete("secret", true).unwrap();
        assert!(!vault.contains_key("secret"));
    }

    #[test]
    fn test_delete_nonexistent() {
        let mut vault = CriticalVault::new();
        let result = vault.delete("nope", false);
        assert!(matches!(result, Err(VaultError::EntryNotFound)));
    }

    #[test]
    fn test_purge_expired() {
        let mut vault = make_test_vault();

        let mut meta = make_metadata("Expiring", DataCategory::Personal);
        meta.expiry_ms = Some(1000); // expired long ago
        vault
            .store("old", b"data", DataTier::Personal, meta)
            .unwrap();

        let meta_fresh = make_metadata("Fresh", DataCategory::Personal);
        vault
            .store("new", b"data", DataTier::Personal, meta_fresh)
            .unwrap();

        let purged = vault.purge_expired(current_timestamp_ms());
        assert_eq!(purged, 1);
        assert!(!vault.contains_key("old"));
        assert!(vault.contains_key("new"));
    }

    #[test]
    fn test_vault_full() {
        let mut vault = make_test_vault();
        for i in 0..MAX_VAULT_ENTRIES {
            let meta = make_metadata("entry", DataCategory::Personal);
            vault
                .store(&format!("key_{i}"), b"v", DataTier::Personal, meta)
                .unwrap();
        }

        let meta = make_metadata("overflow", DataCategory::Personal);
        let result = vault.store("overflow", b"v", DataTier::Personal, meta);
        assert!(matches!(result, Err(VaultError::VaultFull)));
    }

    #[test]
    fn test_invalid_key() {
        let mut vault = make_test_vault();
        let meta = make_metadata("empty key", DataCategory::Personal);
        let result = vault.store("", b"v", DataTier::Personal, meta);
        assert!(matches!(result, Err(VaultError::InvalidKey)));
    }

    #[test]
    fn test_vault_stats() {
        let mut vault = make_test_vault();

        let meta_p = make_metadata("A", DataCategory::Personal);
        vault.store("a", b"1", DataTier::Personal, meta_p).unwrap();

        let meta_s = make_metadata("B", DataCategory::Contact);
        vault.store("b", b"2", DataTier::Sensitive, meta_s).unwrap();

        let stats = vault.vault_stats();
        assert_eq!(stats.total_entries, 2);
        assert_eq!(stats.tier_counts[1], 1); // Personal
        assert_eq!(stats.tier_counts[2], 1); // Sensitive
    }

    #[test]
    fn test_export_manifest_no_values() {
        let mut vault = make_test_vault();
        let meta = make_metadata("Secret stuff", DataCategory::Credential);
        vault
            .store("secret", b"TOP_SECRET_VALUE", DataTier::Critical, meta)
            .unwrap();

        let manifest = vault.export_manifest();
        assert_eq!(manifest.entries.len(), 1);
        assert_eq!(manifest.entries[0].key, "secret");
        assert_eq!(manifest.entries[0].description, "Secret stuff");
        // Value must NOT appear anywhere in the manifest.
        let serialized = serde_json::to_string(&manifest).unwrap_or_default();
        assert!(!serialized.contains("TOP_SECRET_VALUE"));
    }

    #[test]
    fn test_access_log_for_key() {
        let mut vault = make_test_vault();
        let meta = make_metadata("Email", DataCategory::Contact);
        vault
            .store("email", b"a@b.com", DataTier::Sensitive, meta)
            .unwrap();

        // Authenticated retrieval creates a log entry.
        let _ = vault.retrieve("email", "test_caller", true);

        let logs = vault.access_log_for_key("email");
        assert!(!logs.is_empty());
        assert_eq!(
            logs.last().map(|l| &l.caller),
            Some(&"test_caller".to_string())
        );
    }

    #[test]
    fn test_update_existing_entry() {
        let mut vault = make_test_vault();

        let meta1 = make_metadata("Old name", DataCategory::Personal);
        vault
            .store("name", b"Old", DataTier::Personal, meta1)
            .unwrap();

        let meta2 = make_metadata("New name", DataCategory::Personal);
        vault
            .store("name", b"New", DataTier::Personal, meta2)
            .unwrap();

        let val = vault.retrieve("name", "test", false).unwrap();
        assert_eq!(val, b"New");
        assert_eq!(vault.entry_count(), 1);
    }

    // --- DataClassifier tests ---

    #[test]
    fn test_classify_credit_card() {
        let (tier, cat) = CriticalVault::classify_data("4532 0151 2345 6789");
        assert_eq!(tier, DataTier::Critical);
        assert_eq!(cat, DataCategory::Financial);
    }

    #[test]
    fn test_classify_aadhaar() {
        let (tier, cat) = CriticalVault::classify_data("1234 5678 9012");
        // 12 digits = Aadhaar pattern
        assert_eq!(tier, DataTier::Critical);
        assert_eq!(cat, DataCategory::Identity);
    }

    #[test]
    fn test_classify_password() {
        let (tier, cat) = CriticalVault::classify_data("password: hunter2");
        assert_eq!(tier, DataTier::Critical);
        assert_eq!(cat, DataCategory::Credential);
    }

    #[test]
    fn test_classify_phone_number() {
        let (tier, cat) = CriticalVault::classify_data("+91 9876543210");
        assert_eq!(tier, DataTier::Sensitive);
        assert_eq!(cat, DataCategory::Contact);
    }

    #[test]
    fn test_classify_email() {
        let (tier, cat) = CriticalVault::classify_data("user@example.com");
        assert_eq!(tier, DataTier::Sensitive);
        assert_eq!(cat, DataCategory::Contact);
    }

    #[test]
    fn test_classify_plain_text() {
        let (tier, cat) = CriticalVault::classify_data("I like coffee in the morning");
        assert_eq!(tier, DataTier::Personal);
        assert_eq!(cat, DataCategory::Personal);
    }

    #[test]
    fn test_classify_empty() {
        let (tier, _) = CriticalVault::classify_data("");
        assert_eq!(tier, DataTier::Ephemeral);
    }

    #[test]
    fn test_classify_pan() {
        let (tier, cat) = CriticalVault::classify_data("ABCDE1234F");
        assert_eq!(tier, DataTier::Critical);
        assert_eq!(cat, DataCategory::Identity);
    }

    #[test]
    fn test_classify_api_key() {
        let (tier, cat) = CriticalVault::classify_data("api_key: sk_live_abc123xyz");
        assert_eq!(tier, DataTier::Critical);
        assert_eq!(cat, DataCategory::Credential);
    }

    #[test]
    fn test_classify_health() {
        let (tier, cat) = CriticalVault::classify_data("prescription: Metformin 500mg");
        assert_eq!(tier, DataTier::Sensitive);
        assert_eq!(cat, DataCategory::Health);
    }

    #[test]
    fn test_classify_bank_account() {
        let (tier, cat) = CriticalVault::classify_data("account number: 12345678901234");
        assert_eq!(tier, DataTier::Critical);
        assert_eq!(cat, DataCategory::Financial);
    }

    #[test]
    fn test_vault_error_display() {
        assert!(VaultError::BiometricRequired
            .to_string()
            .contains("biometric"));
        assert!(VaultError::VaultFull.to_string().contains("full"));
        assert!(VaultError::InvalidKey.to_string().contains("invalid"));
    }

    #[test]
    fn test_data_category_display() {
        assert_eq!(DataCategory::Financial.to_string(), "Financial");
        assert_eq!(
            DataCategory::Other("custom".to_string()).to_string(),
            "Other(custom)"
        );
    }

    #[test]
    fn test_access_operation_display() {
        assert_eq!(AccessOperation::Read.to_string(), "Read");
        assert_eq!(AccessOperation::Export.to_string(), "Export");
    }

    // --- TEST-HIGH-3: Property-based crypto tests ---

    #[test]
    fn test_vault_roundtrip() {
        // Encrypt-then-decrypt must return the original plaintext.
        // This is the fundamental correctness property of the vault crypto layer.
        let key = [0xBB_u8; 32];
        let test_cases: Vec<&[u8]> = vec![
            b"hello world",
            b"",
            b"\x00\x01\x02\x03",
            &[0xFF; 1024], // 1KB payload
            b"sensitive data: password123!@#",
        ];

        for plaintext in &test_cases {
            let encrypted = vault_encrypt(&key, plaintext).expect("encryption should not fail");

            // Ciphertext must differ from plaintext (unless empty — AES-GCM
            // still produces a 16-byte auth tag for empty input).
            if !plaintext.is_empty() {
                assert_ne!(
                    &encrypted[12..], // skip nonce prefix
                    *plaintext,
                    "ciphertext must not equal plaintext"
                );
            }

            // Ciphertext must be nonce(12) + ciphertext(len + 16 tag).
            assert!(
                encrypted.len() >= 12 + 16,
                "encrypted output too short: {} bytes",
                encrypted.len()
            );

            let decrypted = vault_decrypt(&key, &encrypted).expect("decryption should not fail");
            assert_eq!(
                decrypted.as_slice(),
                *plaintext,
                "roundtrip failed for plaintext of length {}",
                plaintext.len()
            );
        }

        // Wrong key must fail decryption (integrity check).
        let wrong_key = [0xCC_u8; 32];
        let encrypted = vault_encrypt(&key, b"secret").unwrap();
        let result = vault_decrypt(&wrong_key, &encrypted);
        assert!(
            matches!(result, Err(VaultError::DecryptionFailed(_))),
            "decryption with wrong key must fail"
        );
    }

    #[test]
    fn test_constant_time_eq_works() {
        // Equal slices must compare as equal.
        assert!(constant_time_eq_bytes(b"hello", b"hello"));
        assert!(constant_time_eq_bytes(b"", b""));
        assert!(constant_time_eq_bytes(&[0u8; 32], &[0u8; 32]));

        // Different slices must compare as unequal.
        assert!(!constant_time_eq_bytes(b"hello", b"world"));
        assert!(!constant_time_eq_bytes(b"hello", b"hellp"));

        // Single-bit difference must be detected.
        let a = [0b10101010u8; 16];
        let mut b = a;
        b[15] ^= 0x01; // flip lowest bit of last byte
        assert!(!constant_time_eq_bytes(&a, &b));

        // Different lengths must compare as unequal.
        assert!(!constant_time_eq_bytes(b"short", b"longer"));
        assert!(!constant_time_eq_bytes(b"abc", b"ab"));
    }
}
