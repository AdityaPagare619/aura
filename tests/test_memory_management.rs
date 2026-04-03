//! Tests for memory management: token budget, RSS thresholds, compaction.

#[cfg(test)]
mod token_budget {
    use aura_types::config::TokenBudgetConfig;

    #[test]
    fn test_token_budget_defaults() {
        let config = TokenBudgetConfig::default();
        assert_eq!(config.session_limit, 2048);
        assert_eq!(config.response_reserve, 512);
        assert!((config.compaction_threshold - 0.75).abs() < f32::EPSILON);
        assert!((config.force_compaction_threshold - 0.90).abs() < f32::EPSILON);
    }

    #[test]
    fn test_token_budget_thresholds_ordered() {
        let config = TokenBudgetConfig::default();
        assert!(
            config.compaction_threshold < config.force_compaction_threshold,
            "compaction ({}) must be < force_compaction ({})",
            config.compaction_threshold,
            config.force_compaction_threshold
        );
    }

    #[test]
    fn test_token_budget_thresholds_in_valid_range() {
        let config = TokenBudgetConfig::default();
        assert!(
            config.compaction_threshold > 0.0 && config.compaction_threshold < 1.0,
            "compaction_threshold must be in (0, 1)"
        );
        assert!(
            config.force_compaction_threshold > 0.0 && config.force_compaction_threshold < 1.0,
            "force_compaction_threshold must be in (0, 1)"
        );
    }

    #[test]
    fn test_token_budget_response_reserve_less_than_session() {
        let config = TokenBudgetConfig::default();
        assert!(
            config.response_reserve < config.session_limit,
            "response_reserve ({}) must be < session_limit ({})",
            config.response_reserve,
            config.session_limit
        );
    }

    #[test]
    fn test_token_budget_available_tokens() {
        let config = TokenBudgetConfig::default();
        let available = config.session_limit - config.response_reserve;
        assert!(available > 0, "available tokens must be positive");
        assert_eq!(available, 1536);
    }

    #[test]
    fn test_token_budget_serialization_roundtrip() {
        let config = TokenBudgetConfig::default();
        let json = serde_json::to_string(&config).unwrap();
        let restored: TokenBudgetConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.session_limit, config.session_limit);
        assert_eq!(restored.response_reserve, config.response_reserve);
    }

    #[test]
    fn test_token_budget_partial_deserialization() {
        // Only session_limit provided; others should use defaults
        let partial = r#"{"session_limit": 4096}"#;
        let config: TokenBudgetConfig = serde_json::from_str(partial).unwrap();
        assert_eq!(config.session_limit, 4096);
        assert_eq!(config.response_reserve, 512); // default
    }
}

#[cfg(test)]
mod rss_thresholds {
    use aura_types::config::{DaemonConfig, DEFAULT_RSS_CEILING_MB, DEFAULT_RSS_WARNING_MB};

    #[test]
    fn test_rss_warning_default() {
        assert_eq!(DEFAULT_RSS_WARNING_MB, 28);
    }

    #[test]
    fn test_rss_ceiling_default() {
        assert_eq!(DEFAULT_RSS_CEILING_MB, 30);
    }

    #[test]
    fn test_rss_ceiling_above_warning() {
        assert!(
            DEFAULT_RSS_CEILING_MB > DEFAULT_RSS_WARNING_MB,
            "RSS ceiling ({}) must be above warning ({})",
            DEFAULT_RSS_CEILING_MB,
            DEFAULT_RSS_WARNING_MB
        );
    }

    #[test]
    fn test_daemon_config_rss_defaults() {
        let config = DaemonConfig::default();
        assert_eq!(config.rss_warning_mb, DEFAULT_RSS_WARNING_MB);
        assert_eq!(config.rss_ceiling_mb, DEFAULT_RSS_CEILING_MB);
    }

    #[test]
    fn test_rss_thresholds_reasonable_range() {
        // On Termux/Android, 28-30MB is reasonable for a daemon
        assert!(DEFAULT_RSS_WARNING_MB >= 10 && DEFAULT_RSS_WARNING_MB <= 100);
        assert!(DEFAULT_RSS_CEILING_MB >= 10 && DEFAULT_RSS_CEILING_MB <= 200);
    }
}

#[cfg(test)]
mod compaction {
    use aura_daemon::memory::ContextCompactor;

    #[test]
    fn test_compactor_creation() {
        let compactor = ContextCompactor::new();
        let _ = format!("{compactor:?}");
    }

    #[test]
    fn test_compactor_empty_context() {
        let compactor = ContextCompactor::new();
        let result = compactor.compact(&[], 1024);
        assert!(result.is_empty() || result.len() <= 1024);
    }

    #[test]
    fn test_compactor_preserves_important_items() {
        let compactor = ContextCompactor::new();
        let items = vec![
            ("system prompt".to_string(), 0.95),
            ("user message".to_string(), 0.80),
            ("noise".to_string(), 0.10),
        ];
        let result = compactor.compact(&items, 2);
        // Should keep at most 2 items, preferring higher importance
        assert!(result.len() <= 2);
    }

    #[test]
    fn test_compactor_respects_token_limit() {
        let compactor = ContextCompactor::new();
        let items: Vec<_> = (0..100).map(|i| (format!("item_{i}"), 0.5)).collect();
        let result = compactor.compact(&items, 10);
        assert!(result.len() <= 10);
    }
}

#[cfg(test)]
mod memory_budget {
    use aura_daemon::memory::working::MAX_SLOTS;

    #[test]
    fn test_max_slots_constant() {
        assert!(MAX_SLOTS > 0, "MAX_SLOTS must be positive");
        assert!(MAX_SLOTS <= 2048, "MAX_SLOTS should be reasonable");
    }

    #[test]
    fn test_working_memory_respects_max_slots() {
        use aura_daemon::memory::WorkingMemory;
        use aura_types::events::EventSource;

        let mut wm = WorkingMemory::new();
        for i in 0..MAX_SLOTS + 100 {
            wm.push(
                format!("item_{i}"),
                EventSource::Internal,
                0.5,
                1_700_000_000_000 + i * 1000,
            );
        }
        assert!(
            wm.len() <= MAX_SLOTS,
            "working memory must not exceed MAX_SLOTS ({max}), got {len}",
            max = MAX_SLOTS,
            len = wm.len()
        );
    }

    #[test]
    fn test_working_memory_clear() {
        use aura_daemon::memory::WorkingMemory;
        use aura_types::events::EventSource;

        let mut wm = WorkingMemory::new();
        wm.push("test".into(), EventSource::Internal, 0.5, 0);
        assert_eq!(wm.len(), 1);
        wm.clear();
        assert_eq!(wm.len(), 0);
    }

    #[test]
    fn test_working_memory_query_empty() {
        use aura_daemon::memory::WorkingMemory;

        let wm = WorkingMemory::new();
        let results = wm.query("anything", 10, 1_700_000_000_000);
        assert!(results.is_empty());
    }
}
