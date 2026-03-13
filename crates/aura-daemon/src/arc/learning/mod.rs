//! Learning engine — interest modelling and skill acquisition.
//!
//! # Architecture (SPEC-ARC §8.4)
//!
//! The learning engine implements:
//! 1. **Skills** — learned action sequences that improve over time
//! 2. **Interests** — an evolving user interest model
//!
//! NOTE: All reasoning modules (hebbian, patterns, prediction, world_model,
//! dreaming, dimensions) have been removed. All reasoning belongs in the LLM
//! (neocortex) layer.

pub mod interests;
pub mod skills;

pub use interests::InterestModel;
pub use skills::SkillRegistry;

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// LearningEngine
// ---------------------------------------------------------------------------

/// Top-level learning engine aggregate.
///
/// Storage-only: interests and skills.
/// All reasoning is delegated to the LLM (neocortex) layer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LearningEngine {
    /// User interest model.
    pub interests: InterestModel,
    /// Learned skill registry.
    pub skills: SkillRegistry,
}

impl LearningEngine {
    /// Create a new learning engine with empty sub-engines.
    #[must_use]
    pub fn new() -> Self {
        Self {
            interests: InterestModel::new(),
            skills: SkillRegistry::new(),
        }
    }
}

impl Default for LearningEngine {
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
    fn test_new_learning_engine() {
        let engine = LearningEngine::new();
        assert_eq!(engine.interests.interest_count(), 0);
        assert_eq!(engine.skills.skill_count(), 0);
    }
}

