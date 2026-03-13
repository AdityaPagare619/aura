//! Thinking Partner: Cognitive Anti-Atrophy & Socratic Mode
//!
//! # Strategic System Fit
//! While AURA automates tasks to save time, it aims to *enhance* the user's mind
//! rather than replace it. Standard AI assistants cause "cognitive atrophy" by
//! giving instant answers to complex problems. 
//! The `ThinkingPartner` operates in Socratic Mode: when the user faces a complex
//! decision, AURA switches from "Doer" to "Coach", asking questions to help the user
//! arrive at their own conclusions, fostering deep thought and meta-cognition.
//!
//! LLM determines its own reasoning style — Rust does not inject priming directives.

use aura_types::identity::{OceanTraits, RelationshipStage};
use serde::{Deserialize, Serialize};

/// Represents the level of cognitive challenge AURA is currently applying.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChallengeLevel {
    /// Standard execution. Just do the task or give the answer.
    Execution,
    /// Mild pushback. "Are you sure this aligns with your goal for today?"
    Reflective,
    /// Full Socratic dialogue. "What happens if you take the opposite approach?"
    Socratic,
}

pub struct ThinkingPartner {
    pub base_challenge_propensity: f32, // How likely AURA is to challenge by default
}

impl Default for ThinkingPartner {
    fn default() -> Self {
        Self {
            base_challenge_propensity: 0.30, // 30% chance to challenge complex queries
        }
    }
}

impl ThinkingPartner {
    pub fn new() -> Self {
        Self::default()
    }

    /// Determines if AURA should just give an answer or challenge the user structurally.
    ///
    /// # Iron Law — LLM = brain, Rust = body
    ///
    /// Rust cannot determine whether a question warrants Socratic dialogue.
    /// That decision requires understanding user intent, context, and nuance —
    /// which is exactly what the LLM does. OCEAN-weighted formula selecting
    /// ChallengeLevel is Theater AGI: Rust masquerading as intelligence.
    ///
    /// Always returns `Execution`. The LLM chooses its own reasoning style.
    pub fn evaluate_challenge_level(
        &self,
        _task_complexity: f32,
        _user_stress_level: f32,
        _ocean: &OceanTraits,
        _relationship: &RelationshipStage,
    ) -> ChallengeLevel {
        // TODO(llm-brain): ChallengeLevel selection belongs to the LLM, not Rust.
        // The neocortex determines reasoning depth from context; we never inject
        // Socratic/Reflective mode priming into the prompt from this function.
        ChallengeLevel::Execution
    }
}
