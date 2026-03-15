//! Interactive tutorial system — teaches the user how to interact with AURA.
//!
//! Supports three step types:
//! - **Demonstrate**: AURA shows what it can do (user watches).
//! - **Interactive**: User tries an action with AURA's guidance.
//! - **Quiz**: Quick comprehension check (non-punitive).
//!
//! The tutorial supports interruption/resumption and tracks progress
//! persistently so the user can continue where they left off.

use aura_types::errors::OnboardingError;
use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum number of tutorial modules.
const MAX_MODULES: usize = 20;

/// Maximum number of steps per tutorial module.
/// Caps `step_results` at `MAX_MODULES * MAX_STEPS_PER_MODULE = 400` entries.
const MAX_STEPS_PER_MODULE: usize = 20;

/// Maximum retry attempts for a failed interactive step.
#[allow(dead_code)] // Phase 8: used by tutorial step retry loop
const MAX_STEP_RETRIES: u8 = 3;

// ---------------------------------------------------------------------------
// Step types
// ---------------------------------------------------------------------------

/// The kind of tutorial step.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum StepKind {
    /// AURA demonstrates a capability — user watches.
    Demonstrate,
    /// User tries an action with AURA's guidance.
    Interactive,
    /// Quick comprehension quiz (non-punitive, always advances).
    Quiz,
}

/// A single step within a tutorial module.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TutorialStep {
    /// Unique step ID within the module.
    pub id: String,
    /// What kind of step this is.
    pub kind: StepKind,
    /// Title shown to the user.
    pub title: String,
    /// Instructional content / what AURA says.
    pub content: String,
    /// Hint text for interactive/quiz steps.
    pub hint: Option<String>,
    /// For quiz steps: the correct answer index (0-based).
    pub correct_answer: Option<usize>,
    /// For quiz steps: answer choices.
    pub choices: Vec<String>,
    /// Expected duration (ms).
    pub expected_duration_ms: u64,
}

/// Result of completing a tutorial step.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepResult {
    /// Step ID.
    pub step_id: String,
    /// Whether the step was completed successfully.
    pub success: bool,
    /// Time spent on this step (ms).
    pub duration_ms: u64,
    /// For quiz steps: which answer the user chose.
    pub chosen_answer: Option<usize>,
    /// Number of attempts (for interactive steps).
    pub attempts: u8,
}

// ---------------------------------------------------------------------------
// Tutorial module
// ---------------------------------------------------------------------------

/// A tutorial module — a themed collection of steps.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TutorialModule {
    /// Unique module ID.
    pub id: String,
    /// Module title.
    pub title: String,
    /// Module description.
    pub description: String,
    /// Ordered steps in this module.
    pub steps: Vec<TutorialStep>,
    /// Whether this module is required or optional.
    pub required: bool,
}

// ---------------------------------------------------------------------------
// Tutorial progress
// ---------------------------------------------------------------------------

/// Persistent progress tracking for the tutorial system.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TutorialProgress {
    /// Module ID of the current module (empty = not started).
    pub current_module: String,
    /// Index of the current step within the current module.
    pub current_step_index: usize,
    /// Completed module IDs.
    pub completed_modules: Vec<String>,
    /// Results from completed steps.
    pub step_results: Vec<StepResult>,
    /// Total time spent in tutorials (ms).
    pub total_time_ms: u64,
    /// Whether the tutorial system is fully complete.
    pub all_complete: bool,
    /// Timestamp of last progress update (ms).
    pub updated_at_ms: u64,
}

impl Default for TutorialProgress {
    fn default() -> Self {
        Self {
            current_module: String::new(),
            current_step_index: 0,
            completed_modules: Vec::new(),
            step_results: Vec::new(),
            total_time_ms: 0,
            all_complete: false,
            updated_at_ms: 0,
        }
    }
}

// ---------------------------------------------------------------------------
// TutorialEngine
// ---------------------------------------------------------------------------

/// Engine that manages the interactive tutorial experience.
#[derive(Debug)]
pub struct TutorialEngine {
    /// Available tutorial modules.
    modules: Vec<TutorialModule>,
    /// Current progress.
    progress: TutorialProgress,
}

impl TutorialEngine {
    /// Create a new tutorial engine with the default module set.
    pub fn new() -> Self {
        Self {
            modules: Self::build_default_modules(),
            progress: TutorialProgress::default(),
        }
    }

    /// Create a tutorial engine with existing progress (resume).
    pub fn with_progress(progress: TutorialProgress) -> Self {
        Self {
            modules: Self::build_default_modules(),
            progress,
        }
    }

    /// Get the current progress.
    pub fn progress(&self) -> &TutorialProgress {
        &self.progress
    }

    /// Check if the tutorial is fully complete.
    pub fn is_complete(&self) -> bool {
        self.progress.all_complete
    }

    /// Get the current module and step (if any).
    pub fn current_step(&self) -> Option<(&TutorialModule, &TutorialStep)> {
        if self.progress.all_complete || self.progress.current_module.is_empty() {
            return None;
        }
        let module = self
            .modules
            .iter()
            .find(|m| m.id == self.progress.current_module)?;
        let step = module.steps.get(self.progress.current_step_index)?;
        Some((module, step))
    }

    /// Start the tutorial from the beginning (or resume).
    pub fn start(
        &mut self,
        now_ms: u64,
    ) -> Result<Option<(&TutorialModule, &TutorialStep)>, OnboardingError> {
        if self.progress.all_complete {
            return Ok(None);
        }

        // If not started, begin with first required module.
        if self.progress.current_module.is_empty() {
            let first = self
                .modules
                .iter()
                .find(|m| m.required && !self.progress.completed_modules.contains(&m.id));

            match first {
                Some(module) => {
                    self.progress.current_module = module.id.clone();
                    self.progress.current_step_index = 0;
                    self.progress.updated_at_ms = now_ms;
                    info!(module = %module.id, "tutorial started");
                },
                None => {
                    // All required modules done.
                    self.progress.all_complete = true;
                    self.progress.updated_at_ms = now_ms;
                    info!("all required tutorial modules complete");
                    return Ok(None);
                },
            }
        }

        Ok(self.current_step())
    }

    /// Record the result of the current step and advance.
    pub fn complete_step(
        &mut self,
        result: StepResult,
        now_ms: u64,
    ) -> Result<Option<(&TutorialModule, &TutorialStep)>, OnboardingError> {
        let current_module_id = self.progress.current_module.clone();
        if current_module_id.is_empty() {
            return Err(OnboardingError::TutorialStepFailed {
                step: "none".into(),
                reason: "no active module".into(),
            });
        }

        // Record result — cap at MAX_MODULES * MAX_STEPS_PER_MODULE entries.
        self.progress.total_time_ms = self
            .progress
            .total_time_ms
            .saturating_add(result.duration_ms);
        if self.progress.step_results.len() < MAX_MODULES * MAX_STEPS_PER_MODULE {
            self.progress.step_results.push(result);
        } else {
            warn!(
                cap = MAX_MODULES * MAX_STEPS_PER_MODULE,
                "step_results cap reached; discarding oldest entry"
            );
            self.progress.step_results.remove(0);
            self.progress.step_results.push(result);
        }
        self.progress.updated_at_ms = now_ms;

        // Advance to next step.
        let module = self
            .modules
            .iter()
            .find(|m| m.id == current_module_id)
            .ok_or_else(|| OnboardingError::TutorialStepFailed {
                step: current_module_id.clone(),
                reason: "module not found".into(),
            })?;

        let next_index = self.progress.current_step_index + 1;
        if next_index < module.steps.len() {
            // More steps in this module.
            self.progress.current_step_index = next_index;
            debug!(
                module = %current_module_id,
                step = next_index,
                "advanced to next step"
            );
        } else {
            // Module complete — move to next required module.
            // Cap completed_modules at MAX_MODULES to prevent unbounded growth.
            if self.progress.completed_modules.len() < MAX_MODULES {
                self.progress
                    .completed_modules
                    .push(current_module_id.clone());
            } else {
                warn!(
                    cap = MAX_MODULES,
                    module = %current_module_id,
                    "completed_modules cap reached; entry not recorded"
                );
            }
            info!(module = %current_module_id, "tutorial module completed");

            let next_module = self
                .modules
                .iter()
                .find(|m| m.required && !self.progress.completed_modules.contains(&m.id));

            match next_module {
                Some(nm) => {
                    self.progress.current_module = nm.id.clone();
                    self.progress.current_step_index = 0;
                    info!(module = %nm.id, "starting next tutorial module");
                },
                None => {
                    self.progress.current_module.clear();
                    self.progress.all_complete = true;
                    info!("all tutorial modules complete");
                },
            }
        }

        Ok(self.current_step())
    }

    /// Skip the current step (for interactive/quiz steps the user wants to skip).
    pub fn skip_step(
        &mut self,
        now_ms: u64,
    ) -> Result<Option<(&TutorialModule, &TutorialStep)>, OnboardingError> {
        let result = StepResult {
            step_id: self
                .current_step()
                .map(|(_, s)| s.id.clone())
                .unwrap_or_default(),
            success: false,
            duration_ms: 0,
            chosen_answer: None,
            attempts: 0,
        };
        self.complete_step(result, now_ms)
    }

    /// Skip the entire tutorial.
    pub fn skip_all(&mut self, now_ms: u64) {
        self.progress.all_complete = true;
        self.progress.current_module.clear();
        self.progress.updated_at_ms = now_ms;
        warn!("tutorial skipped entirely by user");
    }

    /// Get context-sensitive help for the current step.
    pub fn get_help(&self) -> Option<String> {
        let (module, step) = self.current_step()?;
        let mut help = format!("Module: {}\n", module.title);
        help.push_str(&format!("Step: {}\n\n", step.title));
        help.push_str(&step.content);
        if let Some(hint) = &step.hint {
            help.push_str(&format!("\n\nHint: {hint}"));
        }
        Some(help)
    }

    /// Get completion percentage (0–100).
    pub fn completion_percentage(&self) -> u8 {
        let total_required: usize = self
            .modules
            .iter()
            .filter(|m| m.required)
            .map(|m| m.steps.len())
            .sum();

        if total_required == 0 {
            return 100;
        }

        let completed_steps = self.progress.step_results.len();
        ((completed_steps * 100) / total_required).min(100) as u8
    }

    // -----------------------------------------------------------------------
    // Persistence
    // -----------------------------------------------------------------------

    /// Save progress to a SQLite database.
    pub fn save_progress(&self, db: &rusqlite::Connection) -> Result<(), OnboardingError> {
        db.execute_batch(
            "CREATE TABLE IF NOT EXISTS tutorial_progress (
                id INTEGER PRIMARY KEY CHECK (id = 1),
                data BLOB NOT NULL,
                updated_at_ms INTEGER NOT NULL
            );",
        )
        .map_err(|e| OnboardingError::PersistenceFailed(format!("create table: {e}")))?;

        let json = serde_json::to_vec(&self.progress)
            .map_err(|e| OnboardingError::PersistenceFailed(format!("serialize: {e}")))?;

        db.execute(
            "INSERT INTO tutorial_progress (id, data, updated_at_ms)
             VALUES (1, ?1, ?2)
             ON CONFLICT(id) DO UPDATE SET data = ?1, updated_at_ms = ?2;",
            rusqlite::params![json, self.progress.updated_at_ms as i64],
        )
        .map_err(|e| OnboardingError::PersistenceFailed(format!("upsert: {e}")))?;

        debug!("tutorial progress saved");
        Ok(())
    }

    /// Load progress from a SQLite database.
    pub fn load_progress(
        db: &rusqlite::Connection,
    ) -> Result<Option<TutorialProgress>, OnboardingError> {
        let table_exists: bool = db
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='tutorial_progress';",
                [],
                |row| row.get::<_, i64>(0),
            )
            .map(|c| c > 0)
            .map_err(|e| OnboardingError::PersistenceFailed(format!("check table: {e}")))?;

        if !table_exists {
            return Ok(None);
        }

        let result: Result<Vec<u8>, _> = db.query_row(
            "SELECT data FROM tutorial_progress WHERE id = 1;",
            [],
            |row| row.get(0),
        );

        match result {
            Ok(data) => {
                let progress: TutorialProgress = serde_json::from_slice(&data)
                    .map_err(|e| OnboardingError::PersistenceFailed(format!("deserialize: {e}")))?;
                Ok(Some(progress))
            },
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(OnboardingError::PersistenceFailed(format!("load: {e}"))),
        }
    }

    // -----------------------------------------------------------------------
    // Default module builder
    // -----------------------------------------------------------------------

    fn build_default_modules() -> Vec<TutorialModule> {
        vec![
            TutorialModule {
                id: "basics".into(),
                title: "Getting Started with AURA".into(),
                description: "Learn how to talk to AURA and what it can do for you.".into(),
                required: true,
                steps: vec![
                    TutorialStep {
                        id: "basics_intro".into(),
                        kind: StepKind::Demonstrate,
                        title: "Meet AURA".into(),
                        content: "I'm AURA — your AI companion that lives on your phone. \
                            I can help with tasks, answer questions, manage your schedule, \
                            and learn your preferences over time. Let me show you around!"
                            .into(),
                        hint: None,
                        correct_answer: None,
                        choices: Vec::new(),
                        expected_duration_ms: 5_000,
                    },
                    TutorialStep {
                        id: "basics_talk".into(),
                        kind: StepKind::Interactive,
                        title: "Say Hello".into(),
                        content: "Try talking to me! Just type a message or tap the \
                            microphone button. I'll respond naturally — no special \
                            commands needed."
                            .into(),
                        hint: Some("Try saying: 'Hey AURA, what can you do?'".into()),
                        correct_answer: None,
                        choices: Vec::new(),
                        expected_duration_ms: 10_000,
                    },
                    TutorialStep {
                        id: "basics_quiz".into(),
                        kind: StepKind::Quiz,
                        title: "Quick Check".into(),
                        content: "How does AURA learn your preferences?".into(),
                        hint: Some("Think about how we've been chatting.".into()),
                        correct_answer: Some(1),
                        choices: vec![
                            "You have to manually configure everything".into(),
                            "By observing your interactions over time".into(),
                            "It downloads your data from the cloud".into(),
                        ],
                        expected_duration_ms: 8_000,
                    },
                ],
            },
            TutorialModule {
                id: "privacy".into(),
                title: "Your Privacy Matters".into(),
                description: "Understand how AURA handles your data.".into(),
                required: true,
                steps: vec![
                    TutorialStep {
                        id: "privacy_explain".into(),
                        kind: StepKind::Demonstrate,
                        title: "Privacy First".into(),
                        content: "Everything I learn about you stays on your device. \
                            I don't send your personal data to any cloud server. \
                            You're always in control of what I can see and remember."
                            .into(),
                        hint: None,
                        correct_answer: None,
                        choices: Vec::new(),
                        expected_duration_ms: 5_000,
                    },
                    TutorialStep {
                        id: "privacy_settings".into(),
                        kind: StepKind::Interactive,
                        title: "Review Privacy Settings".into(),
                        content: "Let's review your privacy settings together. \
                            You can change these anytime."
                            .into(),
                        hint: Some("Check the settings I showed you and adjust if needed.".into()),
                        correct_answer: None,
                        choices: Vec::new(),
                        expected_duration_ms: 15_000,
                    },
                ],
            },
            TutorialModule {
                id: "features".into(),
                title: "Key Features".into(),
                description: "Discover AURA's main capabilities.".into(),
                required: true,
                steps: vec![
                    TutorialStep {
                        id: "features_briefing".into(),
                        kind: StepKind::Demonstrate,
                        title: "Morning Briefing".into(),
                        content: "Every morning, I'll give you a quick summary of \
                            your day — weather, calendar, important notifications. \
                            I'll learn what matters to you and tailor the briefing."
                            .into(),
                        hint: None,
                        correct_answer: None,
                        choices: Vec::new(),
                        expected_duration_ms: 5_000,
                    },
                    TutorialStep {
                        id: "features_proactive".into(),
                        kind: StepKind::Demonstrate,
                        title: "Proactive Help".into(),
                        content: "I'll occasionally suggest helpful actions — like \
                            reminding you about an upcoming meeting or suggesting you \
                            call someone back. If you find these annoying, just tell me \
                            and I'll tone it down."
                            .into(),
                        hint: None,
                        correct_answer: None,
                        choices: Vec::new(),
                        expected_duration_ms: 5_000,
                    },
                ],
            },
        ]
    }
}

impl Default for TutorialEngine {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_engine_not_complete() {
        let engine = TutorialEngine::new();
        assert!(!engine.is_complete());
        assert_eq!(engine.completion_percentage(), 0);
    }

    #[test]
    fn test_start_tutorial() {
        let mut engine = TutorialEngine::new();
        let result = engine.start(1000).expect("start");
        assert!(result.is_some());
        let (module, step) = result.unwrap();
        assert_eq!(module.id, "basics");
        assert_eq!(step.id, "basics_intro");
    }

    #[test]
    fn test_complete_step_advances() {
        let mut engine = TutorialEngine::new();
        engine.start(1000).expect("start");

        let result = StepResult {
            step_id: "basics_intro".into(),
            success: true,
            duration_ms: 3000,
            chosen_answer: None,
            attempts: 1,
        };
        let next = engine.complete_step(result, 2000).expect("complete");
        assert!(next.is_some());
        let (_, step) = next.unwrap();
        assert_eq!(step.id, "basics_talk");
    }

    #[test]
    fn test_complete_all_steps() {
        let mut engine = TutorialEngine::new();
        engine.start(1000).expect("start");

        // Complete all steps across all modules.
        let mut time = 2000u64;
        loop {
            let current = engine.current_step();
            if current.is_none() {
                break;
            }
            let (_, step) = current.unwrap();
            let result = StepResult {
                step_id: step.id.clone(),
                success: true,
                duration_ms: 1000,
                chosen_answer: if step.kind == StepKind::Quiz {
                    step.correct_answer
                } else {
                    None
                },
                attempts: 1,
            };
            engine.complete_step(result, time).expect("complete");
            time += 1000;
        }

        assert!(engine.is_complete());
        assert_eq!(engine.completion_percentage(), 100);
    }

    #[test]
    fn test_skip_step() {
        let mut engine = TutorialEngine::new();
        engine.start(1000).expect("start");

        let next = engine.skip_step(2000).expect("skip");
        assert!(next.is_some());
        // Should have advanced past first step.
        let (_, step) = next.unwrap();
        assert_eq!(step.id, "basics_talk");
    }

    #[test]
    fn test_skip_all() {
        let mut engine = TutorialEngine::new();
        engine.start(1000).expect("start");
        engine.skip_all(2000);
        assert!(engine.is_complete());
    }

    #[test]
    fn test_get_help() {
        let mut engine = TutorialEngine::new();
        engine.start(1000).expect("start");

        let help = engine.get_help();
        assert!(help.is_some());
        let help_text = help.unwrap();
        assert!(help_text.contains("Getting Started"));
        assert!(help_text.contains("AURA"));
    }

    #[test]
    fn test_completion_percentage_partial() {
        let mut engine = TutorialEngine::new();
        engine.start(1000).expect("start");

        // Complete one step.
        let result = StepResult {
            step_id: "basics_intro".into(),
            success: true,
            duration_ms: 1000,
            chosen_answer: None,
            attempts: 1,
        };
        engine.complete_step(result, 2000).expect("complete");

        let pct = engine.completion_percentage();
        assert!(pct > 0);
        assert!(pct < 100);
    }

    #[test]
    fn test_with_progress_resumes() {
        let progress = TutorialProgress {
            current_module: "basics".into(),
            current_step_index: 1, // second step
            completed_modules: Vec::new(),
            step_results: vec![StepResult {
                step_id: "basics_intro".into(),
                success: true,
                duration_ms: 1000,
                chosen_answer: None,
                attempts: 1,
            }],
            total_time_ms: 1000,
            all_complete: false,
            updated_at_ms: 1000,
        };

        let engine = TutorialEngine::with_progress(progress);
        let current = engine.current_step();
        assert!(current.is_some());
        let (_, step) = current.unwrap();
        assert_eq!(step.id, "basics_talk");
    }

    #[test]
    fn test_db_save_load_progress() {
        let db = rusqlite::Connection::open_in_memory().expect("open db");
        let mut engine = TutorialEngine::new();
        engine.start(1000).expect("start");

        // Complete a step.
        let result = StepResult {
            step_id: "basics_intro".into(),
            success: true,
            duration_ms: 2000,
            chosen_answer: None,
            attempts: 1,
        };
        engine.complete_step(result, 2000).expect("complete");
        engine.save_progress(&db).expect("save");

        let loaded = TutorialEngine::load_progress(&db)
            .expect("load")
            .expect("should exist");
        assert_eq!(loaded.current_module, "basics");
        assert_eq!(loaded.current_step_index, 1);
        assert_eq!(loaded.step_results.len(), 1);
    }

    #[test]
    fn test_db_load_no_table() {
        let db = rusqlite::Connection::open_in_memory().expect("open db");
        let loaded = TutorialEngine::load_progress(&db).expect("load");
        assert!(loaded.is_none());
    }

    #[test]
    fn test_default_modules_exist() {
        let modules = TutorialEngine::build_default_modules();
        assert!(!modules.is_empty());
        assert!(modules.iter().any(|m| m.id == "basics"));
        assert!(modules.iter().any(|m| m.id == "privacy"));
        assert!(modules.iter().any(|m| m.id == "features"));
    }

    #[test]
    fn test_complete_step_no_active_module() {
        let mut engine = TutorialEngine::new();
        // Don't start — progress.current_module is empty.
        let result = StepResult {
            step_id: "fake".into(),
            success: true,
            duration_ms: 0,
            chosen_answer: None,
            attempts: 0,
        };
        let err = engine.complete_step(result, 1000).unwrap_err();
        assert!(matches!(err, OnboardingError::TutorialStepFailed { .. }));
    }
}
