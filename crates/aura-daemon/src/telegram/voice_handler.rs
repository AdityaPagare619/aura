//! Smart Hybrid Communication Mode for Telegram.
//!
//! Provides intelligent communication mode selection - not just voice or just chat,
//! but smart selection based on user intent, context, and preferences.
//!
//! # Mode Detection Algorithm (O(1))
//!
//! 1. Check for explicit /voice or /chat command → explicit wins
//! 2. If message contains voice file → Voice mode
//! 3. Check user profile preference
//! 4. Default to Text mode (most reliable)
//!
//! # Voice Response Decision
//!
//! - IF user.voice_mode == Always → speak (except technical)
//! - IF user.voice_mode == Smart → speak if (short && !technical && last_was_voice)
//! - IF user.voice_mode == Never → never speak

use serde::{Deserialize, Serialize};

const SHORT_RESPONSE_THRESHOLD: usize = 50;

const TECHNICAL_PATTERNS: &[&str] = &[
    "```",
    "```rust",
    "```python",
    "```javascript",
    "```typescript",
    "```go",
    "```java",
    "```c",
    "```cpp",
    "`",
    "fn ",
    "func ",
    "def ",
    "class ",
    "impl ",
    "struct ",
    "enum ",
    "pub fn",
    "pub async fn",
    "async fn",
    "-> Result",
    "-> impl",
    "::new()",
    "::from(",
    "std::",
    "use ",
    "mod ",
    "trait ",
    "impl Trait",
    "where ",
    "unsafe ",
    "const ",
    "static ",
    "match ",
    "if let ",
    "while let ",
    "for ",
    "iterator",
    " lifetimes",
    " generics",
    "<T>",
    "&str",
    "&mut ",
    "&[",
    "Box<",
    "Rc<",
    "Arc<",
    "Cell<",
    "RefCell<",
    "Mutex<",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum VoiceModePreference {
    Always,
    Smart,
    Never,
}

impl Default for VoiceModePreference {
    fn default() -> Self {
        Self::Smart
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CommunicationMode {
    Voice,
    Text,
}

impl Default for CommunicationMode {
    fn default() -> Self {
        Self::Text
    }
}

#[derive(Debug, Clone)]
pub struct CommunicationContext {
    pub chat_id: i64,
    pub user_preference: VoiceModePreference,
    pub last_response_was_voice: bool,
    pub has_voice_input: bool,
    pub explicit_mode: Option<CommunicationMode>,
}

impl CommunicationContext {
    pub fn new(chat_id: i64) -> Self {
        Self {
            chat_id,
            user_preference: VoiceModePreference::default(),
            last_response_was_voice: false,
            has_voice_input: false,
            explicit_mode: None,
        }
    }

    pub fn with_preference(mut self, preference: VoiceModePreference) -> Self {
        self.user_preference = preference;
        self
    }

    pub fn with_voice_input(mut self, has_voice: bool) -> Self {
        self.has_voice_input = has_voice;
        self
    }

    pub fn with_explicit_mode(mut self, mode: CommunicationMode) -> Self {
        self.explicit_mode = Some(mode);
        self
    }

    pub fn with_last_voice_status(mut self, was_voice: bool) -> Self {
        self.last_response_was_voice = was_voice;
        self
    }
}

pub struct VoiceHandler;

impl VoiceHandler {
    pub fn detect_communication_mode(context: &CommunicationContext) -> CommunicationMode {
        if let Some(explicit) = context.explicit_mode {
            return explicit;
        }

        if context.has_voice_input {
            return CommunicationMode::Voice;
        }

        CommunicationMode::Text
    }

    pub fn should_speak(response_text: &str, context: &CommunicationContext) -> bool {
        if context.user_preference == VoiceModePreference::Never {
            return false;
        }

        if Self::is_technical_content(response_text) {
            return false;
        }

        match context.user_preference {
            VoiceModePreference::Always => true,
            VoiceModePreference::Smart => {
                let is_short = response_text.split_whitespace().count() < SHORT_RESPONSE_THRESHOLD;
                is_short && context.last_response_was_voice
            }
            VoiceModePreference::Never => false,
        }
    }

    fn is_technical_content(text: &str) -> bool {
        let lower = text.to_lowercase();
        TECHNICAL_PATTERNS
            .iter()
            .any(|pattern| lower.contains(*pattern))
    }

    pub fn word_count(text: &str) -> usize {
        text.split_whitespace().count()
    }

    pub fn is_short_response(text: &str) -> bool {
        Self::word_count(text) < SHORT_RESPONSE_THRESHOLD
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_explicit_mode_wins() {
        let ctx = CommunicationContext::new(123)
            .with_explicit_mode(CommunicationMode::Voice)
            .with_preference(VoiceModePreference::Never);

        let mode = VoiceHandler::detect_communication_mode(&ctx);
        assert_eq!(mode, CommunicationMode::Voice);
    }

    #[test]
    fn test_voice_input_forces_voice_mode() {
        let ctx = CommunicationContext::new(123)
            .with_voice_input(true)
            .with_preference(VoiceModePreference::Never);

        let mode = VoiceHandler::detect_communication_mode(&ctx);
        assert_eq!(mode, CommunicationMode::Voice);
    }

    #[test]
    fn test_default_is_text() {
        let ctx = CommunicationContext::new(123);
        let mode = VoiceHandler::detect_communication_mode(&ctx);
        assert_eq!(mode, CommunicationMode::Text);
    }

    #[test]
    fn test_always_voice_preference() {
        let response = "Hello there!";
        let ctx = CommunicationContext::new(123).with_preference(VoiceModePreference::Always);

        assert!(VoiceHandler::should_speak(response, &ctx));
    }

    #[test]
    fn test_never_voice_preference() {
        let response = "Hello there!";
        let ctx = CommunicationContext::new(123).with_preference(VoiceModePreference::Never);

        assert!(!VoiceHandler::should_speak(response, &ctx));
    }

    #[test]
    fn test_smart_voice_short_with_voice_history() {
        let response = "Hi!";
        let ctx = CommunicationContext::new(123)
            .with_preference(VoiceModePreference::Smart)
            .with_last_voice_status(true);

        assert!(VoiceHandler::should_speak(response, &ctx));
    }

    #[test]
    fn test_smart_voice_long_never_speaks() {
        let response = "This is a much longer response that contains many more words and should definitely not be spoken by text to speech because it is too long and would take forever to listen to.";
        let ctx = CommunicationContext::new(123)
            .with_preference(VoiceModePreference::Smart)
            .with_last_voice_status(true);

        assert!(!VoiceHandler::should_speak(response, &ctx));
    }

    #[test]
    fn test_smart_voice_without_voice_history() {
        let response = "Hello!";
        let ctx = CommunicationContext::new(123)
            .with_preference(VoiceModePreference::Smart)
            .with_last_voice_status(false);

        assert!(!VoiceHandler::should_speak(response, &ctx));
    }

    #[test]
    fn test_technical_content_never_speaks() {
        let response = "Here is the Rust code: ```rust fn main() { println!(\"Hello\"); } ```";
        let ctx = CommunicationContext::new(123).with_preference(VoiceModePreference::Always);

        assert!(!VoiceHandler::should_speak(response, &ctx));
    }

    #[test]
    fn test_code_inline_blocks() {
        let response = "Use `let x = 5;` to declare a variable.";
        let ctx = CommunicationContext::new(123).with_preference(VoiceModePreference::Always);

        assert!(!VoiceHandler::should_speak(response, &ctx));
    }

    #[test]
    fn test_rust_syntax_detection() {
        let response = "You need to implement the trait.";
        let ctx = CommunicationContext::new(123).with_preference(VoiceModePreference::Always);

        assert!(VoiceHandler::should_speak(response, &ctx));
    }

    #[test]
    fn test_word_count() {
        assert_eq!(VoiceHandler::word_count("Hello world"), 2);
        assert_eq!(VoiceHandler::word_count(""), 0);
        assert_eq!(VoiceHandler::word_count("  multiple   spaces  "), 2);
    }

    #[test]
    fn test_short_response_detection() {
        assert!(VoiceHandler::is_short_response("Hi!"));
        assert!(VoiceHandler::is_short_response("Hello, how are you?"));
        assert!(VoiceHandler::is_short_response(
            "This is a very long response with many many many many many words"
        ));
    }

    #[test]
    fn test_always_preference_ignores_length() {
        let short = "OK";
        let long = "This is a very long response with many many many many many words and should technically not be spoken according to the smart rules but always mode should override that";

        let ctx_short = CommunicationContext::new(1).with_preference(VoiceModePreference::Always);
        let ctx_long = CommunicationContext::new(1).with_preference(VoiceModePreference::Always);

        assert!(VoiceHandler::should_speak(short, &ctx_short));
        assert!(VoiceHandler::should_speak(long, &ctx_long));
    }

    #[test]
    fn test_context_builder() {
        let ctx = CommunicationContext::new(42)
            .with_preference(VoiceModePreference::Always)
            .with_voice_input(true)
            .with_explicit_mode(CommunicationMode::Text)
            .with_last_voice_status(true);

        assert_eq!(ctx.chat_id, 42);
        assert_eq!(ctx.user_preference, VoiceModePreference::Always);
        assert!(ctx.has_voice_input);
        assert_eq!(ctx.explicit_mode, Some(CommunicationMode::Text));
        assert!(ctx.last_response_was_voice);
    }

    #[test]
    fn test_default_voice_mode_preference() {
        let pref: VoiceModePreference = VoiceModePreference::default();
        assert_eq!(pref, VoiceModePreference::Smart);
    }

    #[test]
    fn test_default_communication_mode() {
        let mode: CommunicationMode = CommunicationMode::default();
        assert_eq!(mode, CommunicationMode::Text);
    }
}
