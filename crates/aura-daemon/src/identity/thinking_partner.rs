//! Thinking Partner: Cognitive Anti-Atrophy & Socratic Mode
//!
//! # Strategic System Fit
//! While AURA automates tasks to save time, it aims to *enhance* the user's mind
//! rather than replace it. Standard AI assistants cause "cognitive atrophy" by
//! giving instant answers to complex problems. 
//! The `ThinkingPartner` operates in Socratic Mode: when the user faces a complex
//! decision, AURA switches from "Doer" to "Coach", asking questions to help the user
//! arrive at their own conclusions, fostering deep thought and meta-cognition.

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
    /// # Inputs
    /// - `task_complexity`: 0.0 to 1.0 (How hard is the user's question?)
    /// - `user_stress_level`: 0.0 to 1.0 (Derived from ETG / typing speed / biometrics)
    /// - `ocean`: Current personality of AURA (High Openness = more Socratic)
    pub fn evaluate_challenge_level(
        &self,
        task_complexity: f32,
        user_stress_level: f32,
        ocean: &OceanTraits,
        relationship: &RelationshipStage,
    ) -> ChallengeLevel {
        // Precise System Modeling: Constraints & Invariants
        
        // 1. If user is highly stressed, NEVER challenge. Reduce cognitive load immediately.
        if user_stress_level > 0.75 {
            return ChallengeLevel::Execution;
        }

        // 2. If task is simple (e.g., "set a timer"), just execute it.
        if task_complexity < 0.40 {
            return ChallengeLevel::Execution;
        }

        // 3. Dynamic Calculation Based on Personality
        let mut challenge_score = self.base_challenge_propensity;

        // High openness loves exploring ideas.
        if ocean.openness > 0.7 {
            challenge_score += 0.2;
        }

        // High conscientiousness wants the *best* answer, not just the *fastest*.
        if ocean.conscientiousness > 0.6 {
            challenge_score += 0.1;
        }

        // High agreeableness lowers the challenge rate (doesn't want to cause friction)
        if ocean.agreeableness > 0.8 {
            challenge_score -= 0.15;
        }

        // 4. Relationship Modifier
        match relationship {
            RelationshipStage::Stranger | RelationshipStage::Acquaintance => {
                // Trust limit: don't challenge people you just met.
                challenge_score *= 0.2; 
            }
            RelationshipStage::Friend => {
                challenge_score *= 0.8;
            }
            RelationshipStage::CloseFriend | RelationshipStage::Soulmate => {
                // Full trust: challenge freely for maximum growth.
                challenge_score *= 1.2;
            }
        }

        // 5. Threshold Resolution
        let final_score = challenge_score.clamp(0.0, 1.0);

        if final_score > 0.70 && task_complexity > 0.70 {
            ChallengeLevel::Socratic
        } else if final_score > 0.45 {
            ChallengeLevel::Reflective
        } else {
            ChallengeLevel::Execution
        }
    }

    /// Modifies the system prompt specifically for Socratic interaction.
    pub fn get_socratic_primer(&self) -> &'static str {
        "Your goal is NO LONGER TO ANSWER. Your goal is to guide the user to their own answer. \
         Ask one piercing, fundamental question that shifts their perspective. Do not give them the solution."
    }

    /// Modifies the system prompt for a mild reflection check.
    pub fn get_reflective_primer(&self) -> &'static str {
        "Before executing or answering fully, ask a brief clarifying question to ensure \
         this aligns with the user's long-term goals or stated values."
    }
}
