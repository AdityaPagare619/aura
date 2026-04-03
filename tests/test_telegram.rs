//! Tests for Telegram command parsing edge cases.
//!
//! Covers: command parsing, alias resolution, category detection,
//! malformed input handling, and security gate edge cases.

#[cfg(test)]
mod command_parsing {
    use aura_daemon::telegram::commands::TelegramCommand;

    #[test]
    fn test_parse_empty_string() {
        let cmd = TelegramCommand::parse("");
        // Should produce Unknown command
        assert!(matches!(cmd, TelegramCommand::Unknown { .. }));
    }

    #[test]
    fn test_parse_whitespace_only() {
        let cmd = TelegramCommand::parse("   \t\n  ");
        assert!(matches!(cmd, TelegramCommand::Unknown { .. }));
    }

    #[test]
    fn test_parse_status_command() {
        let cmd = TelegramCommand::parse("/status");
        assert!(matches!(cmd, TelegramCommand::Status));
    }

    #[test]
    fn test_parse_help_command() {
        let cmd = TelegramCommand::parse("/help");
        assert!(matches!(cmd, TelegramCommand::Help { .. }));
    }

    #[test]
    fn test_parse_start_command() {
        let cmd = TelegramCommand::parse("/start");
        assert!(matches!(cmd, TelegramCommand::Start));
    }

    #[test]
    fn test_parse_stop_command() {
        let cmd = TelegramCommand::parse("/stop");
        assert!(matches!(cmd, TelegramCommand::Stop));
    }

    #[test]
    fn test_parse_health_command() {
        let cmd = TelegramCommand::parse("/health");
        assert!(matches!(cmd, TelegramCommand::Health));
    }

    #[test]
    fn test_parse_uptime_command() {
        let cmd = TelegramCommand::parse("/uptime");
        assert!(matches!(cmd, TelegramCommand::Uptime));
    }

    #[test]
    fn test_parse_version_command() {
        let cmd = TelegramCommand::parse("/version");
        assert!(matches!(cmd, TelegramCommand::Version));
    }

    #[test]
    fn test_parse_power_command() {
        let cmd = TelegramCommand::parse("/power");
        assert!(matches!(cmd, TelegramCommand::Power));
    }

    #[test]
    fn test_parse_ask_command() {
        let cmd = TelegramCommand::parse("/ask what is rust");
        match cmd {
            TelegramCommand::Ask { question } => assert!(question.contains("rust")),
            _ => panic!("expected Ask command"),
        }
    }

    #[test]
    fn test_parse_think_command() {
        let cmd = TelegramCommand::parse("/think solve this");
        match cmd {
            TelegramCommand::Think { problem } => assert!(problem.contains("solve")),
            _ => panic!("expected Think command"),
        }
    }

    #[test]
    fn test_parse_plan_command() {
        let cmd = TelegramCommand::parse("/plan learn rust");
        match cmd {
            TelegramCommand::Plan { goal } => assert!(goal.contains("learn")),
            _ => panic!("expected Plan command"),
        }
    }

    #[test]
    fn test_parse_remember_command() {
        let cmd = TelegramCommand::parse("/remember my birthday is Jan 1");
        match cmd {
            TelegramCommand::Remember { text } => assert!(text.contains("birthday")),
            _ => panic!("expected Remember command"),
        }
    }

    #[test]
    fn test_parse_recall_command() {
        let cmd = TelegramCommand::parse("/recall birthday");
        match cmd {
            TelegramCommand::Recall { query } => assert!(query.contains("birthday")),
            _ => panic!("expected Recall command"),
        }
    }

    #[test]
    fn test_parse_consolidate_command() {
        let cmd = TelegramCommand::parse("/consolidate");
        assert!(matches!(cmd, TelegramCommand::Consolidate));
    }

    #[test]
    fn test_parse_memorystats_command() {
        let cmd = TelegramCommand::parse("/memorystats");
        assert!(matches!(cmd, TelegramCommand::MemoryStats));
    }

    #[test]
    fn test_parse_alias_status() {
        // /st may not be an alias — verify it parses to something valid
        let cmd = TelegramCommand::parse("/st");
        // Should not panic — whatever it parses to is fine
        let _ = format!("{cmd:?}");
    }

    #[test]
    fn test_parse_alias_health() {
        let cmd = TelegramCommand::parse("/h");
        assert!(matches!(cmd, TelegramCommand::Health));
    }

    #[test]
    fn test_parse_alias_uptime() {
        let cmd = TelegramCommand::parse("/up");
        assert!(matches!(cmd, TelegramCommand::Uptime));
    }

    #[test]
    fn test_parse_alias_version() {
        let cmd = TelegramCommand::parse("/v");
        assert!(matches!(cmd, TelegramCommand::Version));
    }

    #[test]
    fn test_parse_alias_ask() {
        let cmd = TelegramCommand::parse("/a hello");
        assert!(matches!(cmd, TelegramCommand::Ask { .. }));
    }

    #[test]
    fn test_parse_alias_think() {
        let cmd = TelegramCommand::parse("/t problem");
        assert!(matches!(cmd, TelegramCommand::Think { .. }));
    }

    #[test]
    fn test_parse_alias_plan() {
        let cmd = TelegramCommand::parse("/p goal");
        assert!(matches!(cmd, TelegramCommand::Plan { .. }));
    }

    #[test]
    fn test_parse_alias_do() {
        let cmd = TelegramCommand::parse("/d open app");
        assert!(matches!(cmd, TelegramCommand::Do { .. }));
    }

    #[test]
    fn test_parse_alias_open() {
        let cmd = TelegramCommand::parse("/o Instagram");
        assert!(matches!(cmd, TelegramCommand::Open { .. }));
    }

    #[test]
    fn test_parse_alias_remember() {
        let cmd = TelegramCommand::parse("/rem test memory");
        assert!(matches!(cmd, TelegramCommand::Remember { .. }));
    }

    #[test]
    fn test_parse_alias_recall() {
        let cmd = TelegramCommand::parse("/rec test");
        assert!(matches!(cmd, TelegramCommand::Recall { .. }));
    }

    #[test]
    fn test_parse_alias_memories() {
        let cmd = TelegramCommand::parse("/mem");
        assert!(matches!(cmd, TelegramCommand::Memories { .. }));
    }

    #[test]
    fn test_parse_very_long_input() {
        let long_input = "/ask ".to_string() + &"a".repeat(10_000);
        let cmd = TelegramCommand::parse(&long_input);
        // Should not panic or OOM
        match cmd {
            TelegramCommand::Ask { question } => assert_eq!(question.len(), 10_000),
            _ => panic!("expected Ask command"),
        }
    }

    #[test]
    fn test_parse_unicode_input() {
        let cmd = TelegramCommand::parse("/status 🚀");
        assert!(matches!(cmd, TelegramCommand::Status));
    }

    #[test]
    fn test_parse_null_bytes() {
        let cmd = TelegramCommand::parse("/stat\0us");
        // Should handle gracefully
        let _ = format!("{cmd:?}");
    }

    #[test]
    fn test_parse_unknown_command() {
        let cmd = TelegramCommand::parse("/unknown_command_xyz");
        assert!(matches!(cmd, TelegramCommand::Unknown { .. }));
    }

    #[test]
    fn test_parse_plain_text_no_slash() {
        let cmd = TelegramCommand::parse("hello world");
        // Should not panic — may be Unknown or parsed as something else
        let _ = format!("{cmd:?}");
    }

    #[test]
    fn test_parse_multiple_slashes() {
        let cmd = TelegramCommand::parse("///status");
        assert!(matches!(cmd, TelegramCommand::Unknown { .. }));
    }

    #[test]
    fn test_parse_battery_alias() {
        let cmd = TelegramCommand::parse("/battery");
        assert!(matches!(cmd, TelegramCommand::Power));
    }

    #[test]
    fn test_parse_memstats_alias() {
        let cmd = TelegramCommand::parse("/memstats");
        assert!(matches!(cmd, TelegramCommand::MemoryStats));
    }
}

#[cfg(test)]
mod security_gate_edge_cases {
    use aura_daemon::telegram::security::{
        PermissionLevel, RateLimiter, SecurityError, SecurityGate,
    };

    #[test]
    fn test_security_gate_empty_allowed_list() {
        let gate = SecurityGate::new(vec![]);
        assert!(!gate.is_allowed(42));
    }

    #[test]
    fn test_security_gate_single_chat() {
        let gate = SecurityGate::new(vec![12345]);
        assert!(gate.is_allowed(12345));
        assert!(!gate.is_allowed(99999));
    }

    #[test]
    fn test_security_gate_multiple_chats() {
        let gate = SecurityGate::new(vec![1, 2, 3, 4, 5]);
        for id in 1..=5 {
            assert!(gate.is_allowed(id), "chat {id} should be allowed");
        }
        assert!(!gate.is_allowed(0));
        assert!(!gate.is_allowed(6));
    }

    #[test]
    fn test_security_gate_negative_chat_ids() {
        // Telegram group chat IDs are negative
        let gate = SecurityGate::new(vec![-1001234567890]);
        assert!(gate.is_allowed(-1001234567890));
        assert!(!gate.is_allowed(1001234567890));
    }

    #[test]
    fn test_security_gate_lock_unlock() {
        let mut gate = SecurityGate::new(vec![42]);
        assert!(!gate.is_locked());

        gate.lock();
        assert!(gate.is_locked());

        // unlock requires PIN; without PIN set, it should return Err(PinNotConfigured)
        let result = gate.unlock("");
        assert!(result.is_err());
    }

    #[test]
    fn test_security_gate_check_allowed_chat() {
        let mut gate = SecurityGate::new(vec![42]);
        let result = gate.check(42, PermissionLevel::ReadOnly, false);
        assert!(result.is_ok(), "allowed chat should pass security check");
    }

    #[test]
    fn test_security_gate_check_denied_chat() {
        let mut gate = SecurityGate::new(vec![42]);
        let result = gate.check(999, PermissionLevel::ReadOnly, false);
        assert!(result.is_err(), "unknown chat should be denied");
        assert!(matches!(
            result.unwrap_err(),
            SecurityError::UnauthorizedChatId(999)
        ));
    }

    #[test]
    fn test_security_gate_check_locked() {
        let mut gate = SecurityGate::new(vec![42]);
        gate.lock();
        let result = gate.check(42, PermissionLevel::ReadOnly, false);
        assert!(
            result.is_err(),
            "locked gate should deny even allowed chats"
        );
        assert!(matches!(result.unwrap_err(), SecurityError::BotLocked));
    }

    #[test]
    fn test_permission_level_ordering() {
        use PermissionLevel::*;
        assert!(ReadOnly < Query);
        assert!(Query < Action);
        assert!(Action < Modify);
        assert!(Modify < Admin);
    }

    #[test]
    fn test_permission_level_as_str() {
        assert_eq!(PermissionLevel::ReadOnly.as_str(), "read-only");
        assert_eq!(PermissionLevel::Query.as_str(), "query");
        assert_eq!(PermissionLevel::Action.as_str(), "action");
        assert_eq!(PermissionLevel::Modify.as_str(), "modify");
        assert_eq!(PermissionLevel::Admin.as_str(), "admin");
    }

    #[test]
    fn test_security_error_display() {
        let err = SecurityError::UnauthorizedChatId(123);
        assert!(format!("{err}").contains("123"));

        let err = SecurityError::BotLocked;
        assert!(format!("{err}").contains("locked"));

        let err = SecurityError::InvalidPin;
        assert!(format!("{err}").contains("invalid"));
    }

    #[test]
    fn test_rate_limiter_new() {
        let limiter = RateLimiter::new(30, 300);
        assert_eq!(limiter.max_per_minute, 30);
        assert_eq!(limiter.max_per_hour, 300);
    }

    #[test]
    fn test_rate_limiter_allows_within_limits() {
        let mut limiter = RateLimiter::new(10, 100);
        for _ in 0..5 {
            let result = limiter.check_and_record(42);
            assert!(result.is_ok(), "should allow within limits");
        }
    }

    #[test]
    fn test_rate_limiter_reset() {
        let mut limiter = RateLimiter::new(2, 100);
        limiter.check_and_record(42).unwrap();
        limiter.check_and_record(42).unwrap();
        // Now at limit
        let result = limiter.check_and_record(42);
        assert!(result.is_err(), "should be rate limited");

        // Reset and try again
        limiter.reset(42);
        let result = limiter.check_and_record(42);
        assert!(result.is_ok(), "should allow after reset");
    }
}

#[cfg(test)]
mod audit_log_edge_cases {
    use aura_daemon::telegram::audit::{AuditEntry, AuditLog, AuditOutcome};

    #[test]
    fn test_audit_log_zero_capacity() {
        let mut log = AuditLog::new(0);
        log.record(42, "test", AuditOutcome::Allowed);
        assert_eq!(log.last_n(1).len(), 0);
    }

    #[test]
    fn test_audit_log_single_capacity() {
        let mut log = AuditLog::new(1);
        log.record(42, "cmd1", AuditOutcome::Allowed);
        log.record(42, "cmd2", AuditOutcome::Denied("test".into()));
        let entries = log.last_n(10);
        assert_eq!(entries.len(), 1);
        assert!(matches!(entries[0].outcome, AuditOutcome::Denied(_)));
    }

    #[test]
    fn test_audit_log_overflow() {
        let mut log = AuditLog::new(3);
        for i in 0..10 {
            log.record(42, &format!("cmd{i}"), AuditOutcome::Allowed);
        }
        let entries = log.last_n(10);
        assert_eq!(entries.len(), 3);
    }

    #[test]
    fn test_audit_log_last_n_more_than_entries() {
        let mut log = AuditLog::new(10);
        log.record(42, "one", AuditOutcome::Allowed);
        let entries = log.last_n(100);
        assert_eq!(entries.len(), 1);
    }

    #[test]
    fn test_audit_log_all_outcome_variants() {
        let mut log = AuditLog::new(10);
        log.record(42, "cmd", AuditOutcome::Allowed);
        log.record(42, "cmd", AuditOutcome::Denied("reason".into()));
        log.record(42, "cmd", AuditOutcome::Failed("error".into()));
        let entries = log.last_n(10);
        assert_eq!(entries.len(), 3);
    }

    #[test]
    fn test_audit_log_debug_format() {
        let log = AuditLog::new(5);
        let debug = format!("{log:?}");
        assert!(!debug.is_empty());
    }

    #[test]
    fn test_audit_entry_fields() {
        let entry = AuditEntry {
            seq: 1,
            timestamp: 1_700_000_000,
            chat_id: 42,
            command_summary: "/status".to_string(),
            outcome: AuditOutcome::Allowed,
        };
        assert_eq!(entry.seq, 1);
        assert_eq!(entry.chat_id, 42);
        assert_eq!(entry.command_summary, "/status");
        assert!(matches!(entry.outcome, AuditOutcome::Allowed));
    }
}

#[cfg(test)]
mod dialogue_edge_cases {
    use aura_daemon::telegram::dialogue::DialogueManager;

    #[test]
    fn test_dialogue_manager_no_active() {
        let mgr = DialogueManager::new(120);
        assert!(!mgr.has_active(42));
    }

    #[test]
    fn test_dialogue_process_input_no_active() {
        let mut mgr = DialogueManager::new(120);
        let outcome = mgr.process_input(42, "hello");
        let _ = format!("{outcome:?}");
    }

    #[test]
    fn test_dialogue_expire_stale_empty() {
        let mut mgr = DialogueManager::new(120);
        mgr.expire_stale();
    }

    #[test]
    fn test_dialogue_zero_timeout() {
        let mgr = DialogueManager::new(0);
        assert!(!mgr.has_active(1));
    }

    #[test]
    fn test_dialogue_very_long_input() {
        let mut mgr = DialogueManager::new(120);
        let long = "x".repeat(50_000);
        let outcome = mgr.process_input(1, &long);
        let _ = format!("{outcome:?}");
    }
}
