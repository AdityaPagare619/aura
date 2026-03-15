//! Onboarding engine — orchestrates AURA's first-run experience.
//!
//! 7-phase onboarding:
//!
//! | Phase | Name                    | Description                                    |
//! |-------|-------------------------|------------------------------------------------|
//! | 1     | Introduction            | Welcome, explain what AURA is                  |
//! | 2     | PermissionSetup         | Request necessary OS permissions               |
//! | 3     | UserIntroduction        | Ask user's name, interests, preferences        |
//! | 4     | TelegramSetup           | Optional Telegram bot integration               |
//! | 5     | PersonalityCalibration  | Brief quiz → initial OCEAN score adjustments   |
//! | 6     | FirstActions            | Device calibration + interactive tutorial       |
//! | 7     | Completion              | Summary, set trust to Acquaintance, schedule    |
//!
//! Supports interruption/resumption at any phase boundary, skip option,
//! and persistent state so onboarding can survive app restarts.

use aura_types::{config::OnboardingConfig, errors::OnboardingError, identity::OceanTraits};
use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

use crate::{
    daemon_core::{
        calibration::{CalibrationEngine, CalibrationResult},
        tutorial::TutorialProgress,
    },
    identity::user_profile::UserProfile,
};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Number of personality calibration questions.
const DEFAULT_CALIBRATION_QUESTIONS: usize = 7;

/// Minimum OCEAN adjustment delta per question.
const OCEAN_DELTA_PER_QUESTION: f32 = 0.09;

/// Maximum number of calibration answers the daemon retains.
///
/// There are only DEFAULT_CALIBRATION_QUESTIONS questions; this cap prevents
/// unbounded growth if the caller submits duplicate/extra answers.
const MAX_CALIBRATION_ANSWERS: usize = DEFAULT_CALIBRATION_QUESTIONS * 2;

/// Maximum number of granted permissions the daemon tracks.
///
/// Android defines a bounded permission set; 64 is a generous hard ceiling.
const MAX_GRANTED_PERMISSIONS: usize = 64;

// ---------------------------------------------------------------------------
// Onboarding phase
// ---------------------------------------------------------------------------

/// The 7 phases of onboarding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OnboardingPhase {
    Introduction,
    PermissionSetup,
    UserIntroduction,
    TelegramSetup,
    PersonalityCalibration,
    FirstActions,
    Completion,
}

impl OnboardingPhase {
    /// Ordered list of all phases.
    pub fn all() -> &'static [OnboardingPhase] {
        &[
            Self::Introduction,
            Self::PermissionSetup,
            Self::UserIntroduction,
            Self::TelegramSetup,
            Self::PersonalityCalibration,
            Self::FirstActions,
            Self::Completion,
        ]
    }

    /// Phase index (0-based).
    pub fn index(&self) -> usize {
        match self {
            Self::Introduction => 0,
            Self::PermissionSetup => 1,
            Self::UserIntroduction => 2,
            Self::TelegramSetup => 3,
            Self::PersonalityCalibration => 4,
            Self::FirstActions => 5,
            Self::Completion => 6,
        }
    }

    /// Phase name as a human-readable string.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Introduction => "Introduction",
            Self::PermissionSetup => "Permission Setup",
            Self::UserIntroduction => "User Introduction",
            Self::TelegramSetup => "Telegram Setup",
            Self::PersonalityCalibration => "Personality Calibration",
            Self::FirstActions => "First Actions",
            Self::Completion => "Completion",
        }
    }

    /// Next phase (None if at Completion).
    pub fn next(&self) -> Option<OnboardingPhase> {
        let phases = Self::all();
        let idx = self.index();
        phases.get(idx + 1).copied()
    }
}

// ---------------------------------------------------------------------------
// Personality calibration question
// ---------------------------------------------------------------------------

/// A personality calibration question with two opposing poles.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CalibrationQuestion {
    /// Question text.
    pub text: String,
    /// Label for the "low" end of the spectrum.
    pub pole_low: String,
    /// Label for the "high" end of the spectrum.
    pub pole_high: String,
    /// Which OCEAN trait this question targets.
    pub target_trait: OceanTrait,
}

/// Which OCEAN trait a calibration question maps to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OceanTrait {
    Openness,
    Conscientiousness,
    Extraversion,
    Agreeableness,
    Neuroticism,
}

/// User's answer to a calibration question (1–5 Likert scale).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CalibrationAnswer {
    /// Question index.
    pub question_index: usize,
    /// Answer value: 1 = strongly pole_low, 5 = strongly pole_high.
    pub value: u8,
}

// ---------------------------------------------------------------------------
// Onboarding state (persistent)
// ---------------------------------------------------------------------------

/// Persistent onboarding state — survives app restarts.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OnboardingState {
    /// Current phase.
    pub phase: OnboardingPhase,
    /// Whether onboarding is fully complete.
    pub completed: bool,
    /// Whether onboarding was skipped.
    pub skipped: bool,
    /// User profile (built up during onboarding).
    pub user_profile: UserProfile,
    /// Calibration answers collected so far.
    pub calibration_answers: Vec<CalibrationAnswer>,
    /// Calibration result (from device profiling).
    pub calibration_result: Option<CalibrationResult>,
    /// Tutorial progress.
    pub tutorial_progress: TutorialProgress,
    /// Whether Telegram was set up.
    pub telegram_setup_done: bool,
    /// Permissions that were granted.
    pub granted_permissions: Vec<String>,
    /// Timestamp of onboarding start (ms).
    pub started_at_ms: u64,
    /// Timestamp of last state change (ms).
    pub updated_at_ms: u64,
}

impl Default for OnboardingState {
    fn default() -> Self {
        Self {
            phase: OnboardingPhase::Introduction,
            completed: false,
            skipped: false,
            user_profile: UserProfile::default(),
            calibration_answers: Vec::new(),
            calibration_result: None,
            tutorial_progress: TutorialProgress::default(),
            telegram_setup_done: false,
            granted_permissions: Vec::new(),
            started_at_ms: 0,
            updated_at_ms: 0,
        }
    }
}

// ---------------------------------------------------------------------------
// Phase result
// ---------------------------------------------------------------------------

/// Result of completing a single onboarding phase.
#[derive(Debug)]
pub struct PhaseResult {
    /// The phase that was completed.
    pub phase: OnboardingPhase,
    /// Whether the phase completed successfully.
    pub success: bool,
    /// Message to display to the user.
    pub message: String,
}

// ---------------------------------------------------------------------------
// OnboardingEngine
// ---------------------------------------------------------------------------

/// The main onboarding orchestrator.
///
/// Manages the 7-phase onboarding flow, personality calibration,
/// device calibration, and tutorial.
pub struct OnboardingEngine {
    /// Current state (persistent).
    state: OnboardingState,
    /// Configuration.
    config: OnboardingConfig,
    /// Calibration questions.
    questions: Vec<CalibrationQuestion>,
}

impl OnboardingEngine {
    /// Create a new onboarding engine for a fresh start.
    pub fn new(config: OnboardingConfig, now_ms: u64) -> Self {
        let state = OnboardingState {
            started_at_ms: now_ms,
            updated_at_ms: now_ms,
            ..Default::default()
        };

        Self {
            state,
            config,
            questions: Self::build_calibration_questions(),
        }
    }

    /// Resume onboarding from a saved state.
    pub fn resume(state: OnboardingState, config: OnboardingConfig) -> Self {
        Self {
            state,
            config,
            questions: Self::build_calibration_questions(),
        }
    }

    /// Get the current onboarding state.
    pub fn state(&self) -> &OnboardingState {
        &self.state
    }

    /// Is onboarding complete (or skipped)?
    pub fn is_done(&self) -> bool {
        self.state.completed || self.state.skipped
    }

    /// Get the current phase.
    pub fn current_phase(&self) -> OnboardingPhase {
        self.state.phase
    }

    /// Get the calibration questions.
    pub fn calibration_questions(&self) -> &[CalibrationQuestion] {
        &self.questions
    }

    /// Get phase progress as percentage (0–100).
    pub fn progress_percent(&self) -> u8 {
        if self.is_done() {
            return 100;
        }
        let total = OnboardingPhase::all().len() as u8;
        let current = self.state.phase.index() as u8;
        (current * 100) / total
    }

    // -----------------------------------------------------------------------
    // Phase execution
    // -----------------------------------------------------------------------

    /// Execute the Introduction phase.
    pub fn run_introduction(&mut self, now_ms: u64) -> Result<PhaseResult, OnboardingError> {
        self.expect_phase(OnboardingPhase::Introduction)?;

        let message = "Hey there! I'm AURA — your personal AI companion. \
            I live right here on your phone, learning how to help you better \
            every day. Everything stays private and on-device.\n\n\
            Let me get to know you a bit so I can be genuinely useful. \
            This'll only take a few minutes, and you can skip any part."
            .to_string();

        self.advance_phase(now_ms);
        info!("onboarding: introduction complete");

        Ok(PhaseResult {
            phase: OnboardingPhase::Introduction,
            success: true,
            message,
        })
    }

    /// Execute the Permission Setup phase.
    ///
    /// The `granted` list should contain permission identifiers the user granted.
    pub fn run_permission_setup(
        &mut self,
        granted: Vec<String>,
        now_ms: u64,
    ) -> Result<PhaseResult, OnboardingError> {
        self.expect_phase(OnboardingPhase::PermissionSetup)?;

        self.state.granted_permissions =
            granted.into_iter().take(MAX_GRANTED_PERMISSIONS).collect();
        let count = self.state.granted_permissions.len();

        let message = if count == 0 {
            "No worries — I can work with limited access for now. \
                You can always grant more permissions later in Settings."
                .to_string()
        } else {
            format!(
                "Thanks! I now have {count} permission{} to help you better. \
                You can change these anytime.",
                if count == 1 { "" } else { "s" }
            )
        };

        self.advance_phase(now_ms);
        info!(permissions = count, "onboarding: permissions set up");

        Ok(PhaseResult {
            phase: OnboardingPhase::PermissionSetup,
            success: true,
            message,
        })
    }

    /// Execute the User Introduction phase.
    ///
    /// Collects the user's name, interests, and preferences.
    pub fn run_user_introduction(
        &mut self,
        name: &str,
        interests: &[String],
        preferences: Option<crate::identity::user_profile::UserPreferences>,
        now_ms: u64,
    ) -> Result<PhaseResult, OnboardingError> {
        self.expect_phase(OnboardingPhase::UserIntroduction)?;

        // Set name.
        self.state.user_profile = UserProfile::new(name, now_ms)?;

        // Add interests.
        for interest in interests {
            self.state.user_profile.add_interest(interest, now_ms);
        }

        // Apply preferences if provided.
        if let Some(prefs) = preferences {
            self.state.user_profile.preferences = prefs;
        }

        let greeting = if name.trim().is_empty() {
            "No name? That's fine — I'll just call you 'friend' for now!".to_string()
        } else {
            format!(
                "Nice to meet you, {}! I'll remember your preferences.",
                name.trim()
            )
        };

        self.advance_phase(now_ms);
        info!(name = %name.trim(), interests = interests.len(), "onboarding: user introduced");

        Ok(PhaseResult {
            phase: OnboardingPhase::UserIntroduction,
            success: true,
            message: greeting,
        })
    }

    /// Execute the Telegram Setup phase.
    ///
    /// Pass `true` for `setup_complete` if the user configured Telegram.
    pub fn run_telegram_setup(
        &mut self,
        setup_complete: bool,
        now_ms: u64,
    ) -> Result<PhaseResult, OnboardingError> {
        self.expect_phase(OnboardingPhase::TelegramSetup)?;

        self.state.telegram_setup_done = setup_complete;

        let message = if setup_complete {
            "Telegram's all set! You can chat with me there too.".to_string()
        } else {
            "No problem — you can set up Telegram later if you want.".to_string()
        };

        self.advance_phase(now_ms);
        info!(setup = setup_complete, "onboarding: telegram setup");

        Ok(PhaseResult {
            phase: OnboardingPhase::TelegramSetup,
            success: true,
            message,
        })
    }

    /// Execute the Personality Calibration phase.
    ///
    /// Takes the user's answers to the calibration quiz and computes
    /// initial OCEAN score adjustments.
    pub fn run_personality_calibration(
        &mut self,
        answers: Vec<CalibrationAnswer>,
        now_ms: u64,
    ) -> Result<PhaseResult, OnboardingError> {
        self.expect_phase(OnboardingPhase::PersonalityCalibration)?;

        // Validate answers.
        for answer in &answers {
            if answer.value < 1 || answer.value > 5 {
                return Err(OnboardingError::PhaseFailed {
                    phase: "PersonalityCalibration".into(),
                    reason: format!(
                        "answer value {} out of range (1–5) for question {}",
                        answer.value, answer.question_index
                    ),
                });
            }
            if answer.question_index >= self.questions.len() {
                return Err(OnboardingError::PhaseFailed {
                    phase: "PersonalityCalibration".into(),
                    reason: format!(
                        "question index {} out of range (max {})",
                        answer.question_index,
                        self.questions.len() - 1
                    ),
                });
            }
        }

        self.state.calibration_answers = answers
            .iter()
            .take(MAX_CALIBRATION_ANSWERS).cloned()
            .collect();

        // Compute OCEAN adjustments — mechanical linear mapping of slider
        // values to f32 deltas; no intelligence or routing decision here.
        let ocean = self.compute_ocean_adjustments(&answers);
        self.state
            .user_profile
            .apply_ocean_calibration(ocean, now_ms);

        // THEATER AGI NOTE: We do NOT describe the personality in Rust.
        // The neocortex receives the raw OceanTraits via identity state and
        // decides how to phrase its own behaviour adjustments in natural language.
        let message = "Got it! I've noted your preferences. \
            I'll keep fine-tuning as I get to know you better."
            .to_string();

        self.advance_phase(now_ms);
        info!("onboarding: personality calibration complete");

        Ok(PhaseResult {
            phase: OnboardingPhase::PersonalityCalibration,
            success: true,
            message,
        })
    }

    /// Execute the First Actions phase (device calibration + tutorial start).
    pub fn run_first_actions(&mut self, now_ms: u64) -> Result<PhaseResult, OnboardingError> {
        self.expect_phase(OnboardingPhase::FirstActions)?;

        // Run device calibration.
        let calibration_engine = CalibrationEngine::new(self.config.benchmark_enabled);
        let cal_result = calibration_engine.run(now_ms)?;

        let tier_desc = cal_result.model_tier.description();
        self.state.calibration_result = Some(cal_result);

        let message = format!(
            "I've checked out your device — running in {} mode. \
            Let me show you a few things I can do!",
            tier_desc
        );

        self.advance_phase(now_ms);
        info!("onboarding: first actions complete");

        Ok(PhaseResult {
            phase: OnboardingPhase::FirstActions,
            success: true,
            message,
        })
    }

    /// Execute the Completion phase.
    ///
    /// Finalises onboarding: marks profile complete, returns summary.
    pub fn run_completion(&mut self, now_ms: u64) -> Result<PhaseResult, OnboardingError> {
        self.expect_phase(OnboardingPhase::Completion)?;

        self.state.user_profile.complete_onboarding(now_ms);
        self.state.completed = true;
        self.state.updated_at_ms = now_ms;

        let name = if self.state.user_profile.name.is_empty() {
            "friend"
        } else {
            &self.state.user_profile.name
        };

        let message = format!(
            "We're all set, {name}! Here's what I'll do next:\n\
            \n\
            - I'll start learning your patterns over the next few days\n\
            - Tomorrow morning, expect your first briefing around {}:00\n\
            - Just talk to me anytime — no special commands needed\n\
            \n\
            Welcome aboard!",
            self.state.user_profile.preferences.morning_briefing_hour
        );

        info!(
            name = %name,
            interests = self.state.user_profile.interests.len(),
            telegram = self.state.telegram_setup_done,
            permissions = self.state.granted_permissions.len(),
            "onboarding complete"
        );

        Ok(PhaseResult {
            phase: OnboardingPhase::Completion,
            success: true,
            message,
        })
    }

    /// Skip the entire onboarding.
    pub fn skip_all(&mut self, now_ms: u64) -> Result<PhaseResult, OnboardingError> {
        if !self.config.skip_allowed {
            return Err(OnboardingError::PhaseFailed {
                phase: "skip".into(),
                reason: "skipping onboarding is not allowed by configuration".into(),
            });
        }

        // Create minimal profile.
        self.state.user_profile = UserProfile::new("", now_ms)?;
        self.state.user_profile.complete_onboarding(now_ms);
        self.state.skipped = true;
        self.state.completed = true;
        self.state.updated_at_ms = now_ms;

        warn!("onboarding skipped by user");

        Ok(PhaseResult {
            phase: self.state.phase,
            success: true,
            message: "Onboarding skipped. You can always set things up later \
                in Settings. I'll figure things out as we go!"
                .to_string(),
        })
    }

    // -----------------------------------------------------------------------
    // OCEAN computation
    // -----------------------------------------------------------------------

    /// Compute OCEAN adjustments from calibration answers.
    ///
    /// Each answer (1–5) maps to a delta from the default:
    /// - 1 = −2δ, 2 = −1δ, 3 = 0, 4 = +1δ, 5 = +2δ
    /// where δ = OCEAN_DELTA_PER_QUESTION.
    fn compute_ocean_adjustments(&self, answers: &[CalibrationAnswer]) -> OceanTraits {
        let mut adj = OceanTraits::DEFAULT;

        for answer in answers {
            if answer.question_index >= self.questions.len() {
                continue;
            }
            let q = &self.questions[answer.question_index];
            let delta = (answer.value as f32 - 3.0) * OCEAN_DELTA_PER_QUESTION;

            match q.target_trait {
                OceanTrait::Openness => adj.openness += delta,
                OceanTrait::Conscientiousness => adj.conscientiousness += delta,
                OceanTrait::Extraversion => adj.extraversion += delta,
                OceanTrait::Agreeableness => adj.agreeableness += delta,
                OceanTrait::Neuroticism => adj.neuroticism += delta,
            }
        }

        adj.clamp_all();
        adj
    }

    // -----------------------------------------------------------------------
    // Phase management helpers
    // -----------------------------------------------------------------------

    /// Verify we're at the expected phase.
    fn expect_phase(&self, expected: OnboardingPhase) -> Result<(), OnboardingError> {
        if self.state.completed {
            return Err(OnboardingError::AlreadyCompleted);
        }
        if self.state.phase != expected {
            return Err(OnboardingError::PhaseFailed {
                phase: expected.name().into(),
                reason: format!(
                    "expected phase {:?} but currently at {:?}",
                    expected, self.state.phase
                ),
            });
        }
        Ok(())
    }

    /// Advance to the next phase.
    fn advance_phase(&mut self, now_ms: u64) {
        if let Some(next) = self.state.phase.next() {
            debug!(
                from = ?self.state.phase,
                to = ?next,
                "onboarding phase advanced"
            );
            self.state.phase = next;
        }
        self.state.updated_at_ms = now_ms;
    }

    // -----------------------------------------------------------------------
    // Persistence
    // -----------------------------------------------------------------------

    /// Save onboarding state to SQLite.
    pub fn save_state(&self, db: &rusqlite::Connection) -> Result<(), OnboardingError> {
        db.execute_batch(
            "CREATE TABLE IF NOT EXISTS onboarding_state (
                id INTEGER PRIMARY KEY CHECK (id = 1),
                data BLOB NOT NULL,
                updated_at_ms INTEGER NOT NULL
            );",
        )
        .map_err(|e| OnboardingError::PersistenceFailed(format!("create table: {e}")))?;

        let json = serde_json::to_vec(&self.state)
            .map_err(|e| OnboardingError::PersistenceFailed(format!("serialize: {e}")))?;

        db.execute(
            "INSERT INTO onboarding_state (id, data, updated_at_ms)
             VALUES (1, ?1, ?2)
             ON CONFLICT(id) DO UPDATE SET data = ?1, updated_at_ms = ?2;",
            rusqlite::params![json, self.state.updated_at_ms as i64],
        )
        .map_err(|e| OnboardingError::PersistenceFailed(format!("upsert: {e}")))?;

        debug!("onboarding state saved");
        Ok(())
    }

    /// Load onboarding state from SQLite.
    pub fn load_state(
        db: &rusqlite::Connection,
    ) -> Result<Option<OnboardingState>, OnboardingError> {
        let table_exists: bool = db
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='onboarding_state';",
                [],
                |row| row.get::<_, i64>(0),
            )
            .map(|c| c > 0)
            .map_err(|e| OnboardingError::PersistenceFailed(format!("check table: {e}")))?;

        if !table_exists {
            return Ok(None);
        }

        let result: Result<Vec<u8>, _> = db.query_row(
            "SELECT data FROM onboarding_state WHERE id = 1;",
            [],
            |row| row.get(0),
        );

        match result {
            Ok(data) => {
                let state: OnboardingState = serde_json::from_slice(&data)
                    .map_err(|e| OnboardingError::PersistenceFailed(format!("deserialize: {e}")))?;
                Ok(Some(state))
            },
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(OnboardingError::PersistenceFailed(format!("load: {e}"))),
        }
    }

    // -----------------------------------------------------------------------
    // Calibration question builder
    // -----------------------------------------------------------------------

    fn build_calibration_questions() -> Vec<CalibrationQuestion> {
        vec![
            CalibrationQuestion {
                text: "When I help you with something, would you prefer I stick to the \
                    tried-and-true approach, or explore creative alternatives?"
                    .into(),
                pole_low: "Stick to what works".into(),
                pole_high: "Explore creative options".into(),
                target_trait: OceanTrait::Openness,
            },
            CalibrationQuestion {
                text: "Should I be proactive about reminders and scheduling, or let you \
                    take the lead?"
                    .into(),
                pole_low: "I'll manage my own schedule".into(),
                pole_high: "Be proactive with reminders".into(),
                target_trait: OceanTrait::Conscientiousness,
            },
            CalibrationQuestion {
                text: "How chatty should I be? Short and direct, or more conversational?".into(),
                pole_low: "Short and direct".into(),
                pole_high: "More conversational".into(),
                target_trait: OceanTrait::Extraversion,
            },
            CalibrationQuestion {
                text: "When you're making a decision, should I challenge your thinking \
                    or be more supportive?"
                    .into(),
                pole_low: "Challenge me — be honest".into(),
                pole_high: "Be supportive and encouraging".into(),
                target_trait: OceanTrait::Agreeableness,
            },
            CalibrationQuestion {
                text: "How cautious should I be about taking actions on your behalf?".into(),
                pole_low: "I trust you — go ahead if you're confident".into(),
                pole_high: "Very cautious — always ask first".into(),
                target_trait: OceanTrait::Neuroticism,
            },
            CalibrationQuestion {
                text: "Would you like me to suggest new things to try (apps, music, \
                    restaurants), or focus on what you already enjoy?"
                    .into(),
                pole_low: "Focus on what I like".into(),
                pole_high: "Suggest new discoveries".into(),
                target_trait: OceanTrait::Openness,
            },
            CalibrationQuestion {
                text: "Should I include a bit of humour and personality, or keep things \
                    strictly professional?"
                    .into(),
                pole_low: "Keep it professional".into(),
                pole_high: "Bring on the personality".into(),
                target_trait: OceanTrait::Extraversion,
            },
        ]
    }
}

// ---------------------------------------------------------------------------
// Detect first run
// ---------------------------------------------------------------------------

/// Check whether this is a first run (no onboarding state exists).
pub fn is_first_run(db: &rusqlite::Connection) -> Result<bool, OnboardingError> {
    match OnboardingEngine::load_state(db)? {
        None => Ok(true),
        Some(state) => Ok(!state.completed && !state.skipped),
    }
}

/// Check whether onboarding was interrupted (started but not finished).
pub fn is_interrupted(db: &rusqlite::Connection) -> Result<bool, OnboardingError> {
    match OnboardingEngine::load_state(db)? {
        None => Ok(false),
        Some(state) => Ok(!state.completed && !state.skipped && state.started_at_ms > 0),
    }
}

impl OnboardingEngine {
    /// Check onboarding status from the database — used by startup to decide
    /// what to do after the 7-phase boot sequence completes.
    pub fn check_status(
        db: &rusqlite::Connection,
    ) -> Result<crate::daemon_core::startup::OnboardingStatus, OnboardingError> {
        use crate::daemon_core::startup::OnboardingStatus;

        match Self::load_state(db)? {
            None => Ok(OnboardingStatus::FirstRun),
            Some(state) if state.completed || state.skipped => Ok(OnboardingStatus::Completed),
            Some(state) if state.started_at_ms > 0 => Ok(OnboardingStatus::Interrupted),
            Some(_) => Ok(OnboardingStatus::FirstRun),
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use aura_types::config::OnboardingConfig;

    use super::*;

    fn default_engine(now_ms: u64) -> OnboardingEngine {
        OnboardingEngine::new(OnboardingConfig::default(), now_ms)
    }

    #[test]
    fn test_new_engine() {
        let engine = default_engine(1000);
        assert!(!engine.is_done());
        assert_eq!(engine.current_phase(), OnboardingPhase::Introduction);
        assert_eq!(engine.progress_percent(), 0);
    }

    #[test]
    fn test_phase_ordering() {
        let phases = OnboardingPhase::all();
        assert_eq!(phases.len(), 7);
        assert_eq!(phases[0], OnboardingPhase::Introduction);
        assert_eq!(phases[6], OnboardingPhase::Completion);
    }

    #[test]
    fn test_phase_next() {
        assert_eq!(
            OnboardingPhase::Introduction.next(),
            Some(OnboardingPhase::PermissionSetup)
        );
        assert_eq!(OnboardingPhase::Completion.next(), None);
    }

    #[test]
    fn test_run_introduction() {
        let mut engine = default_engine(1000);
        let result = engine.run_introduction(2000).expect("intro");
        assert!(result.success);
        assert!(!result.message.is_empty());
        assert_eq!(engine.current_phase(), OnboardingPhase::PermissionSetup);
    }

    #[test]
    fn test_run_permission_setup() {
        let mut engine = default_engine(1000);
        engine.run_introduction(2000).expect("intro");

        let result = engine
            .run_permission_setup(vec!["notifications".into(), "accessibility".into()], 3000)
            .expect("permissions");
        assert!(result.success);
        assert!(result.message.contains("2"));
        assert_eq!(engine.current_phase(), OnboardingPhase::UserIntroduction);
    }

    #[test]
    fn test_run_permission_setup_empty() {
        let mut engine = default_engine(1000);
        engine.run_introduction(2000).expect("intro");

        let result = engine
            .run_permission_setup(vec![], 3000)
            .expect("permissions");
        assert!(result.success);
        assert!(result.message.contains("limited"));
    }

    #[test]
    fn test_run_user_introduction() {
        let mut engine = default_engine(1000);
        engine.run_introduction(2000).expect("intro");
        engine
            .run_permission_setup(vec![], 3000)
            .expect("permissions");

        let result = engine
            .run_user_introduction("Alice", &["coding".into(), "music".into()], None, 4000)
            .expect("user intro");
        assert!(result.success);
        assert!(result.message.contains("Alice"));
        assert_eq!(engine.state().user_profile.name, "Alice");
        assert_eq!(engine.state().user_profile.interests.len(), 2);
    }

    #[test]
    fn test_run_user_introduction_empty_name() {
        let mut engine = default_engine(1000);
        engine.run_introduction(2000).expect("intro");
        engine.run_permission_setup(vec![], 3000).expect("perm");

        let result = engine
            .run_user_introduction("", &[], None, 4000)
            .expect("intro");
        assert!(result.message.contains("friend"));
    }

    #[test]
    fn test_run_telegram_setup() {
        let mut engine = default_engine(1000);
        engine.run_introduction(2000).expect("intro");
        engine.run_permission_setup(vec![], 3000).expect("perm");
        engine
            .run_user_introduction("Bob", &[], None, 4000)
            .expect("user");

        let result = engine.run_telegram_setup(true, 5000).expect("telegram");
        assert!(result.success);
        assert!(result.message.contains("Telegram"));
        assert!(engine.state().telegram_setup_done);
    }

    #[test]
    fn test_run_personality_calibration() {
        let mut engine = default_engine(1000);
        engine.run_introduction(2000).expect("intro");
        engine.run_permission_setup(vec![], 3000).expect("perm");
        engine
            .run_user_introduction("Cal", &[], None, 4000)
            .expect("user");
        engine.run_telegram_setup(false, 5000).expect("telegram");

        let answers: Vec<CalibrationAnswer> = (0..7)
            .map(|i| CalibrationAnswer {
                question_index: i,
                value: 4, // slightly toward "high" pole
            })
            .collect();

        let result = engine
            .run_personality_calibration(answers, 6000)
            .expect("calibration");
        assert!(result.success);
        assert!(!result.message.is_empty());
    }

    #[test]
    fn test_personality_calibration_invalid_value() {
        let mut engine = default_engine(1000);
        engine.run_introduction(2000).expect("intro");
        engine.run_permission_setup(vec![], 3000).expect("perm");
        engine
            .run_user_introduction("X", &[], None, 4000)
            .expect("user");
        engine.run_telegram_setup(false, 5000).expect("telegram");

        let bad_answers = vec![CalibrationAnswer {
            question_index: 0,
            value: 6, // out of range
        }];

        let err = engine
            .run_personality_calibration(bad_answers, 6000)
            .unwrap_err();
        assert!(matches!(err, OnboardingError::PhaseFailed { .. }));
    }

    #[test]
    fn test_run_first_actions() {
        let mut engine = default_engine(1000);
        engine.run_introduction(2000).expect("intro");
        engine.run_permission_setup(vec![], 3000).expect("perm");
        engine
            .run_user_introduction("FA", &[], None, 4000)
            .expect("user");
        engine.run_telegram_setup(false, 5000).expect("telegram");
        let answers: Vec<CalibrationAnswer> = (0..7)
            .map(|i| CalibrationAnswer {
                question_index: i,
                value: 3,
            })
            .collect();
        engine
            .run_personality_calibration(answers, 6000)
            .expect("cal");

        let result = engine.run_first_actions(7000).expect("first actions");
        assert!(result.success);
        assert!(engine.state().calibration_result.is_some());
    }

    #[test]
    fn test_run_completion() {
        let mut engine = default_engine(1000);
        engine.run_introduction(2000).expect("intro");
        engine.run_permission_setup(vec![], 3000).expect("perm");
        engine
            .run_user_introduction("Done", &[], None, 4000)
            .expect("user");
        engine.run_telegram_setup(false, 5000).expect("telegram");
        let answers: Vec<CalibrationAnswer> = (0..7)
            .map(|i| CalibrationAnswer {
                question_index: i,
                value: 3,
            })
            .collect();
        engine
            .run_personality_calibration(answers, 6000)
            .expect("cal");
        engine.run_first_actions(7000).expect("first");

        let result = engine.run_completion(8000).expect("completion");
        assert!(result.success);
        assert!(result.message.contains("Done"));
        assert!(engine.is_done());
        assert!(engine.state().user_profile.onboarding_completed);
    }

    #[test]
    fn test_full_onboarding_flow() {
        let mut engine = default_engine(1000);

        engine.run_introduction(2000).expect("p1");
        engine
            .run_permission_setup(vec!["notifications".into()], 3000)
            .expect("p2");
        engine
            .run_user_introduction("Full", &["tech".into()], None, 4000)
            .expect("p3");
        engine.run_telegram_setup(true, 5000).expect("p4");

        let answers: Vec<CalibrationAnswer> = (0..7)
            .map(|i| CalibrationAnswer {
                question_index: i,
                value: 4,
            })
            .collect();
        engine
            .run_personality_calibration(answers, 6000)
            .expect("p5");
        engine.run_first_actions(7000).expect("p6");
        engine.run_completion(8000).expect("p7");

        assert!(engine.is_done());
        assert_eq!(engine.progress_percent(), 100);
    }

    #[test]
    fn test_skip_all() {
        let mut engine = default_engine(1000);
        let result = engine.skip_all(2000).expect("skip");
        assert!(result.success);
        assert!(engine.is_done());
        assert!(engine.state().skipped);
    }

    #[test]
    fn test_skip_not_allowed() {
        let mut config = OnboardingConfig::default();
        config.skip_allowed = false;
        let mut engine = OnboardingEngine::new(config, 1000);
        let err = engine.skip_all(2000).unwrap_err();
        assert!(matches!(err, OnboardingError::PhaseFailed { .. }));
    }

    #[test]
    fn test_wrong_phase_error() {
        let mut engine = default_engine(1000);
        // Try to run phase 3 without completing phases 1-2.
        let err = engine
            .run_user_introduction("Wrong", &[], None, 2000)
            .unwrap_err();
        assert!(matches!(err, OnboardingError::PhaseFailed { .. }));
    }

    #[test]
    fn test_already_completed_error() {
        let mut engine = default_engine(1000);
        engine.skip_all(2000).expect("skip");
        let err = engine.run_introduction(3000).unwrap_err();
        assert!(matches!(err, OnboardingError::AlreadyCompleted));
    }

    #[test]
    fn test_calibration_questions_exist() {
        let engine = default_engine(1000);
        assert!(!engine.calibration_questions().is_empty());
        assert_eq!(
            engine.calibration_questions().len(),
            DEFAULT_CALIBRATION_QUESTIONS
        );
    }

    #[test]
    fn test_ocean_adjustments_computation() {
        let engine = default_engine(1000);
        // All answers at 3 (neutral) should keep defaults.
        let neutral_answers: Vec<CalibrationAnswer> = (0..7)
            .map(|i| CalibrationAnswer {
                question_index: i,
                value: 3,
            })
            .collect();
        let adj = engine.compute_ocean_adjustments(&neutral_answers);
        assert!((adj.openness - OceanTraits::DEFAULT.openness).abs() < 0.01);
    }

    #[test]
    fn test_ocean_adjustments_high_answers() {
        let engine = default_engine(1000);
        let high_answers: Vec<CalibrationAnswer> = (0..7)
            .map(|i| CalibrationAnswer {
                question_index: i,
                value: 5,
            })
            .collect();
        let adj = engine.compute_ocean_adjustments(&high_answers);
        // With all high answers, traits should shift upward.
        assert!(adj.openness >= OceanTraits::DEFAULT.openness);
        assert!(adj.extraversion >= OceanTraits::DEFAULT.extraversion);
    }

    #[test]
    fn test_db_save_load_state() {
        let db = rusqlite::Connection::open_in_memory().expect("open db");
        let mut engine = default_engine(1000);
        engine.run_introduction(2000).expect("intro");
        engine.save_state(&db).expect("save");

        let loaded = OnboardingEngine::load_state(&db)
            .expect("load")
            .expect("should exist");
        assert_eq!(loaded.phase, OnboardingPhase::PermissionSetup);
    }

    #[test]
    fn test_db_load_no_table() {
        let db = rusqlite::Connection::open_in_memory().expect("open db");
        let loaded = OnboardingEngine::load_state(&db).expect("load");
        assert!(loaded.is_none());
    }

    #[test]
    fn test_is_first_run_empty_db() {
        let db = rusqlite::Connection::open_in_memory().expect("open db");
        assert!(is_first_run(&db).expect("check"));
    }

    #[test]
    fn test_is_first_run_completed() {
        let db = rusqlite::Connection::open_in_memory().expect("open db");
        let mut engine = default_engine(1000);
        engine.skip_all(2000).expect("skip");
        engine.save_state(&db).expect("save");
        assert!(!is_first_run(&db).expect("check"));
    }

    #[test]
    fn test_is_interrupted() {
        let db = rusqlite::Connection::open_in_memory().expect("open db");
        let mut engine = default_engine(1000);
        engine.run_introduction(2000).expect("intro");
        engine.save_state(&db).expect("save");
        assert!(is_interrupted(&db).expect("check"));
    }

    #[test]
    fn test_resume_onboarding() {
        let db = rusqlite::Connection::open_in_memory().expect("open db");
        let mut engine = default_engine(1000);
        engine.run_introduction(2000).expect("intro");
        engine.save_state(&db).expect("save");

        let state = OnboardingEngine::load_state(&db)
            .expect("load")
            .expect("exists");
        let mut resumed = OnboardingEngine::resume(state, OnboardingConfig::default());
        assert_eq!(resumed.current_phase(), OnboardingPhase::PermissionSetup);

        // Continue from where we left off.
        let result = resumed
            .run_permission_setup(vec![], 3000)
            .expect("permissions");
        assert!(result.success);
    }
}
