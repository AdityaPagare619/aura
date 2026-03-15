//! 5-layer security system for the Telegram bot interface.
//!
//! Layers:
//! 1. **Chat ID whitelist** — only pre-approved Telegram chat IDs accepted.
//! 2. **Argon2id PIN** — optional PIN lock requiring constant-time verification.
//! 3. **Per-command permissions** — role-based access control per chat ID.
//! 4. **Rate limiting** — sliding-window limits per chat ID.
//! 5. **Audit trail** — every command attempt logged (delegates to [`super::audit`]).

use std::collections::HashMap;
use std::time::Instant;

use serde::{Deserialize, Serialize};
use tracing::{instrument, warn};

// ─── Error types ────────────────────────────────────────────────────────────

/// Security check failures.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SecurityError {
    /// Chat ID not in the whitelist.
    UnauthorizedChatId(i64),
    /// The bot is locked and the command is not `/unlock`.
    BotLocked,
    /// The user's permission level is insufficient for this command.
    InsufficientPermission {
        required: PermissionLevel,
        actual: PermissionLevel,
    },
    /// Rate limit exceeded.
    RateLimited { retry_after_secs: u32 },
    /// PIN verification failed.
    InvalidPin,
    /// PIN not set but lock was requested.
    PinNotConfigured,
}

impl std::fmt::Display for SecurityError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnauthorizedChatId(id) => write!(f, "unauthorized chat ID: {id}"),
            Self::BotLocked => write!(f, "bot is locked — use /unlock <pin>"),
            Self::InsufficientPermission { required, actual } => {
                write!(
                    f,
                    "insufficient permission: need {required:?}, have {actual:?}"
                )
            }
            Self::RateLimited { retry_after_secs } => {
                write!(f, "rate limited — retry after {retry_after_secs}s")
            }
            Self::InvalidPin => write!(f, "invalid PIN"),
            Self::PinNotConfigured => write!(f, "no PIN configured — use /pin set first"),
        }
    }
}

impl std::error::Error for SecurityError {}

// ─── Permission levels ──────────────────────────────────────────────────────

/// Hierarchical permission levels (lower ordinal = fewer rights).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum PermissionLevel {
    /// Can only read status / help.
    ReadOnly = 0,
    /// Can issue queries (ask, recall).
    Query = 1,
    /// Can trigger actions (do, send, call).
    Action = 2,
    /// Can modify state (forget, personality set).
    Modify = 3,
    /// Full control (restart, PIN, lock).
    Admin = 4,
}

impl PermissionLevel {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::ReadOnly => "read-only",
            Self::Query => "query",
            Self::Action => "action",
            Self::Modify => "modify",
            Self::Admin => "admin",
        }
    }
}

// ─── Rate limiter ───────────────────────────────────────────────────────────

/// Hard cap on the number of unique chat IDs tracked by the rate limiter.
///
/// Prevents unbounded HashMap growth if the bot is exposed to many chat IDs
/// (e.g., misconfigured whitelist or an enumeration attack before Layer 1
/// rejects the chat ID). 1024 is well above any realistic single-user
/// deployment and provides a clear memory ceiling.
const MAX_TRACKED_CHATS: usize = 1024;

/// Sliding-window rate limiter per chat ID.
pub struct RateLimiter {
    /// chat_id -> recent request timestamps.
    /// Bounded to MAX_TRACKED_CHATS entries — enforced in `check_and_record()`.
    windows: HashMap<i64, Vec<Instant>>,
    /// Max requests per 60-second window.
    pub max_per_minute: u32,
    /// Max requests per 3600-second window.
    pub max_per_hour: u32,
}

impl RateLimiter {
    pub fn new(max_per_minute: u32, max_per_hour: u32) -> Self {
        Self {
            windows: HashMap::new(),
            max_per_minute,
            max_per_hour,
        }
    }

    /// Record a request and check if the caller is within limits.
    pub fn check_and_record(&mut self, chat_id: i64) -> Result<(), SecurityError> {
        let now = Instant::now();

        // Enforce bounded capacity: evict the entry with the fewest recent
        // timestamps (least active) if the map is at its limit and this is
        // a new chat ID.
        if !self.windows.contains_key(&chat_id) && self.windows.len() >= MAX_TRACKED_CHATS {
            let evict = self
                .windows
                .iter()
                .min_by_key(|(_, ts)| ts.len())
                .map(|(&id, _)| id);
            if let Some(id) = evict {
                warn!(evicted_chat_id = id, "rate-limiter map full — evicting least-active entry");
                self.windows.remove(&id);
            }
        }

        let timestamps = self.windows.entry(chat_id).or_default();

        // Prune entries older than 1 hour.
        timestamps.retain(|t| now.duration_since(*t).as_secs() < 3600);

        // Check per-hour limit.
        if timestamps.len() as u32 >= self.max_per_hour {
            let oldest = timestamps.first().copied().unwrap_or(now);
            let retry_after = 3600u32.saturating_sub(now.duration_since(oldest).as_secs() as u32);
            return Err(SecurityError::RateLimited {
                retry_after_secs: retry_after,
            });
        }

        // Check per-minute limit.
        let recent_count = timestamps
            .iter()
            .filter(|t| now.duration_since(**t).as_secs() < 60)
            .count() as u32;
        if recent_count >= self.max_per_minute {
            let oldest_in_minute = timestamps
                .iter()
                .filter(|t| now.duration_since(**t).as_secs() < 60)
                .next()
                .copied()
                .unwrap_or(now);
            let retry_after =
                60u32.saturating_sub(now.duration_since(oldest_in_minute).as_secs() as u32);
            return Err(SecurityError::RateLimited {
                retry_after_secs: retry_after,
            });
        }

        timestamps.push(now);
        Ok(())
    }

    /// Evict all state for a given chat ID.
    pub fn reset(&mut self, chat_id: i64) {
        self.windows.remove(&chat_id);
    }
}

// ─── PIN management ─────────────────────────────────────────────────────────

/// Argon2id PIN hash storage.
///
/// Stores a 32-byte Argon2id hash and 16-byte CSPRNG salt.
/// Parameters: time_cost=3, mem_cost=65536 (64 MB), parallelism=1.
#[derive(Debug, Clone)]
pub struct PinStore {
    hash: Option<[u8; 32]>,
    salt: [u8; 16],
}

impl PinStore {
    pub fn new() -> Self {
        Self {
            hash: None,
            salt: Self::generate_salt(),
        }
    }

    /// Restore from persisted values.
    pub fn from_parts(hash: [u8; 32], salt: [u8; 16]) -> Self {
        Self {
            hash: Some(hash),
            salt,
        }
    }

    pub fn is_set(&self) -> bool {
        self.hash.is_some()
    }

    pub fn salt(&self) -> &[u8; 16] {
        &self.salt
    }

    pub fn hash(&self) -> Option<&[u8; 32]> {
        self.hash.as_ref()
    }

    /// Set a new PIN (hashes it with Argon2id parameters).
    ///
    /// Parameters: time_cost=3, mem_cost=65536 (64 KiB), parallelism=1.
    pub fn set_pin(&mut self, pin: &str) {
        self.salt = Self::generate_salt();
        self.hash = Some(Self::hash_pin(pin, &self.salt));
    }

    /// Verify a PIN in constant time.
    pub fn verify(&self, pin: &str) -> bool {
        match &self.hash {
            Some(stored) => {
                let candidate = Self::hash_pin(pin, &self.salt);
                constant_time_eq(stored, &candidate)
            }
            None => false,
        }
    }

    /// Clear the PIN (unlock permanently until re-set).
    pub fn clear(&mut self) {
        self.hash = None;
    }

    // -- Internal helpers --

    fn generate_salt() -> [u8; 16] {
        use rand::Rng;
        let mut rng = rand::thread_rng();
        let mut salt = [0u8; 16];
        rng.fill(&mut salt);
        salt
    }

    /// Hash a PIN using Argon2id (memory-hard KDF resistant to GPU/ASIC attacks).
    ///
    /// Parameters: time_cost=3, mem_cost=65536 (64 MB), parallelism=1.
    fn hash_pin(pin: &str, salt: &[u8; 16]) -> [u8; 32] {
        use argon2::{Algorithm, Argon2, Params, Version};
        let params = Params::new(
            65536, // 64 MB memory cost
            3,     // 3 iterations
            1,     // 1 thread (mobile-friendly)
            Some(32),
        )
        .expect("valid argon2 params");
        let mut hash = [0u8; 32];
        Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
            .hash_password_into(pin.as_bytes(), salt, &mut hash)
            .expect("argon2 hash_password_into");
        hash
    }
}

/// Constant-time byte comparison to prevent timing attacks on PIN.
fn constant_time_eq(a: &[u8; 32], b: &[u8; 32]) -> bool {
    let mut diff = 0u8;
    for i in 0..32 {
        diff |= a[i] ^ b[i];
    }
    diff == 0
}

// ─── SecurityGate ───────────────────────────────────────────────────────────

/// The 5-layer security gate that every incoming Telegram command passes through.
pub struct SecurityGate {
    /// Layer 1: allowed chat IDs.
    allowed_chat_ids: Vec<i64>,
    /// Layer 2: PIN store.
    pin_store: PinStore,
    /// Whether the bot is currently locked (requires PIN to unlock).
    locked: bool,
    /// Layer 3: per-user permission levels.
    permissions: HashMap<i64, PermissionLevel>,
    /// Layer 4: rate limiter.
    rate_limiter: RateLimiter,
}

impl SecurityGate {
    /// Create a new security gate with the given allowed chat IDs.
    ///
    /// The first chat ID is automatically granted Admin permission;
    /// others get Query by default.
    pub fn new(allowed_chat_ids: Vec<i64>) -> Self {
        let mut permissions = HashMap::new();
        if let Some(&primary) = allowed_chat_ids.first() {
            permissions.insert(primary, PermissionLevel::Admin);
        }
        for &cid in allowed_chat_ids.iter().skip(1) {
            permissions.insert(cid, PermissionLevel::Query);
        }

        Self {
            allowed_chat_ids,
            pin_store: PinStore::new(),
            locked: false,
            permissions,
            rate_limiter: RateLimiter::new(30, 300),
        }
    }

    /// Run all 5 security layers for an incoming command.
    ///
    /// The `required_permission` is determined by the command enum.
    /// Layer 5 (audit) is handled by the caller after this returns.
    #[instrument(skip(self), fields(chat_id, perm = ?required_permission))]
    pub fn check(
        &mut self,
        chat_id: i64,
        required_permission: PermissionLevel,
        is_unlock_command: bool,
    ) -> Result<(), SecurityError> {
        // Layer 1: Chat ID whitelist.
        if !self.allowed_chat_ids.contains(&chat_id) {
            warn!(chat_id, "rejected: unauthorized chat ID");
            return Err(SecurityError::UnauthorizedChatId(chat_id));
        }

        // Layer 2: Lock check (skip for /unlock).
        if self.locked && !is_unlock_command {
            return Err(SecurityError::BotLocked);
        }

        // Layer 3: Permission level.
        let user_level = self
            .permissions
            .get(&chat_id)
            .copied()
            .unwrap_or(PermissionLevel::ReadOnly);
        if user_level < required_permission {
            return Err(SecurityError::InsufficientPermission {
                required: required_permission,
                actual: user_level,
            });
        }

        // Layer 4: Rate limiting.
        self.rate_limiter.check_and_record(chat_id)?;

        Ok(())
    }

    // -- PIN operations --

    /// Lock the bot (requires PIN to be set).
    pub fn lock(&mut self) -> Result<(), SecurityError> {
        if !self.pin_store.is_set() {
            return Err(SecurityError::PinNotConfigured);
        }
        self.locked = true;
        Ok(())
    }

    /// Attempt to unlock with a PIN.
    pub fn unlock(&mut self, pin: &str) -> Result<(), SecurityError> {
        if !self.pin_store.is_set() {
            return Err(SecurityError::PinNotConfigured);
        }
        if self.pin_store.verify(pin) {
            self.locked = false;
            Ok(())
        } else {
            Err(SecurityError::InvalidPin)
        }
    }

    /// Set or change the PIN.
    pub fn set_pin(&mut self, pin: &str) {
        self.pin_store.set_pin(pin);
    }

    /// Clear the PIN (also unlocks).
    pub fn clear_pin(&mut self) {
        self.pin_store.clear();
        self.locked = false;
    }

    pub fn is_locked(&self) -> bool {
        self.locked
    }

    pub fn is_pin_set(&self) -> bool {
        self.pin_store.is_set()
    }

    // -- Permission management --

    /// Set a user's permission level.
    pub fn set_permission(&mut self, chat_id: i64, level: PermissionLevel) {
        self.permissions.insert(chat_id, level);
    }

    /// Get a user's permission level.
    pub fn get_permission(&self, chat_id: i64) -> PermissionLevel {
        self.permissions
            .get(&chat_id)
            .copied()
            .unwrap_or(PermissionLevel::ReadOnly)
    }

    /// Check if a chat ID is whitelisted.
    pub fn is_allowed(&self, chat_id: i64) -> bool {
        self.allowed_chat_ids.contains(&chat_id)
    }

    /// Add a chat ID to the whitelist.
    pub fn add_allowed_chat_id(&mut self, chat_id: i64) {
        if !self.allowed_chat_ids.contains(&chat_id) {
            self.allowed_chat_ids.push(chat_id);
        }
    }

    /// Return the list of allowed chat IDs (for status display).
    pub fn allowed_chat_ids(&self) -> &[i64] {
        &self.allowed_chat_ids
    }

    /// Return all permission entries (for audit display).
    pub fn all_permissions(&self) -> &HashMap<i64, PermissionLevel> {
        &self.permissions
    }

    /// Access the PIN store (for persistence).
    pub fn pin_store(&self) -> &PinStore {
        &self.pin_store
    }

    /// Restore PIN store from persisted state.
    pub fn restore_pin_store(&mut self, store: PinStore) {
        self.pin_store = store;
    }
}

impl std::fmt::Debug for SecurityGate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SecurityGate")
            .field("allowed_chat_ids", &self.allowed_chat_ids.len())
            .field("locked", &self.locked)
            .field("pin_set", &self.pin_store.is_set())
            .field("permissions", &self.permissions.len())
            .finish()
    }
}

// ─── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_unauthorized_chat_id_rejected() {
        let mut gate = SecurityGate::new(vec![100]);
        let result = gate.check(999, PermissionLevel::ReadOnly, false);
        assert!(matches!(
            result,
            Err(SecurityError::UnauthorizedChatId(999))
        ));
    }

    #[test]
    fn test_authorized_chat_id_passes() {
        let mut gate = SecurityGate::new(vec![100]);
        assert!(gate.check(100, PermissionLevel::ReadOnly, false).is_ok());
    }

    #[test]
    fn test_primary_user_gets_admin() {
        let gate = SecurityGate::new(vec![100, 200]);
        assert_eq!(gate.get_permission(100), PermissionLevel::Admin);
        assert_eq!(gate.get_permission(200), PermissionLevel::Query);
    }

    #[test]
    fn test_lock_unlock_flow() {
        let mut gate = SecurityGate::new(vec![100]);
        // Can't lock without PIN.
        assert!(matches!(gate.lock(), Err(SecurityError::PinNotConfigured)));

        gate.set_pin("1234");
        gate.lock().unwrap();
        assert!(gate.is_locked());

        // Non-unlock commands rejected.
        assert!(matches!(
            gate.check(100, PermissionLevel::ReadOnly, false),
            Err(SecurityError::BotLocked)
        ));

        // Unlock command allowed through check.
        assert!(gate.check(100, PermissionLevel::Admin, true).is_ok());

        // Wrong PIN.
        assert!(matches!(
            gate.unlock("0000"),
            Err(SecurityError::InvalidPin)
        ));

        // Correct PIN.
        gate.unlock("1234").unwrap();
        assert!(!gate.is_locked());
    }

    #[test]
    fn test_permission_enforcement() {
        let mut gate = SecurityGate::new(vec![100, 200]);
        // 200 has Query permission.
        assert!(gate.check(200, PermissionLevel::Query, false).is_ok());
        assert!(matches!(
            gate.check(200, PermissionLevel::Admin, false),
            Err(SecurityError::InsufficientPermission { .. })
        ));

        // Elevate 200 to Admin.
        gate.set_permission(200, PermissionLevel::Admin);
        assert!(gate.check(200, PermissionLevel::Admin, false).is_ok());
    }

    #[test]
    fn test_rate_limiter_per_minute() {
        let mut limiter = RateLimiter::new(3, 100);
        let chat_id = 42;
        assert!(limiter.check_and_record(chat_id).is_ok());
        assert!(limiter.check_and_record(chat_id).is_ok());
        assert!(limiter.check_and_record(chat_id).is_ok());
        // 4th request should be rate-limited.
        assert!(matches!(
            limiter.check_and_record(chat_id),
            Err(SecurityError::RateLimited { .. })
        ));
    }

    #[test]
    fn test_pin_verify_constant_time() {
        let mut store = PinStore::new();
        store.set_pin("secret123");
        assert!(store.verify("secret123"));
        assert!(!store.verify("wrong"));
        assert!(!store.verify(""));
    }

    #[test]
    fn test_pin_clear() {
        let mut gate = SecurityGate::new(vec![100]);
        gate.set_pin("1234");
        gate.lock().unwrap();
        assert!(gate.is_locked());

        gate.clear_pin();
        assert!(!gate.is_locked());
        assert!(!gate.is_pin_set());
    }

    #[test]
    fn test_constant_time_eq() {
        let a = [0u8; 32];
        let b = [0u8; 32];
        assert!(constant_time_eq(&a, &b));

        let mut c = [0u8; 32];
        c[31] = 1;
        assert!(!constant_time_eq(&a, &c));
    }
}
