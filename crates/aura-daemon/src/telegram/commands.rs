//! Telegram command definitions and parser.
//!
//! 43 commands across 7 categories plus meta commands.
//! Each command knows its required permission level, enabling the security
//! gate to enforce access control before handler dispatch.

use crate::telegram::security::PermissionLevel;

// ─── PIN action ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PinAction {
    /// Set a new PIN: `/pin set <value>`.
    Set(String),
    /// Clear the PIN: `/pin clear`.
    Clear,
    /// Show PIN status: `/pin status`.
    Status,
}

// ─── Command enum ───────────────────────────────────────────────────────────

/// All 43 Telegram commands understood by AURA.
#[derive(Debug, Clone)]
pub enum TelegramCommand {
    // ── Control (3) ──────────────────────────────────────────────────────
    /// `/start` — Start AURA daemon (control interface).
    Start,
    /// `/stop` — Stop AURA daemon (control interface).
    Stop,
    /// `/reboot` — Restart AURA daemon (control interface).
    Reboot,

    // ── System (7) ──────────────────────────────────────────────────────
    /// `/status` — Show system dashboard.
    Status,
    /// `/health` — Quick health check.
    Health,
    /// `/restart` — Restart the daemon.
    Restart,
    /// `/logs [service] [lines]` — Show recent logs.
    Logs {
        service: Option<String>,
        lines: usize,
    },
    /// `/uptime` — Show daemon uptime.
    Uptime,
    /// `/version` — Show build version.
    Version,
    /// `/power` — Show power/battery status.
    Power,

    // ── AI (6) ──────────────────────────────────────────────────────────
    /// `/ask <question>` — Ask AURA a question.
    Ask { question: String },
    /// `/think <problem>` — Deep reasoning on a problem.
    Think { problem: String },
    /// `/plan <goal>` — Generate a plan for a goal.
    Plan { goal: String },
    /// `/explain <topic>` — Explain a topic.
    Explain { topic: String },
    /// `/summarize <text>` — Summarize text.
    Summarize { text: String },
    /// `/translate <text> <lang>` — Translate text.
    Translate { text: String, target_lang: String },

    // ── Memory (6) ──────────────────────────────────────────────────────
    /// `/remember <text>` — Store a memory.
    Remember { text: String },
    /// `/recall <query>` — Search memories.
    Recall { query: String },
    /// `/forget <query>` — Delete matching memories.
    Forget { query: String },
    /// `/memories [filter]` — List memories.
    Memories { filter: Option<String> },
    /// `/consolidate` — Trigger memory consolidation.
    Consolidate,
    /// `/memorystats` — Show memory statistics.
    MemoryStats,

    // ── Agency (8) ──────────────────────────────────────────────────────
    /// `/do <instruction>` — Execute an instruction.
    Do { instruction: String },
    /// `/open <app>` — Open an application.
    Open { app: String },
    /// `/send <app> <contact> <message>` — Send a message via an app.
    Send {
        app: String,
        contact: String,
        message: String,
    },
    /// `/call <contact>` — Make a phone call.
    Call { contact: String },
    /// `/schedule <event> <time>` — Schedule an event.
    Schedule { event: String, time: String },
    /// `/screenshot` — Capture the screen.
    Screenshot,
    /// `/navigate <destination>` — Navigate somewhere.
    Navigate { destination: String },
    /// `/automate <routine>` — Run an automation routine.
    Automate { routine: String },

    // ── Config (10) ──────────────────────────────────────────────────────
    /// `/set <key> <value>` — Set a config value.
    Set { key: String, value: String },
    /// `/get <key>` — Get a config value.
    Get { key: String },
    /// `/personality` — Show personality traits.
    Personality,
    /// `/personality_set <trait> <value>` — Set a personality trait.
    PersonalitySet { trait_name: String, value: f32 },
    /// `/trust` — Show trust level.
    Trust,
    /// `/trust_set <level>` — Set trust level.
    TrustSet { level: f32 },
    /// `/voice` — Set voice mode preference.
    Voice,
    /// `/chat` — Set chat mode preference.
    Chat,
    /// `/quiet` — Disable all proactive suggestions.
    Quiet,
    /// `/wake` — Enable proactive suggestions.
    Wake,

    // ── Security (5) ────────────────────────────────────────────────────
    /// `/pin <action>` — Manage PIN.
    Pin { action: PinAction },
    /// `/lock` — Lock the bot.
    Lock,
    /// `/unlock <pin>` — Unlock the bot.
    Unlock { pin: String },
    /// `/audit [lines]` — Show audit log.
    Audit { lines: usize },
    /// `/permissions` — Show permission table.
    Permissions,

    // ── Debug (5) ───────────────────────────────────────────────────────
    /// `/trace <module>` — Enable tracing for a module.
    Trace { module: String },
    /// `/dump <component>` — Dump component state.
    Dump { component: String },
    /// `/perf` — Show performance metrics.
    Perf,
    /// `/etg [app]` — Show element tree graph.
    Etg { app: Option<String> },
    /// `/goals` — Show active goals.
    Goals,

    // ── Meta ────────────────────────────────────────────────────────────
    /// `/help [command]` — Show help.
    Help { command: Option<String> },
    /// Unrecognized command.
    Unknown { raw: String },
}

impl TelegramCommand {
    /// Parse a Telegram message text into a command.
    ///
    /// Expects `/command arg1 arg2 ...` format.
    /// Supports single-character aliases for common commands.
    pub fn parse(text: &str) -> Self {
        let text = text.trim();
        if !text.starts_with('/') {
            // Treat bare text as an `/ask` shortcut.
            return if text.is_empty() {
                Self::Unknown { raw: String::new() }
            } else {
                Self::Ask {
                    question: text.to_string(),
                }
            };
        }

        // Split into command and arguments.
        // Handle bot suffix: /command@botname -> /command
        let parts: Vec<&str> = text.splitn(2, ' ').collect();
        let cmd_part = parts[0]
            .split('@')
            .next()
            .unwrap_or(parts[0])
            .to_lowercase();
        let args_str = parts.get(1).unwrap_or(&"").trim();

        match cmd_part.as_str() {
            // -- Control --
            "/start" => Self::Start,
            "/stop" => Self::Stop,
            "/reboot" => Self::Reboot,

            // -- Aliases --
            "/s" | "/status" => Self::Status,
            "/h" | "/health" => Self::Health,
            "/restart" => Self::Restart,
            "/logs" | "/log" => {
                let args: Vec<&str> = args_str.splitn(2, ' ').collect();
                let service = if args[0].is_empty() {
                    None
                } else {
                    Some(args[0].to_string())
                };
                let lines = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(20);
                Self::Logs { service, lines }
            }
            "/uptime" | "/up" => Self::Uptime,
            "/version" | "/v" => Self::Version,
            "/power" | "/battery" | "/bat" => Self::Power,

            // AI
            "/ask" | "/a" | "/?" => Self::Ask {
                question: args_str.to_string(),
            },
            "/think" | "/t" => Self::Think {
                problem: args_str.to_string(),
            },
            "/plan" | "/p" => Self::Plan {
                goal: args_str.to_string(),
            },
            "/explain" | "/e" => Self::Explain {
                topic: args_str.to_string(),
            },
            "/summarize" | "/sum" => Self::Summarize {
                text: args_str.to_string(),
            },
            "/translate" | "/tr" => {
                // Last word is target language, rest is text.
                let words: Vec<&str> = args_str.rsplitn(2, ' ').collect();
                if words.len() == 2 {
                    Self::Translate {
                        text: words[1].to_string(),
                        target_lang: words[0].to_string(),
                    }
                } else {
                    Self::Translate {
                        text: args_str.to_string(),
                        target_lang: "en".to_string(),
                    }
                }
            }

            // Memory
            "/remember" | "/rem" => Self::Remember {
                text: args_str.to_string(),
            },
            "/recall" | "/rec" => Self::Recall {
                query: args_str.to_string(),
            },
            "/forget" => Self::Forget {
                query: args_str.to_string(),
            },
            "/memories" | "/mem" => Self::Memories {
                filter: if args_str.is_empty() {
                    None
                } else {
                    Some(args_str.to_string())
                },
            },
            "/consolidate" => Self::Consolidate,
            "/memorystats" | "/memstats" => Self::MemoryStats,

            // Agency
            "/do" | "/d" => Self::Do {
                instruction: args_str.to_string(),
            },
            "/open" | "/o" => Self::Open {
                app: args_str.to_string(),
            },
            "/send" => {
                // /send <app> <contact> <message>
                let parts: Vec<&str> = args_str.splitn(3, ' ').collect();
                if parts.len() >= 3 {
                    Self::Send {
                        app: parts[0].to_string(),
                        contact: parts[1].to_string(),
                        message: parts[2].to_string(),
                    }
                } else {
                    Self::Unknown {
                        raw: text.to_string(),
                    }
                }
            }
            "/call" => Self::Call {
                contact: args_str.to_string(),
            },
            "/schedule" | "/sched" => {
                // /schedule <event> at <time>  or  /schedule <event> <time>
                if let Some(idx) = args_str.find(" at ") {
                    Self::Schedule {
                        event: args_str[..idx].to_string(),
                        time: args_str[idx + 4..].to_string(),
                    }
                } else {
                    let parts: Vec<&str> = args_str.rsplitn(2, ' ').collect();
                    if parts.len() == 2 {
                        Self::Schedule {
                            event: parts[1].to_string(),
                            time: parts[0].to_string(),
                        }
                    } else {
                        Self::Unknown {
                            raw: text.to_string(),
                        }
                    }
                }
            }
            "/screenshot" | "/ss" => Self::Screenshot,
            "/navigate" | "/nav" => Self::Navigate {
                destination: args_str.to_string(),
            },
            "/automate" | "/auto" => Self::Automate {
                routine: args_str.to_string(),
            },

            // Config
            "/set" => {
                let parts: Vec<&str> = args_str.splitn(2, ' ').collect();
                if parts.len() == 2 {
                    Self::Set {
                        key: parts[0].to_string(),
                        value: parts[1].to_string(),
                    }
                } else {
                    Self::Unknown {
                        raw: text.to_string(),
                    }
                }
            }
            "/get" => Self::Get {
                key: args_str.to_string(),
            },
            "/personality" => {
                if args_str.is_empty() {
                    Self::Personality
                } else {
                    let parts: Vec<&str> = args_str.splitn(2, ' ').collect();
                    if parts.len() == 2 {
                        if let Ok(val) = parts[1].parse::<f32>() {
                            Self::PersonalitySet {
                                trait_name: parts[0].to_string(),
                                value: val,
                            }
                        } else {
                            Self::Unknown {
                                raw: text.to_string(),
                            }
                        }
                    } else {
                        Self::Personality
                    }
                }
            }
            "/trust" => {
                if args_str.is_empty() {
                    Self::Trust
                } else {
                    if let Ok(level) = args_str.parse::<f32>() {
                        Self::TrustSet { level }
                    } else {
                        Self::Unknown {
                            raw: text.to_string(),
                        }
                    }
                }
            }
            "/voice" => Self::Voice,
            "/chat" => Self::Chat,
            "/quiet" => Self::Quiet,
            "/wake" => Self::Wake,

            // Security
            "/pin" => {
                let sub: Vec<&str> = args_str.splitn(2, ' ').collect();
                match sub.first().copied() {
                    Some("set") => Self::Pin {
                        action: PinAction::Set(sub.get(1).unwrap_or(&"").to_string()),
                    },
                    Some("clear") => Self::Pin {
                        action: PinAction::Clear,
                    },
                    Some("status") | None => Self::Pin {
                        action: PinAction::Status,
                    },
                    _ => Self::Pin {
                        action: PinAction::Status,
                    },
                }
            }
            "/lock" => Self::Lock,
            "/unlock" => Self::Unlock {
                pin: args_str.to_string(),
            },
            "/audit" => Self::Audit {
                lines: args_str.parse().unwrap_or(20),
            },
            "/permissions" | "/perms" => Self::Permissions,

            // Debug
            "/trace" => Self::Trace {
                module: args_str.to_string(),
            },
            "/dump" => Self::Dump {
                component: args_str.to_string(),
            },
            "/perf" => Self::Perf,
            "/etg" => Self::Etg {
                app: if args_str.is_empty() {
                    None
                } else {
                    Some(args_str.to_string())
                },
            },
            "/goals" | "/g" => Self::Goals,

            // Meta
            "/help" => Self::Help {
                command: if args_str.is_empty() {
                    None
                } else {
                    Some(args_str.to_string())
                },
            },

            _ => Self::Unknown {
                raw: text.to_string(),
            },
        }
    }

    /// The minimum permission level required to execute this command.
    pub fn required_permission(&self) -> PermissionLevel {
        match self {
            // ReadOnly — safe, no state changes.
            Self::Status
            | Self::Health
            | Self::Help { .. }
            | Self::Uptime
            | Self::Version
            | Self::Power
            | Self::Perf
            | Self::Goals => PermissionLevel::ReadOnly,

            // Query — reads data, no side effects.
            Self::Ask { .. }
            | Self::Think { .. }
            | Self::Explain { .. }
            | Self::Summarize { .. }
            | Self::Translate { .. }
            | Self::Recall { .. }
            | Self::Memories { .. }
            | Self::MemoryStats
            | Self::Get { .. }
            | Self::Personality
            | Self::Trust
            | Self::Audit { .. }
            | Self::Permissions
            | Self::Trace { .. }
            | Self::Dump { .. }
            | Self::Etg { .. }
            | Self::Quiet
            | Self::Wake => PermissionLevel::Query,

            // Action — triggers external effects.
            Self::Do { .. }
            | Self::Open { .. }
            | Self::Send { .. }
            | Self::Call { .. }
            | Self::Schedule { .. }
            | Self::Screenshot
            | Self::Navigate { .. }
            | Self::Automate { .. }
            | Self::Plan { .. }
            | Self::Remember { .. }
            | Self::Logs { .. } => PermissionLevel::Action,

            // Modify — changes AURA's internal state.
            Self::Forget { .. }
            | Self::Consolidate
            | Self::PersonalitySet { .. }
            | Self::TrustSet { .. }
            | Self::Set { .. }
            | Self::Voice
            | Self::Chat => PermissionLevel::Modify,

            // Admin — security and lifecycle operations.
            Self::Start
            | Self::Stop
            | Self::Reboot
            | Self::Restart
            | Self::Pin { .. }
            | Self::Lock
            | Self::Unlock { .. } => PermissionLevel::Admin,

            Self::Unknown { .. } => PermissionLevel::ReadOnly,
        }
    }

    /// Whether this command is the `/unlock` command (needed by security gate).
    pub fn is_unlock(&self) -> bool {
        matches!(self, Self::Unlock { .. })
    }

    /// Return a short summary for audit logging (with sensitive data redacted).
    pub fn audit_summary(&self) -> String {
        match self {
            Self::Pin { action } => match action {
                PinAction::Set(_) => "/pin set ***".to_string(),
                PinAction::Clear => "/pin clear".to_string(),
                PinAction::Status => "/pin status".to_string(),
            },
            Self::Unlock { .. } => "/unlock ***".to_string(),
            Self::Ask { question } => format!("/ask {}", truncate(question, 50)),
            Self::Do { instruction } => format!("/do {}", truncate(instruction, 50)),
            Self::Send { app, contact, .. } => format!("/send {app} {contact} ..."),
            Self::Remember { text } => format!("/remember {}", truncate(text, 50)),
            _ => format!("{:?}", self).chars().take(80).collect(),
        }
    }

    /// Command category name for grouping in help.
    pub fn category(&self) -> &'static str {
        match self {
            // ── Control ─────────────────────────────────────────────────
            Self::Start | Self::Stop | Self::Reboot => "Control",

            // ── System ─────────────────────────────────────────────────
            Self::Status
            | Self::Health
            | Self::Restart
            | Self::Logs { .. }
            | Self::Uptime
            | Self::Version
            | Self::Power => "System",

            Self::Ask { .. }
            | Self::Think { .. }
            | Self::Plan { .. }
            | Self::Explain { .. }
            | Self::Summarize { .. }
            | Self::Translate { .. } => "AI",

            Self::Remember { .. }
            | Self::Recall { .. }
            | Self::Forget { .. }
            | Self::Memories { .. }
            | Self::Consolidate
            | Self::MemoryStats => "Memory",

            Self::Do { .. }
            | Self::Open { .. }
            | Self::Send { .. }
            | Self::Call { .. }
            | Self::Schedule { .. }
            | Self::Screenshot
            | Self::Navigate { .. }
            | Self::Automate { .. } => "Agency",

            Self::Set { .. }
            | Self::Get { .. }
            | Self::Personality
            | Self::PersonalitySet { .. }
            | Self::Trust
            | Self::TrustSet { .. }
            | Self::Voice
            | Self::Chat
            | Self::Quiet
            | Self::Wake => "Config",

            Self::Pin { .. }
            | Self::Lock
            | Self::Unlock { .. }
            | Self::Audit { .. }
            | Self::Permissions => "Security",

            Self::Trace { .. }
            | Self::Dump { .. }
            | Self::Perf
            | Self::Etg { .. }
            | Self::Goals => "Debug",

            Self::Help { .. } | Self::Unknown { .. } => "Meta",
        }
    }
}

/// Truncate a string to `max_len` bytes, appending `...` if truncated.
/// Ensures the cut happens at a valid UTF-8 character boundary.
fn truncate(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        let mut end = max_len;
        while end > 0 && !s.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}...", &s[..end])
    }
}

// ─── Help text ──────────────────────────────────────────────────────────────

/// Generate the full help message listing all commands.
pub fn full_help_text() -> String {
    "\
<b>AURA Telegram Commands</b>

<b>System</b>
/status (/s) — Dashboard
/health (/h) — Quick health check
/restart — Restart daemon
/logs [service] [n] — Recent logs
/uptime (/up) — Uptime
/version (/v) — Build version
/power (/bat) — Power status

<b>AI</b>
/ask (/a /?) <question> — Ask AURA
/think (/t) <problem> — Deep reasoning
/plan (/p) <goal> — Generate plan
/explain (/e) <topic> — Explain
/summarize (/sum) <text> — Summarize
/translate (/tr) <text> <lang> — Translate

<b>Memory</b>
/remember (/rem) <text> — Store memory
/recall (/rec) <query> — Search memories
/forget <query> — Delete memories
/memories (/mem) [filter] — List memories
/consolidate — Trigger consolidation
/memorystats — Memory statistics

<b>Agency</b>
/do (/d) <instruction> — Execute
/open (/o) <app> — Open app
/send <app> <contact> <msg> — Send message
/call <contact> — Phone call
/schedule <event> at <time> — Schedule
/screenshot (/ss) — Capture screen
/navigate (/nav) <dest> — Navigate
/automate (/auto) <routine> — Automation

<b>Config</b>
/set <key> <value> — Set config
/get <key> — Get config
/personality [trait value] — Personality
/trust [level] — Trust level
/voice — Voice mode
/chat — Chat mode
/quiet — Disable proactive suggestions
/wake — Enable proactive suggestions

<b>Security</b>
/pin <set|clear|status> — PIN management
/lock — Lock bot
/unlock <pin> — Unlock bot
/audit [n] — Audit log
/permissions — Permission table

<b>Debug</b>
/trace <module> — Enable tracing
/dump <component> — Dump state
/perf — Performance metrics
/etg [app] — Element tree graph
/goals (/g) — Active goals"
        .to_string()
}

/// Generate help text for a specific command.
pub fn command_help(name: &str) -> String {
    match name.trim_start_matches('/') {
        "status" | "s" => "<b>/status</b>\nShow the AURA system dashboard with CPU, RAM, battery, model status, active goals, and memory stats.\nAlias: /s".to_string(),
        "ask" | "a" | "?" => "<b>/ask &lt;question&gt;</b>\nAsk AURA a question. Bare text (without /) also works as /ask.\nAliases: /a /?".to_string(),
        "do" | "d" => "<b>/do &lt;instruction&gt;</b>\nExecute an instruction on the device. Requires Action permission.\nAlias: /d".to_string(),
        "pin" => "<b>/pin &lt;set|clear|status&gt;</b>\n/pin set &lt;value&gt; — Set a new PIN\n/pin clear — Remove PIN\n/pin status — Check if PIN is set\nRequires Admin permission.".to_string(),
        "lock" => "<b>/lock</b>\nLock the bot. All commands except /unlock will be rejected.\nRequires a PIN to be set first.".to_string(),
        "unlock" => "<b>/unlock &lt;pin&gt;</b>\nUnlock the bot with the correct PIN.".to_string(),
        _ => format!("No detailed help for '/{name}'. Use /help for the full list."),
    }
}

// ─── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_status() {
        assert!(matches!(
            TelegramCommand::parse("/status"),
            TelegramCommand::Status
        ));
        assert!(matches!(
            TelegramCommand::parse("/s"),
            TelegramCommand::Status
        ));
    }

    #[test]
    fn test_parse_ask() {
        match TelegramCommand::parse("/ask what is the weather") {
            TelegramCommand::Ask { question } => assert_eq!(question, "what is the weather"),
            other => panic!("expected Ask, got {other:?}"),
        }
    }

    #[test]
    fn test_bare_text_is_ask() {
        match TelegramCommand::parse("hello world") {
            TelegramCommand::Ask { question } => assert_eq!(question, "hello world"),
            other => panic!("expected Ask, got {other:?}"),
        }
    }

    #[test]
    fn test_parse_logs_with_args() {
        match TelegramCommand::parse("/logs daemon 50") {
            TelegramCommand::Logs { service, lines } => {
                assert_eq!(service, Some("daemon".to_string()));
                assert_eq!(lines, 50);
            }
            other => panic!("expected Logs, got {other:?}"),
        }
    }

    #[test]
    fn test_parse_send() {
        match TelegramCommand::parse("/send whatsapp John Hello there!") {
            TelegramCommand::Send {
                app,
                contact,
                message,
            } => {
                assert_eq!(app, "whatsapp");
                assert_eq!(contact, "John");
                assert_eq!(message, "Hello there!");
            }
            other => panic!("expected Send, got {other:?}"),
        }
    }

    #[test]
    fn test_parse_pin_set() {
        match TelegramCommand::parse("/pin set 1234") {
            TelegramCommand::Pin {
                action: PinAction::Set(val),
            } => assert_eq!(val, "1234"),
            other => panic!("expected Pin Set, got {other:?}"),
        }
    }

    #[test]
    fn test_parse_unknown() {
        assert!(matches!(
            TelegramCommand::parse("/nonexistent foo"),
            TelegramCommand::Unknown { .. }
        ));
    }

    #[test]
    fn test_parse_schedule_with_at() {
        match TelegramCommand::parse("/schedule team meeting at 3pm") {
            TelegramCommand::Schedule { event, time } => {
                assert_eq!(event, "team meeting");
                assert_eq!(time, "3pm");
            }
            other => panic!("expected Schedule, got {other:?}"),
        }
    }

    #[test]
    fn test_parse_translate() {
        match TelegramCommand::parse("/translate hello world es") {
            TelegramCommand::Translate { text, target_lang } => {
                assert_eq!(text, "hello world");
                assert_eq!(target_lang, "es");
            }
            other => panic!("expected Translate, got {other:?}"),
        }
    }

    #[test]
    fn test_parse_personality_set() {
        match TelegramCommand::parse("/personality warmth 0.8") {
            TelegramCommand::PersonalitySet { trait_name, value } => {
                assert_eq!(trait_name, "warmth");
                assert!((value - 0.8).abs() < f32::EPSILON);
            }
            other => panic!("expected PersonalitySet, got {other:?}"),
        }
    }

    #[test]
    fn test_required_permissions() {
        assert_eq!(
            TelegramCommand::Status.required_permission(),
            PermissionLevel::ReadOnly
        );
        assert_eq!(
            TelegramCommand::Ask {
                question: "".into()
            }
            .required_permission(),
            PermissionLevel::Query
        );
        assert_eq!(
            TelegramCommand::Do {
                instruction: "".into()
            }
            .required_permission(),
            PermissionLevel::Action
        );
        assert_eq!(
            TelegramCommand::Forget { query: "".into() }.required_permission(),
            PermissionLevel::Modify
        );
        assert_eq!(
            TelegramCommand::Restart.required_permission(),
            PermissionLevel::Admin
        );
    }

    #[test]
    fn test_audit_summary_redacts_pin() {
        let cmd = TelegramCommand::Pin {
            action: PinAction::Set("1234".into()),
        };
        assert_eq!(cmd.audit_summary(), "/pin set ***");

        let cmd2 = TelegramCommand::Unlock { pin: "5678".into() };
        assert_eq!(cmd2.audit_summary(), "/unlock ***");
    }

    #[test]
    fn test_bot_suffix_stripped() {
        assert!(matches!(
            TelegramCommand::parse("/status@aura_bot"),
            TelegramCommand::Status
        ));
    }

    #[test]
    fn test_category() {
        assert_eq!(TelegramCommand::Status.category(), "System");
        assert_eq!(
            TelegramCommand::Ask {
                question: "".into()
            }
            .category(),
            "AI"
        );
        assert_eq!(TelegramCommand::Lock.category(), "Security");
    }

    #[test]
    fn test_parse_quiet() {
        assert!(matches!(
            TelegramCommand::parse("/quiet"),
            TelegramCommand::Quiet
        ));
    }

    #[test]
    fn test_parse_wake() {
        assert!(matches!(
            TelegramCommand::parse("/wake"),
            TelegramCommand::Wake
        ));
    }

    #[test]
    fn test_quiet_permission() {
        assert_eq!(
            TelegramCommand::Quiet.required_permission(),
            PermissionLevel::Query
        );
    }

    #[test]
    fn test_wake_permission() {
        assert_eq!(
            TelegramCommand::Wake.required_permission(),
            PermissionLevel::Query
        );
    }
}
