use std::{collections::HashMap, fmt};

use aura_types::{
    events::ParsedEvent,
    ipc::{ContextPackage, DaemonToNeocortex, InferenceMode},
};
use serde::{Deserialize, Serialize};
use tracing::instrument;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Default timeout for a System2 request (30 s).
const DEFAULT_TIMEOUT_MS: u64 = 30_000;

/// Maximum number of concurrent pending requests. Bounded to prevent OOM.
const MAX_PENDING: usize = 64;

/// Entries older than this (30 s) are considered stale and swept before insert.
const STALE_TIMEOUT_MS: u64 = 30_000;

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Errors originating from the System2 (slow-path) subsystem.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RoutingError {
    /// The pending-request map is at capacity after sweeping stale entries.
    PendingCapacityExceeded { current: usize, max: usize },
}

impl fmt::Display for RoutingError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PendingCapacityExceeded { current, max } => {
                write!(f, "pending request capacity exceeded: {current} / {max}")
            }
        }
    }
}

impl std::error::Error for RoutingError {}

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// A pending request waiting for a Neocortex response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingRequest {
    pub request_id: u64,
    pub mode: InferenceMode,
    pub created_ms: u64,
    pub timeout_ms: u64,
}

/// The message to send to the Neocortex.
#[derive(Debug, Clone)]
pub struct System2Request {
    pub message: DaemonToNeocortex,
    pub request_id: u64,
}

/// Slow path — prepares requests for the Neocortex LLM and tracks pending work.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct System2 {
    next_request_id: u64,
    pending: HashMap<u64, PendingRequest>,
}

// ---------------------------------------------------------------------------
// Implementation
// ---------------------------------------------------------------------------

impl System2 {
    pub fn new() -> Self {
        Self {
            next_request_id: 1,
            pending: HashMap::new(),
        }
    }

    /// Prepare a request for the Neocortex based on the event and mode.
    ///
    /// Allocates a monotonically-increasing request ID, builds a
    /// `ContextPackage` from the event, and registers the request as pending.
    ///
    /// # Errors
    ///
    /// Returns `RoutingError::PendingCapacityExceeded` if the pending map is
    /// full (>= `MAX_PENDING`) after sweeping stale entries.
    #[instrument(skip(self, event), fields(mode = ?mode))]
    pub fn prepare_request(
        &mut self,
        event: &ParsedEvent,
        mode: InferenceMode,
        now_ms: u64,
    ) -> Result<System2Request, RoutingError> {
        // Sweep stale entries before checking capacity.
        self.sweep_stale(now_ms);

        if self.pending.len() >= MAX_PENDING {
            tracing::warn!(
                pending = self.pending.len(),
                max = MAX_PENDING,
                "System2 pending capacity exceeded"
            );
            return Err(RoutingError::PendingCapacityExceeded {
                current: self.pending.len(),
                max: MAX_PENDING,
            });
        }

        let id = self.next_request_id;
        self.next_request_id += 1;

        // Build a minimal context package from the parsed event.
        let mut context = ContextPackage::default();
        context.inference_mode = mode;
        context
            .conversation_history
            .push(aura_types::ipc::ConversationTurn {
                role: aura_types::ipc::Role::User,
                content: event.content.clone(),
                timestamp_ms: event.timestamp_ms,
            });

        let message = match mode {
            InferenceMode::Planner | InferenceMode::Strategist => DaemonToNeocortex::Plan {
                context,
                failure: None,
            },
            InferenceMode::Conversational => DaemonToNeocortex::Converse { context },
            InferenceMode::Composer => DaemonToNeocortex::Compose {
                context,
                template: String::new(),
            },
        };

        self.pending.insert(
            id,
            PendingRequest {
                request_id: id,
                mode,
                created_ms: now_ms,
                timeout_ms: DEFAULT_TIMEOUT_MS,
            },
        );

        tracing::debug!(
            request_id = id,
            mode = ?mode,
            "System2 request prepared"
        );

        Ok(System2Request {
            message,
            request_id: id,
        })
    }

    /// Remove pending entries that are older than `STALE_TIMEOUT_MS`.
    ///
    /// Called automatically by `prepare_request` before each insert.
    #[instrument(skip(self))]
    pub fn sweep_stale(&mut self, now_ms: u64) {
        let before = self.pending.len();
        self.pending
            .retain(|_id, req| now_ms.saturating_sub(req.created_ms) <= STALE_TIMEOUT_MS);
        let swept = before.saturating_sub(self.pending.len());
        if swept > 0 {
            tracing::debug!(
                swept,
                remaining = self.pending.len(),
                "swept stale pending requests"
            );
        }
    }

    /// Check whether a pending request has timed out.
    #[instrument(skip(self))]
    pub fn is_timed_out(&self, request_id: u64, now_ms: u64) -> bool {
        self.pending
            .get(&request_id)
            .is_some_and(|req| now_ms.saturating_sub(req.created_ms) > req.timeout_ms)
    }

    /// Mark a request as complete and remove it from the pending set.
    #[instrument(skip(self))]
    pub fn complete_request(&mut self, request_id: u64) -> Option<PendingRequest> {
        self.pending.remove(&request_id)
    }

    /// Cancel a pending request and return the Cancel message to send.
    #[instrument(skip(self))]
    pub fn cancel_request(&mut self, request_id: u64) -> Option<DaemonToNeocortex> {
        self.pending.remove(&request_id).map(|_| {
            tracing::info!(request_id, "System2 request cancelled");
            DaemonToNeocortex::Cancel
        })
    }

    /// Number of currently pending requests.
    #[must_use]
    pub fn pending_count(&self) -> usize {
        self.pending.len()
    }
}

impl Default for System2 {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use aura_types::events::{EventSource, Intent};

    use super::*;

    fn make_event(content: &str) -> ParsedEvent {
        ParsedEvent {
            source: EventSource::UserCommand,
            intent: Intent::ActionRequest,
            content: content.to_string(),
            entities: vec![],
            timestamp_ms: 1_000_000,
            raw_event_type: 0,
        }
    }

    #[test]
    fn test_request_id_incrementing() {
        let mut s2 = System2::new();
        let r1 = s2
            .prepare_request(&make_event("a"), InferenceMode::Planner, 1_000)
            .expect("should succeed");
        let r2 = s2
            .prepare_request(&make_event("b"), InferenceMode::Planner, 2_000)
            .expect("should succeed");
        assert_eq!(r1.request_id, 1);
        assert_eq!(r2.request_id, 2);
        assert_eq!(s2.pending_count(), 2);
    }

    #[test]
    fn test_timeout_detection() {
        let mut s2 = System2::new();
        let r = s2
            .prepare_request(&make_event("test"), InferenceMode::Planner, 10_000)
            .expect("should succeed");

        // Not timed out yet.
        assert!(!s2.is_timed_out(r.request_id, 20_000));

        // Timed out (30s + 1ms past creation).
        assert!(s2.is_timed_out(r.request_id, 40_001));
    }

    #[test]
    fn test_complete_removes_pending() {
        let mut s2 = System2::new();
        let r = s2
            .prepare_request(&make_event("test"), InferenceMode::Planner, 1_000)
            .expect("should succeed");
        assert_eq!(s2.pending_count(), 1);

        let completed = s2.complete_request(r.request_id);
        assert!(completed.is_some());
        assert_eq!(s2.pending_count(), 0);
    }

    #[test]
    fn test_cancel_produces_cancel_message() {
        let mut s2 = System2::new();
        let r = s2
            .prepare_request(&make_event("test"), InferenceMode::Planner, 1_000)
            .expect("should succeed");

        let cancel_msg = s2.cancel_request(r.request_id);
        assert!(cancel_msg.is_some());
        match cancel_msg.expect("just checked is_some") {
            DaemonToNeocortex::Cancel => { /* correct */ }
            other => panic!("expected Cancel, got {:?}", other),
        }
        assert_eq!(s2.pending_count(), 0);
    }

    #[test]
    fn test_conversational_mode_produces_converse() {
        let mut s2 = System2::new();
        let event = make_event("what is the weather");
        let r = s2
            .prepare_request(&event, InferenceMode::Conversational, 1_000)
            .expect("should succeed");

        match &r.message {
            DaemonToNeocortex::Converse { context } => {
                assert_eq!(context.inference_mode, InferenceMode::Conversational);
                assert!(!context.conversation_history.is_empty());
                assert_eq!(
                    context.conversation_history[0].content,
                    "what is the weather"
                );
            }
            other => panic!("expected Converse, got {:?}", other),
        }
    }

    #[test]
    fn test_planner_mode_produces_plan() {
        let mut s2 = System2::new();
        let r = s2
            .prepare_request(
                &make_event("do complex task"),
                InferenceMode::Planner,
                1_000,
            )
            .expect("should succeed");

        match &r.message {
            DaemonToNeocortex::Plan { context, failure } => {
                assert_eq!(context.inference_mode, InferenceMode::Planner);
                assert!(failure.is_none());
            }
            other => panic!("expected Plan, got {:?}", other),
        }
    }

    #[test]
    fn test_cancel_nonexistent_returns_none() {
        let mut s2 = System2::new();
        assert!(s2.cancel_request(999).is_none());
    }

    // -- New tests for capacity and stale sweep ---

    #[test]
    fn test_capacity_exceeded_returns_error() {
        let mut s2 = System2::new();
        let base_ms = 100_000;

        // Fill to MAX_PENDING.
        for i in 0..MAX_PENDING {
            s2.prepare_request(
                &make_event(&format!("req {i}")),
                InferenceMode::Planner,
                base_ms,
            )
            .expect("should succeed while under capacity");
        }

        assert_eq!(s2.pending_count(), MAX_PENDING);

        // Next request should fail — now_ms is still fresh so sweep won't help.
        let result =
            s2.prepare_request(&make_event("one too many"), InferenceMode::Planner, base_ms);
        assert!(result.is_err());
        assert_eq!(
            result.unwrap_err(),
            RoutingError::PendingCapacityExceeded {
                current: MAX_PENDING,
                max: MAX_PENDING,
            }
        );
    }

    #[test]
    fn test_sweep_stale_removes_old_entries() {
        let mut s2 = System2::new();
        let base_ms = 100_000;

        // Insert 3 requests at base_ms.
        for _ in 0..3 {
            s2.prepare_request(&make_event("old"), InferenceMode::Planner, base_ms)
                .expect("should succeed");
        }
        assert_eq!(s2.pending_count(), 3);

        // Sweep at a time where all are stale (>30s later).
        s2.sweep_stale(base_ms + STALE_TIMEOUT_MS + 1);
        assert_eq!(s2.pending_count(), 0);
    }

    #[test]
    fn test_sweep_keeps_fresh_entries() {
        let mut s2 = System2::new();

        // Insert two entries close in time so both are fresh at insert time.
        s2.prepare_request(&make_event("old"), InferenceMode::Planner, 10_000)
            .expect("should succeed");
        s2.prepare_request(&make_event("fresh"), InferenceMode::Planner, 12_000)
            .expect("should succeed");
        assert_eq!(s2.pending_count(), 2);

        // Sweep at 41_000 — first entry (10_000) is 31s old (stale), second (12_000) is 29s
        // (fresh).
        s2.sweep_stale(41_000);
        assert_eq!(s2.pending_count(), 1);
    }

    #[test]
    fn test_sweep_before_insert_frees_capacity() {
        let mut s2 = System2::new();
        let old_ms = 100_000;

        // Fill to capacity with old requests.
        for i in 0..MAX_PENDING {
            s2.prepare_request(
                &make_event(&format!("req {i}")),
                InferenceMode::Planner,
                old_ms,
            )
            .expect("should succeed");
        }
        assert_eq!(s2.pending_count(), MAX_PENDING);

        // Insert a new request far enough in the future that all old ones are stale.
        // The sweep inside prepare_request should clear them.
        let fresh_ms = old_ms + STALE_TIMEOUT_MS + 1;
        let result =
            s2.prepare_request(&make_event("after sweep"), InferenceMode::Planner, fresh_ms);
        assert!(result.is_ok(), "sweep should have freed capacity");
        assert_eq!(s2.pending_count(), 1);
    }

    #[test]
    fn test_routing_error_display() {
        let err = RoutingError::PendingCapacityExceeded {
            current: 64,
            max: 64,
        };
        assert_eq!(
            err.to_string(),
            "pending request capacity exceeded: 64 / 64"
        );
    }
}
