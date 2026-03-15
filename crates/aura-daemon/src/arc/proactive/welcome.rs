//! Welcome-back greeting system — personality-influenced re-engagement.
//!
//! Cadence:
//! - **Days 1–7**: Daily tips & onboarding reinforcement.
//! - **Weeks 2–4**: Weekly highlights & feature spotlights.
//! - **Month 2+**: Special occasions only (milestones, updates, holidays).
//!
//! The system tracks user engagement with greetings and adaptively reduces
//! frequency if the user ignores them (consecutive_ignored counter).

use aura_types::identity::OceanTraits;
use serde::{Deserialize, Serialize};
use tracing::{debug, info};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum consecutive ignored greetings before the engine silences itself.
const MAX_CONSECUTIVE_IGNORED: u32 = 3;

/// Day threshold for switching from daily tips to weekly highlights.
const DAILY_TIP_END_DAY: u32 = 7;

/// Day threshold for switching from weekly highlights to special-only.
const WEEKLY_HIGHLIGHT_END_DAY: u32 = 28;

/// Maximum number of daily tips available.
const DAILY_TIPS_COUNT: usize = 7;

/// Maximum number of weekly highlights available.
const WEEKLY_HIGHLIGHTS_COUNT: usize = 3;

// ---------------------------------------------------------------------------
// WelcomeGreeting — the output type
// ---------------------------------------------------------------------------

/// A single welcome-back greeting to show the user.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WelcomeGreeting {
    /// Signal key for the greeting — passed to the LLM, not user-facing prose.
    /// Examples: "daily_tip_0", "weekly_highlight_0", "milestone_30d".
    pub greeting_key: &'static str,
    /// Optional signal key for an accompanying tip (passed to LLM).
    pub tip_key: Option<&'static str>,
    /// The category of greeting.
    pub category: GreetingCategory,
    /// Day number since onboarding completion.
    pub day_number: u32,
}

/// Categories of welcome greetings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum GreetingCategory {
    /// Days 1-7: daily tips reinforcing onboarding lessons.
    DailyTip,
    /// Weeks 2-4: weekly highlights showing AURA's value.
    WeeklyHighlight,
    /// Month 2+: special occasions (milestones, holidays, updates).
    SpecialOccasion,
    /// Milestone greeting (e.g., "You've been using AURA for 30 days!").
    Milestone,
}

// ---------------------------------------------------------------------------
// SpecialOccasion
// ---------------------------------------------------------------------------

/// Known special occasions that warrant a greeting.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpecialOccasion {
    /// Name of the occasion.
    pub name: String,
    /// Day-of-year (1–366) when this occasion falls.
    pub day_of_year: u16,
    /// Message template (personality fills in the tone).
    pub template: String,
}

// ---------------------------------------------------------------------------
// WelcomeState — persistent state
// ---------------------------------------------------------------------------

/// Persistent state for the welcome-back system.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WelcomeState {
    /// Day number when onboarding was completed (epoch day).
    pub onboarding_completed_day: u32,
    /// Last day a greeting was shown (epoch day).
    pub last_greeting_day: u32,
    /// Number of greetings shown total.
    pub total_greetings_shown: u32,
    /// Number of consecutive greetings the user ignored (did not interact with).
    pub consecutive_ignored: u32,
    /// Whether the user has explicitly opted out of greetings.
    pub opted_out: bool,
    /// Which daily tips have been shown (bitmask, bits 0..6).
    pub daily_tips_shown: u8,
    /// Which weekly highlights have been shown (bitmask, bits 0..2).
    pub weekly_highlights_shown: u8,
}

impl WelcomeState {
    /// Create a new welcome state anchored to the given onboarding completion day.
    #[must_use]
    pub fn new(onboarding_completed_day: u32) -> Self {
        Self {
            onboarding_completed_day,
            ..Default::default()
        }
    }

    /// Days elapsed since onboarding completed.
    #[must_use]
    pub fn days_since_onboarding(&self, current_day: u32) -> u32 {
        current_day.saturating_sub(self.onboarding_completed_day)
    }
}

// ---------------------------------------------------------------------------
// WelcomeEngine
// ---------------------------------------------------------------------------

/// The welcome-back greeting engine. Generates personality-influenced
/// greetings based on the user's day since onboarding, engagement history,
/// and current OCEAN traits.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WelcomeEngine {
    /// Persistent greeting state.
    pub state: WelcomeState,
    /// Custom special occasions (user birthdays, etc.).
    special_occasions: Vec<SpecialOccasion>,
}

impl WelcomeEngine {
    /// Create a new welcome engine for a user who completed onboarding
    /// on the given epoch day.
    #[must_use]
    pub fn new(onboarding_completed_day: u32) -> Self {
        Self {
            state: WelcomeState::new(onboarding_completed_day),
            special_occasions: Vec::new(),
        }
    }

    /// Add a special occasion (e.g., user's birthday).
    pub fn add_special_occasion(&mut self, occasion: SpecialOccasion) {
        self.special_occasions.push(occasion);
    }

    /// Record that the user acknowledged/interacted with a greeting.
    pub fn record_engagement(&mut self) {
        self.state.consecutive_ignored = 0;
        debug!("welcome greeting acknowledged — ignored counter reset");
    }

    /// Record that the user ignored a greeting.
    pub fn record_ignored(&mut self) {
        self.state.consecutive_ignored = self.state.consecutive_ignored.saturating_add(1);
        debug!(
            consecutive_ignored = self.state.consecutive_ignored,
            "welcome greeting ignored"
        );
    }

    /// User opts out of welcome greetings entirely.
    pub fn opt_out(&mut self) {
        self.state.opted_out = true;
        info!("user opted out of welcome greetings");
    }

    /// User opts back in to welcome greetings.
    pub fn opt_in(&mut self) {
        self.state.opted_out = false;
        self.state.consecutive_ignored = 0;
        info!("user opted back in to welcome greetings");
    }

    /// Whether the engine is currently silenced (opted out or too many ignores).
    #[must_use]
    pub fn is_silenced(&self) -> bool {
        self.state.opted_out || self.state.consecutive_ignored >= MAX_CONSECUTIVE_IGNORED
    }

    /// Check whether a greeting should be generated for the current day.
    ///
    /// Returns `true` if a greeting is due and the user hasn't been over-notified.
    #[must_use]
    pub fn should_greet(&self, current_day: u32) -> bool {
        if self.is_silenced() {
            return false;
        }

        // Don't greet more than once per day.
        if current_day <= self.state.last_greeting_day {
            return false;
        }

        let days_since = self.state.days_since_onboarding(current_day);

        if days_since == 0 {
            // Same day as onboarding — no greeting needed.
            return false;
        }

        if days_since <= DAILY_TIP_END_DAY {
            // Days 1-7: greet daily.
            return true;
        }

        if days_since <= WEEKLY_HIGHLIGHT_END_DAY {
            // Weeks 2-4: greet weekly (every 7 days).
            let days_since_last = current_day.saturating_sub(self.state.last_greeting_day);
            return days_since_last >= 7;
        }

        // Month 2+: only special occasions.
        false
    }

    /// Generate a welcome greeting for the current day, influenced by personality.
    ///
    /// Returns `None` if no greeting is due or the engine is silenced.
    pub fn generate_greeting(
        &mut self,
        current_day: u32,
        day_of_year: u16,
        personality: &OceanTraits,
    ) -> Option<WelcomeGreeting> {
        // Check for special occasions first (these bypass cadence).
        if let Some(greeting) = self.check_special_occasions(current_day, day_of_year, personality)
        {
            return Some(greeting);
        }

        // Check for milestones.
        if let Some(greeting) = self.check_milestones(current_day, personality) {
            return Some(greeting);
        }

        if !self.should_greet(current_day) {
            return None;
        }

        let days_since = self.state.days_since_onboarding(current_day);

        let greeting = if days_since <= DAILY_TIP_END_DAY {
            self.generate_daily_tip(days_since, personality)
        } else if days_since <= WEEKLY_HIGHLIGHT_END_DAY {
            self.generate_weekly_highlight(days_since, personality)
        } else {
            // Beyond week 4, should_greet already returned false for non-special days.
            return None;
        };

        self.state.last_greeting_day = current_day;
        self.state.total_greetings_shown = self.state.total_greetings_shown.saturating_add(1);

        Some(greeting)
    }

    // -----------------------------------------------------------------------
    // Daily tips (days 1-7)
    // -----------------------------------------------------------------------

    fn generate_daily_tip(&mut self, day: u32, personality: &OceanTraits) -> WelcomeGreeting {
        let tip_index = ((day - 1) as usize).min(DAILY_TIPS_COUNT - 1);

        // Mark tip as shown.
        self.state.daily_tips_shown |= 1 << tip_index;

        let (greeting_key, tip_key) = daily_tip_content(tip_index, personality);

        debug!(day, tip_index, "generating daily tip");

        WelcomeGreeting {
            greeting_key,
            tip_key: Some(tip_key),
            category: GreetingCategory::DailyTip,
            day_number: day,
        }
    }

    // -----------------------------------------------------------------------
    // Weekly highlights (weeks 2-4)
    // -----------------------------------------------------------------------

    fn generate_weekly_highlight(
        &mut self,
        day: u32,
        personality: &OceanTraits,
    ) -> WelcomeGreeting {
        let week_number = (day / 7).min(WEEKLY_HIGHLIGHTS_COUNT as u32);
        let highlight_index = week_number.saturating_sub(1) as usize;

        // Mark highlight as shown.
        if highlight_index < 8 {
            self.state.weekly_highlights_shown |= 1 << highlight_index;
        }

        let (greeting_key, tip_key) = weekly_highlight_content(highlight_index, personality);

        debug!(day, week_number, "generating weekly highlight");

        WelcomeGreeting {
            greeting_key,
            tip_key: Some(tip_key),
            category: GreetingCategory::WeeklyHighlight,
            day_number: day,
        }
    }

    // -----------------------------------------------------------------------
    // Special occasions
    // -----------------------------------------------------------------------

    fn check_special_occasions(
        &mut self,
        current_day: u32,
        day_of_year: u16,
        personality: &OceanTraits,
    ) -> Option<WelcomeGreeting> {
        // Don't re-greet on same day.
        if current_day <= self.state.last_greeting_day {
            return None;
        }

        for occasion in &self.special_occasions {
            if occasion.day_of_year == day_of_year {
                let greeting_key = personality_tint_key(personality);

                self.state.last_greeting_day = current_day;
                self.state.total_greetings_shown =
                    self.state.total_greetings_shown.saturating_add(1);

                info!(occasion = %occasion.name, "special occasion greeting");

                return Some(WelcomeGreeting {
                    greeting_key,
                    tip_key: None,
                    category: GreetingCategory::SpecialOccasion,
                    day_number: self.state.days_since_onboarding(current_day),
                });
            }
        }

        None
    }

    // -----------------------------------------------------------------------
    // Milestones
    // -----------------------------------------------------------------------

    fn check_milestones(
        &mut self,
        current_day: u32,
        personality: &OceanTraits,
    ) -> Option<WelcomeGreeting> {
        let days_since = self.state.days_since_onboarding(current_day);

        // Don't re-greet on same day.
        if current_day <= self.state.last_greeting_day {
            return None;
        }

        let milestone_days = [30, 60, 90, 180, 365];
        if !milestone_days.contains(&days_since) {
            return None;
        }

        let greeting_key = milestone_message(days_since, personality);

        self.state.last_greeting_day = current_day;
        self.state.total_greetings_shown = self.state.total_greetings_shown.saturating_add(1);

        info!(days = days_since, "milestone greeting generated");

        Some(WelcomeGreeting {
            greeting_key,
            tip_key: None,
            category: GreetingCategory::Milestone,
            day_number: days_since,
        })
    }

    // -----------------------------------------------------------------------
    // SQLite persistence
    // -----------------------------------------------------------------------

    /// Save welcome state to SQLite.
    pub fn save_to_db(&self, conn: &rusqlite::Connection) -> Result<(), rusqlite::Error> {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS welcome_state (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
            )",
        )?;

        let json = serde_json::to_string(&self.state).unwrap_or_default();
        conn.execute(
            "INSERT OR REPLACE INTO welcome_state (key, value) VALUES ('state', ?1)",
            rusqlite::params![json],
        )?;

        // Save special occasions.
        let occasions_json = serde_json::to_string(&self.special_occasions).unwrap_or_default();
        conn.execute(
            "INSERT OR REPLACE INTO welcome_state (key, value) VALUES ('occasions', ?1)",
            rusqlite::params![occasions_json],
        )?;

        debug!("welcome state saved to database");
        Ok(())
    }

    /// Load welcome state from SQLite.
    pub fn load_from_db(conn: &rusqlite::Connection) -> Result<Option<Self>, rusqlite::Error> {
        // Check if table exists.
        let table_exists: bool = conn
            .prepare(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='welcome_state'",
            )?
            .query_row([], |row| row.get::<_, i64>(0))
            .map(|c| c > 0)?;

        if !table_exists {
            return Ok(None);
        }

        let state_json: Option<String> = conn
            .prepare("SELECT value FROM welcome_state WHERE key = 'state'")?
            .query_row([], |row| row.get(0))
            .ok();

        let state: WelcomeState = match state_json {
            Some(json) => serde_json::from_str(&json).unwrap_or_default(),
            None => return Ok(None),
        };

        let occasions_json: Option<String> = conn
            .prepare("SELECT value FROM welcome_state WHERE key = 'occasions'")?
            .query_row([], |row| row.get(0))
            .ok();

        let special_occasions: Vec<SpecialOccasion> = match occasions_json {
            Some(json) => serde_json::from_str(&json).unwrap_or_default(),
            None => Vec::new(),
        };

        Ok(Some(Self {
            state,
            special_occasions,
        }))
    }
}

impl Default for WelcomeEngine {
    fn default() -> Self {
        Self::new(0)
    }
}

// ---------------------------------------------------------------------------
// Content generation helpers (personality-influenced signal keys)
// ---------------------------------------------------------------------------

/// Return a personality-variant signal key for special-occasion greetings.
///
/// The LLM uses the key together with the `SpecialOccasion.template` field
/// to compose the actual user-facing message.  No prose is assembled here.
fn personality_tint_key(personality: &OceanTraits) -> &'static str {
    if personality.extraversion > 0.65 {
        "personality_extraverted"
    } else if personality.agreeableness > 0.65 {
        "personality_agreeable"
    } else {
        "personality_neutral"
    }
}

/// Return (greeting_key, tip_key) signal pair for a daily tip by index.
///
/// Keys are passed to the LLM — no user-facing prose is assembled here.
fn daily_tip_content(index: usize, personality: &OceanTraits) -> (&'static str, &'static str) {
    let warm = personality.agreeableness > 0.6;
    let direct = personality.extraversion < 0.4;

    match index {
        0 => {
            let greeting = if warm {
                "daily_tip_0_warm"
            } else {
                "daily_tip_0_neutral"
            };
            (greeting, "daily_tip_0_tip")
        },
        1 => {
            let greeting = if direct {
                "daily_tip_1_direct"
            } else {
                "daily_tip_1_friendly"
            };
            (greeting, "daily_tip_1_tip")
        },
        2 => ("daily_tip_2", "daily_tip_2_tip"),
        3 => ("daily_tip_3", "daily_tip_3_tip"),
        4 => ("daily_tip_4", "daily_tip_4_tip"),
        5 => {
            let greeting = if warm {
                "daily_tip_5_warm"
            } else {
                "daily_tip_5_neutral"
            };
            (greeting, "daily_tip_5_tip")
        },
        6 => ("daily_tip_6", "daily_tip_6_tip"),
        _ => ("daily_tip_default", "daily_tip_default_tip"),
    }
}

/// Return (greeting_key, tip_key) signal pair for a weekly highlight by index.
fn weekly_highlight_content(
    index: usize,
    personality: &OceanTraits,
) -> (&'static str, &'static str) {
    let enthusiastic = personality.extraversion > 0.6;

    match index {
        0 => {
            let greeting = if enthusiastic {
                "weekly_0_enthusiastic"
            } else {
                "weekly_0_calm"
            };
            (greeting, "weekly_0_tip")
        },
        1 => ("weekly_1", "weekly_1_tip"),
        2 => ("weekly_2", "weekly_2_tip"),
        _ => ("weekly_default", "weekly_default_tip"),
    }
}

/// Return a milestone greeting signal key.
fn milestone_message(days: u32, personality: &OceanTraits) -> &'static str {
    let warm = personality.agreeableness > 0.6;
    let enthusiastic = personality.extraversion > 0.6;

    match days {
        30 => {
            if enthusiastic {
                "milestone_30d_enthusiastic"
            } else if warm {
                "milestone_30d_warm"
            } else {
                "milestone_30d_neutral"
            }
        },
        60 => "milestone_60d",
        90 => "milestone_90d",
        180 => {
            if warm {
                "milestone_180d_warm"
            } else {
                "milestone_180d_neutral"
            }
        },
        365 => "milestone_365d",
        _ => "milestone_generic",
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn default_personality() -> OceanTraits {
        OceanTraits::DEFAULT
    }

    fn warm_personality() -> OceanTraits {
        OceanTraits {
            openness: 0.8,
            conscientiousness: 0.7,
            extraversion: 0.8,
            agreeableness: 0.8,
            neuroticism: 0.2,
        }
    }

    #[test]
    fn test_welcome_state_new() {
        let state = WelcomeState::new(100);
        assert_eq!(state.onboarding_completed_day, 100);
        assert_eq!(state.last_greeting_day, 0);
        assert_eq!(state.total_greetings_shown, 0);
        assert_eq!(state.consecutive_ignored, 0);
        assert!(!state.opted_out);
    }

    #[test]
    fn test_days_since_onboarding() {
        let state = WelcomeState::new(100);
        assert_eq!(state.days_since_onboarding(100), 0);
        assert_eq!(state.days_since_onboarding(107), 7);
        assert_eq!(state.days_since_onboarding(130), 30);
    }

    #[test]
    fn test_welcome_engine_new() {
        let engine = WelcomeEngine::new(100);
        assert_eq!(engine.state.onboarding_completed_day, 100);
        assert!(!engine.is_silenced());
    }

    #[test]
    fn test_should_greet_same_day() {
        let engine = WelcomeEngine::new(100);
        // Day 0 after onboarding — should not greet.
        assert!(!engine.should_greet(100));
    }

    #[test]
    fn test_should_greet_day_1() {
        let engine = WelcomeEngine::new(100);
        assert!(engine.should_greet(101));
    }

    #[test]
    fn test_should_greet_day_7() {
        let engine = WelcomeEngine::new(100);
        assert!(engine.should_greet(107));
    }

    #[test]
    fn test_should_not_greet_when_silenced() {
        let mut engine = WelcomeEngine::new(100);
        engine.state.consecutive_ignored = MAX_CONSECUTIVE_IGNORED;
        assert!(engine.is_silenced());
        assert!(!engine.should_greet(101));
    }

    #[test]
    fn test_should_not_greet_when_opted_out() {
        let mut engine = WelcomeEngine::new(100);
        engine.opt_out();
        assert!(engine.is_silenced());
        assert!(!engine.should_greet(101));
    }

    #[test]
    fn test_opt_in_resets_ignored() {
        let mut engine = WelcomeEngine::new(100);
        engine.state.consecutive_ignored = 5;
        engine.opt_out();
        engine.opt_in();
        assert!(!engine.is_silenced());
        assert_eq!(engine.state.consecutive_ignored, 0);
    }

    #[test]
    fn test_record_engagement_resets_ignored() {
        let mut engine = WelcomeEngine::new(100);
        engine.record_ignored();
        engine.record_ignored();
        assert_eq!(engine.state.consecutive_ignored, 2);
        engine.record_engagement();
        assert_eq!(engine.state.consecutive_ignored, 0);
    }

    #[test]
    fn test_generate_daily_tip_day_1() {
        let mut engine = WelcomeEngine::new(100);
        let personality = default_personality();
        let greeting = engine.generate_greeting(101, 1, &personality);
        assert!(greeting.is_some());
        let g = greeting.expect("should have greeting");
        assert_eq!(g.category, GreetingCategory::DailyTip);
        assert_eq!(g.day_number, 1);
        assert!(g.tip_key.is_some());
    }

    #[test]
    fn test_no_duplicate_greeting_same_day() {
        let mut engine = WelcomeEngine::new(100);
        let personality = default_personality();
        let g1 = engine.generate_greeting(101, 1, &personality);
        assert!(g1.is_some());
        let g2 = engine.generate_greeting(101, 1, &personality);
        assert!(g2.is_none(), "should not greet twice on the same day");
    }

    #[test]
    fn test_generate_all_7_daily_tips() {
        let mut engine = WelcomeEngine::new(100);
        let personality = default_personality();
        for day_offset in 1..=7u32 {
            let g = engine.generate_greeting(100 + day_offset, day_offset as u16, &personality);
            assert!(g.is_some(), "should greet on day {day_offset}");
            let g = g.expect("greeting");
            assert_eq!(g.category, GreetingCategory::DailyTip);
        }
        // All 7 tips should be marked as shown.
        assert_eq!(engine.state.daily_tips_shown, 0b0111_1111);
    }

    #[test]
    fn test_weekly_highlight_week_2() {
        let mut engine = WelcomeEngine::new(100);
        let personality = default_personality();
        // Simulate days 1-7 greetings.
        for d in 1..=7u32 {
            engine.generate_greeting(100 + d, d as u16, &personality);
        }
        // Day 14 (week 2) — should generate weekly highlight.
        let g = engine.generate_greeting(114, 14, &personality);
        assert!(g.is_some());
        let g = g.expect("greeting");
        assert_eq!(g.category, GreetingCategory::WeeklyHighlight);
    }

    #[test]
    fn test_no_greeting_after_week_4() {
        let mut engine = WelcomeEngine::new(100);
        // Pretend we already got all daily + weekly greetings.
        engine.state.last_greeting_day = 128;
        let personality = default_personality();
        // Day 35 — beyond week 4, no special occasion.
        let g = engine.generate_greeting(135, 35, &personality);
        assert!(g.is_none(), "should not greet after week 4 normally");
    }

    #[test]
    fn test_milestone_30_days() {
        let mut engine = WelcomeEngine::new(100);
        engine.state.last_greeting_day = 129;
        let personality = default_personality();
        let g = engine.generate_greeting(130, 30, &personality);
        assert!(g.is_some());
        let g = g.expect("greeting");
        assert_eq!(g.category, GreetingCategory::Milestone);
        assert_eq!(g.day_number, 30);
    }

    #[test]
    fn test_special_occasion() {
        let mut engine = WelcomeEngine::new(100);
        engine.add_special_occasion(SpecialOccasion {
            name: "User Birthday".to_string(),
            day_of_year: 200,
            template: "Happy birthday!".to_string(),
        });

        let personality = default_personality();
        // Day 200 of the year, even if it's beyond week 4.
        engine.state.last_greeting_day = 199;
        let g = engine.generate_greeting(300, 200, &personality);
        assert!(g.is_some());
        let g = g.expect("greeting");
        assert_eq!(g.category, GreetingCategory::SpecialOccasion);
        // greeting_key reflects personality variant (not hardcoded prose)
        assert!(!g.greeting_key.is_empty());
    }

    #[test]
    fn test_personality_tint_high_extraversion() {
        let personality = warm_personality();
        let key = personality_tint_key(&personality);
        assert_eq!(key, "personality_extraverted", "got: {key}");
    }

    #[test]
    fn test_personality_tint_neutral() {
        let personality = OceanTraits {
            openness: 0.5,
            conscientiousness: 0.5,
            extraversion: 0.3,
            agreeableness: 0.3,
            neuroticism: 0.5,
        };
        let key = personality_tint_key(&personality);
        assert_eq!(key, "personality_neutral");
    }

    #[test]
    fn test_sqlite_persistence() {
        let conn = rusqlite::Connection::open_in_memory().expect("in-memory db");
        let mut engine = WelcomeEngine::new(100);
        engine.state.total_greetings_shown = 5;
        engine.state.daily_tips_shown = 0b0000_0111;
        engine.add_special_occasion(SpecialOccasion {
            name: "Test".to_string(),
            day_of_year: 42,
            template: "Test occasion".to_string(),
        });

        engine.save_to_db(&conn).expect("save");

        let loaded = WelcomeEngine::load_from_db(&conn)
            .expect("load")
            .expect("should find state");

        assert_eq!(loaded.state.total_greetings_shown, 5);
        assert_eq!(loaded.state.daily_tips_shown, 0b0000_0111);
        assert_eq!(loaded.special_occasions.len(), 1);
        assert_eq!(loaded.special_occasions[0].name, "Test");
    }

    #[test]
    fn test_load_from_empty_db() {
        let conn = rusqlite::Connection::open_in_memory().expect("in-memory db");
        let result = WelcomeEngine::load_from_db(&conn).expect("load");
        assert!(result.is_none(), "should return None for empty db");
    }

    #[test]
    fn test_consecutive_ignored_silencing() {
        let mut engine = WelcomeEngine::new(100);
        for _ in 0..MAX_CONSECUTIVE_IGNORED {
            engine.record_ignored();
        }
        assert!(engine.is_silenced());
        // Greeting should not be generated.
        let personality = default_personality();
        let g = engine.generate_greeting(101, 1, &personality);
        assert!(g.is_none());
    }

    #[test]
    fn test_milestone_365_days() {
        let mut engine = WelcomeEngine::new(100);
        engine.state.last_greeting_day = 464;
        let personality = default_personality();
        let g = engine.generate_greeting(465, 1, &personality);
        assert!(g.is_some());
        let g = g.expect("greeting");
        assert_eq!(g.category, GreetingCategory::Milestone);
        assert_eq!(g.greeting_key, "milestone_365d");
    }
}
