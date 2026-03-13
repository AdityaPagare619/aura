//! Morning briefing generator with adaptive timing and LLM integration
//! (SPEC-ARC section 8.3.1).
//!
//! Generates a daily morning briefing composed of configurable sections.
//! The briefing adapts its delivery time based on the user's wake patterns,
//! schedule density, and engagement history.
//!
//! # Adaptive Timing Algorithm
//!
//! The briefing hour is adjusted based on observed wake times:
//! ```text
//! adaptive_hour = ema(observed_wake_hours, alpha=0.2)
//! ```
//!
//! # Schedule Density Detection
//!
//! Sections are prioritized based on how busy the day looks:
//! - **Light day** (0-2 events): Full briefing, relaxed tone
//! - **Normal day** (3-5 events): Standard briefing
//! - **Heavy day** (6+ events): Condensed briefing, critical sections only
//!
//! # Engagement Feedback
//!
//! Tracks which sections the user actually reads/interacts with and adjusts
//! future briefings accordingly.

use serde::{Deserialize, Serialize};
use tracing::{debug, info, instrument, warn};

use super::super::{ArcError, DomainId};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum number of briefing sections per briefing.
const MAX_BRIEFING_SECTIONS: usize = 10;

/// Default hour of day (0..23) for the morning briefing.
const DEFAULT_BRIEFING_HOUR: u8 = 7;

/// EMA alpha for adaptive timing.
const TIMING_EMA_ALPHA: f32 = 0.2;

/// Minimum wake observations before adaptive timing kicks in.
const MIN_WAKE_OBSERVATIONS: u32 = 3;

/// Maximum engagement history entries tracked per section.
const MAX_ENGAGEMENT_HISTORY: usize = 64;

/// Minimum engagement rate to keep a section in the default lineup.
const MIN_ENGAGEMENT_RATE: f32 = 0.15;

/// Schedule density thresholds.
const DENSITY_LIGHT_MAX: u16 = 2;
const DENSITY_NORMAL_MAX: u16 = 5;

/// Maximum number of context items per section in a generated briefing.
const MAX_CONTEXT_ITEMS: usize = 8;

/// Maximum number of generated briefing outputs retained.
const MAX_GENERATED_HISTORY: usize = 30;

// ---------------------------------------------------------------------------
// BriefingSection
// ---------------------------------------------------------------------------

/// A single section within a morning briefing.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum BriefingSection {
    /// Weather forecast summary.
    Weather,
    /// Calendar events for the day.
    Calendar,
    /// Health metrics and medication reminders.
    Health,
    /// Social updates (birthdays, pending replies).
    Social,
    /// Task and productivity overview.
    Tasks,
    /// News headlines.
    News,
    /// User-defined custom section.
    Custom(String),
}

impl BriefingSection {
    /// Relative priority (lower = more important, shown first in condensed view).
    #[must_use]
    pub fn priority(&self) -> u8 {
        match self {
            BriefingSection::Health => 0,
            BriefingSection::Calendar => 1,
            BriefingSection::Tasks => 2,
            BriefingSection::Weather => 3,
            BriefingSection::Social => 4,
            BriefingSection::News => 5,
            BriefingSection::Custom(_) => 6,
        }
    }

    /// Domain that this section is associated with.
    #[must_use]
    pub fn domain(&self) -> DomainId {
        match self {
            BriefingSection::Weather => DomainId::Environment,
            BriefingSection::Calendar => DomainId::Productivity,
            BriefingSection::Health => DomainId::Health,
            BriefingSection::Social => DomainId::Social,
            BriefingSection::Tasks => DomainId::Productivity,
            BriefingSection::News => DomainId::Entertainment,
            BriefingSection::Custom(_) => DomainId::Lifestyle,
        }
    }

    /// Discriminant string for engagement tracking (stable key).
    #[must_use]
    pub fn key(&self) -> String {
        match self {
            BriefingSection::Weather => "weather".into(),
            BriefingSection::Calendar => "calendar".into(),
            BriefingSection::Health => "health".into(),
            BriefingSection::Social => "social".into(),
            BriefingSection::Tasks => "tasks".into(),
            BriefingSection::News => "news".into(),
            BriefingSection::Custom(name) => format!("custom:{name}"),
        }
    }
}

// ---------------------------------------------------------------------------
// ScheduleDensity
// ---------------------------------------------------------------------------

/// How packed the user's day is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ScheduleDensity {
    /// 0-2 events: Full briefing, relaxed tone.
    Light,
    /// 3-5 events: Standard briefing.
    Normal,
    /// 6+ events: Condensed briefing, critical items only.
    Heavy,
}

impl ScheduleDensity {
    /// Classify from event count.
    #[must_use]
    pub fn from_event_count(count: u16) -> Self {
        if count <= DENSITY_LIGHT_MAX {
            ScheduleDensity::Light
        } else if count <= DENSITY_NORMAL_MAX {
            ScheduleDensity::Normal
        } else {
            ScheduleDensity::Heavy
        }
    }

    /// Maximum sections to include for this density level.
    #[must_use]
    pub fn max_sections(self) -> usize {
        match self {
            ScheduleDensity::Light => MAX_BRIEFING_SECTIONS,
            ScheduleDensity::Normal => 6,
            ScheduleDensity::Heavy => 3,
        }
    }
}

// ---------------------------------------------------------------------------
// BriefingContext — context gathered for LLM generation
// ---------------------------------------------------------------------------

/// A single context item gathered for a section.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextItem {
    /// Human-readable label.
    pub label: String,
    /// Optional numeric value (e.g., temperature, step count).
    pub value: Option<f64>,
    /// Optional urgency level [0.0, 1.0].
    pub urgency: f32,
}

/// Context gathered for a full briefing, ready for LLM or template rendering.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BriefingContext {
    /// Per-section context items.
    pub sections: Vec<SectionContext>,
    /// Schedule density for the day.
    pub density: ScheduleDensity,
    /// Adaptive briefing hour used.
    pub briefing_hour: u8,
    /// Day identifier.
    pub day: u32,
}

/// Context for a single section.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SectionContext {
    /// Which section this context is for.
    pub section: BriefingSection,
    /// Data items for this section.
    pub items: Vec<ContextItem>,
}

// ---------------------------------------------------------------------------
// EngagementRecord
// ---------------------------------------------------------------------------

/// Tracks whether the user interacted with a briefing section.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct EngagementRecord {
    /// Section key.
    section_key: String,
    /// Whether the user engaged with this section.
    engaged: bool,
    /// Day this engagement was recorded.
    day: u32,
}

// ---------------------------------------------------------------------------
// GeneratedBriefing — output of generate()
// ---------------------------------------------------------------------------

/// The result of generating a morning briefing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeneratedBriefing {
    /// The sections included in this briefing.
    pub sections: Vec<BriefingSection>,
    /// The context gathered for each section.
    pub context: BriefingContext,
    /// Day this briefing was generated for.
    pub day: u32,
    /// The schedule density that was detected.
    pub density: ScheduleDensity,
}

// ---------------------------------------------------------------------------
// MorningBriefing
// ---------------------------------------------------------------------------

/// Manages daily morning briefing generation with adaptive timing,
/// schedule-density awareness, and engagement-based section selection.
///
/// # Backward compatibility
///
/// The original `generate(day) -> Vec<BriefingSection>` signature is
/// preserved as `generate()`. A richer `generate_with_context()` is
/// also available.
#[derive(Debug, Serialize, Deserialize)]
pub struct MorningBriefing {
    /// Day-of-epoch when the last briefing was delivered.
    last_briefing_day: u32,
    /// Configured briefing hour (static, before adaptive adjustment).
    configured_hour: u8,
    /// Adaptive briefing hour (EMA of observed wake times).
    adaptive_hour: f32,
    /// Number of wake-time observations for adaptive timing.
    wake_observations: u32,
    /// Configured sections to include in each briefing (bounded).
    sections: Vec<BriefingSection>,
    /// Engagement history for section adaptation.
    engagement_history: Vec<EngagementRecord>,
    /// Cached engagement rates per section key.
    engagement_rates: Vec<(String, f32)>,
    /// History of generated briefings (bounded).
    generated_history: Vec<GeneratedBriefing>,
}

impl MorningBriefing {
    /// Create a new morning briefing engine with default configuration.
    #[must_use]
    pub fn new() -> Self {
        Self {
            last_briefing_day: 0,
            configured_hour: DEFAULT_BRIEFING_HOUR,
            adaptive_hour: DEFAULT_BRIEFING_HOUR as f32,
            wake_observations: 0,
            sections: vec![
                BriefingSection::Weather,
                BriefingSection::Calendar,
                BriefingSection::Health,
                BriefingSection::Tasks,
            ],
            engagement_history: Vec::with_capacity(32),
            engagement_rates: Vec::new(),
            generated_history: Vec::with_capacity(8),
        }
    }

    /// Current effective briefing hour (adaptive if enough observations,
    /// otherwise the configured static hour).
    #[must_use]
    pub fn briefing_hour(&self) -> u8 {
        if self.wake_observations >= MIN_WAKE_OBSERVATIONS {
            self.adaptive_hour.round().clamp(0.0, 23.0) as u8
        } else {
            self.configured_hour
        }
    }

    /// Day the last briefing was delivered.
    #[must_use]
    pub fn last_briefing_day(&self) -> u32 {
        self.last_briefing_day
    }

    /// Number of configured sections.
    #[must_use]
    pub fn section_count(&self) -> usize {
        self.sections.len()
    }

    /// Number of wake observations collected.
    #[must_use]
    pub fn wake_observations(&self) -> u32 {
        self.wake_observations
    }

    /// The raw adaptive hour value (before rounding).
    #[must_use]
    pub fn adaptive_hour_raw(&self) -> f32 {
        self.adaptive_hour
    }

    /// Set the configured (static) hour at which the briefing triggers.
    pub fn set_briefing_hour(&mut self, hour: u8) -> Result<(), ArcError> {
        if hour > 23 {
            return Err(ArcError::DomainError {
                domain: DomainId::Productivity,
                detail: format!("invalid briefing hour: {hour} (must be 0..23)"),
            });
        }
        self.configured_hour = hour;
        // If no adaptive data yet, also reset the adaptive value.
        if self.wake_observations < MIN_WAKE_OBSERVATIONS {
            self.adaptive_hour = hour as f32;
        }
        debug!(hour, "briefing hour updated");
        Ok(())
    }

    /// Observe a wake-up time to adapt briefing timing.
    ///
    /// `wake_hour` is the hour (0..23) when the user woke up.
    pub fn observe_wake_time(&mut self, wake_hour: u8) {
        let hour = wake_hour.min(23) as f32;
        self.adaptive_hour =
            TIMING_EMA_ALPHA * hour + (1.0 - TIMING_EMA_ALPHA) * self.adaptive_hour;
        self.wake_observations = self.wake_observations.saturating_add(1);
        debug!(
            wake_hour,
            adaptive = self.adaptive_hour,
            observations = self.wake_observations,
            "wake time observed"
        );
    }

    /// Add a section to the briefing configuration.
    pub fn add_section(&mut self, section: BriefingSection) -> Result<(), ArcError> {
        if self.sections.len() >= MAX_BRIEFING_SECTIONS {
            return Err(ArcError::CapacityExceeded {
                collection: "briefing_sections".into(),
                max: MAX_BRIEFING_SECTIONS,
            });
        }
        self.sections.push(section);
        Ok(())
    }

    /// Remove all configured sections.
    pub fn clear_sections(&mut self) {
        self.sections.clear();
    }

    /// Record user engagement with a briefing section.
    ///
    /// This feedback loop allows future briefings to prioritise sections
    /// the user actually reads.
    pub fn record_engagement(&mut self, section: &BriefingSection, engaged: bool, day: u32) {
        // Bound the history.
        if self.engagement_history.len() >= MAX_ENGAGEMENT_HISTORY {
            self.engagement_history.drain(0..MAX_ENGAGEMENT_HISTORY / 4);
        }
        self.engagement_history.push(EngagementRecord {
            section_key: section.key(),
            engaged,
            day,
        });
        self.recompute_engagement_rates();
    }

    /// Get the engagement rate for a section (0.0 to 1.0).
    #[must_use]
    pub fn engagement_rate(&self, section: &BriefingSection) -> f32 {
        let key = section.key();
        self.engagement_rates
            .iter()
            .find(|(k, _)| k == &key)
            .map(|(_, rate)| *rate)
            .unwrap_or(0.5) // Default to 50% if no data.
    }

    /// Recompute engagement rates from history.
    fn recompute_engagement_rates(&mut self) {
        use std::collections::HashMap;
        let mut totals: HashMap<String, (u32, u32)> = HashMap::new();
        for record in &self.engagement_history {
            let entry = totals.entry(record.section_key.clone()).or_insert((0, 0));
            entry.1 += 1; // total
            if record.engaged {
                entry.0 += 1; // engaged
            }
        }
        self.engagement_rates = totals
            .into_iter()
            .map(|(key, (engaged, total))| {
                let rate = if total > 0 {
                    engaged as f32 / total as f32
                } else {
                    0.5
                };
                (key, rate)
            })
            .collect();
    }

    /// Check whether the morning briefing should trigger right now.
    ///
    /// Returns `true` if:
    /// - The current hour matches the effective briefing hour
    /// - The briefing hasn't been delivered today (day != last_briefing_day)
    #[must_use]
    pub fn should_trigger(&self, hour: u8, day: u32) -> bool {
        hour == self.briefing_hour() && day != self.last_briefing_day
    }

    /// Generate the morning briefing for the given day (backward-compat API).
    ///
    /// Returns the list of sections to present. Marks the day as briefed
    /// so subsequent calls for the same day return an empty vec.
    #[instrument(name = "morning_generate", skip(self))]
    pub fn generate(&mut self, day: u32) -> Result<Vec<BriefingSection>, ArcError> {
        if day == self.last_briefing_day {
            debug!(day, "briefing already delivered today");
            return Ok(Vec::new());
        }

        self.last_briefing_day = day;

        // Use default normal density for backward-compat API.
        let selected = self.select_sections(ScheduleDensity::Normal);
        info!(day, sections = selected.len(), "morning briefing generated");
        Ok(selected)
    }

    /// Generate a full briefing with context, density awareness, and
    /// engagement-based section ordering.
    ///
    /// `event_count` is the number of calendar events for today.
    /// `section_contexts` provides per-section data items (from neocortex
    /// or domain modules). Sections without context are still included
    /// but with empty item lists.
    #[instrument(name = "morning_generate_with_context", skip(self, section_contexts))]
    pub fn generate_with_context(
        &mut self,
        day: u32,
        event_count: u16,
        section_contexts: Vec<SectionContext>,
    ) -> Result<GeneratedBriefing, ArcError> {
        if day == self.last_briefing_day {
            debug!(day, "briefing already delivered today");
            // Return an empty briefing with current density.
            let density = ScheduleDensity::from_event_count(event_count);
            return Ok(GeneratedBriefing {
                sections: Vec::new(),
                context: BriefingContext {
                    sections: Vec::new(),
                    density,
                    briefing_hour: self.briefing_hour(),
                    day,
                },
                day,
                density,
            });
        }

        self.last_briefing_day = day;

        let density = ScheduleDensity::from_event_count(event_count);
        let selected = self.select_sections(density);

        // Build context, matching provided contexts to selected sections.
        let mut section_ctxs: Vec<SectionContext> = Vec::with_capacity(selected.len());
        for section in &selected {
            let existing = section_contexts
                .iter()
                .find(|sc| sc.section == *section)
                .cloned();
            let mut ctx = existing.unwrap_or_else(|| SectionContext {
                section: section.clone(),
                items: Vec::new(),
            });
            // Bound items per section.
            ctx.items.truncate(MAX_CONTEXT_ITEMS);
            section_ctxs.push(ctx);
        }

        let briefing_context = BriefingContext {
            sections: section_ctxs,
            density,
            briefing_hour: self.briefing_hour(),
            day,
        };

        let result = GeneratedBriefing {
            sections: selected,
            context: briefing_context,
            day,
            density,
        };

        // Store in history (bounded).
        if self.generated_history.len() >= MAX_GENERATED_HISTORY {
            self.generated_history.remove(0);
        }
        self.generated_history.push(result.clone());

        info!(
            day,
            density = ?density,
            sections = result.sections.len(),
            "morning briefing with context generated"
        );

        Ok(result)
    }

    /// Order and trim the configured sections based on density and engagement.
    ///
    /// # Architecture contract
    /// This method ORDERS and FILTERS the user's own configured section list —
    /// it does NOT decide what AURA says, only which sections are worth showing
    /// given a busy schedule. The LLM writes the content for every included
    /// section. The composite rank (`priority × 0.6 + engagement × 0.4`) is
    /// a display-ordering heuristic, not a routing decision.
    fn select_sections(&self, density: ScheduleDensity) -> Vec<BriefingSection> {
        let max = density.max_sections();

        // Rank each section: priority (inverse) weighted with engagement rate.
        // Both inputs are display metadata — neither gates AURA's behaviour.
        let mut scored: Vec<(BriefingSection, f32)> = self
            .sections
            .iter()
            .map(|s| {
                let priority_score = 1.0 - (s.priority() as f32 / 7.0); // 0..1
                let engagement = self.engagement_rate(s);
                let rank = priority_score * 0.6 + engagement * 0.4;
                (s.clone(), rank)
            })
            .collect();

        // Sort descending by rank.
        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        // For heavy density, drop sections the user has consistently ignored
        // (engagement below floor). Still a display decision, not content routing.
        if density == ScheduleDensity::Heavy {
            scored.retain(|(s, _)| self.engagement_rate(s) >= MIN_ENGAGEMENT_RATE);
        }

        // Take top-N.
        scored
            .into_iter()
            .take(max)
            .map(|(section, _)| section)
            .collect()
    }

    /// Access the most recent generated briefing.
    #[must_use]
    pub fn last_generated(&self) -> Option<&GeneratedBriefing> {
        self.generated_history.last()
    }

    /// Number of generated briefings in history.
    #[must_use]
    pub fn generated_count(&self) -> usize {
        self.generated_history.len()
    }
}

impl Default for MorningBriefing {
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
    fn test_new_morning_briefing() {
        let mb = MorningBriefing::new();
        assert_eq!(mb.briefing_hour(), DEFAULT_BRIEFING_HOUR);
        assert_eq!(mb.last_briefing_day(), 0);
        assert_eq!(mb.section_count(), 4);
        assert_eq!(mb.wake_observations(), 0);
    }

    #[test]
    fn test_should_trigger() {
        let mb = MorningBriefing::new();
        // Hour matches, day differs from last → true
        assert!(mb.should_trigger(7, 1));
        // Wrong hour → false
        assert!(!mb.should_trigger(8, 1));
        // Same day as last briefing → false
        assert!(!mb.should_trigger(7, 0));
    }

    #[test]
    fn test_generate_once_per_day() {
        let mut mb = MorningBriefing::new();
        let sections = mb.generate(10).expect("first gen");
        assert!(!sections.is_empty());
        assert_eq!(mb.last_briefing_day(), 10);

        // Second call same day → empty
        let sections2 = mb.generate(10).expect("second gen");
        assert!(sections2.is_empty());

        // New day → sections again
        let sections3 = mb.generate(11).expect("third gen");
        assert!(!sections3.is_empty());
    }

    #[test]
    fn test_set_briefing_hour() {
        let mut mb = MorningBriefing::new();
        assert!(mb.set_briefing_hour(6).is_ok());
        assert_eq!(mb.briefing_hour(), 6);

        // Invalid hour
        assert!(mb.set_briefing_hour(24).is_err());
        assert!(mb.set_briefing_hour(255).is_err());
    }

    #[test]
    fn test_add_section_bounded() {
        let mut mb = MorningBriefing::new();
        mb.clear_sections();
        for i in 0..MAX_BRIEFING_SECTIONS {
            let result = mb.add_section(BriefingSection::Custom(format!("section_{i}")));
            assert!(result.is_ok(), "failed at section {i}");
        }
        // One more should fail
        let result = mb.add_section(BriefingSection::News);
        assert!(result.is_err());
    }

    #[test]
    fn test_briefing_section_serde() {
        let section = BriefingSection::Custom("My Section".into());
        let json = serde_json::to_string(&section).expect("serialize");
        let deserialized: BriefingSection = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(section, deserialized);
    }

    // ── Adaptive timing tests ──

    #[test]
    fn test_adaptive_timing_no_observations() {
        let mb = MorningBriefing::new();
        // Without enough observations, should return configured hour.
        assert_eq!(mb.briefing_hour(), DEFAULT_BRIEFING_HOUR);
    }

    #[test]
    fn test_adaptive_timing_with_observations() {
        let mut mb = MorningBriefing::new();
        // Observe wake times at hour 6, 6, 6, 6 → adaptive should drift toward 6.
        for _ in 0..10 {
            mb.observe_wake_time(6);
        }
        assert!(mb.wake_observations() >= MIN_WAKE_OBSERVATIONS);
        // Should now be close to 6.
        let hour = mb.briefing_hour();
        assert!(hour == 6 || hour == 7, "expected 6 or 7, got {hour}");
    }

    #[test]
    fn test_adaptive_hour_ema_drift() {
        let mut mb = MorningBriefing::new();
        // Start at default 7.0
        // Observe hour 9 repeatedly → should drift upward
        for _ in 0..20 {
            mb.observe_wake_time(9);
        }
        assert!(
            mb.adaptive_hour_raw() > 8.0,
            "expected >8.0, got {}",
            mb.adaptive_hour_raw()
        );
    }

    #[test]
    fn test_adaptive_hour_clamps() {
        let mut mb = MorningBriefing::new();
        // Observe extreme wake times
        for _ in 0..100 {
            mb.observe_wake_time(0);
        }
        let hour = mb.briefing_hour();
        assert!(hour <= 23, "hour should be valid, got {hour}");
    }

    // ── Schedule density tests ──

    #[test]
    fn test_schedule_density_classification() {
        assert_eq!(ScheduleDensity::from_event_count(0), ScheduleDensity::Light);
        assert_eq!(ScheduleDensity::from_event_count(2), ScheduleDensity::Light);
        assert_eq!(
            ScheduleDensity::from_event_count(3),
            ScheduleDensity::Normal
        );
        assert_eq!(
            ScheduleDensity::from_event_count(5),
            ScheduleDensity::Normal
        );
        assert_eq!(ScheduleDensity::from_event_count(6), ScheduleDensity::Heavy);
        assert_eq!(
            ScheduleDensity::from_event_count(100),
            ScheduleDensity::Heavy
        );
    }

    #[test]
    fn test_density_max_sections() {
        assert_eq!(ScheduleDensity::Light.max_sections(), MAX_BRIEFING_SECTIONS);
        assert_eq!(ScheduleDensity::Normal.max_sections(), 6);
        assert_eq!(ScheduleDensity::Heavy.max_sections(), 3);
    }

    #[test]
    fn test_generate_with_context_density() {
        let mut mb = MorningBriefing::new();

        // Light day
        let result = mb
            .generate_with_context(1, 1, Vec::new())
            .expect("light gen");
        assert_eq!(result.density, ScheduleDensity::Light);
        assert_eq!(result.sections.len(), 4); // All 4 default sections.

        // New day, heavy schedule
        let result2 = mb
            .generate_with_context(2, 10, Vec::new())
            .expect("heavy gen");
        assert_eq!(result2.density, ScheduleDensity::Heavy);
        // Heavy → max 3 sections.
        assert!(result2.sections.len() <= 3);
    }

    #[test]
    fn test_generate_with_context_dedup() {
        let mut mb = MorningBriefing::new();
        let r1 = mb.generate_with_context(5, 3, Vec::new()).expect("gen 1");
        assert!(!r1.sections.is_empty());

        // Same day → empty.
        let r2 = mb.generate_with_context(5, 3, Vec::new()).expect("gen 2");
        assert!(r2.sections.is_empty());
    }

    #[test]
    fn test_generate_with_provided_context() {
        let mut mb = MorningBriefing::new();
        let ctx = vec![SectionContext {
            section: BriefingSection::Weather,
            items: vec![ContextItem {
                label: "Temperature".into(),
                value: Some(22.0),
                urgency: 0.0,
            }],
        }];

        let result = mb.generate_with_context(1, 3, ctx).expect("gen");
        // Weather should have context.
        let weather_ctx = result
            .context
            .sections
            .iter()
            .find(|sc| sc.section == BriefingSection::Weather);
        assert!(weather_ctx.is_some());
        assert_eq!(weather_ctx.map(|c| c.items.len()).unwrap_or(0), 1);
    }

    // ── Engagement tests ──

    #[test]
    fn test_engagement_recording() {
        let mut mb = MorningBriefing::new();
        mb.record_engagement(&BriefingSection::Weather, true, 1);
        mb.record_engagement(&BriefingSection::Weather, true, 2);
        mb.record_engagement(&BriefingSection::Weather, false, 3);

        let rate = mb.engagement_rate(&BriefingSection::Weather);
        // 2 out of 3 = ~0.667
        assert!((rate - 0.667).abs() < 0.01, "expected ~0.667, got {rate}");
    }

    #[test]
    fn test_engagement_default_rate() {
        let mb = MorningBriefing::new();
        // No data → default 0.5
        assert!((mb.engagement_rate(&BriefingSection::News) - 0.5).abs() < 0.001);
    }

    #[test]
    fn test_engagement_history_bounded() {
        let mut mb = MorningBriefing::new();
        for i in 0..(MAX_ENGAGEMENT_HISTORY + 20) {
            mb.record_engagement(&BriefingSection::Weather, true, i as u32);
        }
        assert!(
            mb.engagement_history.len() <= MAX_ENGAGEMENT_HISTORY,
            "got {}",
            mb.engagement_history.len()
        );
    }

    #[test]
    fn test_section_priority_ordering() {
        // Health should have highest priority (0).
        assert!(BriefingSection::Health.priority() < BriefingSection::News.priority());
        assert!(BriefingSection::Calendar.priority() < BriefingSection::Social.priority());
    }

    #[test]
    fn test_section_domain_mapping() {
        assert_eq!(BriefingSection::Health.domain(), DomainId::Health);
        assert_eq!(BriefingSection::Calendar.domain(), DomainId::Productivity);
        assert_eq!(BriefingSection::Weather.domain(), DomainId::Environment);
    }

    #[test]
    fn test_section_key_stable() {
        assert_eq!(BriefingSection::Weather.key(), "weather");
        assert_eq!(BriefingSection::Custom("foo".into()).key(), "custom:foo");
    }

    #[test]
    fn test_generated_history_bounded() {
        let mut mb = MorningBriefing::new();
        for day in 1..=(MAX_GENERATED_HISTORY as u32 + 10) {
            let _ = mb.generate_with_context(day, 3, Vec::new());
        }
        assert!(
            mb.generated_count() <= MAX_GENERATED_HISTORY,
            "got {}",
            mb.generated_count()
        );
    }

    #[test]
    fn test_last_generated() {
        let mut mb = MorningBriefing::new();
        assert!(mb.last_generated().is_none());

        let _ = mb.generate_with_context(1, 2, Vec::new());
        assert!(mb.last_generated().is_some());
        assert_eq!(mb.last_generated().map(|g| g.day), Some(1));
    }

    #[test]
    fn test_context_items_bounded() {
        let mut mb = MorningBriefing::new();
        let items: Vec<ContextItem> = (0..20)
            .map(|i| ContextItem {
                label: format!("item_{i}"),
                value: Some(i as f64),
                urgency: 0.0,
            })
            .collect();
        let ctx = vec![SectionContext {
            section: BriefingSection::Health,
            items,
        }];

        let result = mb.generate_with_context(1, 3, ctx).expect("gen");
        for sc in &result.context.sections {
            assert!(
                sc.items.len() <= MAX_CONTEXT_ITEMS,
                "section {} has {} items",
                sc.section.key(),
                sc.items.len()
            );
        }
    }

    #[test]
    fn test_morning_briefing_serde_roundtrip() {
        let mut mb = MorningBriefing::new();
        mb.observe_wake_time(6);
        mb.observe_wake_time(6);
        mb.record_engagement(&BriefingSection::Weather, true, 1);

        let json = serde_json::to_string(&mb).expect("serialize");
        let back: MorningBriefing = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.wake_observations(), 2);
        assert_eq!(back.section_count(), 4);
    }
}
