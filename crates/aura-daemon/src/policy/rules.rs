//! Policy rule types, effect enum, and rule loading/validation.

use std::fmt;

use aura_types::config::{PolicyConfig, PolicyRuleConfig};
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// RuleEffect
// ---------------------------------------------------------------------------

/// The effect a policy rule has on an action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RuleEffect {
    /// Action is permitted — proceed normally.
    Allow,
    /// Action is blocked — do not execute.
    Deny,
    /// Action is permitted but must be audit-logged.
    Audit,
    /// Action requires explicit user confirmation before execution.
    Confirm,
}

impl fmt::Display for RuleEffect {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Allow => write!(f, "allow"),
            Self::Deny => write!(f, "deny"),
            Self::Audit => write!(f, "audit"),
            Self::Confirm => write!(f, "confirm"),
        }
    }
}

impl RuleEffect {
    /// Parse a string into a `RuleEffect`.
    ///
    /// Accepts: "allow", "deny", "audit", "confirm" (case-insensitive).
    /// Returns `None` for unrecognized strings.
    pub fn from_str_lenient(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "allow" => Some(Self::Allow),
            "deny" => Some(Self::Deny),
            "audit" => Some(Self::Audit),
            "confirm" => Some(Self::Confirm),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// PolicyRule
// ---------------------------------------------------------------------------

/// A compiled policy rule ready for evaluation.
#[derive(Debug, Clone)]
pub struct PolicyRule {
    /// Human-readable identifier.
    pub name: String,
    /// Glob pattern for action matching (lowercase, compiled from config).
    pub action_pattern: String,
    /// What happens when this rule matches.
    pub effect: RuleEffect,
    /// Explanation surfaced to the user on deny/confirm.
    pub reason: String,
    /// Priority — lower = evaluated first. Rules are pre-sorted by this.
    pub priority: u32,
}

impl PolicyRule {
    /// Create a rule from a config entry, validating the effect string.
    ///
    /// Returns `Err` with a description if the effect string is unrecognized.
    pub fn from_config(cfg: &PolicyRuleConfig) -> Result<Self, String> {
        let effect = RuleEffect::from_str_lenient(&cfg.effect).ok_or_else(|| {
            format!(
                "rule '{}': unrecognized effect '{}' (expected allow|deny|audit|confirm)",
                cfg.name, cfg.effect
            )
        })?;

        Ok(Self {
            name: cfg.name.clone(),
            action_pattern: cfg.action.to_ascii_lowercase(),
            effect,
            reason: cfg.reason.clone(),
            priority: cfg.priority,
        })
    }

    /// Test whether `action` matches this rule's glob pattern.
    ///
    /// Glob semantics:
    /// - `*` matches zero or more characters
    /// - `?` matches exactly one character
    /// - All other characters are literal (case-insensitive)
    pub fn matches(&self, action: &str) -> bool {
        glob_match(&self.action_pattern, &action.to_ascii_lowercase())
    }
}

// ---------------------------------------------------------------------------
// Rule loading
// ---------------------------------------------------------------------------

/// Load and validate rules from a `PolicyConfig`.
///
/// Rules are sorted by priority (ascending) so that first-match-wins
/// evaluation respects the priority ordering.
///
/// Invalid rules (bad effect strings) are collected into the error vec.
/// If `strict` is true, any invalid rule causes the whole load to fail.
pub fn load_rules(config: &PolicyConfig, strict: bool) -> Result<Vec<PolicyRule>, Vec<String>> {
    let mut rules = Vec::with_capacity(config.rules.len());
    let mut errors = Vec::new();

    for rule_cfg in &config.rules {
        match PolicyRule::from_config(rule_cfg) {
            Ok(rule) => rules.push(rule),
            Err(e) => {
                if strict {
                    errors.push(e);
                } else {
                    tracing::warn!("skipping invalid policy rule: {}", e);
                }
            }
        }
    }

    if !errors.is_empty() {
        return Err(errors);
    }

    // Sort by priority (ascending) — lower priority number = evaluated first.
    rules.sort_by_key(|r| r.priority);

    Ok(rules)
}

/// Parse the default effect string from config.
pub fn parse_default_effect(config: &PolicyConfig) -> RuleEffect {
    RuleEffect::from_str_lenient(&config.default_effect).unwrap_or_else(|| {
        tracing::warn!(
            "unrecognized default_effect '{}', falling back to Allow",
            config.default_effect
        );
        RuleEffect::Allow
    })
}

// ---------------------------------------------------------------------------
// Glob matching
// ---------------------------------------------------------------------------

/// Simple glob matcher supporting `*` (zero or more chars) and `?` (one char).
///
/// Both `pattern` and `text` should be pre-lowercased for case-insensitive matching.
fn glob_match(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();
    glob_match_inner(&p, &t, 0, 0)
}

fn glob_match_inner(p: &[char], t: &[char], pi: usize, ti: usize) -> bool {
    let plen = p.len();
    let tlen = t.len();

    if pi == plen {
        return ti == tlen;
    }

    match p[pi] {
        '*' => {
            // Try matching * with 0..=(tlen-ti) characters.
            // Optimization: skip consecutive *'s.
            let mut next_pi = pi;
            while next_pi < plen && p[next_pi] == '*' {
                next_pi += 1;
            }
            // If pattern ends with *, match everything remaining.
            if next_pi == plen {
                return true;
            }
            for skip in 0..=(tlen - ti) {
                if glob_match_inner(p, t, next_pi, ti + skip) {
                    return true;
                }
            }
            false
        }
        '?' => {
            if ti < tlen {
                glob_match_inner(p, t, pi + 1, ti + 1)
            } else {
                false
            }
        }
        ch => {
            if ti < tlen && t[ti] == ch {
                glob_match_inner(p, t, pi + 1, ti + 1)
            } else {
                false
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rule_effect_display() {
        assert_eq!(RuleEffect::Allow.to_string(), "allow");
        assert_eq!(RuleEffect::Deny.to_string(), "deny");
        assert_eq!(RuleEffect::Audit.to_string(), "audit");
        assert_eq!(RuleEffect::Confirm.to_string(), "confirm");
    }

    #[test]
    fn test_rule_effect_from_str() {
        assert_eq!(
            RuleEffect::from_str_lenient("allow"),
            Some(RuleEffect::Allow)
        );
        assert_eq!(RuleEffect::from_str_lenient("DENY"), Some(RuleEffect::Deny));
        assert_eq!(
            RuleEffect::from_str_lenient(" Audit "),
            Some(RuleEffect::Audit)
        );
        assert_eq!(
            RuleEffect::from_str_lenient("confirm"),
            Some(RuleEffect::Confirm)
        );
        assert_eq!(RuleEffect::from_str_lenient("invalid"), None);
        assert_eq!(RuleEffect::from_str_lenient(""), None);
    }

    #[test]
    fn test_glob_exact() {
        assert!(glob_match("hello", "hello"));
        assert!(!glob_match("hello", "world"));
        assert!(!glob_match("hello", "hell"));
        assert!(!glob_match("hello", "helloo"));
    }

    #[test]
    fn test_glob_star() {
        assert!(glob_match("*", "anything"));
        assert!(glob_match("*", ""));
        assert!(glob_match("hello*", "hello"));
        assert!(glob_match("hello*", "helloworld"));
        assert!(glob_match("*world", "helloworld"));
        assert!(glob_match("*world", "world"));
        assert!(!glob_match("*world", "worldx"));
        assert!(glob_match("*factory*reset*", "do factory reset now"));
        assert!(glob_match("*factory*reset*", "factory reset"));
        assert!(!glob_match("*factory*reset*", "factory only"));
    }

    #[test]
    fn test_glob_question() {
        assert!(glob_match("h?llo", "hello"));
        assert!(glob_match("h?llo", "hallo"));
        assert!(!glob_match("h?llo", "hllo"));
        assert!(!glob_match("h?llo", "heello"));
    }

    #[test]
    fn test_glob_combined() {
        assert!(glob_match("*install*app*", "install app now"));
        assert!(glob_match("*install*app*", "please install my app"));
        assert!(glob_match("navigate*", "navigate_home"));
        assert!(glob_match("read*", "read_contacts"));
        assert!(glob_match("search*", "search_web"));
    }

    #[test]
    fn test_rule_from_config() {
        let cfg = PolicyRuleConfig {
            name: "test-rule".to_string(),
            action: "*test*".to_string(),
            effect: "deny".to_string(),
            reason: "Testing".to_string(),
            priority: 5,
        };
        let rule = PolicyRule::from_config(&cfg).unwrap();
        assert_eq!(rule.name, "test-rule");
        assert_eq!(rule.effect, RuleEffect::Deny);
        assert_eq!(rule.priority, 5);
        assert!(rule.matches("this is a test action"));
    }

    #[test]
    fn test_rule_from_config_invalid_effect() {
        let cfg = PolicyRuleConfig {
            name: "bad-rule".to_string(),
            action: "*".to_string(),
            effect: "explode".to_string(),
            reason: "boom".to_string(),
            priority: 0,
        };
        assert!(PolicyRule::from_config(&cfg).is_err());
    }

    #[test]
    fn test_load_rules_sorting() {
        let config = PolicyConfig {
            default_effect: "allow".to_string(),
            log_all_decisions: false,
            max_rules_per_event: 100,
            rules: vec![
                PolicyRuleConfig {
                    name: "low-priority".to_string(),
                    action: "read*".to_string(),
                    effect: "allow".to_string(),
                    reason: "safe".to_string(),
                    priority: 50,
                },
                PolicyRuleConfig {
                    name: "high-priority".to_string(),
                    action: "*reset*".to_string(),
                    effect: "deny".to_string(),
                    reason: "dangerous".to_string(),
                    priority: 0,
                },
                PolicyRuleConfig {
                    name: "mid-priority".to_string(),
                    action: "*install*".to_string(),
                    effect: "confirm".to_string(),
                    reason: "needs approval".to_string(),
                    priority: 10,
                },
            ],
        };

        let rules = load_rules(&config, true).unwrap();
        assert_eq!(rules.len(), 3);
        assert_eq!(rules[0].name, "high-priority");
        assert_eq!(rules[0].priority, 0);
        assert_eq!(rules[1].name, "mid-priority");
        assert_eq!(rules[1].priority, 10);
        assert_eq!(rules[2].name, "low-priority");
        assert_eq!(rules[2].priority, 50);
    }

    #[test]
    fn test_load_rules_strict_failure() {
        let config = PolicyConfig {
            default_effect: "allow".to_string(),
            log_all_decisions: false,
            max_rules_per_event: 100,
            rules: vec![PolicyRuleConfig {
                name: "bad".to_string(),
                action: "*".to_string(),
                effect: "nuke".to_string(),
                reason: "bad".to_string(),
                priority: 0,
            }],
        };

        let result = load_rules(&config, true);
        assert!(result.is_err());
        let errors = result.unwrap_err();
        assert_eq!(errors.len(), 1);
        assert!(errors[0].contains("nuke"));
    }

    #[test]
    fn test_parse_default_effect() {
        let config = PolicyConfig {
            default_effect: "deny".to_string(),
            ..PolicyConfig::default()
        };
        assert_eq!(parse_default_effect(&config), RuleEffect::Deny);

        let bad_config = PolicyConfig {
            default_effect: "garbage".to_string(),
            ..PolicyConfig::default()
        };
        assert_eq!(parse_default_effect(&bad_config), RuleEffect::Allow);
    }
}
