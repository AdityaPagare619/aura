//! PolicyGate — the core decision engine.
//!
//! Evaluates an action string against a priority-sorted rule list.
//! First matching rule wins.  If no rule matches, the configured
//! default effect is used.
//!
//! ## Rate Limiting
//!
//! The gate includes a sliding-window rate limiter that tracks action
//! frequency per action pattern.  If more than `max_actions_per_window`
//! identical actions occur within `window_duration`, the action is
//! automatically denied as suspicious (e.g., 10 taps in 1 second).

use std::collections::{HashMap, VecDeque};
use std::fmt;
use std::time::{Duration, Instant};

use aura_types::config::PolicyConfig;

use super::rules::{load_rules, parse_default_effect, PolicyRule, RuleEffect};

// ---------------------------------------------------------------------------
// RateLimiter
// ---------------------------------------------------------------------------

/// Maximum number of distinct action keys tracked by the rate limiter.
/// Prevents unbounded memory growth from unique action strings.
const MAX_RATE_LIMITER_KEYS: usize = 256;

/// Sliding-window rate limiter for action frequency tracking.
///
/// Tracks timestamps of recent actions keyed by a normalised action
/// string.  When the count within the sliding window exceeds the
/// configured threshold, the action is flagged as rate-limited.
#[derive(Debug, Clone)]
pub struct RateLimiter {
    /// Maximum actions allowed within the sliding window.
    max_actions_per_window: u32,
    /// Duration of the sliding window.
    window_duration: Duration,
    /// Per-action-key timestamps of recent invocations.
    history: HashMap<String, VecDeque<Instant>>,
    /// Total number of rate-limit denials since creation.
    total_denials: u64,
}

impl RateLimiter {
    /// Create a new rate limiter.
    ///
    /// # Arguments
    /// * `max_actions_per_window` — Maximum actions before rate limiting kicks in.
    /// * `window_duration` — The sliding window duration.
    pub fn new(max_actions_per_window: u32, window_duration: Duration) -> Self {
        Self {
            max_actions_per_window,
            window_duration,
            history: HashMap::new(),
            total_denials: 0,
        }
    }

    /// Default rate limiter: 10 actions per 1 second.
    pub fn default_limiter() -> Self {
        Self::new(10, Duration::from_secs(1))
    }

    /// Check whether the given action is rate-limited.
    ///
    /// Records the current timestamp and prunes expired entries.
    /// Returns `true` if the action exceeds the rate limit.
    pub fn is_rate_limited(&mut self, action: &str) -> bool {
        let now = Instant::now();
        let key = action.to_ascii_lowercase();

        // Enforce capacity cap before inserting a new key.
        if !self.history.contains_key(&key) {
            if self.history.len() >= MAX_RATE_LIMITER_KEYS {
                // At capacity: evict the oldest key (least-recently-used proxy:
                // first key in iteration order) and warn.
                if let Some(evict_key) = self.history.keys().next().cloned() {
                    self.history.remove(&evict_key);
                    tracing::warn!(
                        evicted_key = %evict_key,
                        capacity = MAX_RATE_LIMITER_KEYS,
                        "rate limiter at capacity — evicted oldest key"
                    );
                }
            }
        }

        let timestamps = self.history.entry(key).or_default();

        // Prune entries outside the window.
        while let Some(front) = timestamps.front() {
            if now.duration_since(*front) > self.window_duration {
                timestamps.pop_front();
            } else {
                break;
            }
        }

        // Record this invocation.
        timestamps.push_back(now);

        // Check against threshold.
        if timestamps.len() > self.max_actions_per_window as usize {
            self.total_denials += 1;
            tracing::warn!(
                action = action,
                count = timestamps.len(),
                window_ms = self.window_duration.as_millis() as u64,
                "rate limit exceeded"
            );
            true
        } else {
            false
        }
    }

    /// Total number of rate-limit denials since creation.
    pub fn total_denials(&self) -> u64 {
        self.total_denials
    }

    /// Clear all tracking history.
    pub fn clear(&mut self) {
        self.history.clear();
    }

    /// Number of distinct action keys currently tracked.
    pub fn tracked_keys(&self) -> usize {
        self.history.len()
    }
}

// ---------------------------------------------------------------------------
// PolicyDecision
// ---------------------------------------------------------------------------

/// The outcome of a policy evaluation.
#[derive(Debug, Clone)]
pub struct PolicyDecision {
    /// The effect applied to the action.
    pub effect: RuleEffect,
    /// Which rule matched (None if default was applied).
    pub matched_rule: Option<String>,
    /// Human-readable reason for the decision.
    pub reason: String,
    /// Number of rules evaluated before a match was found.
    pub rules_evaluated: u32,
}

impl fmt::Display for PolicyDecision {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.matched_rule {
            Some(rule) => write!(
                f,
                "{} (rule: {}, reason: {})",
                self.effect, rule, self.reason
            ),
            None => write!(f, "{} (default, reason: {})", self.effect, self.reason),
        }
    }
}

impl PolicyDecision {
    /// Whether this decision allows the action to proceed (Allow or Audit).
    pub fn is_allowed(&self) -> bool {
        matches!(self.effect, RuleEffect::Allow | RuleEffect::Audit)
    }

    /// Whether this decision blocks the action outright.
    pub fn is_denied(&self) -> bool {
        self.effect == RuleEffect::Deny
    }

    /// Whether this decision requires user confirmation.
    pub fn needs_confirmation(&self) -> bool {
        self.effect == RuleEffect::Confirm
    }

    /// Whether this decision triggers an audit log entry.
    pub fn needs_audit(&self) -> bool {
        matches!(
            self.effect,
            RuleEffect::Audit | RuleEffect::Confirm | RuleEffect::Deny
        )
    }
}

// ---------------------------------------------------------------------------
// PolicyGate
// ---------------------------------------------------------------------------

/// The PolicyGate evaluates actions against safety rules.
///
/// Combines rule-based evaluation with sliding-window rate limiting.
/// If an action is rate-limited (too many identical actions in a short
/// window), it is denied *before* rule evaluation even runs.
///
/// # Usage
///
/// ```ignore
/// let gate = PolicyGate::from_config(&config.policy)?;
/// let decision = gate.evaluate("install app com.example");
/// if decision.is_denied() {
///     tracing::warn!("blocked: {}", decision);
/// }
/// ```
#[derive(Debug, Clone)]
pub struct PolicyGate {
    /// Priority-sorted rules (ascending — lowest priority number first).
    rules: Vec<PolicyRule>,
    /// Default effect when no rule matches.
    default_effect: RuleEffect,
    /// Whether to log all decisions (not just deny/confirm).
    log_all: bool,
    /// Max rules to evaluate per action (safety cap against pathological configs).
    max_rules_per_event: u32,
    /// Sliding-window rate limiter to detect suspicious action bursts.
    rate_limiter: RateLimiter,
}

impl PolicyGate {
    /// Create a PolicyGate from configuration.
    ///
    /// Rules are validated and sorted by priority. Invalid rules cause an error
    /// in strict mode, or are skipped with a warning in lenient mode.
    pub fn from_config(config: &PolicyConfig) -> Result<Self, Vec<String>> {
        let rules = load_rules(config, true)?;
        let default_effect = parse_default_effect(config);

        Ok(Self {
            rules,
            default_effect,
            log_all: config.log_all_decisions,
            max_rules_per_event: config.max_rules_per_event,
            rate_limiter: RateLimiter::default_limiter(),
        })
    }

    /// Create a PolicyGate in lenient mode — invalid rules are skipped.
    pub fn from_config_lenient(config: &PolicyConfig) -> Self {
        let rules = load_rules(config, false).unwrap_or_default();
        let default_effect = parse_default_effect(config);

        Self {
            rules,
            default_effect,
            log_all: config.log_all_decisions,
            max_rules_per_event: config.max_rules_per_event,
            rate_limiter: RateLimiter::default_limiter(),
        }
    }

    /// Create an empty PolicyGate that allows everything.
    ///
    /// # Security
    ///
    /// This method exists **only for tests**.  Production code must use
    /// [`PolicyGate::deny_by_default`] (or [`PolicyGate::from_config`])
    /// and explicitly add allow-rules for permitted actions.
    #[cfg(test)]
    pub fn allow_all() -> Self {
        Self {
            rules: Vec::new(),
            default_effect: RuleEffect::Allow,
            log_all: false,
            max_rules_per_event: 100,
            rate_limiter: RateLimiter::default_limiter(),
        }
    }

    /// Create an empty PolicyGate that **denies** unknown actions by default.
    ///
    /// This is the secure starting point for production policy construction.
    /// The caller adds explicit allow/confirm/audit rules on top.  Any action
    /// that does not match a rule is denied and logged.
    pub fn deny_by_default() -> Self {
        Self {
            rules: Vec::new(),
            default_effect: RuleEffect::Deny,
            log_all: true,
            max_rules_per_event: 100,
            rate_limiter: RateLimiter::default_limiter(),
        }
    }

    /// Create an empty PolicyGate that allows everything by default, intended
    /// as a builder starting point where hardened rules are added immediately
    /// after construction (see [`build_hardened_policy_gate`]).
    ///
    /// Prefer [`PolicyGate::deny_by_default`] for new code.
    pub(crate) fn allow_all_builder() -> Self {
        Self {
            rules: Vec::new(),
            default_effect: RuleEffect::Allow,
            log_all: false,
            max_rules_per_event: 100,
            rate_limiter: RateLimiter::default_limiter(),
        }
    }

    /// Evaluate an action against the rule set, including rate limiting.
    ///
    /// **Rate limiting runs first**: if the action frequency exceeds the
    /// configured threshold, the action is denied immediately without
    /// checking any rules.
    ///
    /// Otherwise, iterates rules in priority order (first match wins).
    /// Returns a `PolicyDecision` with the matched effect, rule name,
    /// reason, and count of rules evaluated.
    pub fn evaluate(&mut self, action: &str) -> PolicyDecision {
        // --- Rate limit check ---
        if self.rate_limiter.is_rate_limited(action) {
            return PolicyDecision {
                effect: RuleEffect::Deny,
                matched_rule: None,
                reason: format!(
                    "rate limited: too many identical actions within {}ms window",
                    self.rate_limiter.window_duration.as_millis()
                ),
                rules_evaluated: 0,
            };
        }

        // --- Rule evaluation ---
        let mut evaluated: u32 = 0;

        for rule in &self.rules {
            if evaluated >= self.max_rules_per_event {
                tracing::warn!(
                    "policy rule evaluation cap reached ({}) for action '{}'",
                    self.max_rules_per_event,
                    action
                );
                break;
            }

            evaluated += 1;

            if rule.matches(action) {
                let decision = PolicyDecision {
                    effect: rule.effect,
                    matched_rule: Some(rule.name.clone()),
                    reason: rule.reason.clone(),
                    rules_evaluated: evaluated,
                };

                if self.log_all || !matches!(rule.effect, RuleEffect::Allow) {
                    tracing::info!(
                        action = action,
                        rule = rule.name.as_str(),
                        effect = %rule.effect,
                        "policy decision"
                    );
                }

                return decision;
            }
        }

        // No rule matched — apply default.
        let decision = PolicyDecision {
            effect: self.default_effect,
            matched_rule: None,
            reason: "no matching rule; default effect applied".to_string(),
            rules_evaluated: evaluated,
        };

        if self.log_all {
            tracing::info!(
                action = action,
                effect = %self.default_effect,
                "policy decision (default)"
            );
        }

        decision
    }

    /// Number of loaded rules.
    pub fn rule_count(&self) -> usize {
        self.rules.len()
    }

    /// The configured default effect.
    pub fn default_effect(&self) -> RuleEffect {
        self.default_effect
    }

    /// Add a rule dynamically (inserted in priority order).
    pub fn add_rule(&mut self, rule: PolicyRule) {
        let pos = self.rules.partition_point(|r| r.priority <= rule.priority);
        self.rules.insert(pos, rule);
    }

    /// Remove a rule by name. Returns true if a rule was removed.
    pub fn remove_rule(&mut self, name: &str) -> bool {
        let before = self.rules.len();
        self.rules.retain(|r| r.name != name);
        self.rules.len() < before
    }

    /// Mutable access to the rate limiter for configuration or inspection.
    pub fn rate_limiter_mut(&mut self) -> &mut RateLimiter {
        &mut self.rate_limiter
    }

    /// Read-only access to the rate limiter.
    pub fn rate_limiter(&self) -> &RateLimiter {
        &self.rate_limiter
    }

    /// Replace the rate limiter with a custom one.
    pub fn set_rate_limiter(&mut self, limiter: RateLimiter) {
        self.rate_limiter = limiter;
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use aura_types::config::{PolicyConfig, PolicyRuleConfig};

    fn test_config() -> PolicyConfig {
        PolicyConfig {
            default_effect: "allow".to_string(),
            log_all_decisions: false,
            max_rules_per_event: 100,
            rules: vec![
                PolicyRuleConfig {
                    name: "block-reset".to_string(),
                    action: "*factory*reset*".to_string(),
                    effect: "deny".to_string(),
                    reason: "Factory reset is destructive.".to_string(),
                    priority: 0,
                },
                PolicyRuleConfig {
                    name: "block-wipe".to_string(),
                    action: "*wipe*data*".to_string(),
                    effect: "deny".to_string(),
                    reason: "Data wipe is irreversible.".to_string(),
                    priority: 0,
                },
                PolicyRuleConfig {
                    name: "confirm-install".to_string(),
                    action: "*install*app*".to_string(),
                    effect: "confirm".to_string(),
                    reason: "App installation requires confirmation.".to_string(),
                    priority: 10,
                },
                PolicyRuleConfig {
                    name: "audit-credentials".to_string(),
                    action: "*credential*".to_string(),
                    effect: "audit".to_string(),
                    reason: "Credential access is logged.".to_string(),
                    priority: 20,
                },
                PolicyRuleConfig {
                    name: "allow-navigation".to_string(),
                    action: "navigate*".to_string(),
                    effect: "allow".to_string(),
                    reason: "Navigation is safe.".to_string(),
                    priority: 50,
                },
                PolicyRuleConfig {
                    name: "allow-read".to_string(),
                    action: "read*".to_string(),
                    effect: "allow".to_string(),
                    reason: "Read ops are safe.".to_string(),
                    priority: 50,
                },
            ],
        }
    }

    #[test]
    fn test_gate_from_config() {
        let gate = PolicyGate::from_config(&test_config()).unwrap();
        assert_eq!(gate.rule_count(), 6);
        assert_eq!(gate.default_effect(), RuleEffect::Allow);
    }

    #[test]
    fn test_deny_factory_reset() {
        let mut gate = PolicyGate::from_config(&test_config()).unwrap();
        let decision = gate.evaluate("perform factory reset");
        assert!(decision.is_denied());
        assert_eq!(decision.matched_rule.as_deref(), Some("block-reset"));
    }

    #[test]
    fn test_deny_wipe_data() {
        let mut gate = PolicyGate::from_config(&test_config()).unwrap();
        let decision = gate.evaluate("wipe all data now");
        assert!(decision.is_denied());
        assert_eq!(decision.matched_rule.as_deref(), Some("block-wipe"));
    }

    #[test]
    fn test_confirm_install() {
        let mut gate = PolicyGate::from_config(&test_config()).unwrap();
        let decision = gate.evaluate("install app com.example");
        assert!(decision.needs_confirmation());
        assert_eq!(decision.effect, RuleEffect::Confirm);
    }

    #[test]
    fn test_audit_credentials() {
        let mut gate = PolicyGate::from_config(&test_config()).unwrap();
        let decision = gate.evaluate("access credential store");
        assert_eq!(decision.effect, RuleEffect::Audit);
        assert!(decision.needs_audit());
        assert!(decision.is_allowed()); // audit still allows
    }

    #[test]
    fn test_allow_navigation() {
        let mut gate = PolicyGate::from_config(&test_config()).unwrap();
        let decision = gate.evaluate("navigate_to_home");
        assert!(decision.is_allowed());
        assert_eq!(decision.effect, RuleEffect::Allow);
    }

    #[test]
    fn test_allow_read() {
        let mut gate = PolicyGate::from_config(&test_config()).unwrap();
        let decision = gate.evaluate("read_contacts");
        assert!(decision.is_allowed());
        assert_eq!(decision.matched_rule.as_deref(), Some("allow-read"));
    }

    #[test]
    fn test_default_effect_no_match() {
        let mut gate = PolicyGate::from_config(&test_config()).unwrap();
        let decision = gate.evaluate("some_random_action_xyz");
        assert!(decision.is_allowed());
        assert!(decision.matched_rule.is_none());
        assert_eq!(decision.effect, RuleEffect::Allow);
    }

    #[test]
    fn test_default_effect_deny() {
        let config = PolicyConfig {
            default_effect: "deny".to_string(),
            log_all_decisions: false,
            max_rules_per_event: 100,
            rules: vec![],
        };
        let mut gate = PolicyGate::from_config(&config).unwrap();
        let decision = gate.evaluate("anything");
        assert!(decision.is_denied());
    }

    #[test]
    fn test_first_match_wins() {
        // Both rules could match "install app reset" — first match (lower priority) wins.
        let config = PolicyConfig {
            default_effect: "allow".to_string(),
            log_all_decisions: false,
            max_rules_per_event: 100,
            rules: vec![
                PolicyRuleConfig {
                    name: "deny-reset".to_string(),
                    action: "*reset*".to_string(),
                    effect: "deny".to_string(),
                    reason: "blocked".to_string(),
                    priority: 0,
                },
                PolicyRuleConfig {
                    name: "confirm-install".to_string(),
                    action: "*install*".to_string(),
                    effect: "confirm".to_string(),
                    reason: "confirm".to_string(),
                    priority: 10,
                },
            ],
        };
        let mut gate = PolicyGate::from_config(&config).unwrap();
        let decision = gate.evaluate("install app with reset");
        // deny-reset (priority 0) should match before confirm-install (priority 10)
        assert!(decision.is_denied());
        assert_eq!(decision.matched_rule.as_deref(), Some("deny-reset"));
    }

    #[test]
    fn test_case_insensitive() {
        let mut gate = PolicyGate::from_config(&test_config()).unwrap();
        let decision = gate.evaluate("PERFORM FACTORY RESET");
        assert!(decision.is_denied());
    }

    #[test]
    fn test_allow_all_gate() {
        let mut gate = PolicyGate::allow_all();
        assert_eq!(gate.rule_count(), 0);
        let decision = gate.evaluate("anything_at_all");
        assert!(decision.is_allowed());
    }

    #[test]
    fn test_add_and_remove_rule() {
        let mut gate = PolicyGate::allow_all();
        assert_eq!(gate.rule_count(), 0);

        gate.add_rule(PolicyRule {
            name: "dynamic-deny".to_string(),
            action_pattern: "*dangerous*".to_string(),
            effect: RuleEffect::Deny,
            reason: "dynamic rule".to_string(),
            priority: 5,
        });
        assert_eq!(gate.rule_count(), 1);

        let decision = gate.evaluate("do dangerous thing");
        assert!(decision.is_denied());

        assert!(gate.remove_rule("dynamic-deny"));
        assert_eq!(gate.rule_count(), 0);

        let decision = gate.evaluate("do dangerous thing");
        assert!(decision.is_allowed());
    }

    #[test]
    fn test_rules_evaluated_count() {
        let mut gate = PolicyGate::from_config(&test_config()).unwrap();
        // "navigate_home" should match allow-navigation at priority 50.
        // Rules at priority 0, 0, 10, 20 are checked first (4 non-matches), then 50 matches.
        let decision = gate.evaluate("navigate_home");
        assert!(decision.rules_evaluated >= 5);
    }

    #[test]
    fn test_max_rules_cap() {
        let config = PolicyConfig {
            default_effect: "allow".to_string(),
            log_all_decisions: false,
            max_rules_per_event: 2, // very low cap
            rules: vec![
                PolicyRuleConfig {
                    name: "r1".to_string(),
                    action: "aaa*".to_string(),
                    effect: "deny".to_string(),
                    reason: "r1".to_string(),
                    priority: 0,
                },
                PolicyRuleConfig {
                    name: "r2".to_string(),
                    action: "bbb*".to_string(),
                    effect: "deny".to_string(),
                    reason: "r2".to_string(),
                    priority: 1,
                },
                PolicyRuleConfig {
                    name: "r3".to_string(),
                    action: "ccc*".to_string(),
                    effect: "deny".to_string(),
                    reason: "r3".to_string(),
                    priority: 2,
                },
            ],
        };
        let mut gate = PolicyGate::from_config(&config).unwrap();
        // "ccc_action" matches r3 at priority 2, but cap is 2 so we stop after r1 & r2.
        let decision = gate.evaluate("ccc_action");
        // Should get default allow because cap was hit before r3.
        assert!(decision.is_allowed());
        assert_eq!(decision.rules_evaluated, 2);
    }

    #[test]
    fn test_decision_display() {
        let decision = PolicyDecision {
            effect: RuleEffect::Deny,
            matched_rule: Some("block-reset".to_string()),
            reason: "dangerous".to_string(),
            rules_evaluated: 1,
        };
        let s = decision.to_string();
        assert!(s.contains("deny"));
        assert!(s.contains("block-reset"));
    }

    #[test]
    fn test_from_config_lenient_skips_bad_rules() {
        let config = PolicyConfig {
            default_effect: "allow".to_string(),
            log_all_decisions: false,
            max_rules_per_event: 100,
            rules: vec![
                PolicyRuleConfig {
                    name: "good".to_string(),
                    action: "*test*".to_string(),
                    effect: "deny".to_string(),
                    reason: "ok".to_string(),
                    priority: 0,
                },
                PolicyRuleConfig {
                    name: "bad".to_string(),
                    action: "*".to_string(),
                    effect: "INVALID_EFFECT".to_string(),
                    reason: "bad".to_string(),
                    priority: 1,
                },
            ],
        };
        let gate = PolicyGate::from_config_lenient(&config);
        assert_eq!(gate.rule_count(), 1); // bad rule skipped
    }

    // --- Rate Limiter Tests ---

    #[test]
    fn test_rate_limiter_allows_within_threshold() {
        let mut limiter = RateLimiter::new(5, Duration::from_secs(1));
        for _ in 0..5 {
            assert!(!limiter.is_rate_limited("tap button"));
        }
        assert_eq!(limiter.total_denials(), 0);
    }

    #[test]
    fn test_rate_limiter_denies_above_threshold() {
        let mut limiter = RateLimiter::new(3, Duration::from_secs(60));
        assert!(!limiter.is_rate_limited("tap"));
        assert!(!limiter.is_rate_limited("tap"));
        assert!(!limiter.is_rate_limited("tap"));
        // 4th call exceeds threshold of 3
        assert!(limiter.is_rate_limited("tap"));
        assert_eq!(limiter.total_denials(), 1);
    }

    #[test]
    fn test_rate_limiter_different_keys_independent() {
        let mut limiter = RateLimiter::new(2, Duration::from_secs(60));
        assert!(!limiter.is_rate_limited("action_a"));
        assert!(!limiter.is_rate_limited("action_a"));
        assert!(!limiter.is_rate_limited("action_b"));
        assert!(!limiter.is_rate_limited("action_b"));
        // action_a hits limit, action_b does not affect it
        assert!(limiter.is_rate_limited("action_a"));
        assert!(limiter.is_rate_limited("action_b"));
        assert_eq!(limiter.tracked_keys(), 2);
    }

    #[test]
    fn test_rate_limiter_clear() {
        let mut limiter = RateLimiter::new(2, Duration::from_secs(60));
        assert!(!limiter.is_rate_limited("x"));
        assert!(!limiter.is_rate_limited("x"));
        limiter.clear();
        // After clear, should allow again
        assert!(!limiter.is_rate_limited("x"));
        assert_eq!(limiter.tracked_keys(), 1);
    }

    #[test]
    fn test_gate_evaluate_rate_limited() {
        let mut gate = PolicyGate::allow_all();
        // Set a very low threshold: 2 actions per window.
        gate.set_rate_limiter(RateLimiter::new(2, Duration::from_secs(60)));

        assert!(gate.evaluate("do_thing").is_allowed());
        assert!(gate.evaluate("do_thing").is_allowed());
        // 3rd time should be rate-limited → denied
        let decision = gate.evaluate("do_thing");
        assert!(decision.is_denied());
        assert!(decision.reason.contains("rate limited"));
        assert!(decision.matched_rule.is_none());
        assert_eq!(decision.rules_evaluated, 0);
    }
}
