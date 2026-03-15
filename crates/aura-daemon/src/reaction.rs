//! Reaction detection — classifies the user's next input after AURA responds.
//!
//! # Concept
//!
//! After AURA delivers a response, we open a time-limited "observation window".
//! When the user's next input arrives within that window, the
//! [`ReactionDetector`] classifies it as a [`UserReaction`] variant by
//! combining signals from AURA's cognitive subsystems:
//!
//! - **Sentiment** — emotional valence from the Amygdala's 4-channel importance scoring (urgency,
//!   emotional valence, novelty, relevance). We use the emotional valence channel, NOT keyword
//!   matching.
//!
//! - **Semantic similarity** — cosine similarity computed externally by the Contextor / embedding
//!   engine, comparing the new input against both the original user input and AURA's response.
//!
//! These two orthogonal signals, combined with configurable thresholds,
//! produce six reaction types that close the feedback loop for the
//! OutcomeBus, learning engine, and BDI scheduler.
//!
//! # Usage
//!
//! ```ignore
//! let mut detector = ReactionDetector::new();
//!
//! // After AURA responds to the user:
//! detector.open_window("set a timer", "Timer set for 5 minutes", "timer", 1000, 1000);
//!
//! // When the next user input arrives:
//! if let Some(classified) = detector.classify_reaction(
//!     "thanks!",
//!     0.8,   // sentiment from Amygdala
//!     0.1,   // similarity to original input
//!     0.2,   // similarity to AURA's response
//!     5000,  // current time
//! ) {
//!     // classified.reaction == UserReaction::ExplicitPositive
//!     // → retroactively update the ExecutionOutcome via OutcomeBus
//! }
//! ```

use aura_types::outcome::UserReaction;
use tracing::debug;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Result of reaction classification, including correlation data for
/// retroactively updating the [`aura_types::outcome::ExecutionOutcome`].
///
/// The `correlation_intent` and `correlation_timestamp` pair uniquely
/// identifies the outcome in the OutcomeBus so the main loop can update
/// the `user_reaction` field after classification.
#[derive(Debug, Clone)]
pub struct ClassifiedReaction {
    /// The classified reaction.
    pub reaction: UserReaction,
    /// Correlation intent to find the matching ExecutionOutcome.
    pub correlation_intent: String,
    /// Correlation timestamp to find the matching ExecutionOutcome.
    pub correlation_timestamp: u64,
    /// Confidence in the classification (0.0--1.0).
    ///
    /// Derived from the signal strength that drove the decision:
    /// - Sentiment score magnitude for explicit positive/negative
    /// - Cosine similarity magnitude for repetition/follow-up/topic-change
    /// - 1.0 for window expiry (NoReaction is certain)
    pub confidence: f32,
}

// ---------------------------------------------------------------------------
// Observation window (private)
// ---------------------------------------------------------------------------

/// Internal state for an active observation window.
///
/// Created when AURA sends a response; consumed when the next user input
/// arrives or the window expires.  One window at a time — opening a new
/// window replaces any existing one.
struct ObservationWindow {
    /// When the window was opened (epoch ms).
    opened_at_ms: u64,
    /// The user's original input that triggered AURA's response.
    original_input: String,
    /// AURA's response text (truncated for storage).
    #[allow(dead_code)]
    response_text: String,
    /// The intent string used as a correlation key into the OutcomeBus.
    correlation_intent: String,
    /// The timestamp of the outcome for correlation.
    correlation_timestamp: u64,
}

// ---------------------------------------------------------------------------
// ReactionDetector
// ---------------------------------------------------------------------------

/// Observes the user's next input after AURA responds and classifies it
/// as a [`UserReaction`] variant.
///
/// The detector maintains at most one active observation window.  The
/// classification uses externally-computed cognitive signals (Amygdala
/// sentiment, Contextor similarity) rather than keyword matching, ensuring
/// all intelligence comes from AURA's actual neural subsystems.
///
/// All thresholds are configurable and designed to be tuned by the
/// learning engine over time as AURA observes which classifications
/// lead to accurate effectiveness scores.
pub struct ReactionDetector {
    /// The observation window state.
    active_window: Option<ObservationWindow>,

    /// How long to wait for user reaction before expiring (ms).
    /// Default: 30 000 ms (30 seconds).
    window_duration_ms: u64,

    /// Cosine similarity threshold above which we consider the new input
    /// a repetition of the original.  Combined with negative sentiment to
    /// distinguish genuine repetition (failure signal) from confirmation.
    /// Default: 0.80.
    repetition_threshold: f32,

    /// Cosine similarity threshold above which we consider the new input
    /// a follow-up to the previous topic.  Below this the input is
    /// classified as a topic change.  Default: 0.30.
    follow_up_threshold: f32,

    /// Amygdala emotional-valence score above which the reaction is
    /// classified as `ExplicitPositive`.  Default: 0.60.
    sentiment_positive_threshold: f32,

    /// Amygdala emotional-valence score below which the reaction is
    /// classified as `ExplicitNegative`.  Default: -0.40.
    sentiment_negative_threshold: f32,
}

impl ReactionDetector {
    /// Create a new detector with sensible defaults.
    ///
    /// Default thresholds:
    /// - Window duration: 30 000 ms (30 seconds)
    /// - Repetition threshold: 0.80 (very similar to original input)
    /// - Follow-up threshold: 0.30 (moderate topic relatedness)
    /// - Positive sentiment threshold: 0.60
    /// - Negative sentiment threshold: -0.40
    ///
    /// These are NOT hardcoded intelligence.  They are configurable knobs
    /// that the learning engine can tune via the ARC dimension system
    /// as it observes which settings lead to accurate classifications.
    #[must_use]
    pub fn new() -> Self {
        Self {
            active_window: None,
            window_duration_ms: 30_000,
            repetition_threshold: 0.80,
            follow_up_threshold: 0.30,
            sentiment_positive_threshold: 0.60,
            sentiment_negative_threshold: -0.40,
        }
    }

    /// Open an observation window after AURA responds.
    ///
    /// Called by the main loop immediately after sending a response to the
    /// user.  If a window is already active it is silently replaced — this
    /// handles the case where AURA responds to multiple inputs rapidly.
    ///
    /// # Parameters
    ///
    /// - `original_input` — the user's input that triggered AURA's response.
    /// - `response_text` — AURA's response (stored for potential future use; similarity is computed
    ///   externally).
    /// - `correlation_intent` — the intent label from classification, used to correlate with the
    ///   ExecutionOutcome in the OutcomeBus.
    /// - `correlation_timestamp` — the outcome's started_at_ms timestamp.
    /// - `now_ms` — current epoch timestamp in milliseconds.
    pub fn open_window(
        &mut self,
        original_input: &str,
        response_text: &str,
        correlation_intent: &str,
        correlation_timestamp: u64,
        now_ms: u64,
    ) {
        if self.active_window.is_some() {
            debug!(
                new_intent = %correlation_intent,
                "reaction window replaced — previous window was still active"
            );
        }

        self.active_window = Some(ObservationWindow {
            opened_at_ms: now_ms,
            original_input: original_input.to_owned(),
            response_text: response_text.to_owned(),
            correlation_intent: correlation_intent.to_owned(),
            correlation_timestamp,
        });

        debug!(
            intent = %correlation_intent,
            window_ms = self.window_duration_ms,
            "reaction observation window opened"
        );
    }

    /// Classify the user's next input relative to the active window.
    ///
    /// Returns `Some(ClassifiedReaction)` if a window was active (and closes
    /// it), or `None` if no window was active or the window had already
    /// expired.
    ///
    /// # Cognitive signals (all computed externally)
    ///
    /// - `next_input` — the user's new input text (used only for tracing).
    /// - `sentiment_score` — emotional valence from the Amygdala's 4-channel scoring system. Range:
    ///   -1.0 (very negative) to 1.0 (very positive). This is NOT a keyword lookup.
    /// - `similarity_to_original` — cosine similarity between the new input and the original input
    ///   that AURA responded to.  High values indicate the user is repeating themselves.
    /// - `similarity_to_response` — cosine similarity between the new input and AURA's response
    ///   text.  High values indicate the user is referencing or following up on AURA's answer.
    /// - `now_ms` — current epoch timestamp.
    ///
    /// # Decision tree
    ///
    /// Applied in order of specificity (most specific match wins):
    ///
    /// 1. **Expired** — window timed out → `NoReaction`
    /// 2. **Repetition** — high similarity to original AND negative sentiment
    /// 3. **ExplicitPositive** — strong positive sentiment
    /// 4. **ExplicitNegative** — strong negative sentiment
    /// 5. **FollowUp** — moderate similarity to response or original
    /// 6. **TopicChange** — low similarity everywhere, neutral sentiment
    pub fn classify_reaction(
        &mut self,
        next_input: &str,
        sentiment_score: f32,
        similarity_to_original: f32,
        similarity_to_response: f32,
        now_ms: u64,
    ) -> Option<ClassifiedReaction> {
        let window = self.active_window.take()?;

        // 1. Check window expiry.
        let elapsed = now_ms.saturating_sub(window.opened_at_ms);
        if elapsed > self.window_duration_ms {
            debug!(
                elapsed_ms = elapsed,
                window_ms = self.window_duration_ms,
                "reaction window expired before classification"
            );
            return Some(ClassifiedReaction {
                reaction: UserReaction::NoReaction,
                correlation_intent: window.correlation_intent,
                correlation_timestamp: window.correlation_timestamp,
                confidence: 1.0,
            });
        }

        // 2. Window is consumed — one classification per window.

        // 3. Clamp input signals to expected ranges for robustness.
        let sentiment = sentiment_score.clamp(-1.0, 1.0);
        let sim_original = similarity_to_original.clamp(0.0, 1.0);
        let sim_response = similarity_to_response.clamp(0.0, 1.0);

        // 4. Decision tree (ordered by specificity).

        // 4a. Repetition: user is repeating the same request with frustration.
        //     High similarity to their ORIGINAL input + negative sentiment
        //     means AURA's response was inadequate and they're trying again.
        if sim_original > self.repetition_threshold && sentiment < 0.0 {
            debug!(
                reaction = "Repetition",
                sim_original,
                sentiment,
                input = %truncate_for_log(next_input),
                "classified as repetition — high similarity to original + negative sentiment"
            );
            return Some(ClassifiedReaction {
                reaction: UserReaction::Repetition,
                correlation_intent: window.correlation_intent,
                correlation_timestamp: window.correlation_timestamp,
                confidence: sim_original,
            });
        }

        // 4b. ExplicitPositive: user expressed satisfaction via positive
        //     emotional valence (from Amygdala, NOT keyword matching).
        if sentiment > self.sentiment_positive_threshold {
            debug!(
                reaction = "ExplicitPositive",
                sentiment,
                input = %truncate_for_log(next_input),
                "classified as explicit positive — high Amygdala emotional valence"
            );
            return Some(ClassifiedReaction {
                reaction: UserReaction::ExplicitPositive,
                correlation_intent: window.correlation_intent,
                correlation_timestamp: window.correlation_timestamp,
                confidence: sentiment,
            });
        }

        // 4c. ExplicitNegative: user expressed dissatisfaction via negative
        //     emotional valence (from Amygdala, NOT keyword matching).
        if sentiment < self.sentiment_negative_threshold {
            debug!(
                reaction = "ExplicitNegative",
                sentiment,
                input = %truncate_for_log(next_input),
                "classified as explicit negative — low Amygdala emotional valence"
            );
            return Some(ClassifiedReaction {
                reaction: UserReaction::ExplicitNegative,
                correlation_intent: window.correlation_intent,
                correlation_timestamp: window.correlation_timestamp,
                confidence: sentiment.abs(),
            });
        }

        // 4d. FollowUp: user asked a related question or referenced AURA's
        //     response.  Moderate similarity to either the response or the
        //     original input signals continued engagement with the topic.
        let max_sim = f32::max(sim_response, sim_original);
        if max_sim > self.follow_up_threshold {
            debug!(
                reaction = "FollowUp",
                sim_response,
                sim_original,
                max_sim,
                input = %truncate_for_log(next_input),
                "classified as follow-up — moderate semantic similarity"
            );
            return Some(ClassifiedReaction {
                reaction: UserReaction::FollowUp,
                correlation_intent: window.correlation_intent,
                correlation_timestamp: window.correlation_timestamp,
                confidence: max_sim,
            });
        }

        // 4e. TopicChange: none of the above matched — the user shifted to
        //     an unrelated topic.  Confidence is inversely proportional to
        //     similarity (lower similarity = more certain topic change).
        let topic_change_confidence = (1.0 - max_sim).clamp(0.0, 1.0);
        debug!(
            reaction = "TopicChange",
            sim_response,
            sim_original,
            topic_change_confidence,
            input = %truncate_for_log(next_input),
            "classified as topic change — low semantic similarity"
        );
        Some(ClassifiedReaction {
            reaction: UserReaction::TopicChange,
            correlation_intent: window.correlation_intent,
            correlation_timestamp: window.correlation_timestamp,
            confidence: topic_change_confidence,
        })
    }

    /// Check if the active window has expired without user input.
    ///
    /// Called periodically by the main loop's tick handler.  Returns
    /// `Some(ClassifiedReaction)` with `NoReaction` if expired, `None`
    /// if the window is still active or no window exists.
    pub fn check_expiry(&mut self, now_ms: u64) -> Option<ClassifiedReaction> {
        let window = self.active_window.as_ref()?;
        let elapsed = now_ms.saturating_sub(window.opened_at_ms);

        if elapsed > self.window_duration_ms {
            // Take ownership and close the window.
            let window = self.active_window.take().expect("checked above");
            debug!(
                elapsed_ms = elapsed,
                intent = %window.correlation_intent,
                "reaction window expired — no user input received"
            );
            Some(ClassifiedReaction {
                reaction: UserReaction::NoReaction,
                correlation_intent: window.correlation_intent,
                correlation_timestamp: window.correlation_timestamp,
                confidence: 1.0,
            })
        } else {
            None
        }
    }

    /// Whether an observation window is currently active.
    #[must_use]
    pub fn is_active(&self) -> bool {
        self.active_window.is_some()
    }

    /// Get the original user input stored in the active observation window.
    ///
    /// Returns `None` if no window is active.  Used by the main loop to
    /// compute similarity signals before calling [`classify_reaction`].
    #[must_use]
    pub fn window_original_input(&self) -> Option<&str> {
        self.active_window
            .as_ref()
            .map(|w| w.original_input.as_str())
    }

    /// Get AURA's response text stored in the active observation window.
    ///
    /// Returns `None` if no window is active.  Used by the main loop to
    /// compute similarity signals before calling [`classify_reaction`].
    #[must_use]
    pub fn window_response_text(&self) -> Option<&str> {
        self.active_window
            .as_ref()
            .map(|w| w.response_text.as_str())
    }
}

impl Default for ReactionDetector {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Truncate a string for tracing output (max 80 chars).
fn truncate_for_log(s: &str) -> &str {
    if s.len() <= 80 {
        return s;
    }
    // Find the last char boundary at or before 80 bytes.
    let mut end = 80;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: create a detector and open a standard window.
    fn detector_with_window() -> ReactionDetector {
        let mut d = ReactionDetector::new();
        d.open_window(
            "set a timer for 5 minutes",
            "Timer set for 5 minutes.",
            "timer.set",
            1000,
            1000, // opened at t=1000
        );
        d
    }

    // -----------------------------------------------------------------------
    // Core classification tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_explicit_positive() {
        let mut d = detector_with_window();

        // High positive sentiment from Amygdala, low similarity everywhere.
        let result = d
            .classify_reaction("thanks, perfect!", 0.85, 0.1, 0.15, 5000)
            .expect("should classify");

        assert_eq!(result.reaction, UserReaction::ExplicitPositive);
        assert_eq!(result.correlation_intent, "timer.set");
        assert_eq!(result.correlation_timestamp, 1000);
        // Confidence should equal the sentiment score.
        assert!((result.confidence - 0.85).abs() < f32::EPSILON);
        // Window should be closed.
        assert!(!d.is_active());
    }

    #[test]
    fn test_explicit_negative() {
        let mut d = detector_with_window();

        // Negative sentiment from Amygdala, moderate similarity.
        let result = d
            .classify_reaction("that's wrong", -0.7, 0.2, 0.3, 5000)
            .expect("should classify");

        assert_eq!(result.reaction, UserReaction::ExplicitNegative);
        // Confidence should be the absolute value of sentiment.
        assert!((result.confidence - 0.7).abs() < f32::EPSILON);
    }

    #[test]
    fn test_repetition() {
        let mut d = detector_with_window();

        // Very high similarity to original input + negative sentiment
        // → user is repeating because AURA failed.
        let result = d
            .classify_reaction(
                "set a timer for 5 minutes",
                -0.2, // mildly negative
                0.95, // very similar to original
                0.1,  // not similar to response
                3000,
            )
            .expect("should classify");

        assert_eq!(result.reaction, UserReaction::Repetition);
        // Confidence equals similarity_to_original.
        assert!((result.confidence - 0.95).abs() < f32::EPSILON);
    }

    #[test]
    fn test_repetition_requires_negative_sentiment() {
        let mut d = detector_with_window();

        // High similarity to original but POSITIVE sentiment → not repetition.
        // Should fall through to ExplicitPositive (sentiment 0.7 > 0.6 threshold).
        let result = d
            .classify_reaction(
                "set a timer for 5 minutes",
                0.7,  // positive sentiment
                0.95, // very similar to original
                0.1,
                3000,
            )
            .expect("should classify");

        assert_eq!(result.reaction, UserReaction::ExplicitPositive);
    }

    #[test]
    fn test_follow_up_via_response_similarity() {
        let mut d = detector_with_window();

        // Moderate similarity to AURA's response, neutral sentiment.
        let result = d
            .classify_reaction(
                "can you also set one for 10 minutes?",
                0.1,  // neutral sentiment
                0.25, // moderate similarity to original
                0.5,  // higher similarity to response
                4000,
            )
            .expect("should classify");

        assert_eq!(result.reaction, UserReaction::FollowUp);
        // Confidence = max(0.5, 0.25) = 0.5
        assert!((result.confidence - 0.5).abs() < f32::EPSILON);
    }

    #[test]
    fn test_follow_up_via_original_similarity() {
        let mut d = detector_with_window();

        // Moderate similarity to original input, neutral sentiment.
        let result = d
            .classify_reaction(
                "what about a 10 minute timer?",
                0.1,  // neutral sentiment
                0.45, // above follow-up threshold via original
                0.15, // low similarity to response
                4000,
            )
            .expect("should classify");

        assert_eq!(result.reaction, UserReaction::FollowUp);
        assert!((result.confidence - 0.45).abs() < f32::EPSILON);
    }

    #[test]
    fn test_topic_change() {
        let mut d = detector_with_window();

        // Low similarity everywhere, neutral sentiment.
        let result = d
            .classify_reaction(
                "what's the weather like?",
                0.0,  // neutral
                0.05, // very low similarity to original
                0.08, // very low similarity to response
                6000,
            )
            .expect("should classify");

        assert_eq!(result.reaction, UserReaction::TopicChange);
        // Confidence = 1.0 - max(0.08, 0.05) = 0.92
        let expected = 1.0 - 0.08_f32;
        assert!((result.confidence - expected).abs() < f32::EPSILON);
    }

    // -----------------------------------------------------------------------
    // Window lifecycle tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_no_reaction_expired() {
        let mut d = detector_with_window();

        // 31 seconds later — window should have expired.
        let result = d.check_expiry(32_000).expect("should detect expiry");

        assert_eq!(result.reaction, UserReaction::NoReaction);
        assert_eq!(result.correlation_intent, "timer.set");
        assert!((result.confidence - 1.0).abs() < f32::EPSILON);
        assert!(!d.is_active());
    }

    #[test]
    fn test_no_reaction_expired_via_classify() {
        let mut d = detector_with_window();

        // Classifying after expiry also returns NoReaction.
        let result = d
            .classify_reaction("anything", 0.0, 0.0, 0.0, 32_000)
            .expect("should classify as expired");

        assert_eq!(result.reaction, UserReaction::NoReaction);
        assert!((result.confidence - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn test_check_expiry_not_yet() {
        let mut d = detector_with_window();

        // Only 5 seconds — window still active.
        let result = d.check_expiry(6000);
        assert!(result.is_none());
        assert!(d.is_active());
    }

    #[test]
    fn test_no_window_active() {
        let mut d = ReactionDetector::new();

        // No window was opened — classify should return None.
        let result = d.classify_reaction("hello", 0.5, 0.1, 0.1, 1000);
        assert!(result.is_none());

        // check_expiry should also return None.
        let result = d.check_expiry(1000);
        assert!(result.is_none());
    }

    #[test]
    fn test_window_replaces_previous() {
        let mut d = detector_with_window();

        // Open a second window — should replace the first.
        d.open_window(
            "play some music",
            "Playing your favorites.",
            "music.play",
            2000,
            2000,
        );

        assert!(d.is_active());

        // Classification should use the second window's correlation data.
        let result = d
            .classify_reaction("nice", 0.7, 0.1, 0.1, 3000)
            .expect("should classify");

        assert_eq!(result.correlation_intent, "music.play");
        assert_eq!(result.correlation_timestamp, 2000);
    }

    #[test]
    fn test_classify_consumes_window() {
        let mut d = detector_with_window();

        // First classification consumes the window.
        let first = d.classify_reaction("thanks", 0.8, 0.1, 0.1, 2000);
        assert!(first.is_some());
        assert!(!d.is_active());

        // Second classification has no window.
        let second = d.classify_reaction("another input", 0.0, 0.0, 0.0, 3000);
        assert!(second.is_none());
    }

    // -----------------------------------------------------------------------
    // Edge cases
    // -----------------------------------------------------------------------

    #[test]
    fn test_clamps_out_of_range_sentiment() {
        let mut d = detector_with_window();

        // Sentiment > 1.0 should be clamped to 1.0 → still ExplicitPositive.
        let result = d
            .classify_reaction("wow", 2.5, 0.0, 0.0, 2000)
            .expect("should classify");

        assert_eq!(result.reaction, UserReaction::ExplicitPositive);
        // Confidence should be clamped sentiment = 1.0.
        assert!((result.confidence - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn test_clamps_out_of_range_similarity() {
        let mut d = detector_with_window();

        // Similarity > 1.0 should be clamped.
        let result = d
            .classify_reaction("same thing", -0.1, 1.5, 0.0, 2000)
            .expect("should classify");

        // sim_original clamped to 1.0 > 0.8 threshold, sentiment -0.1 < 0.0 → Repetition.
        assert_eq!(result.reaction, UserReaction::Repetition);
        assert!((result.confidence - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn test_default_trait() {
        let d = ReactionDetector::default();
        assert!(!d.is_active());
    }

    #[test]
    fn test_truncate_for_log_short() {
        assert_eq!(truncate_for_log("hello"), "hello");
    }

    #[test]
    fn test_truncate_for_log_long() {
        let long = "a".repeat(200);
        let truncated = truncate_for_log(&long);
        assert_eq!(truncated.len(), 80);
    }
}
