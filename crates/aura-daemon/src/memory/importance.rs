//! Importance scoring engine — the Amygdala of memory.
//!
//! This module computes and updates the importance score for memories using
//! the v4 formula:
//!   importance = source_weight * recency_decay * access_bonus * domain_priority
//!
//! Source weights, recency decay constants, and domain priorities are all from
//! the AURA-V4-ENGINEERING-BLUEPRINT.md §4.1.2.

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Event source for importance weighting (memory-specific, distinct from pipeline
/// EventSource to allow finer-grained weights).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EventSource {
    UserExplicit,
    Conversation,
    Notification,
    SystemEvent,
    Cron,
}

impl EventSource {
    /// Base weight per source type.
    pub fn weight(self) -> f32 {
        match self {
            Self::UserExplicit => 1.0,
            Self::Conversation => 0.8,
            Self::Notification => 0.5,
            Self::SystemEvent => 0.3,
            Self::Cron => 0.2,
        }
    }
}

/// Map from pipeline EventSource to memory EventSource.
impl From<aura_types::events::EventSource> for EventSource {
    fn from(src: aura_types::events::EventSource) -> Self {
        match src {
            aura_types::events::EventSource::UserCommand => Self::UserExplicit,
            aura_types::events::EventSource::Notification => Self::Notification,
            aura_types::events::EventSource::Accessibility => Self::SystemEvent,
            aura_types::events::EventSource::Cron => Self::Cron,
            aura_types::events::EventSource::Internal => Self::SystemEvent,
        }
    }
}

/// Content domain for priority weighting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Domain {
    Health,
    Finance,
    Social,
    Productivity,
    Entertainment,
    General,
}

impl Domain {
    /// Priority multiplier.
    pub fn priority(self) -> f32 {
        match self {
            Self::Health => 1.2,
            Self::Finance => 1.1,
            Self::Social => 1.0,
            Self::Productivity => 0.9,
            Self::Entertainment => 0.7,
            Self::General => 0.8,
        }
    }
}

/// Events that adjust an existing importance score.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ImportanceEvent {
    /// User explicitly referenced this memory.
    UserReferenced,
    /// User corrected or contradicted this memory.
    UserCorrected,
    /// Passage of time.
    TimePassed { hours: f64 },
    /// A related memory was strengthened.
    RelatedStrengthened,
}

// ---------------------------------------------------------------------------
// Core scoring
// ---------------------------------------------------------------------------

/// Calculate base importance for a NEW memory.
///
/// Formula: `source_weight * recency_decay * access_bonus * domain_priority`
///
/// For new memories, `hours_old` is typically 0 and `access_count` is 0,
/// so the effective formula simplifies to `source_weight * 1.0 * 1.0 * domain_priority`.
pub fn calculate_importance(
    source: EventSource,
    hours_old: f64,
    access_count: u32,
    domain: Domain,
) -> f32 {
    let source_weight = source.weight();
    let recency = recency_decay(hours_old);
    let access = access_bonus(access_count);
    let priority = domain.priority();

    let raw = source_weight * recency * access * priority;
    raw.clamp(0.0, 2.0) // theoretical max is 1.0 * 1.0 * 2.0 * 1.2 = 2.4, clamp for safety
}

/// Update an existing importance value based on an event.
///
/// Returns the new importance, clamped to [0.0, 2.0].
pub fn update_importance(current: f32, event: ImportanceEvent) -> f32 {
    let adjusted = match event {
        ImportanceEvent::UserReferenced => current + 0.1,
        ImportanceEvent::UserCorrected => current - 0.2,
        ImportanceEvent::TimePassed { hours } => {
            // Apply recency decay multiplicatively
            current * recency_decay(hours)
        }
        ImportanceEvent::RelatedStrengthened => current + 0.05,
    };
    adjusted.clamp(0.0, 2.0)
}

/// Recency decay function: e^(-0.001 * hours).
///
/// Very slow decay — memories stay relevant for weeks.
/// Half-life: ln(2) / 0.001 = ~693 hours ≈ 29 days.
#[inline]
pub fn recency_decay(hours: f64) -> f32 {
    (-0.001 * hours).exp() as f32
}

/// Access bonus: min(2.0, 1.0 + 0.1 * access_count).
///
/// More accessed = more important, capped at 2x.
#[inline]
pub fn access_bonus(access_count: u32) -> f32 {
    (1.0 + 0.1 * access_count as f32).min(2.0)
}

/// Compute the recall score for ranking retrieval results.
///
/// score = similarity*0.4 + recency*0.2 + importance*0.2 + activation*0.2
///
/// Where:
/// - similarity: cosine similarity between query and memory embedding [0, 1]
/// - recency: exp(-0.1 * hours_ago) — ~7 hour half-life
/// - importance: pre-computed importance score [0, 2], normalized to [0, 1]
/// - activation: access_bonus / 2.0 (normalized to [0, 1])
pub fn recall_score(
    similarity: f32,
    hours_ago: f64,
    importance: f32,
    access_count: u32,
) -> f32 {
    let recency = (-0.1 * hours_ago).exp() as f32; // ~7 hour half-life
    let norm_importance = (importance / 2.0).clamp(0.0, 1.0);
    let activation = (access_bonus(access_count) / 2.0).clamp(0.0, 1.0);

    similarity * 0.4 + recency * 0.2 + norm_importance * 0.2 + activation * 0.2
}

/// Consolidation score for deciding what to promote between tiers.
///
/// score = recency*0.3 + frequency*0.3 + base_importance*0.4
///
/// Threshold for promotion is typically 0.7.
pub fn consolidation_score(
    hours_ago: f64,
    access_count: u32,
    base_importance: f32,
) -> f32 {
    let recency = (-0.1 * hours_ago).exp() as f32;
    let frequency = (access_count as f32 / 10.0).min(1.0); // normalize: 10 accesses = 1.0
    let norm_importance = (base_importance / 2.0).clamp(0.0, 1.0);

    recency * 0.3 + frequency * 0.3 + norm_importance * 0.4
}

/// Initial confidence for a new semantic entry.
///
/// confidence = min(0.9, 0.3 + importance * 0.4)
pub fn initial_semantic_confidence(importance: f32) -> f32 {
    (0.3 + importance * 0.4).min(0.9)
}

/// Generalization confidence for semantic entries derived from episode clusters.
///
/// confidence = min(0.95, 0.5 + num_episodes * 0.1)
pub fn generalization_confidence(num_episodes: usize) -> f32 {
    (0.5 + num_episodes as f32 * 0.1).min(0.95)
}

// ---------------------------------------------------------------------------
// Domain detection
// ---------------------------------------------------------------------------

/// Simple keyword-based domain detection from content text.
///
/// Scans for domain-indicating keywords and returns the domain with the
/// most keyword matches. Returns General if no matches.
pub fn detect_domain(content: &str) -> Domain {
    let lower = content.to_lowercase();

    let scores = [
        (Domain::Health, health_keywords(&lower)),
        (Domain::Finance, finance_keywords(&lower)),
        (Domain::Social, social_keywords(&lower)),
        (Domain::Productivity, productivity_keywords(&lower)),
        (Domain::Entertainment, entertainment_keywords(&lower)),
    ];

    scores
        .iter()
        .max_by_key(|(_, count)| *count)
        .filter(|(_, count)| *count > 0)
        .map(|(domain, _)| *domain)
        .unwrap_or(Domain::General)
}

fn health_keywords(text: &str) -> u32 {
    let words = [
        "health", "doctor", "medicine", "hospital", "exercise", "workout",
        "sleep", "diet", "symptom", "pain", "weight", "calories", "fitness",
        "meditation", "therapy", "vitamin", "prescription", "appointment",
        "medical", "clinic",
    ];
    words.iter().filter(|w| text.contains(*w)).count() as u32
}

fn finance_keywords(text: &str) -> u32 {
    let words = [
        "money", "payment", "bank", "transfer", "salary", "invoice",
        "budget", "expense", "tax", "invest", "stock", "credit",
        "debit", "loan", "mortgage", "bill", "price", "cost",
        "purchase", "financial",
    ];
    words.iter().filter(|w| text.contains(*w)).count() as u32
}

fn social_keywords(text: &str) -> u32 {
    let words = [
        "friend", "family", "birthday", "party", "dinner", "meeting",
        "call", "message", "chat", "social", "relationship", "date",
        "wedding", "reunion", "group", "contact", "invite",
    ];
    words.iter().filter(|w| text.contains(*w)).count() as u32
}

fn productivity_keywords(text: &str) -> u32 {
    let words = [
        "work", "project", "deadline", "task", "meeting", "email",
        "report", "schedule", "calendar", "presentation", "office",
        "code", "commit", "review", "sprint", "kanban", "todo",
    ];
    words.iter().filter(|w| text.contains(*w)).count() as u32
}

fn entertainment_keywords(text: &str) -> u32 {
    let words = [
        "movie", "music", "game", "watch", "play", "stream",
        "podcast", "show", "series", "concert", "youtube", "netflix",
        "spotify", "book", "read", "fun", "hobby",
    ];
    words.iter().filter(|w| text.contains(*w)).count() as u32
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_source_weights() {
        assert_eq!(EventSource::UserExplicit.weight(), 1.0);
        assert_eq!(EventSource::Conversation.weight(), 0.8);
        assert_eq!(EventSource::Notification.weight(), 0.5);
        assert_eq!(EventSource::SystemEvent.weight(), 0.3);
        assert_eq!(EventSource::Cron.weight(), 0.2);
    }

    #[test]
    fn test_domain_priorities() {
        assert_eq!(Domain::Health.priority(), 1.2);
        assert_eq!(Domain::Finance.priority(), 1.1);
        assert_eq!(Domain::Social.priority(), 1.0);
        assert_eq!(Domain::Productivity.priority(), 0.9);
        assert_eq!(Domain::Entertainment.priority(), 0.7);
        assert_eq!(Domain::General.priority(), 0.8);
    }

    #[test]
    fn test_calculate_importance_new_memory() {
        // New memory: hours_old=0, access_count=0
        let imp = calculate_importance(EventSource::UserExplicit, 0.0, 0, Domain::Health);
        // 1.0 * exp(0) * min(2.0, 1.0 + 0) * 1.2 = 1.0 * 1.0 * 1.0 * 1.2 = 1.2
        assert!((imp - 1.2).abs() < 1e-5, "expected 1.2, got {}", imp);
    }

    #[test]
    fn test_calculate_importance_old_memory() {
        // 720 hours old (30 days), 5 accesses
        let imp = calculate_importance(EventSource::Conversation, 720.0, 5, Domain::Social);
        let expected = 0.8 * (-0.001 * 720.0_f64).exp() as f32 * (1.0 + 0.5) * 1.0;
        assert!((imp - expected).abs() < 1e-4, "expected {}, got {}", expected, imp);
    }

    #[test]
    fn test_recency_decay() {
        assert!((recency_decay(0.0) - 1.0).abs() < f32::EPSILON);
        // At 693 hours (half-life), should be ~0.5
        let half = recency_decay(693.0);
        assert!((half - 0.5).abs() < 0.01, "half-life decay should be ~0.5, got {}", half);
        // Very old should approach 0
        let old = recency_decay(10000.0);
        assert!(old < 0.001);
    }

    #[test]
    fn test_access_bonus() {
        assert_eq!(access_bonus(0), 1.0);
        assert_eq!(access_bonus(5), 1.5);
        assert_eq!(access_bonus(10), 2.0);
        assert_eq!(access_bonus(100), 2.0); // capped
    }

    #[test]
    fn test_recall_score() {
        // Perfect match, just created, high importance, many accesses
        let score = recall_score(1.0, 0.0, 2.0, 10);
        // 1.0*0.4 + 1.0*0.2 + 1.0*0.2 + 1.0*0.2 = 1.0
        assert!((score - 1.0).abs() < 1e-5, "max recall score should be 1.0, got {}", score);
    }

    #[test]
    fn test_recall_score_old_low_importance() {
        let score = recall_score(0.5, 24.0, 0.3, 1);
        // similarity=0.5*0.4=0.2, recency=exp(-2.4)*0.2≈0.018, importance=0.15*0.2=0.03, access=0.55*0.2=0.11
        assert!(score > 0.0 && score < 1.0);
    }

    #[test]
    fn test_consolidation_score() {
        // Fresh, frequently accessed, important
        let score = consolidation_score(0.0, 10, 2.0);
        // 1.0*0.3 + 1.0*0.3 + 1.0*0.4 = 1.0
        assert!((score - 1.0).abs() < 1e-5);
    }

    #[test]
    fn test_consolidation_score_below_threshold() {
        // Old, rarely accessed, low importance
        let score = consolidation_score(100.0, 0, 0.2);
        assert!(score < 0.7, "should be below promotion threshold, got {}", score);
    }

    #[test]
    fn test_update_importance_user_referenced() {
        let new = update_importance(0.5, ImportanceEvent::UserReferenced);
        assert!((new - 0.6).abs() < f32::EPSILON);
    }

    #[test]
    fn test_update_importance_user_corrected() {
        let new = update_importance(0.5, ImportanceEvent::UserCorrected);
        assert!((new - 0.3).abs() < f32::EPSILON);
    }

    #[test]
    fn test_update_importance_clamp() {
        // Should not go below 0
        let new = update_importance(0.1, ImportanceEvent::UserCorrected);
        assert_eq!(new, 0.0);
        // Should not go above 2.0
        let new = update_importance(1.95, ImportanceEvent::UserReferenced);
        assert_eq!(new, 2.0);
    }

    #[test]
    fn test_initial_semantic_confidence() {
        assert!((initial_semantic_confidence(0.0) - 0.3).abs() < f32::EPSILON);
        assert!((initial_semantic_confidence(1.0) - 0.7).abs() < f32::EPSILON);
        assert!((initial_semantic_confidence(2.0) - 0.9).abs() < f32::EPSILON); // capped
        assert!((initial_semantic_confidence(5.0) - 0.9).abs() < f32::EPSILON); // capped
    }

    #[test]
    fn test_generalization_confidence() {
        assert!((generalization_confidence(3) - 0.8).abs() < f32::EPSILON);
        assert!((generalization_confidence(5) - 0.95).abs() < f32::EPSILON); // capped
        assert!((generalization_confidence(10) - 0.95).abs() < f32::EPSILON); // capped
    }

    #[test]
    fn test_detect_domain() {
        assert_eq!(detect_domain("doctor appointment at hospital"), Domain::Health);
        assert_eq!(detect_domain("bank transfer payment"), Domain::Finance);
        assert_eq!(detect_domain("friend birthday party"), Domain::Social);
        assert_eq!(detect_domain("project deadline sprint"), Domain::Productivity);
        assert_eq!(detect_domain("movie music game"), Domain::Entertainment);
        assert_eq!(detect_domain("random gibberish text"), Domain::General);
    }

    #[test]
    fn test_detect_domain_empty() {
        assert_eq!(detect_domain(""), Domain::General);
    }

    #[test]
    fn test_pipeline_event_source_conversion() {
        let src = aura_types::events::EventSource::UserCommand;
        let mem_src: EventSource = src.into();
        assert_eq!(mem_src, EventSource::UserExplicit);
    }
}
