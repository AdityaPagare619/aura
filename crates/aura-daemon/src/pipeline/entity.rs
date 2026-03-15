//! Structural parameter extraction for AURA v4's pipeline.
//!
//! Extracts typed parameters from user input for slot filling:
//! - Time expressions ("3pm", "tomorrow at noon", "in 5 minutes")
//! - Durations ("5 minutes", "half an hour")
//! - Numbers and number words ("five" → 5)
//! - URLs
//! - Setting names (structural keyword match against a fixed enum)
//!
//! Architecture note — Theater AGI guard:
//! Contact/app fuzzy matching (NLU in Rust) is stubbed out. All fuzzy
//! entity resolution is deferred to the LLM. Only structural/typed
//! parameter extraction remains here (Iron Law #4).

// Cap on entities emitted per extraction call.
const MAX_PIPELINE_ENTITIES: usize = 512;

use tracing::{debug, instrument, trace};

// ---------------------------------------------------------------------------
// Entity types
// ---------------------------------------------------------------------------

/// Classification of an extracted entity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EntityType {
    /// Absolute or relative time expression.
    Time,
    /// Duration (span of time).
    Duration,
    /// Person/contact name.
    Contact,
    /// Application name.
    App,
    /// Numeric value.
    Number,
    /// Web URL.
    Url,
    /// System setting name.
    Setting,
    /// Unclassified but potentially significant.
    Unknown,
}

/// A single extracted entity with position and confidence.
#[derive(Debug, Clone)]
pub struct Entity {
    /// What kind of entity this is.
    pub entity_type: EntityType,
    /// The raw text span from the input.
    pub raw: String,
    /// Normalized/resolved value.
    pub value: String,
    /// Start position (character index) in the original input.
    pub span_start: usize,
    /// End position (character index, exclusive).
    pub span_end: usize,
    /// Confidence score (0.0–1.0).
    pub confidence: f32,
}

/// Stateful entity extractor — holds known contacts and apps for matching.
pub struct EntityExtractor {
    /// Known contact names for fuzzy matching.
    known_contacts: Vec<String>,
    /// Known app names (display names) for fuzzy matching.
    known_apps: Vec<String>,
}

impl EntityExtractor {
    /// Create a new extractor with known contacts and apps.
    #[instrument(skip(contacts, apps), fields(contacts = contacts.len(), apps = apps.len()))]
    pub fn new(contacts: Vec<String>, apps: Vec<String>) -> Self {
        trace!("EntityExtractor initialized");
        Self {
            known_contacts: contacts,
            known_apps: apps,
        }
    }

    /// Create an extractor with no known entities (for testing / degraded mode).
    pub fn empty() -> Self {
        Self {
            known_contacts: Vec::new(),
            known_apps: Vec::new(),
        }
    }

    /// Update the known contacts list.
    pub fn set_contacts(&mut self, contacts: Vec<String>) {
        self.known_contacts = contacts;
    }

    /// Update the known apps list.
    pub fn set_apps(&mut self, apps: Vec<String>) {
        self.known_apps = apps;
    }

    /// Extract all entities from input text.
    #[instrument(skip(self, input), fields(input_len = input.len()))]
    pub fn extract(&self, input: &str) -> Vec<Entity> {
        let mut entities = Vec::new();
        let lower = input.to_lowercase();

        // Order matters: more specific patterns first to avoid overlapping.
        self.extract_urls(input, &mut entities);
        self.extract_time_expressions(input, &lower, &mut entities);
        self.extract_durations(input, &lower, &mut entities);
        self.extract_numbers(input, &lower, &mut entities);
        self.extract_settings(input, &lower, &mut entities);

        // Architecture note — Theater AGI guard:
        // Contact/app fuzzy matching (NLU in Rust) is removed (Iron Law #4).
        // The LLM resolves contact and app names from its context.

        // Sort by position for stable output.
        entities.sort_by_key(|e| e.span_start);

        // Cap output to prevent unbounded growth.
        entities.truncate(MAX_PIPELINE_ENTITIES);

        debug!(entity_count = entities.len(), "entities extracted");
        entities
    }

    // -- Time extraction -----------------------------------------------------

    fn extract_time_expressions(&self, input: &str, lower: &str, out: &mut Vec<Entity>) {
        // Pattern: "at {time}" or bare time patterns
        // Handles: "3pm", "3:30pm", "15:00", "noon", "midnight"
        // Relative: "in 5 minutes", "tomorrow", "next monday"

        // Named times
        let named_times = [
            ("noon", "12:00"),
            ("midnight", "00:00"),
            ("morning", "09:00"),
            ("afternoon", "14:00"),
            ("evening", "18:00"),
            ("tonight", "20:00"),
        ];
        for (name, value) in &named_times {
            if let Some(pos) = lower.find(name) {
                // Make sure it's a word boundary.
                if is_word_boundary(lower, pos, name.len()) {
                    out.push(Entity {
                        entity_type: EntityType::Time,
                        raw: input[pos..pos + name.len()].to_string(),
                        value: value.to_string(),
                        span_start: pos,
                        span_end: pos + name.len(),
                        confidence: 0.9,
                    });
                }
            }
        }

        // Relative day references
        let day_refs = [
            "today",
            "tomorrow",
            "yesterday",
            "next monday",
            "next tuesday",
            "next wednesday",
            "next thursday",
            "next friday",
            "next saturday",
            "next sunday",
        ];
        for day in &day_refs {
            if let Some(pos) = lower.find(day) {
                if is_word_boundary(lower, pos, day.len()) {
                    out.push(Entity {
                        entity_type: EntityType::Time,
                        raw: input[pos..pos + day.len()].to_string(),
                        value: day.to_string(),
                        span_start: pos,
                        span_end: pos + day.len(),
                        confidence: 0.85,
                    });
                }
            }
        }

        // Clock times: scan for patterns like "3pm", "3:30pm", "15:00", "3:30 pm"
        self.extract_clock_times(input, lower, out);
    }

    fn extract_clock_times(&self, input: &str, lower: &str, out: &mut Vec<Entity>) {
        let bytes = lower.as_bytes();
        let len = bytes.len();
        let mut i = 0;

        while i < len {
            // Look for a digit that starts a potential time.
            if bytes[i].is_ascii_digit() {
                let num_start = i;
                // Consume digits.
                while i < len && bytes[i].is_ascii_digit() {
                    i += 1;
                }
                let first_num = &lower[num_start..i];

                // Check for colon (e.g., "3:30").
                let mut has_minutes = false;
                let mut end = i;
                if i < len && bytes[i] == b':' {
                    let colon = i;
                    i += 1;
                    let min_start = i;
                    while i < len && bytes[i].is_ascii_digit() {
                        i += 1;
                    }
                    if i - min_start == 2 {
                        has_minutes = true;
                        end = i;
                    } else {
                        // Not a valid time format, reset.
                        i = colon;
                        end = colon;
                    }
                }

                // Check for am/pm suffix (with optional space).
                let mut skip_space = end;
                if skip_space < len && bytes[skip_space] == b' ' {
                    skip_space += 1;
                }
                let has_ampm = if skip_space + 2 <= len {
                    let suffix = &lower[skip_space..skip_space + 2];
                    suffix == "am" || suffix == "pm"
                } else {
                    false
                };

                if has_ampm {
                    let ampm_end = skip_space + 2;
                    let raw = &input[num_start..ampm_end];
                    let normalized = normalize_clock_time(first_num, has_minutes, lower, end);
                    out.push(Entity {
                        entity_type: EntityType::Time,
                        raw: raw.to_string(),
                        value: normalized,
                        span_start: num_start,
                        span_end: ampm_end,
                        confidence: 0.95,
                    });
                    i = ampm_end;
                    continue;
                }

                // 24-hour format: "15:00" (has_minutes, hour > 12 or == 0).
                if has_minutes {
                    if let Ok(hour) = first_num.parse::<u32>() {
                        if hour <= 23 {
                            let raw = &input[num_start..end];
                            out.push(Entity {
                                entity_type: EntityType::Time,
                                raw: raw.to_string(),
                                value: raw.to_string(),
                                span_start: num_start,
                                span_end: end,
                                confidence: 0.75,
                            });
                        }
                    }
                    i = end;
                    continue;
                }
            }

            i += 1;
        }
    }

    // -- Duration extraction -------------------------------------------------

    fn extract_durations(&self, input: &str, lower: &str, out: &mut Vec<Entity>) {
        // "half an hour", "an hour", "a minute"
        let named_durations = [
            ("half an hour", "30m"),
            ("an hour and a half", "90m"),
            ("a half hour", "30m"),
            ("an hour", "60m"),
            ("a minute", "1m"),
        ];
        for (pattern, value) in &named_durations {
            if let Some(pos) = lower.find(pattern) {
                out.push(Entity {
                    entity_type: EntityType::Duration,
                    raw: input[pos..pos + pattern.len()].to_string(),
                    value: value.to_string(),
                    span_start: pos,
                    span_end: pos + pattern.len(),
                    confidence: 0.9,
                });
            }
        }

        // "{N} {unit}" patterns — "5 minutes", "2 hours", "30 seconds"
        let units = [
            ("second", "s"),
            ("seconds", "s"),
            ("sec", "s"),
            ("minute", "m"),
            ("minutes", "m"),
            ("min", "m"),
            ("mins", "m"),
            ("hour", "h"),
            ("hours", "h"),
            ("hr", "h"),
            ("hrs", "h"),
        ];

        let words: Vec<&str> = lower
            .split_whitespace()
            .take(MAX_PIPELINE_ENTITIES)
            .collect();
        let input_words: Vec<&str> = input
            .split_whitespace()
            .take(MAX_PIPELINE_ENTITIES)
            .collect();
        for i in 0..words.len().saturating_sub(1) {
            if let Some(num) = parse_number_word(words[i]) {
                for (unit, suffix) in &units {
                    if words.get(i + 1) == Some(unit) {
                        let raw_start = byte_offset_of_word(input, &input_words, i);
                        let raw_end = byte_offset_of_word_end(input, &input_words, i + 1);
                        out.push(Entity {
                            entity_type: EntityType::Duration,
                            raw: input[raw_start..raw_end].to_string(),
                            value: format!("{}{}", num, suffix),
                            span_start: raw_start,
                            span_end: raw_end,
                            confidence: 0.9,
                        });
                        break;
                    }
                }
            }
        }
    }

    // -- Number extraction ---------------------------------------------------

    fn extract_numbers(&self, _input: &str, lower: &str, out: &mut Vec<Entity>) {
        // Extract number words not already captured by duration/time.
        let words: Vec<&str> = lower
            .split_whitespace()
            .take(MAX_PIPELINE_ENTITIES)
            .collect();
        for (i, word) in words.iter().enumerate() {
            if let Some(num) = parse_number_word(word) {
                let start = byte_offset_of_word(lower, &words, i);
                let end = start + word.len();
                // Skip if this position overlaps with an already-extracted entity.
                let overlaps = out.iter().any(|e| start < e.span_end && end > e.span_start);
                if !overlaps {
                    out.push(Entity {
                        entity_type: EntityType::Number,
                        raw: word.to_string(),
                        value: num.to_string(),
                        span_start: start,
                        span_end: end,
                        confidence: 0.85,
                    });
                }
            }
        }
    }

    // -- URL extraction ------------------------------------------------------

    fn extract_urls(&self, input: &str, out: &mut Vec<Entity>) {
        for word in input.split_whitespace() {
            if (word.starts_with("http://") || word.starts_with("https://")) && word.len() > 10 {
                let start = input.find(word).unwrap_or(0);
                out.push(Entity {
                    entity_type: EntityType::Url,
                    raw: word.to_string(),
                    value: word.to_string(),
                    span_start: start,
                    span_end: start + word.len(),
                    confidence: 0.95,
                });
            }
        }
    }

    // -- Settings extraction -------------------------------------------------

    fn extract_settings(&self, _input: &str, lower: &str, out: &mut Vec<Entity>) {
        let settings = [
            ("wifi", "wifi"),
            ("wi-fi", "wifi"),
            ("bluetooth", "bluetooth"),
            ("airplane mode", "airplane_mode"),
            ("flight mode", "airplane_mode"),
            ("mobile data", "mobile_data"),
            ("cellular data", "mobile_data"),
            ("location", "location"),
            ("gps", "location"),
            ("auto rotate", "auto_rotate"),
            ("auto-rotate", "auto_rotate"),
            ("do not disturb", "do_not_disturb"),
            ("dnd", "do_not_disturb"),
            ("hotspot", "hotspot"),
            ("nfc", "nfc"),
            ("flashlight", "flashlight"),
            ("torch", "flashlight"),
        ];

        for (pattern, normalized) in &settings {
            if let Some(pos) = lower.find(pattern) {
                if is_word_boundary(lower, pos, pattern.len()) {
                    out.push(Entity {
                        entity_type: EntityType::Setting,
                        raw: pattern.to_string(),
                        value: normalized.to_string(),
                        span_start: pos,
                        span_end: pos + pattern.len(),
                        confidence: 0.9,
                    });
                }
            }
        }
    }

    // -- Contact fuzzy matching ----------------------------------------------

    // Phase 8 wire point: stubbed per Iron Law #4 (Theater AGI guard).
    // Re-enable only if contacts are needed as typed structured parameters
    // for tool dispatch (not intent classification).
    #[allow(dead_code)]
    fn extract_contacts(&self, _input: &str, _out: &mut Vec<Entity>) {
        // Architecture note — Theater AGI guard (Iron Law #4):
        // Contact fuzzy matching (NLU in Rust — Levenshtein over user names) is
        // deliberately stubbed. All contact resolution is deferred to the LLM,
        // which has access to the full contact list via tool context. Doing this
        // in Rust would constitute Theater AGI: the system pretending to
        // understand who the user means rather than passing the raw input to the
        // model. Wire point: re-enable only if contacts are needed as typed
        // structured parameters for tool dispatch (not intent classification).
        return;
    }

    // -- App fuzzy matching --------------------------------------------------

    // Phase 8 wire point: stubbed per Iron Law #4 (Theater AGI guard).
    // Re-enable only if app names are required as typed structural parameters
    // for tool dispatch (not intent classification).
    #[allow(dead_code)]
    fn extract_apps(&self, _input: &str, _lower: &str, _out: &mut Vec<Entity>) {
        // Architecture note — Theater AGI guard (Iron Law #4):
        // App name fuzzy matching (NLU in Rust) is deliberately stubbed. App
        // resolution is deferred to the LLM. Wire point: re-enable only if app
        // names are required as typed structural parameters for tool dispatch.
        return;
    }
}

// ---------------------------------------------------------------------------
// Helper functions
// ---------------------------------------------------------------------------

/// Parse a number word to its numeric value.
/// Handles: digit strings, English word numbers, and common shorthand.
pub fn parse_number_word(word: &str) -> Option<u64> {
    // Direct digit parse.
    if let Ok(n) = word.parse::<u64>() {
        return Some(n);
    }

    match word {
        "zero" => Some(0),
        "one" | "a" | "an" => Some(1),
        "two" => Some(2),
        "three" => Some(3),
        "four" => Some(4),
        "five" => Some(5),
        "six" => Some(6),
        "seven" => Some(7),
        "eight" => Some(8),
        "nine" => Some(9),
        "ten" => Some(10),
        "eleven" => Some(11),
        "twelve" | "a dozen" => Some(12),
        "thirteen" => Some(13),
        "fourteen" => Some(14),
        "fifteen" => Some(15),
        "sixteen" => Some(16),
        "seventeen" => Some(17),
        "eighteen" => Some(18),
        "nineteen" => Some(19),
        "twenty" => Some(20),
        "thirty" => Some(30),
        "forty" => Some(40),
        "fifty" => Some(50),
        "sixty" => Some(60),
        "hundred" => Some(100),
        _ => None,
    }
}

/// Levenshtein edit distance between two strings.
/// Bounded: returns early if distance exceeds `max_dist` (default 3).
/// Wire point: used by extract_contacts() which is stubbed (Theater AGI guard).
/// Keep for future re-enable if structural contact dispatch is needed.
#[allow(dead_code)]
pub fn levenshtein(a: &str, b: &str) -> usize {
    let a_len = a.len();
    let b_len = b.len();

    // Quick exit for obvious cases.
    if a_len == 0 {
        return b_len;
    }
    if b_len == 0 {
        return a_len;
    }
    if a == b {
        return 0;
    }

    // Early exit if length difference alone exceeds threshold.
    let len_diff = if a_len > b_len {
        a_len - b_len
    } else {
        b_len - a_len
    };
    if len_diff > 3 {
        return len_diff;
    }

    let a_bytes = a.as_bytes();
    let b_bytes = b.as_bytes();

    // Single-row DP.
    let mut prev_row: Vec<usize> = (0..=b_len).collect();
    let mut curr_row = vec![0usize; b_len + 1];

    for i in 1..=a_len {
        curr_row[0] = i;
        for j in 1..=b_len {
            let cost = if a_bytes[i - 1] == b_bytes[j - 1] {
                0
            } else {
                1
            };
            curr_row[j] = (prev_row[j] + 1)
                .min(curr_row[j - 1] + 1)
                .min(prev_row[j - 1] + cost);
        }
        std::mem::swap(&mut prev_row, &mut curr_row);
    }

    prev_row[b_len]
}

/// Check if a substring at `pos` with `len` is on word boundaries.
fn is_word_boundary(text: &str, pos: usize, len: usize) -> bool {
    let bytes = text.as_bytes();
    let before_ok = pos == 0 || !bytes[pos - 1].is_ascii_alphanumeric();
    let after_pos = pos + len;
    let after_ok = after_pos >= bytes.len() || !bytes[after_pos].is_ascii_alphanumeric();
    before_ok && after_ok
}

/// Extract capitalized words that might be names.
/// Returns (word, start_byte, end_byte).
/// Wire point: used by extract_contacts() which is stubbed (Theater AGI guard).
#[allow(dead_code)]
fn extract_capitalized_words(input: &str) -> Vec<(String, usize, usize)> {
    let mut results = Vec::new();
    let mut offset = 0;

    for (i, word) in input.split_whitespace().enumerate() {
        // Find the actual byte offset.
        if let Some(pos) = input[offset..].find(word) {
            let start = offset + pos;
            let end = start + word.len();

            // Capitalized, not first word, length > 1.
            if i > 0 && word.len() > 1 {
                if let Some(first) = word.chars().next() {
                    if first.is_uppercase() {
                        let clean: String =
                            word.chars().take_while(|c| c.is_alphanumeric()).collect();
                        if clean.len() > 1 {
                            results.push((clean, start, end));
                        }
                    }
                }
            }

            offset = end;
        }
    }

    results
}

/// Normalize a clock time string to "HH:MM" format.
fn normalize_clock_time(hour_str: &str, _has_minutes: bool, lower: &str, end: usize) -> String {
    let hour: u32 = hour_str.parse().unwrap_or(0);

    // Find am/pm after the number.
    let rest = &lower[end..];
    let is_pm = rest.trim_start().starts_with("pm");

    let hour_24 = if is_pm && hour < 12 {
        hour + 12
    } else if !is_pm && hour == 12 {
        0
    } else {
        hour
    };

    format!("{:02}:00", hour_24)
}

/// Get the byte offset of the i-th word in input.
fn byte_offset_of_word(input: &str, words: &[&str], index: usize) -> usize {
    let mut offset = 0;
    for (i, word) in words.iter().enumerate() {
        if let Some(pos) = input[offset..].find(word) {
            if i == index {
                return offset + pos;
            }
            offset += pos + word.len();
        }
    }
    0
}

/// Get the byte offset of the end of the i-th word.
fn byte_offset_of_word_end(input: &str, words: &[&str], index: usize) -> usize {
    let start = byte_offset_of_word(input, words, index);
    start + words.get(index).map_or(0, |w| w.len())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn extractor() -> EntityExtractor {
        EntityExtractor::new(
            vec![
                "John Smith".to_string(),
                "Alice".to_string(),
                "Bob Johnson".to_string(),
            ],
            vec![
                "WhatsApp".to_string(),
                "Chrome".to_string(),
                "Spotify".to_string(),
                "Camera".to_string(),
            ],
        )
    }

    #[test]
    fn test_extract_time_3pm() {
        let ext = EntityExtractor::empty();
        let entities = ext.extract("meet me at 3pm");
        let times: Vec<_> = entities
            .iter()
            .filter(|e| e.entity_type == EntityType::Time)
            .collect();
        assert!(!times.is_empty(), "should extract time from '3pm'");
        assert!(times[0].raw.contains("3pm") || times[0].raw.contains("3 pm"));
    }

    #[test]
    fn test_extract_time_noon() {
        let ext = EntityExtractor::empty();
        let entities = ext.extract("lunch at noon tomorrow");
        let times: Vec<_> = entities
            .iter()
            .filter(|e| e.entity_type == EntityType::Time)
            .collect();
        assert!(times.iter().any(|t| t.value == "12:00"));
        assert!(times.iter().any(|t| t.value == "tomorrow"));
    }

    #[test]
    fn test_extract_duration_5_minutes() {
        let ext = EntityExtractor::empty();
        let entities = ext.extract("set timer for 5 minutes");
        let durations: Vec<_> = entities
            .iter()
            .filter(|e| e.entity_type == EntityType::Duration)
            .collect();
        assert!(!durations.is_empty());
        assert_eq!(durations[0].value, "5m");
    }

    #[test]
    fn test_extract_duration_half_hour() {
        let ext = EntityExtractor::empty();
        let entities = ext.extract("remind me in half an hour");
        let durations: Vec<_> = entities
            .iter()
            .filter(|e| e.entity_type == EntityType::Duration)
            .collect();
        assert!(!durations.is_empty());
        assert_eq!(durations[0].value, "30m");
    }

    #[test]
    fn test_extract_contact_exact() {
        // Architecture note: extract_contacts() is stubbed (Theater AGI guard).
        // Contact resolution is deferred to the LLM.
        let ext = extractor();
        let entities = ext.extract("call Alice right now");
        let contacts: Vec<_> = entities
            .iter()
            .filter(|e| e.entity_type == EntityType::Contact)
            .collect();
        assert!(
            contacts.is_empty(),
            "contact extraction is stubbed — LLM handles this"
        );
    }

    #[test]
    fn test_extract_app_name() {
        // Architecture note: extract_apps() is stubbed (Theater AGI guard).
        // App resolution is deferred to the LLM.
        let ext = extractor();
        let entities = ext.extract("open whatsapp please");
        let apps: Vec<_> = entities
            .iter()
            .filter(|e| e.entity_type == EntityType::App)
            .collect();
        assert!(
            apps.is_empty(),
            "app extraction is stubbed — LLM handles this"
        );
    }

    #[test]
    fn test_extract_url() {
        let ext = EntityExtractor::empty();
        let entities = ext.extract("visit https://example.com/page for info");
        let urls: Vec<_> = entities
            .iter()
            .filter(|e| e.entity_type == EntityType::Url)
            .collect();
        assert!(!urls.is_empty());
        assert!(urls[0].value.contains("example.com"));
    }

    #[test]
    fn test_extract_setting() {
        let ext = EntityExtractor::empty();
        let entities = ext.extract("turn on bluetooth");
        let settings: Vec<_> = entities
            .iter()
            .filter(|e| e.entity_type == EntityType::Setting)
            .collect();
        assert!(!settings.is_empty());
        assert_eq!(settings[0].value, "bluetooth");
    }

    #[test]
    fn test_levenshtein_exact() {
        assert_eq!(levenshtein("hello", "hello"), 0);
    }

    #[test]
    fn test_levenshtein_one_edit() {
        assert_eq!(levenshtein("hello", "helo"), 1);
        assert_eq!(levenshtein("cat", "hat"), 1);
    }

    #[test]
    fn test_levenshtein_two_edits() {
        assert_eq!(levenshtein("kitten", "sittin"), 2);
    }

    #[test]
    fn test_parse_number_word() {
        assert_eq!(parse_number_word("five"), Some(5));
        assert_eq!(parse_number_word("12"), Some(12));
        assert_eq!(parse_number_word("twenty"), Some(20));
        assert_eq!(parse_number_word("xyz"), None);
    }

    #[test]
    fn test_empty_input() {
        let ext = EntityExtractor::empty();
        let entities = ext.extract("");
        assert!(entities.is_empty());
    }

    #[test]
    fn test_no_panic_on_garbage() {
        let ext = EntityExtractor::empty();
        let entities = ext.extract("asdf 1234 !@#$ 🎉 ñ漢字");
        // Should not panic, may extract the number.
        let _ = entities;
    }

    #[test]
    fn test_multiple_entities() {
        let ext = extractor();
        let entities = ext.extract("send Hello to Alice on WhatsApp at 3pm");
        // Architecture note: contact/app extraction was removed (Theater AGI guard —
        // Iron Law #4). Only structural entities (time, duration, numbers, settings)
        // are extracted by Rust. The LLM resolves names from context.
        // This input yields at least the "3pm" time entity.
        assert!(
            entities.len() >= 1,
            "should extract at least the time entity, got {}",
            entities.len()
        );
        assert!(
            entities.iter().any(|e| e.entity_type == EntityType::Time),
            "should extract time entity from '3pm'"
        );
    }
}
