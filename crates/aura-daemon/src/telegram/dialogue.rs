//! FSM-based multi-step dialogue flows for the Telegram bot.
//!
//! Some commands require multi-step confirmation or data gathering before
//! execution. For example, "delete all memories" requires 3 separate
//! confirmations to prevent accidental data loss.
//!
//! The FSM (Finite State Machine) tracks per-chat dialogue state and
//! routes subsequent messages to the correct flow handler until the
//! dialogue completes, times out, or is cancelled.

use std::{
    collections::HashMap,
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use tracing::{instrument, warn};

// ─── Types ──────────────────────────────────────────────────────────────────

/// Identifier for a dialogue flow type.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DialogueKind {
    /// `/forget *` — Confirm deletion of all memories (3 steps).
    ForgetAll,
    /// `/pin set` when changing an existing PIN — requires old PIN first.
    PinChange,
    /// `/automate` — Multi-step routine setup.
    AutomateSetup,
    /// Generic confirmation for destructive actions.
    GenericConfirm { action: String },
}

/// A single step in a dialogue flow.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DialogueStep {
    /// The prompt to show the user for this step.
    pub prompt: String,
    /// Expected response type.
    pub expect: ExpectedInput,
    /// Step number (0-indexed).
    pub index: usize,
}

/// What kind of input the current step expects.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ExpectedInput {
    /// User must type the exact confirmation phrase.
    ExactPhrase(String),
    /// User must type "yes" or "no" (case-insensitive).
    YesNo,
    /// Free-form text input.
    FreeText,
    /// A PIN value.
    Pin,
}

/// State of an active dialogue.
#[derive(Debug, Clone)]
pub struct ActiveDialogue {
    /// What kind of flow this is.
    pub kind: DialogueKind,
    /// The steps in this flow.
    pub steps: Vec<DialogueStep>,
    /// Current step index.
    pub current_step: usize,
    /// Collected responses from previous steps.
    pub responses: Vec<String>,
    /// When the dialogue was started (unix seconds).
    pub started_at: u64,
    /// Timeout in seconds.
    pub timeout_secs: u64,
    /// Chat ID owning this dialogue.
    pub chat_id: i64,
}

impl ActiveDialogue {
    /// Check if the dialogue has timed out.
    pub fn is_timed_out(&self) -> bool {
        let now = unix_now();
        now.saturating_sub(self.started_at) > self.timeout_secs
    }

    /// Check if the dialogue is complete (all steps answered).
    pub fn is_complete(&self) -> bool {
        self.current_step >= self.steps.len()
    }

    /// Get the current step, if any.
    pub fn current(&self) -> Option<&DialogueStep> {
        self.steps.get(self.current_step)
    }

    /// Get the prompt for the current step.
    pub fn current_prompt(&self) -> Option<String> {
        self.current().map(|s| {
            format!(
                "Step {}/{}: {}",
                self.current_step + 1,
                self.steps.len(),
                s.prompt
            )
        })
    }
}

// ─── Result of processing a dialogue step ───────────────────────────────────

/// Outcome of feeding user input into a dialogue.
#[derive(Debug, Clone)]
pub enum DialogueOutcome {
    /// Step accepted, dialogue continues — send the next prompt.
    Continue(String),
    /// All steps completed — the dialogue is done.
    Completed {
        kind: DialogueKind,
        responses: Vec<String>,
    },
    /// The user's input didn't match the expected pattern.
    InvalidInput(String),
    /// The dialogue timed out.
    TimedOut,
    /// The user cancelled (typed /cancel).
    Cancelled,
}

// ─── DialogueManager ────────────────────────────────────────────────────────

/// Hard cap on concurrent active dialogues.
///
/// In a single-user or small-group deployment this limit is never reached,
/// but it prevents unbounded HashMap growth in the presence of many chat IDs
/// or if dialogues are started faster than they complete.
const MAX_ACTIVE_DIALOGUES: usize = 256;

/// Manages active dialogue flows per chat ID.
pub struct DialogueManager {
    /// Active dialogues keyed by chat ID (one per chat).
    /// Bounded to MAX_ACTIVE_DIALOGUES entries — enforced in `start()`.
    active: HashMap<i64, ActiveDialogue>,
    /// Default timeout for new dialogues.
    default_timeout_secs: u64,
}

impl DialogueManager {
    /// Create a new dialogue manager.
    pub fn new(default_timeout_secs: u64) -> Self {
        Self {
            active: HashMap::new(),
            default_timeout_secs,
        }
    }

    /// Check if a chat has an active dialogue.
    pub fn has_active(&self, chat_id: i64) -> bool {
        self.active
            .get(&chat_id)
            .is_some_and(|d| !d.is_timed_out() && !d.is_complete())
    }

    /// Start a new dialogue flow. Returns the first prompt.
    ///
    /// If a dialogue is already active for this chat, it is replaced.
    /// If the active map is at capacity, the oldest timed-out dialogue is
    /// evicted first; if none are timed out, the oldest by start time is
    /// evicted to keep the map bounded.
    #[instrument(skip(self, steps), fields(chat_id, kind = ?kind))]
    pub fn start(
        &mut self,
        chat_id: i64,
        kind: DialogueKind,
        steps: Vec<DialogueStep>,
    ) -> Option<String> {
        if steps.is_empty() {
            return None;
        }

        // Enforce bounded capacity when inserting a new chat ID.
        if !self.active.contains_key(&chat_id) && self.active.len() >= MAX_ACTIVE_DIALOGUES {
            // Prefer evicting already-timed-out dialogues first.
            let evict = self
                .active
                .iter()
                .find(|(_, d)| d.is_timed_out())
                .map(|(&id, _)| id)
                .or_else(|| {
                    // Fallback: evict the oldest (smallest started_at).
                    self.active
                        .iter()
                        .min_by_key(|(_, d)| d.started_at)
                        .map(|(&id, _)| id)
                });
            if let Some(id) = evict {
                warn!(
                    evicted_chat_id = id,
                    "dialogue map full — evicting oldest entry"
                );
                self.active.remove(&id);
            }
        }

        let dialogue = ActiveDialogue {
            kind,
            steps,
            current_step: 0,
            responses: Vec::new(),
            started_at: unix_now(),
            timeout_secs: self.default_timeout_secs,
            chat_id,
        };

        let prompt = dialogue.current_prompt();
        self.active.insert(chat_id, dialogue);
        prompt
    }

    /// Feed user input into the active dialogue for a chat.
    #[instrument(skip(self, input), fields(chat_id))]
    pub fn process_input(&mut self, chat_id: i64, input: &str) -> DialogueOutcome {
        let dialogue = match self.active.get_mut(&chat_id) {
            Some(d) => d,
            None => return DialogueOutcome::InvalidInput("No active dialogue.".into()),
        };

        // Check timeout.
        if dialogue.is_timed_out() {
            self.active.remove(&chat_id);
            return DialogueOutcome::TimedOut;
        }

        // Check for cancel.
        let trimmed = input.trim().to_lowercase();
        if trimmed == "/cancel" || trimmed == "cancel" {
            self.active.remove(&chat_id);
            return DialogueOutcome::Cancelled;
        }

        // Validate against expected input.
        let step = match dialogue.current() {
            Some(s) => s.clone(),
            None => {
                self.active.remove(&chat_id);
                return DialogueOutcome::InvalidInput("Dialogue already complete.".into());
            },
        };

        match &step.expect {
            ExpectedInput::ExactPhrase(expected) => {
                if !trimmed.eq_ignore_ascii_case(expected) {
                    return DialogueOutcome::InvalidInput(format!(
                        "Please type exactly: <b>{expected}</b>\nOr type /cancel to abort."
                    ));
                }
            },
            ExpectedInput::YesNo => {
                if trimmed != "yes" && trimmed != "no" {
                    return DialogueOutcome::InvalidInput(
                        "Please answer <b>yes</b> or <b>no</b>.".into(),
                    );
                }
                if trimmed == "no" {
                    self.active.remove(&chat_id);
                    return DialogueOutcome::Cancelled;
                }
            },
            ExpectedInput::FreeText => {
                // Any non-empty input is accepted.
                if trimmed.is_empty() {
                    return DialogueOutcome::InvalidInput("Please enter some text.".into());
                }
            },
            ExpectedInput::Pin => {
                if trimmed.is_empty() {
                    return DialogueOutcome::InvalidInput("Please enter a PIN.".into());
                }
            },
        }

        // Accept the response and advance.
        dialogue.responses.push(input.trim().to_string());
        dialogue.current_step += 1;

        if dialogue.is_complete() {
            let kind = dialogue.kind.clone();
            let responses = dialogue.responses.clone();
            self.active.remove(&chat_id);
            return DialogueOutcome::Completed { kind, responses };
        }

        // Return next prompt.
        let prompt = dialogue
            .current_prompt()
            .unwrap_or_else(|| "Continue...".to_string());
        DialogueOutcome::Continue(prompt)
    }

    /// Cancel and remove an active dialogue.
    pub fn cancel(&mut self, chat_id: i64) -> bool {
        self.active.remove(&chat_id).is_some()
    }

    /// Expire all timed-out dialogues.
    pub fn expire_stale(&mut self) -> usize {
        let before = self.active.len();
        self.active.retain(|_, d| !d.is_timed_out());
        before - self.active.len()
    }

    /// Get the active dialogue for a chat (read-only).
    pub fn get(&self, chat_id: i64) -> Option<&ActiveDialogue> {
        self.active.get(&chat_id)
    }
}

impl std::fmt::Debug for DialogueManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DialogueManager")
            .field("active", &self.active.len())
            .field("default_timeout_secs", &self.default_timeout_secs)
            .finish()
    }
}

// ─── Predefined flows ──────────────────────────────────────────────────────

/// Build the "forget all memories" confirmation flow (3 steps).
pub fn forget_all_flow() -> (DialogueKind, Vec<DialogueStep>) {
    let kind = DialogueKind::ForgetAll;
    let steps = vec![
        DialogueStep {
            prompt: "This will <b>permanently delete ALL memories</b>. Are you sure?".into(),
            expect: ExpectedInput::YesNo,
            index: 0,
        },
        DialogueStep {
            prompt: "Type <b>DELETE ALL</b> to confirm.".into(),
            expect: ExpectedInput::ExactPhrase("DELETE ALL".into()),
            index: 1,
        },
        DialogueStep {
            prompt: "Final confirmation: type <b>yes</b> to proceed with deletion.".into(),
            expect: ExpectedInput::YesNo,
            index: 2,
        },
    ];
    (kind, steps)
}

/// Build a generic destructive action confirmation flow.
pub fn generic_confirm_flow(action: &str) -> (DialogueKind, Vec<DialogueStep>) {
    let kind = DialogueKind::GenericConfirm {
        action: action.to_string(),
    };
    let steps = vec![DialogueStep {
        prompt: format!("Confirm: <b>{action}</b>\nType <b>yes</b> to proceed."),
        expect: ExpectedInput::YesNo,
        index: 0,
    }];
    (kind, steps)
}

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
    fn test_forget_all_three_step_flow() {
        let mut mgr = DialogueManager::new(300);
        let (kind, steps) = forget_all_flow();

        let prompt = mgr.start(42, kind, steps).unwrap();
        assert!(prompt.contains("Step 1/3"));
        assert!(mgr.has_active(42));

        // Step 1: yes/no.
        match mgr.process_input(42, "yes") {
            DialogueOutcome::Continue(p) => assert!(p.contains("Step 2/3")),
            other => panic!("expected Continue, got {other:?}"),
        }

        // Step 2: exact phrase.
        match mgr.process_input(42, "DELETE ALL") {
            DialogueOutcome::Continue(p) => assert!(p.contains("Step 3/3")),
            other => panic!("expected Continue, got {other:?}"),
        }

        // Step 3: final yes.
        match mgr.process_input(42, "yes") {
            DialogueOutcome::Completed { kind, responses } => {
                assert_eq!(kind, DialogueKind::ForgetAll);
                assert_eq!(responses.len(), 3);
            },
            other => panic!("expected Completed, got {other:?}"),
        }

        assert!(!mgr.has_active(42));
    }

    #[test]
    fn test_wrong_exact_phrase_rejected() {
        let mut mgr = DialogueManager::new(300);
        let (kind, steps) = forget_all_flow();
        mgr.start(42, kind, steps);

        // Pass step 1.
        mgr.process_input(42, "yes");

        // Wrong phrase for step 2.
        match mgr.process_input(42, "wrong phrase") {
            DialogueOutcome::InvalidInput(msg) => {
                assert!(msg.contains("DELETE ALL"));
            },
            other => panic!("expected InvalidInput, got {other:?}"),
        }

        // Dialogue should still be active.
        assert!(mgr.has_active(42));
    }

    #[test]
    fn test_cancel_flow() {
        let mut mgr = DialogueManager::new(300);
        let (kind, steps) = forget_all_flow();
        mgr.start(42, kind, steps);

        match mgr.process_input(42, "/cancel") {
            DialogueOutcome::Cancelled => {},
            other => panic!("expected Cancelled, got {other:?}"),
        }

        assert!(!mgr.has_active(42));
    }

    #[test]
    fn test_no_answer_cancels_yes_no() {
        let mut mgr = DialogueManager::new(300);
        let (kind, steps) = forget_all_flow();
        mgr.start(42, kind, steps);

        match mgr.process_input(42, "no") {
            DialogueOutcome::Cancelled => {},
            other => panic!("expected Cancelled, got {other:?}"),
        }
    }

    #[test]
    fn test_generic_confirm() {
        let mut mgr = DialogueManager::new(300);
        let (kind, steps) = generic_confirm_flow("restart daemon");

        let prompt = mgr.start(42, kind, steps).unwrap();
        assert!(prompt.contains("restart daemon"));

        match mgr.process_input(42, "yes") {
            DialogueOutcome::Completed { kind, .. } => match kind {
                DialogueKind::GenericConfirm { action } => {
                    assert_eq!(action, "restart daemon");
                },
                other => panic!("expected GenericConfirm, got {other:?}"),
            },
            other => panic!("expected Completed, got {other:?}"),
        }
    }

    #[test]
    fn test_invalid_yes_no_input() {
        let mut mgr = DialogueManager::new(300);
        let (kind, steps) = forget_all_flow();
        mgr.start(42, kind, steps);

        match mgr.process_input(42, "maybe") {
            DialogueOutcome::InvalidInput(msg) => {
                assert!(msg.contains("yes"));
                assert!(msg.contains("no"));
            },
            other => panic!("expected InvalidInput, got {other:?}"),
        }
    }

    #[test]
    fn test_multiple_chats_independent() {
        let mut mgr = DialogueManager::new(300);
        let (k1, s1) = forget_all_flow();
        let (k2, s2) = generic_confirm_flow("test");

        mgr.start(1, k1, s1);
        mgr.start(2, k2, s2);

        assert!(mgr.has_active(1));
        assert!(mgr.has_active(2));

        mgr.process_input(2, "yes");
        assert!(mgr.has_active(1));
        assert!(!mgr.has_active(2));
    }
}
