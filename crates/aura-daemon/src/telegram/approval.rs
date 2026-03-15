//! Last Mile Approval — PolicyGate routing for critical actions.
//!
//! When AURA autonomously decides to perform a critical action (sending a
//! message, making a call, modifying system settings), the PolicyGate
//! intercepts and routes the request to the user's Telegram for explicit
//! confirmation before execution.
//!
//! The flow:
//! 1. AURA's planner proposes an action.
//! 2. PolicyGate evaluates the action against the current trust level.
//! 3. If approval is required, a pending request is created and the user
//!    is prompted via Telegram with approve/reject buttons.
//! 4. The action executes only after explicit approval (or auto-approves
//!    at high trust levels).
//! 5. Expired requests are automatically rejected after the TTL.

use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tracing::{instrument, warn};

// ─── Types ──────────────────────────────────────────────────────────────────

/// Unique identifier for an approval request.
pub type ApprovalId = u64;

/// Risk level of an action, determining whether approval is needed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum RiskLevel {
    /// Safe — never needs approval (e.g., reading status).
    Low = 0,
    /// Moderate — may need approval depending on trust level.
    Medium = 1,
    /// High — always needs approval unless trust is maximal.
    High = 2,
    /// Critical — always needs approval regardless of trust.
    Critical = 3,
}

/// Current state of an approval request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ApprovalState {
    /// Waiting for user decision.
    Pending,
    /// User approved the action.
    Approved,
    /// User rejected the action.
    Rejected,
    /// Request expired without a decision.
    Expired,
    /// Auto-approved due to high trust level.
    AutoApproved,
}

/// A pending approval request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovalRequest {
    /// Unique ID for this request.
    pub id: ApprovalId,
    /// Human-readable description of what AURA wants to do.
    pub description: String,
    /// Risk level of the proposed action.
    pub risk: RiskLevel,
    /// Chat ID of the user who should approve.
    pub chat_id: i64,
    /// Current state.
    pub state: ApprovalState,
    /// When the request was created (unix timestamp seconds).
    pub created_at: u64,
    /// TTL in seconds — request expires after this.
    pub ttl_secs: u64,
    /// Optional Telegram message ID for the approval prompt (for editing).
    pub message_id: Option<i64>,
}

impl ApprovalRequest {
    /// Check if this request has expired.
    pub fn is_expired(&self) -> bool {
        let now = unix_now();
        now.saturating_sub(self.created_at) > self.ttl_secs
    }

    /// Format as a Telegram approval prompt.
    pub fn to_prompt_html(&self) -> String {
        let risk_label = match self.risk {
            RiskLevel::Low => "Low",
            RiskLevel::Medium => "Medium",
            RiskLevel::High => "HIGH",
            RiskLevel::Critical => "CRITICAL",
        };

        format!(
            "<b>Approval Required</b> (#{id})\n\n\
             <b>Action:</b> {desc}\n\
             <b>Risk:</b> {risk_label}\n\
             <b>Expires:</b> {ttl}s\n\n\
             Reply with:\n\
             /approve {id} — to allow\n\
             /reject {id} — to deny",
            id = self.id,
            desc = self.description,
            ttl = self.ttl_secs,
        )
    }
}

// ─── PolicyGate ─────────────────────────────────────────────────────────────

/// Hard cap on concurrent pending approval requests.
///
/// Prevents unbounded memory growth if the LLM generates many actions quickly.
/// Oldest pending requests are dropped when the cap is reached.
const MAX_PENDING_REQUESTS: usize = 64;

/// The approval gate that intercepts critical actions.
pub struct PolicyGate {
    /// Active approval requests keyed by ID.
    /// Bounded to MAX_PENDING_REQUESTS entries — enforced in `evaluate()`.
    requests: HashMap<ApprovalId, ApprovalRequest>,
    /// Next ID to assign.
    next_id: ApprovalId,
    /// Default TTL for new requests.
    default_ttl_secs: u64,
}

impl PolicyGate {
    /// Create a new policy gate.
    pub fn new(default_ttl_secs: u64) -> Self {
        Self {
            requests: HashMap::new(),
            next_id: 1,
            default_ttl_secs,
        }
    }

    /// Evaluate whether an action needs approval.
    ///
    /// `Low` risk actions are auto-approved (read-only, safe).
    /// `Medium`, `High`, and `Critical` always require user confirmation —
    /// the LLM decides what risk level to assign; Rust does not weight-score
    /// trust to make that routing decision.
    ///
    /// Returns `None` if auto-approved, or `Some(request)` if user confirmation
    /// is needed.
    #[instrument(skip(self), fields(risk = ?risk))]
    pub fn evaluate(
        &mut self,
        description: String,
        risk: RiskLevel,
        chat_id: i64,
    ) -> Option<&ApprovalRequest> {
        // Structural routing only — no weighted scoring.
        // Low risk is safe (read-only); everything else requires human approval.
        if risk == RiskLevel::Low {
            return None;
        }

        // Enforce bounded capacity: evict the oldest resolved request if full.
        if self.requests.len() >= MAX_PENDING_REQUESTS {
            // Prefer evicting already-resolved entries first.
            let evict_id = self
                .requests
                .iter()
                .find(|(_, r)| r.state != ApprovalState::Pending)
                .map(|(&id, _)| id)
                .or_else(|| {
                    // Fallback: evict the oldest pending entry.
                    self.requests
                        .iter()
                        .min_by_key(|(_, r)| r.created_at)
                        .map(|(&id, _)| id)
                });
            if let Some(id) = evict_id {
                warn!(evicted_id = id, "approval map full — evicting oldest entry");
                self.requests.remove(&id);
            }
        }

        let id = self.next_id;
        self.next_id += 1;

        let request = ApprovalRequest {
            id,
            description,
            risk,
            chat_id,
            state: ApprovalState::Pending,
            created_at: unix_now(),
            ttl_secs: self.default_ttl_secs,
            message_id: None,
        };

        self.requests.insert(id, request);
        self.requests.get(&id)
    }

    /// Approve a pending request.
    #[instrument(skip(self))]
    pub fn approve(&mut self, id: ApprovalId) -> Result<&ApprovalRequest, ApprovalError> {
        let req = self
            .requests
            .get_mut(&id)
            .ok_or(ApprovalError::NotFound(id))?;

        if req.is_expired() {
            req.state = ApprovalState::Expired;
            return Err(ApprovalError::Expired(id));
        }

        if req.state != ApprovalState::Pending {
            return Err(ApprovalError::AlreadyResolved(id));
        }

        req.state = ApprovalState::Approved;
        Ok(self.requests.get(&id).expect("request was just modified via get_mut; key must exist"))
    }

    /// Reject a pending request.
    #[instrument(skip(self))]
    pub fn reject(&mut self, id: ApprovalId) -> Result<&ApprovalRequest, ApprovalError> {
        let req = self
            .requests
            .get_mut(&id)
            .ok_or(ApprovalError::NotFound(id))?;

        if req.state != ApprovalState::Pending {
            return Err(ApprovalError::AlreadyResolved(id));
        }

        req.state = ApprovalState::Rejected;
        Ok(self.requests.get(&id).expect("request was just modified via get_mut; key must exist"))
    }

    /// Expire all timed-out requests.
    #[instrument(skip(self))]
    pub fn expire_stale(&mut self) -> usize {
        let mut count = 0;
        for req in self.requests.values_mut() {
            if req.state == ApprovalState::Pending && req.is_expired() {
                req.state = ApprovalState::Expired;
                warn!(id = req.id, "approval request expired");
                count += 1;
            }
        }
        count
    }

    /// Get all pending requests for a chat ID.
    pub fn pending_for_chat(&self, chat_id: i64) -> Vec<&ApprovalRequest> {
        self.requests
            .values()
            .filter(|r| {
                r.chat_id == chat_id && r.state == ApprovalState::Pending && !r.is_expired()
            })
            .collect()
    }

    /// Get a request by ID.
    pub fn get(&self, id: ApprovalId) -> Option<&ApprovalRequest> {
        self.requests.get(&id)
    }

    /// Set the message ID for an approval prompt (for later editing).
    pub fn set_message_id(&mut self, id: ApprovalId, message_id: i64) {
        if let Some(req) = self.requests.get_mut(&id) {
            req.message_id = Some(message_id);
        }
    }

    /// Remove resolved (non-pending) requests older than `max_age_secs`.
    pub fn cleanup(&mut self, max_age_secs: u64) -> usize {
        let now = unix_now();
        let before = self.requests.len();
        self.requests.retain(|_, r| {
            r.state == ApprovalState::Pending || now.saturating_sub(r.created_at) < max_age_secs
        });
        before - self.requests.len()
    }
}

impl std::fmt::Debug for PolicyGate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PolicyGate")
            .field("requests", &self.requests.len())
            .finish()
    }
}

// ─── Errors ─────────────────────────────────────────────────────────────────

/// Errors from approval operations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApprovalError {
    /// No request with this ID exists.
    NotFound(ApprovalId),
    /// The request has already been resolved.
    AlreadyResolved(ApprovalId),
    /// The request expired.
    Expired(ApprovalId),
}

impl std::fmt::Display for ApprovalError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound(id) => write!(f, "approval request #{id} not found"),
            Self::AlreadyResolved(id) => write!(f, "approval request #{id} already resolved"),
            Self::Expired(id) => write!(f, "approval request #{id} expired"),
        }
    }
}

impl std::error::Error for ApprovalError {}

// ─── Helpers ────────────────────────────────────────────────────────────────

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

// ─── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_low_risk_auto_approved() {
        let mut gate = PolicyGate::new(300);
        let result = gate.evaluate("read status".into(), RiskLevel::Low, 42);
        assert!(result.is_none(), "low risk should auto-approve");
    }

    #[test]
    fn test_critical_always_needs_approval() {
        let mut gate = PolicyGate::new(300);
        let result = gate.evaluate("delete all data".into(), RiskLevel::Critical, 42);
        assert!(result.is_some(), "critical should always need approval");
    }

    #[test]
    fn test_medium_risk_always_needs_approval() {
        let mut gate = PolicyGate::new(300);
        let result = gate.evaluate("send message".into(), RiskLevel::Medium, 42);
        assert!(
            result.is_some(),
            "medium risk always requires user approval — LLM assigns risk, Rust does not weight-score trust"
        );
    }

    #[test]
    fn test_high_risk_always_needs_approval() {
        let mut gate = PolicyGate::new(300);
        let result = gate.evaluate("delete file".into(), RiskLevel::High, 42);
        assert!(
            result.is_some(),
            "high risk always requires user approval"
        );
    }

    #[test]
    fn test_approve_flow() {
        let mut gate = PolicyGate::new(300);
        let req = gate
            .evaluate("call mom".into(), RiskLevel::Medium, 42)
            .unwrap();
        let id = req.id;

        let approved = gate.approve(id).unwrap();
        assert_eq!(approved.state, ApprovalState::Approved);
    }

    #[test]
    fn test_reject_flow() {
        let mut gate = PolicyGate::new(300);
        let req = gate
            .evaluate("send money".into(), RiskLevel::High, 42)
            .unwrap();
        let id = req.id;

        let rejected = gate.reject(id).unwrap();
        assert_eq!(rejected.state, ApprovalState::Rejected);
    }

    #[test]
    fn test_double_resolve_fails() {
        let mut gate = PolicyGate::new(300);
        let req = gate
            .evaluate("do thing".into(), RiskLevel::Medium, 42)
            .unwrap();
        let id = req.id;

        gate.approve(id).unwrap();
        let err = gate.approve(id).unwrap_err();
        assert_eq!(err, ApprovalError::AlreadyResolved(id));
    }

    #[test]
    fn test_not_found() {
        let mut gate = PolicyGate::new(300);
        let err = gate.approve(999).unwrap_err();
        assert_eq!(err, ApprovalError::NotFound(999));
    }

    #[test]
    fn test_prompt_html() {
        let req = ApprovalRequest {
            id: 7,
            description: "send WhatsApp to Alice".into(),
            risk: RiskLevel::High,
            chat_id: 42,
            state: ApprovalState::Pending,
            created_at: unix_now(),
            ttl_secs: 300,
            message_id: None,
        };
        let html = req.to_prompt_html();
        assert!(html.contains("Approval Required"));
        assert!(html.contains("#7"));
        assert!(html.contains("HIGH"));
        assert!(html.contains("/approve 7"));
        assert!(html.contains("/reject 7"));
    }

    #[test]
    fn test_pending_for_chat() {
        let mut gate = PolicyGate::new(300);
        gate.evaluate("action 1".into(), RiskLevel::Medium, 42);
        gate.evaluate("action 2".into(), RiskLevel::High, 42);
        gate.evaluate("action 3".into(), RiskLevel::Medium, 99);

        let pending = gate.pending_for_chat(42);
        assert_eq!(pending.len(), 2);

        let pending_99 = gate.pending_for_chat(99);
        assert_eq!(pending_99.len(), 1);
    }
}
