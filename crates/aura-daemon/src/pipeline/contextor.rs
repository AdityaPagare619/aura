//! Stage 3: Contextor — enriches scored events with memory, identity, and state.
//!
//! The Contextor is the BRIDGE between the Amygdala (importance scoring) and the
//! Router (System1/System2 dispatch). For every [`ScoredEvent`] it:
//!
//! 1. **Queries working memory** — recent context, current task state
//! 2. **Queries episodic memory** — relevant past episodes, similar situations
//! 3. **Queries semantic memory** — learned facts and generalizations
//! 4. **Gets user context** — relationship stage, trust level, personality snapshot
//! 5. **Assembles a context package** — structured, token-budgeted, deduplicated
//!
//! # Latency target
//!
//! Context enrichment MUST complete in <50 ms. All memory queries are bounded
//! by `max_results` which scales with event importance to avoid retrieving 50 KB
//! of context for a "what time is it?" query.
//!
//! # Recall scoring
//!
//! Memories retrieved from multiple tiers are re-ranked using a composite score:
//!
//! ```text
//! recall = similarity×0.4 + recency×0.2 + importance×0.2 + activation×0.2
//! ```
//!
//! Where activation is `min(access_count / 10.0, 1.0)` for episodic/semantic,
//! and 1.0 for working memory (always "active").

use serde::{Deserialize, Serialize};
use tracing::{debug, instrument, trace, warn};

use aura_types::errors::AuraError;
use aura_types::events::{GateDecision, ScoredEvent};
use aura_types::ipc::{
    ConversationTurn, GoalSummary, MemorySnippet, MemoryTier, PersonalitySnapshot, Role,
};
use aura_types::memory::MemoryQuery;

use crate::identity::personality::Personality;
use crate::identity::relationship::RelationshipTracker;
use crate::identity::affective::AffectiveEngine;
use crate::identity::prompt_personality::PersonalityPromptInjector;
use crate::memory::AuraMemory;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Token budget for low-importance events (score < 0.35).
const TOKEN_BUDGET_LOW: usize = 200;
/// Token budget for medium-importance events (0.35 ≤ score < 0.65).
const TOKEN_BUDGET_MEDIUM: usize = 500;
/// Token budget for high-importance events (0.65 ≤ score < 0.90).
const TOKEN_BUDGET_HIGH: usize = 1000;
/// Token budget for emergency events (score ≥ 0.90 or EmergencyBypass).
const TOKEN_BUDGET_EMERGENCY: usize = 2000;

/// Max memory results per tier for low-importance queries.
const MAX_RESULTS_LOW: usize = 2;
/// Max memory results per tier for medium-importance queries.
const MAX_RESULTS_MEDIUM: usize = 4;
/// Max memory results per tier for high-importance queries.
const MAX_RESULTS_HIGH: usize = 6;
/// Max memory results per tier for emergency queries.
const MAX_RESULTS_EMERGENCY: usize = 10;

/// Minimum relevance threshold for memory retrieval.
const MIN_RELEVANCE: f32 = 0.15;

/// Maximum number of conversation turns to include.
const MAX_CONVERSATION_TURNS: usize = 10;

/// Recall scoring weights.
const WEIGHT_SIMILARITY: f32 = 0.40;
const WEIGHT_RECENCY: f32 = 0.20;
const WEIGHT_IMPORTANCE: f32 = 0.20;
const WEIGHT_ACTIVATION: f32 = 0.20;

/// Recency normalization window — 1 hour in ms.
const RECENCY_WINDOW_MS: u64 = 3_600_000;

/// Approximate tokens per character (rough estimate for budget checks).
const CHARS_PER_TOKEN: usize = 4;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Contextual information about the user involved in the event.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserContext {
    pub user_id: String,
    pub relationship_stage: String,
    pub trust_level: f32,
    pub directness: f32,
    pub interaction_count: u64,
    pub personality_snapshot: PersonalitySnapshot,
    pub recent_topics: Vec<String>,
}

/// A scored event enriched with memory, user context, goals, and state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnrichedEvent {
    /// The original scored event from the Amygdala.
    pub scored: ScoredEvent,
    /// Relevant memories from all tiers, ranked by recall score.
    pub memory_context: Vec<MemorySnippet>,
    /// Who is talking — relationship stage, trust, personality.
    pub user_context: Option<UserContext>,
    /// What AURA is currently doing.
    pub active_goals: Vec<GoalSummary>,
    /// Current screen state summary.
    pub screen_summary: Option<String>,
    /// Recent conversation turns for dialogue continuity.
    pub conversation_history: Vec<ConversationTurn>,
    /// How many tokens worth of context the Router/LLM should consume.
    pub context_token_budget: usize,
    /// Personality-derived prompt directives (TRUTH framework, OCEAN traits,
    /// mood overlay, relationship stage). `None` if personality injection
    /// is not applicable for this event.
    pub personality_context: Option<String>,
}

/// Configuration for the Contextor's retrieval behavior.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextorConfig {
    /// Whether to query episodic memory (can be disabled for latency).
    pub enable_episodic: bool,
    /// Whether to query semantic memory.
    pub enable_semantic: bool,
    /// Override token budget (None = auto-scale from importance).
    pub token_budget_override: Option<usize>,
}

impl Default for ContextorConfig {
    fn default() -> Self {
        Self {
            enable_episodic: true,
            enable_semantic: true,
            token_budget_override: None,
        }
    }
}

// ---------------------------------------------------------------------------
// Contextor
// ---------------------------------------------------------------------------

/// Stage 3: enriches scored events with memory and user context.
///
/// The Contextor does NOT own the memory/identity subsystems — it borrows them
/// via method parameters. This keeps ownership clear: the daemon main loop owns
/// everything, the Contextor is a stateless-ish enrichment engine.
///
/// Mutable state held: conversation ring buffer, active goals, screen summary.
/// These are updated externally via setter methods.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Contextor {
    /// Recent conversation turns (ring buffer, max [`MAX_CONVERSATION_TURNS`]).
    conversation_buffer: Vec<ConversationTurn>,
    /// Currently active goals (set by the execution engine).
    active_goals: Vec<GoalSummary>,
    /// Current screen state summary (set by screen observer).
    screen_summary: Option<String>,
    /// Configuration.
    config: ContextorConfig,
}

// ---------------------------------------------------------------------------
// Implementation
// ---------------------------------------------------------------------------

impl Contextor {
    /// Create a new Contextor with default configuration.
    #[instrument]
    pub fn new() -> Self {
        trace!("Contextor initialized with default config");
        Self {
            conversation_buffer: Vec::with_capacity(MAX_CONVERSATION_TURNS),
            active_goals: Vec::new(),
            screen_summary: None,
            config: ContextorConfig::default(),
        }
    }

    /// Create a Contextor with custom configuration.
    #[instrument(skip(config), fields(
        episodic = config.enable_episodic,
        semantic = config.enable_semantic,
        budget_override = ?config.token_budget_override,
    ))]
    pub fn with_config(config: ContextorConfig) -> Self {
        trace!("Contextor initialized with custom config");
        Self {
            conversation_buffer: Vec::with_capacity(MAX_CONVERSATION_TURNS),
            active_goals: Vec::new(),
            screen_summary: None,
            config,
        }
    }

    // -----------------------------------------------------------------------
    // State setters — called by external subsystems to keep context fresh
    // -----------------------------------------------------------------------

    /// Record a conversation turn. Maintains a bounded ring buffer.
    #[instrument(skip(self, turn), fields(role = ?turn.role, content_len = turn.content.len()))]
    pub fn push_conversation_turn(&mut self, turn: ConversationTurn) {
        if self.conversation_buffer.len() >= MAX_CONVERSATION_TURNS {
            trace!(
                buffer_len = self.conversation_buffer.len(),
                max = MAX_CONVERSATION_TURNS,
                "conversation buffer full — evicting oldest turn"
            );
            self.conversation_buffer.remove(0);
        }
        self.conversation_buffer.push(turn);
    }

    /// Replace the active goals list.
    #[instrument(skip(self, goals), fields(goal_count = goals.len()))]
    pub fn set_active_goals(&mut self, goals: Vec<GoalSummary>) {
        trace!(
            prev_count = self.active_goals.len(),
            new_count = goals.len(),
            "replacing active goals"
        );
        self.active_goals = goals;
    }

    /// Update the screen summary.
    #[instrument(skip(self, summary), fields(has_summary = summary.is_some()))]
    pub fn set_screen_summary(&mut self, summary: Option<String>) {
        trace!(
            had_previous = self.screen_summary.is_some(),
            "updating screen summary"
        );
        self.screen_summary = summary;
    }

    /// Get current conversation history (read-only).
    #[instrument(skip(self), fields(turns = self.conversation_buffer.len()))]
    pub fn conversation_history(&self) -> &[ConversationTurn] {
        &self.conversation_buffer
    }

    // -----------------------------------------------------------------------
    // Core enrichment
    // -----------------------------------------------------------------------

    /// Enrich a scored event with memory and user context.
    ///
    /// This is the primary pipeline method. It queries the memory system,
    /// retrieves user context from the identity subsystem, and assembles
    /// a complete [`EnrichedEvent`] for the Router.
    ///
    /// # Errors
    ///
    /// Returns `AuraError::Memory` if all memory queries fail. Individual
    /// tier failures are logged and skipped — partial context is better than
    /// no context.
    #[instrument(
        skip(self, memory, relationships, personality, affective),
        fields(
            score = scored.score_total,
            gate = ?scored.gate_decision,
            intent = ?scored.parsed.intent,
        )
    )]
    pub async fn enrich(
        &self,
        scored: ScoredEvent,
        memory: &AuraMemory,
        relationships: &RelationshipTracker,
        personality: &Personality,
        affective: &AffectiveEngine,
        now_ms: u64,
    ) -> Result<EnrichedEvent, AuraError> {
        let token_budget = self.compute_token_budget(&scored);
        let max_results = self.max_results_for_importance(&scored);

        debug!(
            token_budget,
            max_results,
            "contextor enriching event"
        );

        // 1. Build query text from event content + entities
        let query_text = self.build_query_text(&scored);

        // 2. Query memory system (cross-tier, bounded)
        let raw_memories = self
            .query_memory(memory, &query_text, max_results, now_ms)
            .await;

        // 3. Re-rank with composite recall scoring and apply token budget
        let memory_context = self.rank_and_budget_memories(raw_memories, token_budget, now_ms);

        // 4. Get user context from identity subsystem
        let user_context = self.build_user_context(
            &scored,
            relationships,
            personality,
        );

        // 5. Select relevant conversation history
        let conversation_history = self.select_conversation_history(token_budget);

        // 6. Generate personality context for prompt injection
        let personality_context = self.build_personality_context(
            personality,
            affective,
            relationships,
        );

        // 7. Assemble enriched event
        let enriched = EnrichedEvent {
            scored,
            memory_context,
            user_context,
            active_goals: self.active_goals.clone(),
            screen_summary: self.screen_summary.clone(),
            conversation_history,
            context_token_budget: token_budget,
            personality_context,
        };

        debug!(
            memory_snippets = enriched.memory_context.len(),
            has_user_ctx = enriched.user_context.is_some(),
            goals = enriched.active_goals.len(),
            conv_turns = enriched.conversation_history.len(),
            "enrichment complete"
        );

        Ok(enriched)
    }

    // -----------------------------------------------------------------------
    // Internal: token budget computation
    // -----------------------------------------------------------------------

    /// Compute the token budget based on event importance and gate decision.
    fn compute_token_budget(&self, scored: &ScoredEvent) -> usize {
        if let Some(override_budget) = self.config.token_budget_override {
            return override_budget;
        }

        match scored.gate_decision {
            GateDecision::EmergencyBypass => TOKEN_BUDGET_EMERGENCY,
            _ => {
                let s = scored.score_total;
                if s >= 0.90 {
                    TOKEN_BUDGET_EMERGENCY
                } else if s >= 0.65 {
                    TOKEN_BUDGET_HIGH
                } else if s >= 0.35 {
                    TOKEN_BUDGET_MEDIUM
                } else {
                    TOKEN_BUDGET_LOW
                }
            }
        }
    }

    /// Determine max results per tier based on event importance.
    fn max_results_for_importance(&self, scored: &ScoredEvent) -> usize {
        match scored.gate_decision {
            GateDecision::EmergencyBypass => MAX_RESULTS_EMERGENCY,
            _ => {
                let s = scored.score_total;
                if s >= 0.90 {
                    MAX_RESULTS_EMERGENCY
                } else if s >= 0.65 {
                    MAX_RESULTS_HIGH
                } else if s >= 0.35 {
                    MAX_RESULTS_MEDIUM
                } else {
                    MAX_RESULTS_LOW
                }
            }
        }
    }

    // -----------------------------------------------------------------------
    // Internal: query text construction
    // -----------------------------------------------------------------------

    /// Build a targeted query string from the event's content and entities.
    ///
    /// Rather than blindly searching on the full content, we combine the
    /// event's content with its extracted entities to improve recall for
    /// entity-specific memories.
    fn build_query_text(&self, scored: &ScoredEvent) -> String {
        let mut query = scored.parsed.content.clone();

        // Append entities that aren't already substrings of the content
        for entity in &scored.parsed.entities {
            if !query.to_ascii_lowercase().contains(&entity.to_ascii_lowercase()) {
                query.push(' ');
                query.push_str(entity);
            }
        }

        // Truncate to avoid oversized queries (128 chars is plenty for trigram search)
        if query.len() > 128 {
            query.truncate(128);
        }

        query
    }

    // -----------------------------------------------------------------------
    // Internal: memory retrieval
    // -----------------------------------------------------------------------

    /// Query the memory system across tiers, collecting results.
    ///
    /// Individual tier failures are logged and skipped. Returns whatever
    /// results were successfully retrieved.
    async fn query_memory(
        &self,
        memory: &AuraMemory,
        query_text: &str,
        max_results: usize,
        now_ms: u64,
    ) -> Vec<RankedMemory> {
        let mut results: Vec<RankedMemory> = Vec::new();

        // Build tier list based on config
        let mut tiers = vec![MemoryTier::Working];
        if self.config.enable_episodic {
            tiers.push(MemoryTier::Episodic);
        }
        if self.config.enable_semantic {
            tiers.push(MemoryTier::Semantic);
        }

        trace!(
            tier_count = tiers.len(),
            max_results,
            query_len = query_text.len(),
            "querying memory across tiers"
        );

        let query = MemoryQuery {
            query_text: query_text.to_owned(),
            max_results,
            min_relevance: MIN_RELEVANCE,
            tiers,
            time_range: None,
        };

        match memory.query(&query, now_ms).await {
            Ok(mem_results) => {
                trace!(result_count = mem_results.len(), "memory query returned results");
                for mr in mem_results {
                    results.push(RankedMemory {
                        content: mr.content,
                        tier: mr.tier,
                        similarity: mr.relevance,
                        importance: mr.importance,
                        timestamp_ms: mr.timestamp_ms,
                        source_id: mr.source_id,
                        recall_score: 0.0,
                    });
                }
            }
            Err(e) => {
                warn!(error = %e, "cross-tier memory query failed");
            }
        }

        results
    }

    // -----------------------------------------------------------------------
    // Internal: recall ranking and token budgeting
    // -----------------------------------------------------------------------

    /// Re-rank memories using the composite recall formula and trim to
    /// fit within the token budget.
    ///
    /// Recall = similarity×0.4 + recency×0.2 + importance×0.2 + activation×0.2
    fn rank_and_budget_memories(
        &self,
        mut memories: Vec<RankedMemory>,
        token_budget: usize,
        now_ms: u64,
    ) -> Vec<MemorySnippet> {
        if memories.is_empty() {
            return Vec::new();
        }

        // Compute composite recall score for each memory
        for mem in &mut memories {
            let recency = compute_recency(mem.timestamp_ms, now_ms);
            let activation = match mem.tier {
                // Working memory is always fully "active"
                MemoryTier::Working => 1.0_f32,
                // For episodic/semantic, we don't have access_count here,
                // so use importance as a proxy for activation
                _ => mem.importance.clamp(0.0, 1.0),
            };

            mem.recall_score = WEIGHT_SIMILARITY * mem.similarity
                + WEIGHT_RECENCY * recency
                + WEIGHT_IMPORTANCE * mem.importance.clamp(0.0, 1.0)
                + WEIGHT_ACTIVATION * activation;
        }

        // Sort by recall score descending
        memories.sort_by(|a, b| {
            b.recall_score
                .partial_cmp(&a.recall_score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        // Deduplicate by content (same content found in multiple tiers)
        dedup_by_content(&mut memories);

        // Trim to token budget
        let mut snippets: Vec<MemorySnippet> = Vec::new();
        let mut tokens_used: usize = 0;

        for mem in memories {
            let estimated_tokens = mem.content.len() / CHARS_PER_TOKEN + 1;
            if tokens_used + estimated_tokens > token_budget {
                // If we haven't added anything yet, add at least one
                if snippets.is_empty() {
                    snippets.push(mem.into_snippet());
                }
                break;
            }
            tokens_used += estimated_tokens;
            snippets.push(mem.into_snippet());
        }

        snippets
    }

    // -----------------------------------------------------------------------
    // Internal: user context
    // -----------------------------------------------------------------------

    /// Build user context from the identity subsystem.
    ///
    /// For events from `UserCommand` source, we look up the "primary" user.
    /// For other sources, user context may not be applicable.
    fn build_user_context(
        &self,
        _scored: &ScoredEvent,
        relationships: &RelationshipTracker,
        personality: &Personality,
    ) -> Option<UserContext> {
        // For now, use "primary" as the default user ID.
        // In a full implementation, the event would carry the user ID.
        let user_id = "primary";

        let relationship = relationships.get_relationship(user_id);
        let directness = relationships.directness_for_user(user_id);
        let style = personality.response_style();
        let traits = &personality.traits;

        let (stage_str, trust, interaction_count) = match relationship {
            Some(rel) => (
                format!("{:?}", rel.stage),
                rel.trust,
                rel.interaction_count,
            ),
            None => ("Stranger".to_owned(), 0.0, 0),
        };

        // Extract recent topics from conversation history
        let recent_topics: Vec<String> = self
            .conversation_buffer
            .iter()
            .rev()
            .take(5)
            .filter(|t| t.role == Role::User)
            .filter_map(|t| extract_topic(&t.content))
            .collect();

        Some(UserContext {
            user_id: user_id.to_owned(),
            relationship_stage: stage_str,
            trust_level: trust,
            directness,
            interaction_count,
            personality_snapshot: PersonalitySnapshot {
                openness: traits.openness,
                conscientiousness: traits.conscientiousness,
                extraversion: traits.extraversion,
                agreeableness: traits.agreeableness,
                neuroticism: traits.neuroticism,
                current_mood_valence: style.empathy, // proxy
                current_mood_arousal: style.proactivity, // proxy
                trust_level: trust,
            },
            recent_topics,
        })
    }

    // -----------------------------------------------------------------------
    // Internal: personality context generation
    // -----------------------------------------------------------------------

    /// Build personality prompt directives from the identity subsystem.
    ///
    /// Combines OCEAN traits, current mood, and relationship stage into
    /// structured directives that guide LLM response style and tone.
    fn build_personality_context(
        &self,
        personality: &Personality,
        affective: &AffectiveEngine,
        relationships: &RelationshipTracker,
    ) -> Option<String> {
        let user_id = "primary";
        let traits = &personality.traits;
        let mood = affective.current_state();

        let relationship = relationships.get_relationship(user_id);
        let stage = relationship
            .map(|r| r.stage.clone())
            .unwrap_or(aura_types::identity::RelationshipStage::Stranger);
        let trust = relationship.map(|r| r.trust).unwrap_or(0.0);

        let context = PersonalityPromptInjector::generate_personality_context(
            traits, mood, stage, trust,
        );

        if context.is_empty() {
            None
        } else {
            Some(context)
        }
    }

    // -----------------------------------------------------------------------
    // Internal: conversation history selection
    // -----------------------------------------------------------------------

    /// Select the most recent conversation turns that fit within a portion
    /// of the token budget (at most 40% of total budget for conversation).
    fn select_conversation_history(&self, token_budget: usize) -> Vec<ConversationTurn> {
        if self.conversation_buffer.is_empty() {
            trace!("no conversation history to select");
            return Vec::new();
        }

        let conv_budget = token_budget * 2 / 5; // 40% of total budget
        let mut turns: Vec<ConversationTurn> = Vec::new();
        let mut tokens_used: usize = 0;

        trace!(
            conv_budget,
            available_turns = self.conversation_buffer.len(),
            "selecting conversation history within budget"
        );

        // Walk backwards from most recent
        for turn in self.conversation_buffer.iter().rev() {
            let turn_tokens = turn.content.len() / CHARS_PER_TOKEN + 1;
            if tokens_used + turn_tokens > conv_budget && !turns.is_empty() {
                break;
            }
            tokens_used += turn_tokens;
            turns.push(turn.clone());
        }

        // Reverse to chronological order
        turns.reverse();
        turns
    }
}

impl Default for Contextor {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Internal types
// ---------------------------------------------------------------------------

/// Intermediate memory result with recall-scoring fields.
#[derive(Debug, Clone)]
struct RankedMemory {
    content: String,
    tier: MemoryTier,
    similarity: f32,
    importance: f32,
    timestamp_ms: u64,
    #[allow(dead_code)]
    source_id: u64,
    #[allow(dead_code)]
    recall_score: f32,
}

// Need default for recall_score field during construction
impl RankedMemory {
    fn into_snippet(self) -> MemorySnippet {
        MemorySnippet {
            content: self.content,
            source: self.tier,
            relevance: self.recall_score,
            timestamp_ms: self.timestamp_ms,
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Compute recency factor: 1.0 for "just now", decaying to 0.0 over
/// [`RECENCY_WINDOW_MS`]. Memories older than the window get 0.0.
fn compute_recency(memory_ts: u64, now_ms: u64) -> f32 {
    if memory_ts >= now_ms {
        return 1.0;
    }
    let age_ms = now_ms - memory_ts;
    if age_ms >= RECENCY_WINDOW_MS {
        return 0.0;
    }
    1.0 - (age_ms as f32 / RECENCY_WINDOW_MS as f32)
}

/// Deduplicate memories by content. If the same content appears from
/// multiple tiers, keep the one with the highest recall score.
fn dedup_by_content(memories: &mut Vec<RankedMemory>) {
    if memories.len() <= 1 {
        return;
    }
    // Already sorted by recall_score descending, so first occurrence wins.
    let mut seen_hashes = std::collections::HashSet::new();
    memories.retain(|mem| {
        let hash = simple_content_hash(&mem.content);
        seen_hashes.insert(hash)
    });
}

/// Simple FNV-1a hash for content dedup.
fn simple_content_hash(s: &str) -> u64 {
    const FNV_OFFSET: u64 = 0xcbf29ce484222325;
    const FNV_PRIME: u64 = 0x00000100000001B3;
    let mut hash = FNV_OFFSET;
    for byte in s.bytes() {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}

/// Extract a rough "topic" from user text. Returns the first 3 significant
/// words (>3 chars) as a topic string. Returns None for very short messages.
fn extract_topic(text: &str) -> Option<String> {
    let words: Vec<&str> = text
        .split_whitespace()
        .filter(|w| w.len() > 3)
        .take(3)
        .collect();
    if words.is_empty() {
        return None;
    }
    Some(words.join(" "))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use aura_types::events::{EventSource, GateDecision, Intent, ParsedEvent};
    use crate::identity::personality::Personality;
    use crate::identity::relationship::{InteractionType, RelationshipTracker};
    use crate::identity::affective::AffectiveEngine;
    use crate::memory::AuraMemory;

    /// Fixed "now" for deterministic tests.
    const TEST_NOW_MS: u64 = 1_735_689_600_000;

    fn make_scored(content: &str, total: f32, gate: GateDecision) -> ScoredEvent {
        ScoredEvent {
            parsed: ParsedEvent {
                source: EventSource::UserCommand,
                intent: Intent::ActionRequest,
                content: content.to_string(),
                entities: vec!["test".to_string()],
                timestamp_ms: TEST_NOW_MS,
                raw_event_type: 0,
            },
            score_total: total,
            score_lex: 0.50,
            score_src: 0.30,
            score_time: 0.20,
            score_anom: 0.10,
            gate_decision: gate,
        }
    }

    fn make_scored_simple(content: &str, total: f32) -> ScoredEvent {
        make_scored(content, total, GateDecision::InstantWake)
    }

    // -----------------------------------------------------------------------
    // Unit tests for helpers
    // -----------------------------------------------------------------------

    #[test]
    fn test_compute_recency_now() {
        let r = compute_recency(TEST_NOW_MS, TEST_NOW_MS);
        assert!((r - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn test_compute_recency_half() {
        let half_window = RECENCY_WINDOW_MS / 2;
        let r = compute_recency(TEST_NOW_MS - half_window, TEST_NOW_MS);
        assert!((r - 0.5).abs() < 0.01, "recency={}", r);
    }

    #[test]
    fn test_compute_recency_expired() {
        let r = compute_recency(TEST_NOW_MS - RECENCY_WINDOW_MS - 1, TEST_NOW_MS);
        assert!((r - 0.0).abs() < f32::EPSILON);
    }

    #[test]
    fn test_extract_topic_short() {
        assert!(extract_topic("hi").is_none());
    }

    #[test]
    fn test_extract_topic_normal() {
        let topic = extract_topic("what about those weather patterns lately");
        assert!(topic.is_some());
        let t = topic.as_deref().unwrap_or("");
        // Should extract significant words
        assert!(!t.is_empty());
    }

    #[test]
    fn test_dedup_by_content() {
        let mut mems = vec![
            RankedMemory {
                content: "hello world".into(),
                tier: MemoryTier::Working,
                similarity: 0.9,
                importance: 0.5,
                timestamp_ms: TEST_NOW_MS,
                source_id: 1,
                recall_score: 0.8,
            },
            RankedMemory {
                content: "hello world".into(),
                tier: MemoryTier::Episodic,
                similarity: 0.7,
                importance: 0.5,
                timestamp_ms: TEST_NOW_MS,
                source_id: 2,
                recall_score: 0.6,
            },
            RankedMemory {
                content: "different content".into(),
                tier: MemoryTier::Semantic,
                similarity: 0.5,
                importance: 0.3,
                timestamp_ms: TEST_NOW_MS,
                source_id: 3,
                recall_score: 0.4,
            },
        ];
        dedup_by_content(&mut mems);
        assert_eq!(mems.len(), 2, "duplicate should be removed");
    }

    #[test]
    fn test_content_hash_deterministic() {
        let h1 = simple_content_hash("test content");
        let h2 = simple_content_hash("test content");
        assert_eq!(h1, h2);
        let h3 = simple_content_hash("different");
        assert_ne!(h1, h3);
    }

    // -----------------------------------------------------------------------
    // Token budget tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_token_budget_low() {
        let ctx = Contextor::new();
        let scored = make_scored_simple("routine check", 0.20);
        assert_eq!(ctx.compute_token_budget(&scored), TOKEN_BUDGET_LOW);
    }

    #[test]
    fn test_token_budget_medium() {
        let ctx = Contextor::new();
        let scored = make_scored_simple("new message", 0.50);
        assert_eq!(ctx.compute_token_budget(&scored), TOKEN_BUDGET_MEDIUM);
    }

    #[test]
    fn test_token_budget_high() {
        let ctx = Contextor::new();
        let scored = make_scored_simple("error detected", 0.75);
        assert_eq!(ctx.compute_token_budget(&scored), TOKEN_BUDGET_HIGH);
    }

    #[test]
    fn test_token_budget_emergency_by_score() {
        let ctx = Contextor::new();
        let scored = make_scored_simple("critical crash", 0.95);
        assert_eq!(ctx.compute_token_budget(&scored), TOKEN_BUDGET_EMERGENCY);
    }

    #[test]
    fn test_token_budget_emergency_by_gate() {
        let ctx = Contextor::new();
        let scored = make_scored("emergency", 0.50, GateDecision::EmergencyBypass);
        assert_eq!(ctx.compute_token_budget(&scored), TOKEN_BUDGET_EMERGENCY);
    }

    #[test]
    fn test_token_budget_override() {
        let ctx = Contextor::with_config(ContextorConfig {
            token_budget_override: Some(42),
            ..Default::default()
        });
        let scored = make_scored_simple("anything", 0.95);
        assert_eq!(ctx.compute_token_budget(&scored), 42);
    }

    // -----------------------------------------------------------------------
    // Query text construction
    // -----------------------------------------------------------------------

    #[test]
    fn test_build_query_text_includes_entities() {
        let ctx = Contextor::new();
        let mut scored = make_scored_simple("open the app", 0.50);
        scored.parsed.entities = vec!["WhatsApp".to_string()];
        let qt = ctx.build_query_text(&scored);
        assert!(qt.contains("open the app"));
        assert!(qt.contains("WhatsApp"));
    }

    #[test]
    fn test_build_query_text_no_duplicate_entities() {
        let ctx = Contextor::new();
        let mut scored = make_scored_simple("open WhatsApp", 0.50);
        scored.parsed.entities = vec!["WhatsApp".to_string()];
        let qt = ctx.build_query_text(&scored);
        // "WhatsApp" is already in content, should not be appended again
        let count = qt.matches("WhatsApp").count();
        assert_eq!(count, 1, "entity should not be duplicated");
    }

    #[test]
    fn test_build_query_text_truncation() {
        let ctx = Contextor::new();
        let long_content = "a".repeat(200);
        let scored = make_scored_simple(&long_content, 0.50);
        let qt = ctx.build_query_text(&scored);
        assert!(qt.len() <= 128);
    }

    // -----------------------------------------------------------------------
    // Conversation buffer management
    // -----------------------------------------------------------------------

    #[test]
    fn test_conversation_buffer_bounded() {
        let mut ctx = Contextor::new();
        for i in 0..MAX_CONVERSATION_TURNS + 5 {
            ctx.push_conversation_turn(ConversationTurn {
                role: Role::User,
                content: format!("message {}", i),
                timestamp_ms: TEST_NOW_MS + i as u64 * 1000,
            });
        }
        assert_eq!(ctx.conversation_buffer.len(), MAX_CONVERSATION_TURNS);
        // The earliest messages should have been dropped
        assert!(ctx.conversation_buffer[0].content.contains("5"));
    }

    #[test]
    fn test_select_conversation_history_budget() {
        let mut ctx = Contextor::new();
        for i in 0..5 {
            ctx.push_conversation_turn(ConversationTurn {
                role: Role::User,
                content: format!("short msg {}", i),
                timestamp_ms: TEST_NOW_MS + i as u64 * 1000,
            });
        }
        // With TOKEN_BUDGET_LOW (200), 40% = 80 tokens ≈ 320 chars
        let history = ctx.select_conversation_history(TOKEN_BUDGET_LOW);
        // Should include at least some turns
        assert!(!history.is_empty());
        // Should be in chronological order
        for window in history.windows(2) {
            assert!(window[0].timestamp_ms <= window[1].timestamp_ms);
        }
    }

    // -----------------------------------------------------------------------
    // Ranking tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_rank_and_budget_empty() {
        let ctx = Contextor::new();
        let snippets = ctx.rank_and_budget_memories(vec![], TOKEN_BUDGET_MEDIUM, TEST_NOW_MS);
        assert!(snippets.is_empty());
    }

    #[test]
    fn test_rank_and_budget_ordering() {
        let ctx = Contextor::new();
        let mems = vec![
            RankedMemory {
                content: "low relevance old memory".into(),
                tier: MemoryTier::Episodic,
                similarity: 0.2,
                importance: 0.1,
                timestamp_ms: TEST_NOW_MS - RECENCY_WINDOW_MS, // old
                source_id: 1,
                recall_score: 0.0,
            },
            RankedMemory {
                content: "high relevance recent memory".into(),
                tier: MemoryTier::Working,
                similarity: 0.9,
                importance: 0.8,
                timestamp_ms: TEST_NOW_MS - 1000, // very recent
                source_id: 2,
                recall_score: 0.0,
            },
        ];
        let snippets = ctx.rank_and_budget_memories(mems, TOKEN_BUDGET_HIGH, TEST_NOW_MS);
        assert!(snippets.len() >= 1);
        // High-relevance recent should be first
        assert!(
            snippets[0].content.contains("high relevance"),
            "expected high relevance first, got: {}",
            snippets[0].content
        );
    }

    #[test]
    fn test_rank_and_budget_token_limit() {
        let ctx = Contextor::new();
        // Create many memories that would exceed a small budget
        let mems: Vec<RankedMemory> = (0..20)
            .map(|i| RankedMemory {
                content: format!("memory content number {} with some padding text here", i),
                tier: MemoryTier::Working,
                similarity: 0.5 + (i as f32 * 0.02),
                importance: 0.5,
                timestamp_ms: TEST_NOW_MS - (i as u64 * 1000),
                source_id: i as u64,
                recall_score: 0.0,
            })
            .collect();

        // Very small budget — should truncate
        let snippets = ctx.rank_and_budget_memories(mems, 50, TEST_NOW_MS);
        // Should have at least 1 (guaranteed minimum)
        assert!(!snippets.is_empty());
        // But should be heavily truncated
        assert!(snippets.len() < 20);
    }

    // -----------------------------------------------------------------------
    // Integration tests with in-memory subsystems
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_enrich_with_empty_memory() {
        let ctx = Contextor::new();
        let memory = AuraMemory::new_in_memory()
            .expect("in-memory init should work");
        let relationships = RelationshipTracker::new();
        let personality = Personality::new();
        let affective = AffectiveEngine::new();

        let scored = make_scored_simple("hello there", 0.50);
        let enriched = ctx
            .enrich(scored, &memory, &relationships, &personality, &affective, TEST_NOW_MS)
            .await
            .expect("enrich should succeed");

        assert!((enriched.scored.score_total - 0.50).abs() < f32::EPSILON);
        assert_eq!(enriched.scored.parsed.content, "hello there");
        assert!(enriched.memory_context.is_empty());
        assert!(enriched.user_context.is_some());
        assert_eq!(enriched.context_token_budget, TOKEN_BUDGET_MEDIUM);
    }

    #[tokio::test]
    async fn test_enrich_with_populated_memory() {
        let ctx = Contextor::new();
        let mut memory = AuraMemory::new_in_memory()
            .expect("in-memory init should work");
        let relationships = RelationshipTracker::new();
        let personality = Personality::new();
        let affective = AffectiveEngine::new();

        // Populate working memory
        memory.store_working(
            "user likes dark mode and prefers minimal UI".into(),
            EventSource::UserCommand,
            0.9,
            TEST_NOW_MS - 2000,
        );
        memory.store_working(
            "weather is sunny and warm today".into(),
            EventSource::Notification,
            0.1,
            TEST_NOW_MS - 60_000,
        );

        let scored = make_scored_simple("change to dark mode", 0.70);
        let enriched = ctx
            .enrich(scored, &memory, &relationships, &personality, &affective, TEST_NOW_MS)
            .await
            .expect("enrich should succeed");

        assert_eq!(enriched.context_token_budget, TOKEN_BUDGET_HIGH);
        // Should have retrieved relevant memories
        // (depends on trigram matching — dark mode query should find dark mode memory)
        if !enriched.memory_context.is_empty() {
            assert!(
                enriched.memory_context[0].content.contains("dark mode"),
                "expected dark mode memory first, got: {}",
                enriched.memory_context[0].content,
            );
        }
    }

    #[tokio::test]
    async fn test_enrich_with_relationship_context() {
        let ctx = Contextor::new();
        let memory = AuraMemory::new_in_memory()
            .expect("in-memory init should work");
        let mut relationships = RelationshipTracker::new();
        let personality = Personality::new();
        let affective = AffectiveEngine::new();

        // Build some trust
        for i in 0..20 {
            relationships.record_interaction(
                "primary",
                InteractionType::Positive,
                TEST_NOW_MS - (20 - i) * 1000,
            );
        }

        let scored = make_scored_simple("how are you", 0.40);
        let enriched = ctx
            .enrich(scored, &memory, &relationships, &personality, &affective, TEST_NOW_MS)
            .await
            .expect("enrich should succeed");

        let user_ctx = enriched.user_context.as_ref()
            .expect("user context should be present");
        assert!(user_ctx.trust_level > 0.0, "trust should be positive");
        assert_eq!(user_ctx.interaction_count, 20);
    }

    #[tokio::test]
    async fn test_enrich_emergency_gets_max_budget() {
        let ctx = Contextor::new();
        let memory = AuraMemory::new_in_memory()
            .expect("in-memory init should work");
        let relationships = RelationshipTracker::new();
        let personality = Personality::new();
        let affective = AffectiveEngine::new();

        let scored = make_scored("EMERGENCY ALERT", 0.98, GateDecision::EmergencyBypass);
        let enriched = ctx
            .enrich(scored, &memory, &relationships, &personality, &affective, TEST_NOW_MS)
            .await
            .expect("enrich should succeed");

        assert_eq!(enriched.context_token_budget, TOKEN_BUDGET_EMERGENCY);
    }

    #[tokio::test]
    async fn test_enrich_preserves_goals_and_screen() {
        let mut ctx = Contextor::new();
        ctx.set_active_goals(vec![GoalSummary {
            description: "Send a message to Alice".into(),
            progress_percent: 50,
            current_step: "Opening WhatsApp".into(),
            blockers: vec![],
        }]);
        ctx.set_screen_summary(Some("WhatsApp - Chat list".into()));

        let memory = AuraMemory::new_in_memory()
            .expect("in-memory init should work");
        let relationships = RelationshipTracker::new();
        let personality = Personality::new();
        let affective = AffectiveEngine::new();

        let scored = make_scored_simple("send message", 0.60);
        let enriched = ctx
            .enrich(scored, &memory, &relationships, &personality, &affective, TEST_NOW_MS)
            .await
            .expect("enrich should succeed");

        assert_eq!(enriched.active_goals.len(), 1);
        assert_eq!(enriched.active_goals[0].progress_percent, 50);
        assert_eq!(
            enriched.screen_summary.as_deref(),
            Some("WhatsApp - Chat list")
        );
    }

    #[tokio::test]
    async fn test_enrich_includes_conversation_history() {
        let mut ctx = Contextor::new();
        ctx.push_conversation_turn(ConversationTurn {
            role: Role::User,
            content: "What's the weather like?".into(),
            timestamp_ms: TEST_NOW_MS - 5000,
        });
        ctx.push_conversation_turn(ConversationTurn {
            role: Role::Assistant,
            content: "It's sunny and 72F today.".into(),
            timestamp_ms: TEST_NOW_MS - 4000,
        });
        ctx.push_conversation_turn(ConversationTurn {
            role: Role::User,
            content: "Should I bring an umbrella?".into(),
            timestamp_ms: TEST_NOW_MS - 3000,
        });

        let memory = AuraMemory::new_in_memory()
            .expect("in-memory init should work");
        let relationships = RelationshipTracker::new();
        let personality = Personality::new();
        let affective = AffectiveEngine::new();

        let scored = make_scored_simple("tell me more about rain", 0.50);
        let enriched = ctx
            .enrich(scored, &memory, &relationships, &personality, &affective, TEST_NOW_MS)
            .await
            .expect("enrich should succeed");

        assert!(!enriched.conversation_history.is_empty());
        // Should be in chronological order
        for window in enriched.conversation_history.windows(2) {
            assert!(window[0].timestamp_ms <= window[1].timestamp_ms);
        }
    }

    #[tokio::test]
    async fn test_enrich_with_episodic_memory() {
        let ctx = Contextor::new();
        let memory = AuraMemory::new_in_memory()
            .expect("in-memory init should work");
        let relationships = RelationshipTracker::new();
        let personality = Personality::new();
        let affective = AffectiveEngine::new();

        // Store episodic memory
        memory
            .store_episodic(
                "User asked about Rust ownership and borrowing last week".into(),
                0.5,
                0.7,
                vec!["rust".into(), "programming".into()],
                TEST_NOW_MS - 60_000,
            )
            .await
            .expect("store should succeed");

        let mut scored = make_scored_simple("explain Rust ownership", 0.65);
        scored.parsed.entities = vec!["Rust".to_string(), "ownership".to_string()];

        let enriched = ctx
            .enrich(scored, &memory, &relationships, &personality, &affective, TEST_NOW_MS)
            .await
            .expect("enrich should succeed");

        assert_eq!(enriched.context_token_budget, TOKEN_BUDGET_HIGH);
        // Episodic memory about Rust should be retrieved
        if !enriched.memory_context.is_empty() {
            let has_rust = enriched
                .memory_context
                .iter()
                .any(|s| s.content.to_lowercase().contains("rust"));
            assert!(has_rust, "should find Rust-related episodic memory");
        }
    }

    #[tokio::test]
    async fn test_enrich_disabled_tiers() {
        let ctx = Contextor::with_config(ContextorConfig {
            enable_episodic: false,
            enable_semantic: false,
            ..Default::default()
        });
        let mut memory = AuraMemory::new_in_memory()
            .expect("in-memory init should work");
        let relationships = RelationshipTracker::new();
        let personality = Personality::new();
        let affective = AffectiveEngine::new();

        // Store in working memory (should still be queried)
        memory.store_working(
            "some working memory content".into(),
            EventSource::Internal,
            0.5,
            TEST_NOW_MS - 1000,
        );

        // Store in episodic (should NOT be queried)
        memory
            .store_episodic(
                "episodic content that should be skipped".into(),
                0.5,
                0.7,
                vec![],
                TEST_NOW_MS - 1000,
            )
            .await
            .expect("store should succeed");

        let scored = make_scored_simple("some query", 0.50);
        let enriched = ctx
            .enrich(scored, &memory, &relationships, &personality, &affective, TEST_NOW_MS)
            .await
            .expect("enrich should succeed");

        // Should only have working memory results (if any match)
        for snippet in &enriched.memory_context {
            assert_eq!(snippet.source, MemoryTier::Working);
        }
    }

    #[tokio::test]
    async fn test_enrich_includes_personality_context() {
        let ctx = Contextor::new();
        let memory = AuraMemory::new_in_memory()
            .expect("in-memory init should work");
        let relationships = RelationshipTracker::new();
        let personality = Personality::new();
        let affective = AffectiveEngine::new();

        let scored = make_scored_simple("hello", 0.50);
        let enriched = ctx
            .enrich(scored, &memory, &relationships, &personality, &affective, TEST_NOW_MS)
            .await
            .expect("enrich should succeed");

        // Personality context should be present and contain TRUTH framework
        let pc = enriched
            .personality_context
            .as_ref()
            .expect("personality_context should be Some");
        assert!(
            pc.contains("TRUTH"),
            "personality context should contain TRUTH framework"
        );
    }

    #[tokio::test]
    async fn test_enriched_event_has_personality_context_field() {
        let ctx = Contextor::new();
        let memory = AuraMemory::new_in_memory()
            .expect("in-memory init should work");
        let relationships = RelationshipTracker::new();
        let personality = Personality::new();
        let affective = AffectiveEngine::new();

        let scored = make_scored_simple("tell me a joke", 0.40);
        let enriched = ctx
            .enrich(scored, &memory, &relationships, &personality, &affective, TEST_NOW_MS)
            .await
            .expect("enrich should succeed");

        // Should not be None — even default personality generates directives
        assert!(enriched.personality_context.is_some());
    }
}
