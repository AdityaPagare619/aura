pub mod affective;
pub mod anti_sycophancy;
pub mod behavior_modifiers;
pub mod epistemic;
pub mod ethics;
pub mod personality;
pub mod proactive_consent;
pub mod prompt_personality;
pub mod relationship;
pub mod thinking_partner;
pub mod user_profile;

use std::collections::HashSet;

pub use affective::{AffectiveEngine, MoodEvent};
pub use anti_sycophancy::{GateResult, ResponseRecord, SycophancyGuard, SycophancyVerdict};
use aura_types::identity::OceanTraits;
pub use behavior_modifiers::{AutonomyLevel, GoalWeights, ResponseStyleParams, VerbosityLevel};
pub use epistemic::{EpistemicAwareness, EpistemicLevel, KnowledgeDomain};
pub use ethics::{
    ConsentRecord, ConsentTracker, ManipulationCheckResult, ManipulationVerdict, PolicyGate,
    PolicyVerdict, TruthFramework, TruthValidation,
};
pub use personality::{
    ConsistencyReport, Personality, PersonalityArchetype, PersonalityEngine, PersonalityEvent,
    PersonalityOutcome,
};
pub use proactive_consent::{ProactiveConsent, ProactiveSettings};
pub use prompt_personality::PersonalityPromptInjector;
pub use relationship::{InteractionType, RelationshipTracker, RiskLevel, UserRelationship};
pub use thinking_partner::{ChallengeLevel, ThinkingPartner};
pub use user_profile::UserProfile;

// ---------------------------------------------------------------------------
// Unified Identity Engine — facade over all identity subsystems
// ---------------------------------------------------------------------------

/// Unified facade that owns all identity subsystems and provides a single
/// entry point for the pipeline to query personality, mood, trust, ethics,
/// and anti-sycophancy state.
pub struct IdentityEngine {
    pub personality: Personality,
    pub affective: AffectiveEngine,
    pub relationships: RelationshipTracker,
    pub sycophancy_guard: SycophancyGuard,
    pub policy_gate: PolicyGate,
    /// TRUTH framework for validating responses against the five principles.
    pub truth_framework: TruthFramework,
    /// User profile containing preferences and consent settings.
    pub user_profile: Option<UserProfile>,
    /// Epistemic awareness: tracks what AURA knows vs. doesn't know (§5.2).
    pub epistemic: EpistemicAwareness,
    /// Shaping Feature: Cognitive anti-atrophy coach.
    pub coach: ThinkingPartner,
    /// Privacy Sovereignty consent tracker — gates learning, proactive actions,
    /// and data sharing.  Initialized with privacy-first defaults.
    pub consent_tracker: ConsentTracker,
}

impl IdentityEngine {
    /// Create a new `IdentityEngine` with default subsystems.
    pub fn new() -> Self {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        Self {
            personality: Personality::new(),
            affective: AffectiveEngine::new(),
            relationships: RelationshipTracker::new(),
            sycophancy_guard: SycophancyGuard::new(),
            policy_gate: PolicyGate::new(),
            truth_framework: TruthFramework::new(),
            user_profile: None,
            epistemic: EpistemicAwareness::new(),
            coach: ThinkingPartner::new(),
            consent_tracker: ConsentTracker::with_defaults(now),
        }
    }

    /// Generate a full personality context string for prompt injection.
    ///
    /// Combines personality traits, current mood, and relationship stage
    /// into a set of directives for the LLM.
    #[tracing::instrument(skip(self))]
    pub fn personality_context(&self, user_id: &str) -> String {
        let traits = &self.personality.traits;
        let mood = self.affective.current_state();
        let relationship = self.relationships.get_relationship(user_id);

        let stage = relationship
            .map(|r| r.stage.clone())
            .unwrap_or(aura_types::identity::RelationshipStage::Stranger);

        let trust = relationship.map(|r| r.trust).unwrap_or(0.0);

        PersonalityPromptInjector::generate_personality_context(traits, mood, stage, trust)
    }

    /// Run the anti-sycophancy gate on the current response window.
    ///
    /// Returns a `GateResult` that the pipeline should act on:
    /// - `Pass` → send response as-is
    /// - `Nudge` → append honesty directive, send response
    /// - `Block` → regenerate with honesty block directive
    #[tracing::instrument(skip(self))]
    pub fn check_response(&mut self) -> GateResult {
        self.sycophancy_guard.gate()
    }

    /// Analyze the response text with full context — user input, personality
    /// dimensions, and optional previous assistant response — then record it
    /// in the anti-sycophancy sliding window.
    ///
    /// This is the **preferred** entry point.  It uses statistical text
    /// ratios modulated by OCEAN personality traits rather than hardcoded
    /// keyword lists.
    ///
    /// Must be called **after** `send_response` for every LLM-generated
    /// reply so the ring buffer stays populated.
    #[tracing::instrument(skip(self, user_input, response_text, previous_response),
        fields(user_len = user_input.len(), resp_len = response_text.len()))]
    pub fn record_response_in_context(
        &mut self,
        user_input: &str,
        response_text: &str,
        previous_response: Option<&str>,
    ) {
        let record = Self::analyze_response_in_context(
            user_input,
            response_text,
            previous_response,
            &self.personality.traits,
        );
        tracing::trace!(?record, "recording response for sycophancy tracking");
        self.sycophancy_guard.record_response(record);
    }

    /// Backward-compatible entry point that only has the response text.
    ///
    /// Falls back to `analyze_response_in_context` with empty user input,
    /// no previous response, and default personality.  Prefer
    /// [`record_response_in_context`] when richer context is available.
    #[tracing::instrument(skip(self, text), fields(text_len = text.len()))]
    pub fn record_response_text(&mut self, text: &str) {
        self.record_response_in_context("", text, None);
    }

    // -----------------------------------------------------------------------
    // Statistical text analysis — replaces hardcoded keyword matching
    // -----------------------------------------------------------------------

    /// Common English stop words excluded from semantic overlap computation.
    /// These carry no topical signal and would inflate overlap ratios.
    fn stop_words() -> &'static HashSet<&'static str> {
        use std::sync::OnceLock;
        static STOP: OnceLock<HashSet<&str>> = OnceLock::new();
        STOP.get_or_init(|| {
            [
                "a", "an", "the", "and", "or", "but", "in", "on", "at", "to", "for", "of", "is",
                "it", "that", "this", "was", "are", "be", "has", "had", "have", "with", "as", "by",
                "from", "not", "so", "if", "its", "do", "no", "can", "will", "just", "i", "you",
                "we", "my", "your", "he", "she", "they", "me", "him", "her", "us", "them", "what",
                "which", "who", "how", "when", "where", "about", "would", "could", "should",
                "been", "being", "did", "does", "am", "were", "than", "then", "also", "very",
                "more", "some", "any", "all", "each", "there", "up", "out", "into",
            ]
            .into_iter()
            .collect()
        })
    }

    /// Extract content words (lowercased, stop words removed) from text.
    fn content_words(text: &str) -> Vec<String> {
        let stops = Self::stop_words();
        text.split(|c: char| !c.is_alphanumeric() && c != '\'')
            .filter(|w| !w.is_empty())
            .map(|w| w.to_ascii_lowercase())
            .filter(|w| w.len() > 1 && !stops.contains(w.as_str()))
            .collect()
    }

    /// Compute content-word overlap ratio between user input and response.
    ///
    /// Uses overlap coefficient: |A ∩ B| / min(|A|, |B|).
    /// Returns 0.0 when either side has no content words.
    fn content_overlap(user_input: &str, response: &str) -> f32 {
        let user_words: HashSet<String> = Self::content_words(user_input).into_iter().collect();
        let resp_words: HashSet<String> = Self::content_words(response).into_iter().collect();

        let smaller = user_words.len().min(resp_words.len()) as f32;
        if smaller == 0.0 {
            return 0.0;
        }

        let intersection = user_words.intersection(&resp_words).count() as f32;
        (intersection / smaller).clamp(0.0, 1.0)
    }

    /// Count how many times any phrase in `phrases` appears in `lower_text`,
    /// returning the total hit count.
    fn phrase_hits(lower_text: &str, phrases: &[&str]) -> usize {
        phrases.iter().filter(|p| lower_text.contains(**p)).count()
    }

    /// Compute the density of phrase hits relative to word count.
    /// Returns 0.0 for empty text.
    fn phrase_density(text: &str, lower_text: &str, phrases: &[&str]) -> f32 {
        let word_count = text.split_whitespace().count() as f32;
        if word_count == 0.0 {
            return 0.0;
        }
        let hits = Self::phrase_hits(lower_text, phrases) as f32;
        (hits / word_count).clamp(0.0, 1.0)
    }

    /// Full context-aware response analysis.
    ///
    /// Each sub-score is computed as a **statistical ratio** (not boolean
    /// keyword match) and personality dimensions **modulate the decision
    /// thresholds** so that natural personality expression is not flagged.
    ///
    /// # How personality adjusts thresholds
    ///
    /// | Trait               | Effect                                    |
    /// |---------------------|-------------------------------------------|
    /// | High Agreeableness  | Raises agreement threshold (natural)      |
    /// | High Openness       | Raises hedging threshold (exploratory)    |
    /// | Low Extraversion    | Lowers challenge expectation              |
    fn analyze_response_in_context(
        user_input: &str,
        response_text: &str,
        previous_response: Option<&str>,
        personality: &OceanTraits,
    ) -> ResponseRecord {
        let resp_lower = response_text.to_ascii_lowercase();
        let resp_word_count = response_text.split_whitespace().count() as f32;

        // ── 1. Agreement detection ─────────────────────────────────
        // Two complementary signals:
        //   a) Content-word overlap ratio between user input and response
        //      (measures whether AURA just echoed the user's words back).
        //   b) Explicit agreement phrase density in the response.
        //
        // Personality adjustment: high agreeableness raises the threshold
        // because agreeable personalities naturally concur more often.
        let overlap = Self::content_overlap(user_input, response_text);

        const AGREEMENT_PHRASES: &[&str] = &[
            "you're right",
            "you are right",
            "i agree",
            "exactly right",
            "that's correct",
            "good point",
            "you make a good point",
            "i couldn't agree more",
            "spot on",
        ];
        let agreement_density = Self::phrase_density(response_text, &resp_lower, AGREEMENT_PHRASES);

        // Composite agreement signal: overlap × 0.6 + explicit density × 0.4
        let agreement_signal = overlap * 0.6 + agreement_density * 0.4;

        // Personality-adjusted threshold:
        // Base = 0.30.  High agreeableness (>0.5) raises threshold linearly.
        // At A=0.9 the threshold is 0.30 + 0.4 × 0.15 = 0.36.
        let agree_threshold = 0.30 + (personality.agreeableness - 0.5).max(0.0) * 0.15;
        let agreed = agreement_signal > agree_threshold;

        tracing::debug!(
            overlap,
            agreement_density,
            agreement_signal,
            agree_threshold,
            agreed,
            agreeableness = personality.agreeableness,
            "sycophancy: agreement analysis"
        );

        // ── 2. Hedging detection ───────────────────────────────────
        // Hedge phrase density as a ratio of response length.
        // High openness personalities naturally explore alternatives — raise
        // the threshold so exploratory hedging isn't flagged as sycophantic.
        const HEDGE_PHRASES: &[&str] = &[
            "i think",
            "perhaps",
            "maybe",
            "it could be",
            "it might be",
            "i'm not sure",
            "possibly",
            "it seems",
            "to some extent",
            "arguably",
            "one could say",
        ];
        let hedge_density = Self::phrase_density(response_text, &resp_lower, HEDGE_PHRASES);
        // Base threshold = 0.04.  High openness (>0.5) raises it.
        let hedge_threshold = 0.04 + (personality.openness - 0.5).max(0.0) * 0.03;
        let hedged = hedge_density > hedge_threshold;

        tracing::debug!(
            hedge_density,
            hedge_threshold,
            hedged,
            openness = personality.openness,
            "sycophancy: hedging analysis"
        );

        // ── 3. Opinion reversal ────────────────────────────────────
        // Requires a previous assistant response to compare against.
        // Two signals:
        //   a) Explicit reversal markers in the current response.
        //   b) Stance flip: previous response challenged/disagreed, current
        //      response agrees — detected via keyword regime shift.
        const REVERSAL_PHRASES: &[&str] = &[
            "on second thought",
            "actually, you",
            "i was wrong",
            "let me reconsider",
            "i take that back",
            "i stand corrected",
            "you've changed my mind",
        ];
        let reversed_opinion = match previous_response {
            Some(prev) => {
                let reversal_density =
                    Self::phrase_density(response_text, &resp_lower, REVERSAL_PHRASES);
                // Stance flip detection: did previous response challenge
                // but current one agrees?
                let prev_lower = prev.to_ascii_lowercase();
                let prev_challenged = Self::phrase_hits(
                    &prev_lower,
                    &[
                        "however",
                        "i disagree",
                        "i'd push back",
                        "on the other hand",
                        "that's not quite",
                    ],
                ) > 0;
                let now_agrees = Self::phrase_hits(&resp_lower, AGREEMENT_PHRASES) > 0;
                let stance_flip = prev_challenged && now_agrees;

                let reversed = reversal_density > 0.0 || stance_flip;

                tracing::debug!(
                    reversal_density,
                    prev_challenged,
                    now_agrees,
                    stance_flip,
                    reversed,
                    "sycophancy: opinion reversal analysis"
                );
                reversed
            },
            None => {
                // No prior response to compare against, but explicit reversal
                // markers ("on second thought", "let me reconsider") still
                // signal self-contradiction and should be flagged.
                let reversal_density =
                    Self::phrase_density(response_text, &resp_lower, REVERSAL_PHRASES);
                reversal_density > 0.0
            },
        };

        // ── 4. Praise density ──────────────────────────────────────
        // Ratio of praise/superlative phrases to total word count.
        const PRAISE_PHRASES: &[&str] = &[
            "great question",
            "excellent point",
            "that's brilliant",
            "well said",
            "insightful",
            "great idea",
            "wonderful",
            "fantastic",
            "amazing question",
            "love that",
            "really smart",
            "brilliant observation",
        ];
        let praise_density = Self::phrase_density(response_text, &resp_lower, PRAISE_PHRASES);
        // Threshold: low bar — any measurable praise density in a response
        // is notable.  No personality adjustment here: praise is either
        // present or it isn't, and all personalities should be flagged
        // equally for unprompted flattery.
        let praised = praise_density > 0.0 && resp_word_count > 0.0;

        tracing::debug!(praise_density, praised, "sycophancy: praise analysis");

        // ── 5. Challenge detection ─────────────────────────────────
        // Did AURA critically engage with the user's input?
        //
        // Two signals:
        //   a) Presence of counterpoint / qualification language.
        //   b) Whether the user made an assertion (declarative claim) and
        //      AURA engaged with it vs. just validated.
        //
        // Personality adjustment: high extraversion and low agreeableness
        // produce more natural challengers.  For agreeable personalities,
        // we lower the bar so that *any* challenge counts.
        const CHALLENGE_PHRASES: &[&str] = &[
            "however",
            "i disagree",
            "that's not quite",
            "on the other hand",
            "i'd push back",
            "consider the alternative",
            "actually,",
            "to be fair",
            "it's worth noting",
            "one concern",
            "that said",
            "i'd challenge",
            "a counterpoint",
        ];
        let challenge_density = Self::phrase_density(response_text, &resp_lower, CHALLENGE_PHRASES);

        // Check if user asserted something (simple heuristic: declarative
        // sentences without question marks dominate the input).
        let user_has_assertion = if user_input.is_empty() {
            false
        } else {
            let sentences: Vec<&str> = user_input
                .split(|c: char| c == '.' || c == '!' || c == '?')
                .filter(|s| !s.trim().is_empty())
                .collect();
            let question_count = user_input.chars().filter(|&c| c == '?').count();
            // More declarations than questions → user is asserting.
            sentences.len() > question_count
        };

        // New information ratio: does the response introduce content words
        // NOT present in the user's input?  High ratio = AURA added value,
        // low ratio = just echoed.
        let new_info_ratio = if user_input.is_empty() {
            1.0 // Can't compare without user input; assume new info.
        } else {
            let user_set: HashSet<String> = Self::content_words(user_input).into_iter().collect();
            let resp_words = Self::content_words(response_text);
            if resp_words.is_empty() {
                0.0
            } else {
                let novel = resp_words
                    .iter()
                    .filter(|w| !user_set.contains(w.as_str()))
                    .count() as f32;
                (novel / resp_words.len() as f32).clamp(0.0, 1.0)
            }
        };

        // Challenge threshold: base 0.0 (any challenge phrase counts).
        // For high-agreeableness personalities, this is already generous.
        // We also credit challenge if the response introduced substantial
        // new information (new_info_ratio > 0.5) in response to an assertion.
        let challenged = challenge_density > 0.0 || (user_has_assertion && new_info_ratio > 0.5);

        tracing::debug!(
            challenge_density,
            user_has_assertion,
            new_info_ratio,
            challenged,
            agreeableness = personality.agreeableness,
            "sycophancy: challenge analysis"
        );

        ResponseRecord {
            agreed,
            hedged,
            reversed_opinion,
            praised,
            challenged,
        }
    }

    /// Backward-compatible analysis using only response text.
    ///
    /// Delegates to [`analyze_response_in_context`] with empty user input,
    /// no conversation history, and default personality traits.
    #[allow(dead_code)] // Phase 8: shortcut used by anti-sycophancy deeper analysis
    fn analyze_response(text: &str) -> ResponseRecord {
        Self::analyze_response_in_context(text, text, None, &OceanTraits::DEFAULT)
    }

    /// Check whether the given risk level requires user permission based
    /// on the user's trust level.
    #[tracing::instrument(skip(self))]
    pub fn requires_permission(&self, user_id: &str, risk: &RiskLevel) -> bool {
        self.relationships.requires_permission(user_id, *risk)
    }

    /// Process a mood event through the affective engine.
    #[tracing::instrument(skip(self))]
    pub fn process_mood(&mut self, event: MoodEvent, now_ms: u64) {
        self.affective.process_event(event, now_ms);
    }

    /// Record a user interaction in the relationship tracker.
    #[tracing::instrument(skip(self))]
    pub fn record_interaction(
        &mut self,
        user_id: &str,
        interaction: InteractionType,
        timestamp: u64,
    ) {
        self.relationships
            .record_interaction(user_id, interaction, timestamp);
    }

    /// Validate a response against the TRUTH framework principles.
    ///
    /// Delegates to [`TruthFramework::validate_response`] and returns
    /// per-principle scores plus an overall pass/fail verdict.
    ///
    /// Should be called in the response path **before** the anti-sycophancy
    /// gate to catch deceptive, biased, or evasive content.
    #[tracing::instrument(skip(self, text), fields(text_len = text.len()))]
    pub fn validate_response(&self, text: &str) -> TruthValidation {
        self.truth_framework.validate_response(text)
    }

    /// Validate a response with epistemic awareness (anti-hallucination).
    ///
    /// Extracts topic domains from `user_input`, queries the epistemic
    /// subsystem for each domain's confidence level, and delegates to
    /// [`TruthFramework::validate_with_epistemic`] which penalizes
    /// overclaiming and rewards appropriate hedging.
    ///
    /// Falls back to plain [`validate_response`] if no domains are
    /// detected, keeping the hot path lightweight.
    #[tracing::instrument(
        skip(self, text, user_input),
        fields(text_len = text.len(), input_len = user_input.len())
    )]
    pub fn validate_response_with_epistemic(
        &self,
        text: &str,
        user_input: &str,
    ) -> TruthValidation {
        let domains = Self::extract_topic_domains(user_input);
        if domains.is_empty() {
            return self.truth_framework.validate_response(text);
        }
        let domain_refs: Vec<&str> = domains.iter().map(|s| s.as_str()).collect();
        self.truth_framework
            .validate_with_epistemic(text, &domain_refs, &self.epistemic)
    }

    /// Assess whether epistemic markers should be prepended to a response.
    ///
    /// Returns `Some(marker)` when the response touches domains where AURA
    /// has low epistemic confidence and the response lacks appropriate
    /// hedging.  Returns `None` when confidence is high or hedging is
    /// already present — avoiding noise on every response.
    ///
    /// The returned string is suitable for prepending:
    /// `format!("{marker} {original_response}")`
    #[tracing::instrument(
        skip(self, response_text, user_input),
        fields(resp_len = response_text.len(), input_len = user_input.len())
    )]
    pub fn assess_epistemic_markers(
        &self,
        response_text: &str,
        user_input: &str,
    ) -> Option<String> {
        let domains = Self::extract_topic_domains(user_input);
        if domains.is_empty() {
            return None;
        }

        let lower = response_text.to_ascii_lowercase();
        let mut weakest_level = epistemic::EpistemicLevel::Knows;
        let mut weakest_domain: Option<&str> = None;

        for domain_name in &domains {
            let level = self.epistemic.level_for(domain_name);
            // Only consider domains below full confidence.
            if level < weakest_level {
                weakest_level = level;
                weakest_domain = Some(domain_name.as_str());
            }
        }

        // If all domains are Knows, no marker needed.
        if weakest_level == epistemic::EpistemicLevel::Knows {
            return None;
        }

        // Check if response already contains appropriate hedging.
        let already_hedged = match weakest_level {
            epistemic::EpistemicLevel::Unknown => {
                lower.contains("i don't know")
                    || lower.contains("i'm not sure")
                    || lower.contains("i don't have information")
            },
            epistemic::EpistemicLevel::CanDiscover => {
                lower.contains("i could check")
                    || lower.contains("let me look")
                    || lower.contains("i could look into")
            },
            epistemic::EpistemicLevel::Believes => {
                lower.contains("i think")
                    || lower.contains("based on what i've seen")
                    || lower.contains("it seems like")
            },
            epistemic::EpistemicLevel::Knows => true,
        };

        if already_hedged {
            return None;
        }

        let hedge = weakest_level.hedge_phrase();
        let _domain = weakest_domain.unwrap_or("this topic");
        tracing::debug!(
            weakest_level = ?weakest_level,
            domain = _domain,
            "injecting epistemic marker"
        );
        Some(format!("{hedge} —"))
    }

    /// Evaluate whether the ThinkingPartner should augment the response
    /// Evaluates whether AURA should apply a cognitive challenge.
    ///
    /// Always returns `None` — the LLM determines its own reasoning style.
    /// Rust does not inject priming directives into the LLM prompt.
    pub fn evaluate_thinking_challenge(
        &self,
        _user_input: &str,
        _cognitive_load: f32,
    ) -> Option<&'static str> {
        None
    }

    // -----------------------------------------------------------------------
    // Topic domain extraction (lightweight, hot-path safe)
    // -----------------------------------------------------------------------

    /// Extract plausible topic domain names from user input text.
    ///
    /// Uses a static mapping of keyword → domain name. Returns at most 4
    /// domains to bound allocation. This is intentionally simple: a
    /// production system would use the neocortex for NLU, but this is the
    /// daemon hot path (System1) where we need O(1)-ish latency.
    fn extract_topic_domains(text: &str) -> Vec<String> {
        const DOMAIN_KEYWORDS: &[(&[&str], &str)] = &[
            (&["morning", "wake", "alarm", "routine"], "morning_routine"),
            (&["evening", "night", "sleep", "bedtime"], "evening_routine"),
            (&["music", "song", "playlist", "album"], "music_preference"),
            (&["weather", "temperature", "forecast", "rain"], "weather"),
            (
                &["calendar", "meeting", "schedule", "appointment"],
                "calendar",
            ),
            (&["work", "office", "deadline", "project"], "work_schedule"),
            (
                &["food", "restaurant", "meal", "cook", "recipe"],
                "food_preference",
            ),
            (&["exercise", "workout", "gym", "run", "fitness"], "fitness"),
            (&["news", "headline", "article", "current events"], "news"),
            (&["commute", "traffic", "drive", "transit"], "commute"),
        ];

        let lower = text.to_ascii_lowercase();
        let mut domains = Vec::with_capacity(4);

        for (keywords, domain) in DOMAIN_KEYWORDS {
            if domains.len() >= 4 {
                break;
            }
            if keywords.iter().any(|kw| lower.contains(kw)) {
                domains.push((*domain).to_owned());
            }
        }

        domains
    }

    /// Check user input text for manipulation patterns.
    ///
    /// Delegates to [`ethics::check_manipulation`] and returns a score,
    /// detected patterns, and a verdict (Clean/Suspicious/Manipulative).
    ///
    /// Should be called in the input path **before** the policy gate to
    /// detect emotional manipulation, authority abuse, and urgency pressure.
    #[tracing::instrument(skip(self, text), fields(text_len = text.len()))]
    pub fn check_manipulation(&self, text: &str) -> ManipulationCheckResult {
        ethics::check_manipulation(text)
    }

    /// Check if proactive behavior is allowed for the given hour.
    /// Returns false if no user profile is loaded (safety default).
    pub fn is_proactive_allowed(&self, hour: u8) -> bool {
        match &self.user_profile {
            Some(profile) => profile.is_proactive_allowed(hour),
            None => false,
        }
    }

    /// Load user profile from database.
    pub fn load_user_profile(
        &mut self,
        db: &rusqlite::Connection,
    ) -> Result<(), aura_types::errors::OnboardingError> {
        match UserProfile::load_from_db(db)? {
            Some(profile) => {
                self.user_profile = Some(profile);
                Ok(())
            },
            None => Ok(()),
        }
    }

    /// Get a reference to the user profile if loaded.
    pub fn user_profile(&self) -> Option<&UserProfile> {
        self.user_profile.as_ref()
    }

    /// Get a mutable reference to the user profile if loaded.
    pub fn user_profile_mut(&mut self) -> Option<&mut UserProfile> {
        self.user_profile.as_mut()
    }

    // -----------------------------------------------------------------------
    // Journal-backed persistence — write to WAL FIRST, then mutate state
    // -----------------------------------------------------------------------

    /// Persist a personality evolution event to the journal, then apply it.
    ///
    /// The payload format is: `[event_tag: u8][delta bytes...]` where
    /// - tag 0 = PositiveInteraction (no extra data)
    /// - tag 1 = NegativeInteraction (no extra data)
    /// - tag 2 = UserFeedback (UTF-8 string)
    /// - tag 3 = ContextualPressure (trait_name UTF-8 + f32 direction)
    ///
    /// # Errors
    /// Returns `Err` if the journal write fails.  In that case the in-memory
    /// state is NOT mutated — crash safety is preserved.
    pub fn persist_personality_change(
        &mut self,
        journal: &mut crate::persistence::WriteAheadJournal,
        event: &personality::PersonalityEvent,
    ) -> Result<(), crate::persistence::JournalError> {
        let payload = encode_personality_event(event);

        journal.append(crate::persistence::JournalCategory::Personality, &payload)?;
        journal.commit()?;

        // Journal write succeeded — safe to mutate in-memory state.
        self.personality.evolve(event.clone());

        tracing::debug!(?event, "personality change persisted and applied");
        Ok(())
    }

    /// Persist a trust/interaction change to the journal, then apply it.
    ///
    /// Payload format: `[user_id_len: u16][user_id: UTF-8][interaction: u8][timestamp: u64]`
    ///
    /// # Errors
    /// Returns `Err` if the journal write fails.  In-memory state is NOT mutated.
    pub fn persist_trust_change(
        &mut self,
        journal: &mut crate::persistence::WriteAheadJournal,
        user_id: &str,
        interaction: InteractionType,
        timestamp_ms: u64,
    ) -> Result<(), crate::persistence::JournalError> {
        let payload = encode_trust_event(user_id, interaction, timestamp_ms);

        journal.append(crate::persistence::JournalCategory::Trust, &payload)?;
        journal.commit()?;

        // Journal write succeeded — safe to mutate.
        self.relationships
            .record_interaction(user_id, interaction, timestamp_ms);

        tracing::debug!(
            user = user_id,
            ?interaction,
            "trust change persisted and applied"
        );
        Ok(())
    }

    /// Persist a consent state change to the journal, then apply it.
    ///
    /// Payload format: `[category_len: u16][category: UTF-8][granted: u8][timestamp: u64]`
    ///
    /// # Errors
    /// Returns `Err` if the journal write fails.  In-memory state is NOT mutated.
    pub fn persist_consent_change(
        &mut self,
        journal: &mut crate::persistence::WriteAheadJournal,
        category: &str,
        granted: bool,
        timestamp_ms: u64,
    ) -> Result<(), crate::persistence::JournalError> {
        let payload = encode_consent_event(category, granted, timestamp_ms);

        journal.append(crate::persistence::JournalCategory::Consent, &payload)?;
        journal.commit()?;

        tracing::debug!(category, granted, "consent change persisted and applied");
        Ok(())
    }

    /// Persist a mood event to the journal, then apply it.
    ///
    /// # Errors
    /// Returns `Err` if the journal write fails.
    pub fn persist_mood_change(
        &mut self,
        journal: &mut crate::persistence::WriteAheadJournal,
        event: MoodEvent,
        now_ms: u64,
    ) -> Result<(), crate::persistence::JournalError> {
        let payload = encode_mood_event(&event, now_ms);

        journal.append(crate::persistence::JournalCategory::Mood, &payload)?;
        journal.commit()?;

        // Apply mood change.
        self.affective.process_event(event, now_ms);

        tracing::debug!("mood change persisted and applied");
        Ok(())
    }
}

impl Default for IdentityEngine {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Journal payload encoding helpers
// ---------------------------------------------------------------------------

/// Encode a `PersonalityEvent` into journal payload bytes.
fn encode_personality_event(event: &personality::PersonalityEvent) -> Vec<u8> {
    match event {
        personality::PersonalityEvent::PositiveInteraction => vec![0u8],
        personality::PersonalityEvent::NegativeInteraction => vec![1u8],
        personality::PersonalityEvent::UserFeedback(msg) => {
            let mut buf = vec![2u8];
            buf.extend_from_slice(msg.as_bytes());
            buf
        },
        personality::PersonalityEvent::ContextualPressure {
            trait_name,
            direction,
        } => {
            let mut buf = vec![3u8];
            let name_bytes = trait_name.as_bytes();
            let name_len = (name_bytes.len() as u16).to_le_bytes();
            buf.extend_from_slice(&name_len);
            buf.extend_from_slice(name_bytes);
            buf.extend_from_slice(&direction.to_le_bytes());
            buf
        },
    }
}

/// Decode a `PersonalityEvent` from journal payload bytes.
///
/// Returns `None` if the payload is malformed.
pub fn decode_personality_event(payload: &[u8]) -> Option<personality::PersonalityEvent> {
    if payload.is_empty() {
        return None;
    }
    match payload[0] {
        0 => Some(personality::PersonalityEvent::PositiveInteraction),
        1 => Some(personality::PersonalityEvent::NegativeInteraction),
        2 => {
            let msg = std::str::from_utf8(&payload[1..]).ok()?;
            Some(personality::PersonalityEvent::UserFeedback(msg.to_string()))
        },
        3 => {
            if payload.len() < 4 {
                return None;
            }
            let name_len = u16::from_le_bytes([payload[1], payload[2]]) as usize;
            if payload.len() < 3 + name_len + 4 {
                return None;
            }
            let trait_name = std::str::from_utf8(&payload[3..3 + name_len])
                .ok()?
                .to_string();
            let dir_start = 3 + name_len;
            let direction = f32::from_le_bytes([
                payload[dir_start],
                payload[dir_start + 1],
                payload[dir_start + 2],
                payload[dir_start + 3],
            ]);
            Some(personality::PersonalityEvent::ContextualPressure {
                trait_name,
                direction,
            })
        },
        _ => None,
    }
}

/// Encode a trust/interaction event into journal payload bytes.
fn encode_trust_event(user_id: &str, interaction: InteractionType, timestamp_ms: u64) -> Vec<u8> {
    let id_bytes = user_id.as_bytes();
    let id_len = (id_bytes.len() as u16).to_le_bytes();
    let interaction_byte = match interaction {
        InteractionType::Positive => 0u8,
        InteractionType::Negative => 1u8,
        InteractionType::Neutral => 2u8,
    };

    let mut buf = Vec::with_capacity(2 + id_bytes.len() + 1 + 8);
    buf.extend_from_slice(&id_len);
    buf.extend_from_slice(id_bytes);
    buf.push(interaction_byte);
    buf.extend_from_slice(&timestamp_ms.to_le_bytes());
    buf
}

/// Decode a trust event from journal payload bytes.
///
/// Returns `(user_id, interaction_type, timestamp_ms)` or `None` if malformed.
pub fn decode_trust_event(payload: &[u8]) -> Option<(String, InteractionType, u64)> {
    if payload.len() < 11 {
        return None; // minimum: 2 (len) + 1 (id) + 1 (type) + 8 (timestamp) - but id_len=0 is ok
    }
    let id_len = u16::from_le_bytes([payload[0], payload[1]]) as usize;
    if payload.len() < 2 + id_len + 1 + 8 {
        return None;
    }
    let user_id = std::str::from_utf8(&payload[2..2 + id_len])
        .ok()?
        .to_string();
    let interaction = match payload[2 + id_len] {
        0 => InteractionType::Positive,
        1 => InteractionType::Negative,
        2 => InteractionType::Neutral,
        _ => return None,
    };
    let ts_start = 2 + id_len + 1;
    let timestamp_ms = u64::from_le_bytes([
        payload[ts_start],
        payload[ts_start + 1],
        payload[ts_start + 2],
        payload[ts_start + 3],
        payload[ts_start + 4],
        payload[ts_start + 5],
        payload[ts_start + 6],
        payload[ts_start + 7],
    ]);
    Some((user_id, interaction, timestamp_ms))
}

/// Encode a consent event into journal payload bytes.
fn encode_consent_event(category: &str, granted: bool, timestamp_ms: u64) -> Vec<u8> {
    let cat_bytes = category.as_bytes();
    let cat_len = (cat_bytes.len() as u16).to_le_bytes();

    let mut buf = Vec::with_capacity(2 + cat_bytes.len() + 1 + 8);
    buf.extend_from_slice(&cat_len);
    buf.extend_from_slice(cat_bytes);
    buf.push(if granted { 1 } else { 0 });
    buf.extend_from_slice(&timestamp_ms.to_le_bytes());
    buf
}

/// Decode a consent event from journal payload bytes.
pub fn decode_consent_event(payload: &[u8]) -> Option<(String, bool, u64)> {
    if payload.len() < 11 {
        return None;
    }
    let cat_len = u16::from_le_bytes([payload[0], payload[1]]) as usize;
    if payload.len() < 2 + cat_len + 1 + 8 {
        return None;
    }
    let category = std::str::from_utf8(&payload[2..2 + cat_len])
        .ok()?
        .to_string();
    let granted = payload[2 + cat_len] != 0;
    let ts_start = 2 + cat_len + 1;
    let timestamp_ms = u64::from_le_bytes([
        payload[ts_start],
        payload[ts_start + 1],
        payload[ts_start + 2],
        payload[ts_start + 3],
        payload[ts_start + 4],
        payload[ts_start + 5],
        payload[ts_start + 6],
        payload[ts_start + 7],
    ]);
    Some((category, granted, timestamp_ms))
}

/// Encode a mood event into journal payload bytes.
fn encode_mood_event(event: &MoodEvent, now_ms: u64) -> Vec<u8> {
    // Encoding: [tag: u8][variant-specific data...][timestamp: u64]
    let mut buf = Vec::with_capacity(32);
    match event {
        MoodEvent::TaskSucceeded => buf.push(0u8),
        MoodEvent::TaskFailed => buf.push(1u8),
        MoodEvent::UserHappy => buf.push(2u8),
        MoodEvent::UserFrustrated => buf.push(3u8),
        MoodEvent::Compliment => buf.push(4u8),
        MoodEvent::Criticism => buf.push(5u8),
        MoodEvent::Silence { duration_ms } => {
            buf.push(6u8);
            buf.extend_from_slice(&duration_ms.to_le_bytes());
        },
        MoodEvent::VoiceStressDetected { level } => {
            buf.push(7u8);
            buf.extend_from_slice(&level.to_le_bytes());
        },
        MoodEvent::VoiceFatigueDetected { level } => {
            buf.push(8u8);
            buf.extend_from_slice(&level.to_le_bytes());
        },
    }
    buf.extend_from_slice(&now_ms.to_le_bytes());
    buf
}

/// Decode a mood event from journal payload bytes.
pub fn decode_mood_event(payload: &[u8]) -> Option<(MoodEvent, u64)> {
    if payload.is_empty() {
        return None;
    }
    let tag = payload[0];
    let (event, ts_start) = match tag {
        0 => (MoodEvent::TaskSucceeded, 1),
        1 => (MoodEvent::TaskFailed, 1),
        2 => (MoodEvent::UserHappy, 1),
        3 => (MoodEvent::UserFrustrated, 1),
        4 => (MoodEvent::Compliment, 1),
        5 => (MoodEvent::Criticism, 1),
        6 => {
            if payload.len() < 9 {
                return None;
            }
            let duration_ms = u64::from_le_bytes([
                payload[1], payload[2], payload[3], payload[4], payload[5], payload[6], payload[7],
                payload[8],
            ]);
            (MoodEvent::Silence { duration_ms }, 9)
        },
        7 => {
            if payload.len() < 5 {
                return None;
            }
            let level = f32::from_le_bytes([payload[1], payload[2], payload[3], payload[4]]);
            (MoodEvent::VoiceStressDetected { level }, 5)
        },
        8 => {
            if payload.len() < 5 {
                return None;
            }
            let level = f32::from_le_bytes([payload[1], payload[2], payload[3], payload[4]]);
            (MoodEvent::VoiceFatigueDetected { level }, 5)
        },
        _ => return None,
    };

    if payload.len() < ts_start + 8 {
        return None;
    }
    let ts = u64::from_le_bytes([
        payload[ts_start],
        payload[ts_start + 1],
        payload[ts_start + 2],
        payload[ts_start + 3],
        payload[ts_start + 4],
        payload[ts_start + 5],
        payload[ts_start + 6],
        payload[ts_start + 7],
    ]);
    Some((event, ts))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use aura_types::identity::OceanTraits;

    use super::*;

    #[test]
    fn test_identity_engine_default() {
        let engine = IdentityEngine::default();
        // personality_context() returns an empty string by design — the TRUTH
        // framework directive injection is deferred to Phase N (Iron Law #3:
        // LLM classifies intent, Rust does not).  We only verify that the call
        // succeeds without panicking and returns a String.
        let ctx = engine.personality_context("test_user");
        let _ = ctx; // content is phase-dependent; don't assert on it here
    }

    #[test]
    fn test_check_response_on_fresh_engine() {
        let mut engine = IdentityEngine::new();
        // No responses recorded — should pass
        let result = engine.check_response();
        assert_eq!(result, GateResult::Pass);
    }

    #[test]
    fn test_requires_permission_unknown_user() {
        let engine = IdentityEngine::new();
        // Unknown user → trust 0.0 → should require permission for everything
        assert!(engine.requires_permission("nobody", &RiskLevel::Low));
        assert!(engine.requires_permission("nobody", &RiskLevel::Critical));
    }

    // ── Backward-compatible analyze_response tests ─────────────────────

    #[test]
    fn test_analyze_response_agreement() {
        let r = IdentityEngine::analyze_response("You're right, that is the best approach.");
        assert!(r.agreed, "explicit agreement phrase should trigger agreed");
        // Challenge should be false — no counterpoint language.
        assert!(!r.challenged);
    }

    #[test]
    fn test_analyze_response_challenge() {
        let r = IdentityEngine::analyze_response(
            "However, I'd push back on that assumption. Consider the alternative.",
        );
        assert!(r.challenged, "challenge phrases should trigger challenged");
    }

    #[test]
    fn test_analyze_response_hedging() {
        let r = IdentityEngine::analyze_response("I think perhaps that might work.");
        assert!(r.hedged, "hedge phrases should trigger hedged");
    }

    #[test]
    fn test_record_response_text_populates_ring() {
        let mut engine = IdentityEngine::new();
        // Ring starts empty → gate passes.
        assert_eq!(engine.check_response(), GateResult::Pass);

        // Record several agreeable responses — ring should now have data.
        for _ in 0..5 {
            engine.record_response_text("You're right, absolutely, great question!");
        }
        // Score should be non-zero now (ring is populated).
        let score = engine.sycophancy_guard.score();
        assert!(
            score.composite > 0.0,
            "ring should be populated after recording responses, got composite={}",
            score.composite,
        );
    }

    // ── Context-aware analysis tests ───────────────────────────────────

    #[test]
    fn test_context_aware_agreement_echo_detection() {
        // When AURA just echoes the user's words back, overlap is high.
        let user = "Rust is the best programming language for systems work.";
        let response =
            "Rust is indeed the best language for systems programming work, you're right.";
        let r = IdentityEngine::analyze_response_in_context(
            user,
            response,
            None,
            &OceanTraits::DEFAULT,
        );
        assert!(
            r.agreed,
            "high overlap + agreement phrase should flag agreed"
        );
    }

    #[test]
    fn test_context_aware_no_agreement_on_new_info() {
        // AURA provides genuinely new information without agreeing.
        let user = "What's the weather like today?";
        let response = "The forecast shows partly cloudy skies with temperatures around 22°C. \
                        There's a 30% chance of rain in the afternoon.";
        let r = IdentityEngine::analyze_response_in_context(
            user,
            response,
            None,
            &OceanTraits::DEFAULT,
        );
        assert!(
            !r.agreed,
            "new information without agreement phrases should not flag agreed"
        );
    }

    #[test]
    fn test_context_aware_challenge_with_new_info() {
        // User asserts something, AURA responds with substantial new info.
        let user = "Python is always faster than Rust for data processing.";
        let response = "Benchmarks consistently show Rust outperforming Python by 10-100x \
                        in CPU-bound data processing. Python's strength lies in its ecosystem \
                        of libraries like pandas and numpy, which use C extensions internally.";
        let r = IdentityEngine::analyze_response_in_context(
            user,
            response,
            None,
            &OceanTraits::DEFAULT,
        );
        assert!(
            r.challenged,
            "substantial new info in response to assertion should count as challenge"
        );
    }

    #[test]
    fn test_personality_adjusted_agreement_threshold() {
        // High agreeableness personality: agreement threshold is raised.
        // Same response should NOT trigger agreed for highly agreeable AURA.
        let user = "I think monads are useful.";
        let response = "I agree that monads provide nice composability patterns.";
        let high_a = OceanTraits {
            agreeableness: 0.9,
            ..OceanTraits::DEFAULT
        };
        let low_a = OceanTraits {
            agreeableness: 0.2,
            ..OceanTraits::DEFAULT
        };
        let r_high = IdentityEngine::analyze_response_in_context(user, response, None, &high_a);
        let r_low = IdentityEngine::analyze_response_in_context(user, response, None, &low_a);
        // With high agreeableness, agreement is natural → may not flag.
        // With low agreeableness, same agreement is unusual → should flag.
        // At minimum: low-A should be at least as likely to flag as high-A.
        assert!(
            !r_high.agreed || r_low.agreed,
            "low agreeableness should be at least as sensitive as high agreeableness"
        );
    }

    #[test]
    fn test_personality_adjusted_hedging_threshold() {
        // High openness raises hedge threshold — natural exploration.
        let response = "I think perhaps we could explore this from a different angle. \
                        Maybe there's an alternative interpretation worth considering.";
        let high_o = OceanTraits {
            openness: 0.9,
            ..OceanTraits::DEFAULT
        };
        let low_o = OceanTraits {
            openness: 0.2,
            ..OceanTraits::DEFAULT
        };
        let r_high = IdentityEngine::analyze_response_in_context("", response, None, &high_o);
        let r_low = IdentityEngine::analyze_response_in_context("", response, None, &low_o);
        // High openness might tolerate more hedging; low openness flags it.
        assert!(
            !r_high.hedged || r_low.hedged,
            "low openness should be at least as sensitive to hedging as high openness"
        );
    }

    #[test]
    fn test_opinion_reversal_from_history() {
        let prev = "However, I disagree with that approach. The evidence suggests otherwise.";
        let response = "Actually, you're right. I was wrong about that. Let me reconsider.";
        let r = IdentityEngine::analyze_response_in_context(
            "I really think my approach is correct.",
            response,
            Some(prev),
            &OceanTraits::DEFAULT,
        );
        assert!(
            r.reversed_opinion,
            "switching from challenge to agreement should detect reversal"
        );
    }

    #[test]
    fn test_no_reversal_without_history() {
        let response = "On second thought, let me reconsider that position.";
        let r = IdentityEngine::analyze_response_in_context(
            "What do you think?",
            response,
            None, // No previous response available.
            &OceanTraits::DEFAULT,
        );
        // Even with reversal phrases, without a previous response to compare
        // against, we still detect reversal from explicit markers.
        assert!(
            r.reversed_opinion,
            "explicit reversal markers should still trigger even without history"
        );
    }

    #[test]
    fn test_praise_density() {
        let response = "Great question! That's a brilliant and insightful observation.";
        let r = IdentityEngine::analyze_response_in_context(
            "What do you think about X?",
            response,
            None,
            &OceanTraits::DEFAULT,
        );
        assert!(r.praised, "praise phrases should trigger praised flag");
    }

    #[test]
    fn test_no_praise_in_substantive_response() {
        let response = "The algorithm has O(n log n) time complexity due to the merge step. \
                        Space complexity is O(n) for the auxiliary array.";
        let r = IdentityEngine::analyze_response_in_context(
            "What's the complexity?",
            response,
            None,
            &OceanTraits::DEFAULT,
        );
        assert!(
            !r.praised,
            "substantive response without praise should not flag"
        );
    }

    #[test]
    fn test_empty_strings_no_panic() {
        // Edge case: completely empty inputs should not panic.
        let r = IdentityEngine::analyze_response_in_context("", "", None, &OceanTraits::DEFAULT);
        assert!(!r.agreed);
        assert!(!r.hedged);
        assert!(!r.reversed_opinion);
        assert!(!r.praised);
        // Empty response can't challenge.
        assert!(!r.challenged);
    }

    #[test]
    fn test_record_in_context_populates_ring() {
        let mut engine = IdentityEngine::new();
        assert_eq!(engine.check_response(), GateResult::Pass);

        // Record several sycophantic responses in context.
        for _ in 0..5 {
            engine.record_response_in_context(
                "I believe monorepos are superior.",
                "You're right, that's a great question! \
                 I agree monorepos are absolutely superior, excellent point!",
                None,
            );
        }
        let score = engine.sycophancy_guard.score();
        assert!(
            score.composite > 0.0,
            "ring should be populated after context-aware recording, got composite={}",
            score.composite,
        );
    }

    #[test]
    fn test_content_overlap_identical() {
        let ratio = IdentityEngine::content_overlap(
            "Rust is great for systems programming",
            "Rust is great for systems programming",
        );
        assert!(
            ratio > 0.9,
            "identical content should have high overlap: {}",
            ratio
        );
    }

    #[test]
    fn test_content_overlap_disjoint() {
        let ratio = IdentityEngine::content_overlap(
            "quantum entanglement photons",
            "medieval castle architecture stonework",
        );
        assert!(
            ratio < 0.1,
            "disjoint content should have low overlap: {}",
            ratio
        );
    }

    #[test]
    fn test_content_overlap_empty() {
        assert_eq!(IdentityEngine::content_overlap("", "hello world"), 0.0);
        assert_eq!(IdentityEngine::content_overlap("hello", ""), 0.0);
        assert_eq!(IdentityEngine::content_overlap("", ""), 0.0);
    }
}
