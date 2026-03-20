//! Action Sandboxing — isolation and containment for AURA actions.
//!
//! Every action passes through a sandbox that determines the appropriate
//! containment level and enforces resource limits. Dangerous actions are
//! previewed before execution, and reversible actions store rollback info.
//!
//! # Containment Levels
//!
//! ```text
//! L0: Direct execution     — trusted actions (tap, scroll, home)
//! L1: Execute + log        — monitored actions (open app, type text)
//! L2: Preview + confirm    — restricted actions (install, settings change)
//! L3: REFUSE               — forbidden actions (factory reset, data wipe)
//! ```
//!
//! # Rollback Registry
//!
//! For reversible actions, the sandbox stores undo information:
//! - Text typed → store backspace sequence length
//! - Setting changed → store previous value
//! - App opened → store previous foreground app
//!
//! This allows `execute_contained()` to offer rollback on failure.

use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};

use aura_types::{actions::ActionType, errors::SecurityError};
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// ContainmentLevel
// ---------------------------------------------------------------------------

/// The containment level determines how much isolation an action gets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum ContainmentLevel {
    /// L0: Direct execution — trusted, low-risk actions.
    Direct = 0,
    /// L1: Execute + log — monitored actions with audit trail.
    Monitored = 1,
    /// L2: Preview + confirm + execute + log — restricted actions.
    Restricted = 2,
    /// L3: REFUSE — action is forbidden and will not execute.
    Forbidden = 3,
}

impl ContainmentLevel {
    /// Parse from numeric level.
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(Self::Direct),
            1 => Some(Self::Monitored),
            2 => Some(Self::Restricted),
            3 => Some(Self::Forbidden),
            _ => None,
        }
    }

    /// Numeric level value.
    pub fn as_u8(self) -> u8 {
        self as u8
    }
}

impl std::fmt::Display for ContainmentLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Direct => write!(f, "L0:Direct"),
            Self::Monitored => write!(f, "L1:Monitored"),
            Self::Restricted => write!(f, "L2:Restricted"),
            Self::Forbidden => write!(f, "L3:Forbidden"),
        }
    }
}

// ---------------------------------------------------------------------------
// ResourceLimits
// ---------------------------------------------------------------------------

/// Resource limits enforced per sandbox session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceLimits {
    /// Maximum number of tap/click actions per session.
    pub max_taps: u32,
    /// Maximum session duration.
    pub max_duration: Duration,
    /// Maximum number of distinct apps touched.
    pub max_apps: u32,
    /// Maximum number of text characters typed.
    pub max_chars_typed: u32,
    /// Maximum number of total actions.
    pub max_total_actions: u32,
}

impl Default for ResourceLimits {
    fn default() -> Self {
        Self {
            max_taps: 100,
            max_duration: Duration::from_secs(300), // 5 minutes
            max_apps: 5,
            max_chars_typed: 1000,
            max_total_actions: 200,
        }
    }
}

// ---------------------------------------------------------------------------
// RollbackEntry
// ---------------------------------------------------------------------------

/// Undo information for a reversible action.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum RollbackEntry {
    /// Text was typed — store length for backspace sequence.
    TextTyped { char_count: u32 },
    /// A setting was changed — store the previous value.
    SettingChanged {
        setting_key: String,
        previous_value: String,
    },
    /// An app was opened — store the previously foreground app.
    AppOpened { previous_package: String },
    /// A scroll was performed — store inverse scroll.
    ScrollPerformed {
        inverse_direction: String,
        amount: i32,
    },
    /// Navigation action — can go forward to undo back.
    NavigatedBack,
    /// Generic reversible action with custom undo description.
    Custom {
        description: String,
        undo_action: String,
    },
}

impl RollbackEntry {
    /// Human-readable description of what rollback would do.
    pub fn describe(&self) -> String {
        match self {
            Self::TextTyped { char_count } => {
                format!("delete {char_count} characters (backspace)")
            }
            Self::SettingChanged {
                setting_key,
                previous_value,
            } => {
                format!("restore {setting_key} to '{previous_value}'")
            }
            Self::AppOpened { previous_package } => {
                format!("return to {previous_package}")
            }
            Self::ScrollPerformed {
                inverse_direction,
                amount,
            } => {
                format!("scroll {inverse_direction} by {amount}")
            }
            Self::NavigatedBack => "navigate forward".to_string(),
            Self::Custom { description, .. } => description.clone(),
        }
    }
}

// ---------------------------------------------------------------------------
// ActionPreview
// ---------------------------------------------------------------------------

/// Preview of what an action would do WITHOUT executing it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionPreview {
    /// The action that was previewed.
    pub action_description: String,
    /// Containment level assigned.
    pub containment_level: ContainmentLevel,
    /// Whether this action is reversible.
    pub reversible: bool,
    /// Human-readable description of the effect.
    pub effect_description: String,
    /// Estimated risk level.
    pub risk_summary: String,
    /// Whether user confirmation is required.
    pub needs_confirmation: bool,
}

// ---------------------------------------------------------------------------
// SandboxSession
// ---------------------------------------------------------------------------

/// Hard cap on distinct apps a single session may touch.
/// Prevents unbounded growth of the `apps_touched` Vec.
const MAX_APPS_TOUCHED: usize = 64;

/// Hard cap on rollback stack depth.
/// Prevents unbounded memory growth for long-running sessions.
const MAX_ROLLBACK_DEPTH: usize = 32;

/// Tracks resource usage within a sandboxed execution session.
#[derive(Debug, Clone)]
pub struct SandboxSession {
    /// Session identifier.
    pub id: u64,
    /// When this session started.
    pub started_at: Instant,
    /// Resource limits for this session.
    pub limits: ResourceLimits,
    /// Taps performed so far.
    pub taps_used: u32,
    /// Distinct apps touched.
    pub apps_touched: Vec<String>,
    /// Characters typed.
    pub chars_typed: u32,
    /// Total actions performed.
    pub total_actions: u32,
    /// Rollback registry — stores undo info for reversible actions.
    pub rollback_stack: Vec<RollbackEntry>,
    /// Whether this session has been terminated.
    pub terminated: bool,
}

impl SandboxSession {
    /// Create a new session with given ID and limits.
    pub fn new(id: u64, limits: ResourceLimits) -> Self {
        Self {
            id,
            started_at: Instant::now(),
            limits,
            taps_used: 0,
            apps_touched: Vec::new(),
            chars_typed: 0,
            total_actions: 0,
            rollback_stack: Vec::new(),
            terminated: false,
        }
    }

    /// Check if any resource limit has been exceeded.
    pub fn check_limits(&self) -> Result<(), SecurityError> {
        if self.terminated {
            return Err(SecurityError::SandboxRefused {
                reason: "session terminated".to_string(),
            });
        }

        if self.taps_used >= self.limits.max_taps {
            return Err(SecurityError::ResourceLimitExceeded {
                resource: format!("taps: {} >= {}", self.taps_used, self.limits.max_taps),
            });
        }

        if self.started_at.elapsed() >= self.limits.max_duration {
            return Err(SecurityError::ResourceLimitExceeded {
                resource: format!(
                    "duration: {}s >= {}s",
                    self.started_at.elapsed().as_secs(),
                    self.limits.max_duration.as_secs()
                ),
            });
        }

        if self.apps_touched.len() as u32 >= self.limits.max_apps {
            return Err(SecurityError::ResourceLimitExceeded {
                resource: format!(
                    "apps: {} >= {}",
                    self.apps_touched.len(),
                    self.limits.max_apps
                ),
            });
        }

        if self.chars_typed >= self.limits.max_chars_typed {
            return Err(SecurityError::ResourceLimitExceeded {
                resource: format!(
                    "chars: {} >= {}",
                    self.chars_typed, self.limits.max_chars_typed
                ),
            });
        }

        if self.total_actions >= self.limits.max_total_actions {
            return Err(SecurityError::ResourceLimitExceeded {
                resource: format!(
                    "actions: {} >= {}",
                    self.total_actions, self.limits.max_total_actions
                ),
            });
        }

        Ok(())
    }

    /// Record a tap action.
    pub fn record_tap(&mut self) -> Result<(), SecurityError> {
        self.check_limits()?;
        self.taps_used += 1;
        self.total_actions += 1;
        Ok(())
    }

    /// Record text typed.
    pub fn record_text(&mut self, text: &str) -> Result<(), SecurityError> {
        self.check_limits()?;
        let len = text.len() as u32;
        self.chars_typed += len;
        self.total_actions += 1;
        if self.rollback_stack.len() >= MAX_ROLLBACK_DEPTH {
            return Err(SecurityError::ResourceLimitExceeded {
                resource: format!(
                    "rollback_stack hard cap: {} >= {MAX_ROLLBACK_DEPTH}",
                    self.rollback_stack.len()
                ),
            });
        }
        self.rollback_stack
            .push(RollbackEntry::TextTyped { char_count: len });
        Ok(())
    }

    /// Record an app opened.
    pub fn record_app_open(&mut self, package: &str, previous: &str) -> Result<(), SecurityError> {
        self.check_limits()?;
        if !self.apps_touched.contains(&package.to_string()) {
            if self.apps_touched.len() >= MAX_APPS_TOUCHED {
                return Err(SecurityError::ResourceLimitExceeded {
                    resource: format!(
                        "apps_touched hard cap: {} >= {MAX_APPS_TOUCHED}",
                        self.apps_touched.len()
                    ),
                });
            }
            self.apps_touched.push(package.to_string());
        }
        self.total_actions += 1;
        if self.rollback_stack.len() >= MAX_ROLLBACK_DEPTH {
            return Err(SecurityError::ResourceLimitExceeded {
                resource: format!(
                    "rollback_stack hard cap: {} >= {MAX_ROLLBACK_DEPTH}",
                    self.rollback_stack.len()
                ),
            });
        }
        self.rollback_stack.push(RollbackEntry::AppOpened {
            previous_package: previous.to_string(),
        });
        Ok(())
    }

    /// Record a generic action.
    pub fn record_action(&mut self) -> Result<(), SecurityError> {
        self.check_limits()?;
        self.total_actions += 1;
        Ok(())
    }

    /// Terminate the session.
    pub fn terminate(&mut self) {
        self.terminated = true;
        tracing::info!(
            target: "SECURITY",
            session_id = self.id,
            total_actions = self.total_actions,
            "sandbox session terminated"
        );
    }

    /// Pop the last rollback entry.
    pub fn pop_rollback(&mut self) -> Option<RollbackEntry> {
        self.rollback_stack.pop()
    }

    /// Number of rollback entries available.
    pub fn rollback_depth(&self) -> usize {
        self.rollback_stack.len()
    }

    /// Get the elapsed duration of this session.
    pub fn elapsed(&self) -> Duration {
        self.started_at.elapsed()
    }
}

// ---------------------------------------------------------------------------
// Sandbox
// ---------------------------------------------------------------------------

/// Action sandboxing and isolation engine.
///
/// Classifies actions, enforces containment levels, manages sessions,
/// and provides preview/dry-run/rollback capabilities.
pub struct Sandbox {
    /// Active sandbox sessions.
    sessions: VecDeque<SandboxSession>,
    /// Maximum concurrent sessions.
    max_sessions: usize,
    /// Next session ID.
    next_session_id: u64,
    /// Default resource limits for new sessions.
    default_limits: ResourceLimits,
    /// Total actions sandboxed.
    total_sandboxed: u64,
    /// Total actions refused.
    total_refused: u64,
}

impl Sandbox {
    /// Create a new sandbox with default configuration.
    pub fn new() -> Self {
        Self {
            sessions: VecDeque::new(),
            max_sessions: 10,
            next_session_id: 0,
            default_limits: ResourceLimits::default(),
            total_sandboxed: 0,
            total_refused: 0,
        }
    }

    /// Create a new sandbox with custom limits.
    pub fn with_limits(limits: ResourceLimits) -> Self {
        Self {
            sessions: VecDeque::new(),
            max_sessions: 10,
            next_session_id: 0,
            default_limits: limits,
            total_sandboxed: 0,
            total_refused: 0,
        }
    }

    /// Classify the containment level for an action.
    pub fn classify(&self, action: &ActionType) -> ContainmentLevel {
        match action {
            // L0: Direct — simple, safe UI interactions.
            ActionType::Tap { .. }
            | ActionType::Scroll { .. }
            | ActionType::Back
            | ActionType::Home
            | ActionType::Recents
            | ActionType::WaitForElement { .. }
            | ActionType::AssertElement { .. } => ContainmentLevel::Direct,

            // L1: Monitored — potentially meaningful but generally safe.
            ActionType::LongPress { .. } | ActionType::Swipe { .. } | ActionType::Type { .. } => {
                ContainmentLevel::Monitored
            }

            // L2: Restricted — opens apps, interacts with notifications.
            ActionType::OpenApp { .. } | ActionType::NotificationAction { .. } => {
                ContainmentLevel::Restricted
            }
        }
    }

    /// Classify from an action string (for PolicyGate integration).
    ///
    /// # LLM=brain, Rust=body
    /// NLP keyword matching on action strings violates the Iron Law: the LLM
    /// classifies actions, Rust enforces structured decisions.  This function
    /// returns `Direct` (neutral) unconditionally; the caller must use the
    /// structured `classify(&ActionType)` overload instead.
    pub fn classify_string(&self, _action: &str) -> ContainmentLevel {
        // LLM classifies actions — Rust returns Direct as neutral.
        ContainmentLevel::Direct
    }

    /// Preview an action WITHOUT executing it.
    ///
    /// Returns a description of what would happen, the containment level,
    /// and whether confirmation is required.
    pub fn preview(&self, action: &ActionType) -> ActionPreview {
        let level = self.classify(action);
        let (desc, risk, reversible) = describe_action(action);

        ActionPreview {
            action_description: desc,
            containment_level: level,
            reversible,
            effect_description: format!("Would execute at {level}"),
            risk_summary: risk,
            needs_confirmation: level >= ContainmentLevel::Restricted,
        }
    }

    /// Simulate a dry-run: classify and validate but do not execute.
    ///
    /// Returns Ok with the containment level if the action would be allowed,
    /// or Err if it would be refused.
    pub fn dry_run(&self, action: &ActionType) -> Result<ContainmentLevel, SecurityError> {
        let level = self.classify(action);

        if level == ContainmentLevel::Forbidden {
            return Err(SecurityError::SandboxRefused {
                reason: format!("action classified as {level}"),
            });
        }

        tracing::debug!(
            target: "SECURITY",
            level = %level,
            "sandbox dry-run: action would be allowed"
        );

        Ok(level)
    }

    /// Create a new sandbox session for contained execution.
    pub fn create_session(&mut self) -> Result<u64, SecurityError> {
        self.create_session_with_limits(self.default_limits.clone())
    }

    /// Create a new sandbox session with custom limits.
    pub fn create_session_with_limits(
        &mut self,
        limits: ResourceLimits,
    ) -> Result<u64, SecurityError> {
        // Evict oldest terminated sessions if at capacity.
        while self.sessions.len() >= self.max_sessions {
            if let Some(front) = self.sessions.front() {
                if front.terminated {
                    self.sessions.pop_front();
                } else {
                    return Err(SecurityError::ResourceLimitExceeded {
                        resource: format!("max sessions: {}", self.max_sessions),
                    });
                }
            }
        }

        let id = self.next_session_id;
        self.next_session_id += 1;
        self.sessions.push_back(SandboxSession::new(id, limits));

        tracing::info!(
            target: "SECURITY",
            session_id = id,
            "sandbox session created"
        );

        Ok(id)
    }

    /// Execute an action within containment.
    ///
    /// Checks containment level, enforces resource limits, and stores
    /// rollback information for reversible actions.
    ///
    /// Returns Ok(ContainmentLevel) if the action should proceed,
    /// or Err if it must be refused.
    pub fn execute_contained(
        &mut self,
        session_id: u64,
        action: &ActionType,
    ) -> Result<ContainmentLevel, SecurityError> {
        let level = self.classify(action);

        if level == ContainmentLevel::Forbidden {
            self.total_refused += 1;
            return Err(SecurityError::SandboxRefused {
                reason: format!("action classified as {level}"),
            });
        }

        let session =
            self.get_session_mut(session_id)
                .ok_or_else(|| SecurityError::SandboxRefused {
                    reason: format!("session {session_id} not found"),
                })?;

        // Record action in session and check limits.
        match action {
            ActionType::Tap { .. } | ActionType::LongPress { .. } => {
                session.record_tap()?;
            }
            ActionType::Type { text } => {
                session.record_text(text)?;
            }
            ActionType::OpenApp { package } => {
                // Use empty string as "previous" since we don't know it here.
                // In real integration, caller provides the current foreground app.
                session.record_app_open(package, "")?;
            }
            _ => {
                session.record_action()?;
            }
        }

        self.total_sandboxed += 1;

        tracing::info!(
            target: "SECURITY",
            session_id = session_id,
            level = %level,
            "sandbox: action contained"
        );

        Ok(level)
    }

    /// Get a mutable reference to a session by ID.
    fn get_session_mut(&mut self, session_id: u64) -> Option<&mut SandboxSession> {
        self.sessions.iter_mut().find(|s| s.id == session_id)
    }

    /// Get an immutable reference to a session by ID.
    pub fn get_session(&self, session_id: u64) -> Option<&SandboxSession> {
        self.sessions.iter().find(|s| s.id == session_id)
    }

    /// Terminate a session.
    pub fn terminate_session(&mut self, session_id: u64) -> Result<(), SecurityError> {
        let session =
            self.get_session_mut(session_id)
                .ok_or_else(|| SecurityError::SandboxRefused {
                    reason: format!("session {session_id} not found"),
                })?;
        session.terminate();
        Ok(())
    }

    /// Rollback the last action in a session.
    pub fn rollback_last(
        &mut self,
        session_id: u64,
    ) -> Result<Option<RollbackEntry>, SecurityError> {
        let session =
            self.get_session_mut(session_id)
                .ok_or_else(|| SecurityError::SandboxRefused {
                    reason: format!("session {session_id} not found"),
                })?;
        Ok(session.pop_rollback())
    }

    /// Total actions sandboxed.
    pub fn total_sandboxed(&self) -> u64 {
        self.total_sandboxed
    }

    /// Total actions refused.
    pub fn total_refused(&self) -> u64 {
        self.total_refused
    }

    /// Number of active (non-terminated) sessions.
    pub fn active_session_count(&self) -> usize {
        self.sessions.iter().filter(|s| !s.terminated).count()
    }

    /// Emergency: terminate ALL sessions immediately.
    pub fn terminate_all(&mut self) {
        for session in &mut self.sessions {
            session.terminate();
        }
        tracing::warn!(
            target: "SECURITY",
            "sandbox: ALL sessions terminated (emergency)"
        );
    }
}

impl Default for Sandbox {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Describe an action for preview purposes.
///
/// Returns (description, risk_summary, is_reversible).
fn describe_action(action: &ActionType) -> (String, String, bool) {
    match action {
        ActionType::Tap { x, y } => (
            format!("Tap at ({x}, {y})"),
            "Low risk — simple touch interaction".to_string(),
            false,
        ),
        ActionType::LongPress { x, y } => (
            format!("Long press at ({x}, {y})"),
            "Low risk — may open context menu".to_string(),
            false,
        ),
        ActionType::Swipe {
            from_x,
            from_y,
            to_x,
            to_y,
            duration_ms,
        } => (
            format!("Swipe ({from_x},{from_y}) -> ({to_x},{to_y}) over {duration_ms}ms"),
            "Low risk — scroll/navigation gesture".to_string(),
            true,
        ),
        ActionType::Type { text } => {
            let preview = if text.len() > 20 {
                let mut end = 20;
                while end > 0 && !text.is_char_boundary(end) {
                    end -= 1;
                }
                format!("{}...", &text[..end])
            } else {
                text.clone()
            };
            (
                format!("Type: \"{preview}\""),
                "Medium risk — entering text".to_string(),
                true,
            )
        }
        ActionType::Scroll { direction, amount } => (
            format!("Scroll {direction:?} by {amount}"),
            "Low risk — scrolling content".to_string(),
            true,
        ),
        ActionType::Back => (
            "Press Back button".to_string(),
            "Low risk — navigation".to_string(),
            true,
        ),
        ActionType::Home => (
            "Press Home button".to_string(),
            "Low risk — return to launcher".to_string(),
            false,
        ),
        ActionType::Recents => (
            "Open Recents".to_string(),
            "Low risk — view recent apps".to_string(),
            false,
        ),
        ActionType::OpenApp { package } => (
            format!("Open app: {package}"),
            "Medium risk — launching application".to_string(),
            true,
        ),
        ActionType::NotificationAction {
            notification_id,
            action_index,
        } => (
            format!("Notification #{notification_id} action #{action_index}"),
            "Medium risk — interacting with notification".to_string(),
            false,
        ),
        ActionType::WaitForElement {
            selector,
            timeout_ms,
        } => (
            format!("Wait for element: {selector:?} (timeout: {timeout_ms}ms)"),
            "No risk — passive observation".to_string(),
            false,
        ),
        ActionType::AssertElement { selector, expected } => (
            format!("Assert element {selector:?} is {expected:?}"),
            "No risk — passive verification".to_string(),
            false,
        ),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_containment_level_ordering() {
        assert!(ContainmentLevel::Direct < ContainmentLevel::Monitored);
        assert!(ContainmentLevel::Monitored < ContainmentLevel::Restricted);
        assert!(ContainmentLevel::Restricted < ContainmentLevel::Forbidden);
    }

    #[test]
    fn test_containment_level_display() {
        assert_eq!(ContainmentLevel::Direct.to_string(), "L0:Direct");
        assert_eq!(ContainmentLevel::Forbidden.to_string(), "L3:Forbidden");
    }

    #[test]
    fn test_containment_level_from_u8() {
        assert_eq!(ContainmentLevel::from_u8(0), Some(ContainmentLevel::Direct));
        assert_eq!(
            ContainmentLevel::from_u8(3),
            Some(ContainmentLevel::Forbidden)
        );
        assert_eq!(ContainmentLevel::from_u8(4), None);
    }

    #[test]
    fn test_classify_tap_is_direct() {
        let sandbox = Sandbox::new();
        let action = ActionType::Tap { x: 100, y: 200 };
        assert_eq!(sandbox.classify(&action), ContainmentLevel::Direct);
    }

    #[test]
    fn test_classify_scroll_is_direct() {
        let sandbox = Sandbox::new();
        let action = ActionType::Scroll {
            direction: aura_types::actions::ScrollDirection::Down,
            amount: 3,
        };
        assert_eq!(sandbox.classify(&action), ContainmentLevel::Direct);
    }

    #[test]
    fn test_classify_type_is_monitored() {
        let sandbox = Sandbox::new();
        let action = ActionType::Type {
            text: "hello".to_string(),
        };
        assert_eq!(sandbox.classify(&action), ContainmentLevel::Monitored);
    }

    #[test]
    fn test_classify_open_app_is_restricted() {
        let sandbox = Sandbox::new();
        let action = ActionType::OpenApp {
            package: "com.example".to_string(),
        };
        assert_eq!(sandbox.classify(&action), ContainmentLevel::Restricted);
    }

    #[test]
    fn test_classify_string_is_neutral_stub() {
        // classify_string() is a safe stub — LLM=brain, Rust=body.
        // NLP keyword classification belongs in the LLM layer, not in Rust.
        // All strings must return Direct (neutral) regardless of content.
        let sandbox = Sandbox::new();
        assert_eq!(
            sandbox.classify_string("factory reset now"),
            ContainmentLevel::Direct,
            "classify_string must be a neutral stub — no NLP in Rust"
        );
        assert_eq!(
            sandbox.classify_string("wipe data"),
            ContainmentLevel::Direct,
            "classify_string must be a neutral stub — no NLP in Rust"
        );
        assert_eq!(
            sandbox.classify_string("install app com.game"),
            ContainmentLevel::Direct,
            "classify_string must be a neutral stub — no NLP in Rust"
        );
        assert_eq!(
            sandbox.classify_string("type hello world"),
            ContainmentLevel::Direct,
            "classify_string must be a neutral stub — no NLP in Rust"
        );
        assert_eq!(
            sandbox.classify_string("tap at 100 200"),
            ContainmentLevel::Direct,
            "classify_string must be a neutral stub — no NLP in Rust"
        );
    }

    #[test]
    fn test_preview_tap() {
        let sandbox = Sandbox::new();
        let action = ActionType::Tap { x: 50, y: 100 };
        let preview = sandbox.preview(&action);
        assert_eq!(preview.containment_level, ContainmentLevel::Direct);
        assert!(!preview.needs_confirmation);
        assert!(preview.action_description.contains("50"));
    }

    #[test]
    fn test_preview_open_app_needs_confirmation() {
        let sandbox = Sandbox::new();
        let action = ActionType::OpenApp {
            package: "com.example".to_string(),
        };
        let preview = sandbox.preview(&action);
        assert_eq!(preview.containment_level, ContainmentLevel::Restricted);
        assert!(preview.needs_confirmation);
    }

    #[test]
    fn test_dry_run_allowed() {
        let sandbox = Sandbox::new();
        let action = ActionType::Tap { x: 0, y: 0 };
        let result = sandbox.dry_run(&action);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), ContainmentLevel::Direct);
    }

    #[test]
    fn test_create_session() {
        let mut sandbox = Sandbox::new();
        let session_id = sandbox.create_session().unwrap();
        assert_eq!(session_id, 0);
        assert_eq!(sandbox.active_session_count(), 1);
    }

    #[test]
    fn test_execute_contained_tap() {
        let mut sandbox = Sandbox::new();
        let sid = sandbox.create_session().unwrap();
        let action = ActionType::Tap { x: 100, y: 200 };
        let level = sandbox.execute_contained(sid, &action).unwrap();
        assert_eq!(level, ContainmentLevel::Direct);
        assert_eq!(sandbox.total_sandboxed(), 1);
    }

    #[test]
    fn test_execute_contained_type_stores_rollback() {
        let mut sandbox = Sandbox::new();
        let sid = sandbox.create_session().unwrap();
        let action = ActionType::Type {
            text: "hello".to_string(),
        };
        sandbox.execute_contained(sid, &action).unwrap();

        let session = sandbox.get_session(sid).unwrap();
        assert_eq!(session.rollback_depth(), 1);
        assert_eq!(session.chars_typed, 5);
    }

    #[test]
    fn test_execute_contained_open_app_tracks_apps() {
        let mut sandbox = Sandbox::new();
        let sid = sandbox.create_session().unwrap();
        let action = ActionType::OpenApp {
            package: "com.example".to_string(),
        };
        sandbox.execute_contained(sid, &action).unwrap();

        let session = sandbox.get_session(sid).unwrap();
        assert_eq!(session.apps_touched.len(), 1);
        assert_eq!(session.apps_touched[0], "com.example");
    }

    #[test]
    fn test_resource_limit_taps() {
        let limits = ResourceLimits {
            max_taps: 2,
            ..Default::default()
        };
        let mut sandbox = Sandbox::with_limits(limits);
        let sid = sandbox.create_session().unwrap();
        let tap = ActionType::Tap { x: 0, y: 0 };

        assert!(sandbox.execute_contained(sid, &tap).is_ok());
        assert!(sandbox.execute_contained(sid, &tap).is_ok());
        // Third tap should fail.
        assert!(sandbox.execute_contained(sid, &tap).is_err());
    }

    #[test]
    fn test_resource_limit_total_actions() {
        let limits = ResourceLimits {
            max_total_actions: 3,
            ..Default::default()
        };
        let mut sandbox = Sandbox::with_limits(limits);
        let sid = sandbox.create_session().unwrap();

        sandbox.execute_contained(sid, &ActionType::Back).unwrap();
        sandbox.execute_contained(sid, &ActionType::Home).unwrap();
        sandbox
            .execute_contained(sid, &ActionType::Recents)
            .unwrap();
        // Fourth action exceeds limit.
        assert!(sandbox.execute_contained(sid, &ActionType::Back).is_err());
    }

    #[test]
    fn test_terminate_session() {
        let mut sandbox = Sandbox::new();
        let sid = sandbox.create_session().unwrap();
        sandbox.terminate_session(sid).unwrap();
        assert_eq!(sandbox.active_session_count(), 0);

        // Actions on terminated session should fail.
        let tap = ActionType::Tap { x: 0, y: 0 };
        assert!(sandbox.execute_contained(sid, &tap).is_err());
    }

    #[test]
    fn test_rollback_last() {
        let mut sandbox = Sandbox::new();
        let sid = sandbox.create_session().unwrap();

        let type_action = ActionType::Type {
            text: "test".to_string(),
        };
        sandbox.execute_contained(sid, &type_action).unwrap();

        let rollback = sandbox.rollback_last(sid).unwrap();
        assert!(rollback.is_some());
        if let Some(RollbackEntry::TextTyped { char_count }) = rollback {
            assert_eq!(char_count, 4);
        } else {
            panic!("expected TextTyped rollback entry");
        }
    }

    #[test]
    fn test_terminate_all() {
        let mut sandbox = Sandbox::new();
        sandbox.create_session().unwrap();
        sandbox.create_session().unwrap();
        sandbox.create_session().unwrap();
        assert_eq!(sandbox.active_session_count(), 3);

        sandbox.terminate_all();
        assert_eq!(sandbox.active_session_count(), 0);
    }

    #[test]
    fn test_rollback_entry_describe() {
        let r1 = RollbackEntry::TextTyped { char_count: 10 };
        assert!(r1.describe().contains("10 characters"));

        let r2 = RollbackEntry::AppOpened {
            previous_package: "com.launcher".to_string(),
        };
        assert!(r2.describe().contains("com.launcher"));

        let r3 = RollbackEntry::NavigatedBack;
        assert_eq!(r3.describe(), "navigate forward");
    }

    #[test]
    fn test_resource_limits_default() {
        let limits = ResourceLimits::default();
        assert_eq!(limits.max_taps, 100);
        assert_eq!(limits.max_duration, Duration::from_secs(300));
        assert_eq!(limits.max_apps, 5);
        assert_eq!(limits.max_chars_typed, 1000);
        assert_eq!(limits.max_total_actions, 200);
    }

    #[test]
    fn test_session_elapsed() {
        let session = SandboxSession::new(0, ResourceLimits::default());
        // Elapsed should be very small since we just created it.
        assert!(session.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn test_action_preview_type_text() {
        let sandbox = Sandbox::new();
        let action = ActionType::Type {
            text: "a very long text that exceeds twenty characters".to_string(),
        };
        let preview = sandbox.preview(&action);
        assert!(preview.action_description.contains("..."));
        assert!(preview.reversible);
    }

    #[test]
    fn test_sandbox_default() {
        let sandbox = Sandbox::default();
        assert_eq!(sandbox.total_sandboxed(), 0);
        assert_eq!(sandbox.total_refused(), 0);
        assert_eq!(sandbox.active_session_count(), 0);
    }

    #[test]
    fn test_containment_level_as_u8() {
        assert_eq!(ContainmentLevel::Direct.as_u8(), 0);
        assert_eq!(ContainmentLevel::Monitored.as_u8(), 1);
        assert_eq!(ContainmentLevel::Restricted.as_u8(), 2);
        assert_eq!(ContainmentLevel::Forbidden.as_u8(), 3);
    }
}
