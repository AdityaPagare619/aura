//! Goal registry and capability catalog — what AURA can do.
//!
//! The registry maintains a bounded set of capabilities (actions AURA knows
//! how to perform) and per-app action mappings. Capability confidence is
//! updated via Bayesian inference after each execution outcome, allowing
//! AURA to learn which actions it's good at over time.

use std::collections::BTreeMap;

use aura_types::errors::GoalError;
use serde::{Deserialize, Serialize};
use tracing::instrument;

use super::BoundedMap;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum number of registered capabilities.
const MAX_CAPABILITIES: usize = 512;

/// Maximum number of tracked app packages.
const MAX_APP_PACKAGES: usize = 256;

/// Maximum number of actions per app package.
const MAX_ACTIONS_PER_APP: usize = 64;

/// Bayesian prior pseudo-count (controls how quickly confidence changes).
const BAYESIAN_PRIOR_ALPHA: f32 = 2.0;
const BAYESIAN_PRIOR_BETA: f32 = 1.0;

/// Minimum confidence floor — never drop below this.
const MIN_CONFIDENCE: f32 = 0.05;

/// Maximum confidence ceiling.
const MAX_CONFIDENCE: f32 = 0.99;

/// Maximum number of goal templates.
const MAX_TEMPLATES: usize = 128;

/// Maximum number of parameters per template.
const MAX_PARAMS_PER_TEMPLATE: usize = 16;

/// Maximum number of learned templates.
const MAX_LEARNED_TEMPLATES: usize = 64;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// A single capability that AURA knows how to perform.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Capability {
    /// Unique identifier (e.g. "send_whatsapp_message").
    pub id: String,
    /// Human-readable name.
    pub name: String,
    /// Description of what this capability does.
    pub description: String,
    /// Android permissions required (e.g. "BIND_ACCESSIBILITY_SERVICE").
    pub required_permissions: Vec<String>,
    /// App packages this capability works with.
    pub supported_apps: Vec<String>,
    /// Confidence score (0.0–1.0) — how reliably AURA can perform this.
    pub confidence: f32,
    /// Timestamp of last successful use (epoch ms).
    pub last_used: Option<u64>,
    /// Running success/failure counts for Bayesian updates.
    pub success_count: u32,
    /// Running failure count for Bayesian updates.
    pub failure_count: u32,
}

impl Capability {
    /// Compute the success rate from historical execution data.
    #[must_use]
    pub fn success_rate(&self) -> f32 {
        let total = self.success_count + self.failure_count;
        if total == 0 {
            return self.confidence; // Use prior confidence if no data.
        }
        self.success_count as f32 / total as f32
    }
}

/// An action that AURA can perform within a specific app.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppAction {
    /// Action identifier (e.g. "tap_send_button").
    pub id: String,
    /// Human-readable description.
    pub description: String,
    /// The capability this action is part of.
    pub capability_id: String,
    /// Estimated duration in milliseconds.
    pub estimated_duration_ms: u32,
    /// Whether an ETG path exists for this action.
    pub has_etg_path: bool,
}

/// The goal registry — tracks what AURA can do and how well.
///
/// Capabilities are stored in a `BTreeMap` for deterministic ordering.
/// App actions are stored per-package with bounded inner vecs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GoalRegistry {
    /// All registered capabilities, keyed by capability ID.
    capabilities: BTreeMap<String, Capability>,
    /// Per-package app actions (package_name → actions).
    app_actions: BoundedMap<String, Vec<AppAction>, MAX_APP_PACKAGES>,
    /// Maximum capabilities this registry will hold.
    max_capabilities: usize,
    /// Goal templates — reusable patterns for common goals.
    templates: Vec<GoalTemplate>,
    /// Learned templates from successful completions.
    learned_templates: Vec<GoalTemplate>,
}

/// Result of a capability query — did we find a matching capability?
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapabilityMatch {
    /// The matched capability.
    pub capability: Capability,
    /// Relevance score (0.0–1.0) combining text match + confidence.
    pub relevance: f32,
}

// ---------------------------------------------------------------------------
// Goal Template types
// ---------------------------------------------------------------------------

/// Well-known goal template categories.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum GoalTemplateKind {
    /// Send a message (WhatsApp, SMS, Telegram, etc.).
    SendMessage,
    /// Set an alarm or timer.
    SetAlarm,
    /// Perform a web search.
    SearchWeb,
    /// Navigate to a location.
    NavigateTo,
    /// Take a photo or screenshot.
    TakePhoto,
    /// Install an app from the store.
    InstallApp,
    /// Custom / user-defined template.
    Custom(String),
}

/// Type of a template parameter for validation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ParamType {
    /// Free-form text.
    Text,
    /// Integer number.
    Integer,
    /// Floating-point number.
    Float,
    /// Boolean flag.
    Boolean,
    /// One of a set of allowed values.
    Enum(Vec<String>),
    /// Phone number pattern.
    PhoneNumber,
    /// URL.
    Url,
    /// Time of day (HH:MM).
    TimeOfDay,
}

/// A typed parameter definition for a goal template.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TemplateParam {
    /// Parameter name (e.g. "recipient", "message_body", "time").
    pub name: String,
    /// Human-readable description.
    pub description: String,
    /// Expected type.
    pub param_type: ParamType,
    /// Whether this parameter is required.
    pub required: bool,
    /// Default value (if any).
    pub default_value: Option<String>,
}

/// A goal template — a reusable pattern for common goals.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GoalTemplate {
    /// Template kind / category.
    pub kind: GoalTemplateKind,
    /// Human-readable name (e.g. "Send WhatsApp Message").
    pub name: String,
    /// Template description.
    pub description: String,
    /// Keywords used for matching user intent to this template.
    pub keywords: Vec<String>,
    /// Required capabilities (references to Capability IDs).
    pub required_capabilities: Vec<String>,
    /// Typed parameter definitions.
    pub params: Vec<TemplateParam>,
    /// Priority hint for the created goal.
    pub default_priority: String,
    /// How many times this template has been used successfully.
    pub usage_count: u32,
    /// Success rate when this template is used (0.0–1.0).
    pub success_rate: f32,
    /// Whether this template was learned from successful completions.
    pub learned: bool,
}

/// Result of template matching against user intent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TemplateMatch {
    /// The matched template.
    pub template: GoalTemplate,
    /// Match score (0.0–1.0).
    pub score: f32,
    /// Which parameters were extracted from the intent text.
    pub extracted_params: Vec<(String, String)>,
}

/// Validation result for template parameters.
#[derive(Debug, Clone)]
pub struct ParamValidationResult {
    /// Whether all required params are present and valid.
    pub valid: bool,
    /// Missing required parameters.
    pub missing: Vec<String>,
    /// Parameters with invalid types/values.
    pub invalid: Vec<(String, String)>,
}

// ---------------------------------------------------------------------------
// Implementation
// ---------------------------------------------------------------------------

impl GoalRegistry {
    /// Create a new empty registry.
    pub fn new() -> Self {
        Self {
            capabilities: BTreeMap::new(),
            app_actions: BoundedMap::new(),
            max_capabilities: MAX_CAPABILITIES,
            templates: Vec::new(),
            learned_templates: Vec::new(),
        }
    }

    /// Register a new capability. Returns error if at capacity.
    #[instrument(skip(self), fields(cap_id = %capability.id))]
    pub fn register_capability(&mut self, capability: Capability) -> Result<(), GoalError> {
        if self.capabilities.len() >= self.max_capabilities
            && !self.capabilities.contains_key(&capability.id)
        {
            return Err(GoalError::CapacityExceeded {
                max: self.max_capabilities,
            });
        }
        tracing::debug!(
            capability_id = %capability.id,
            confidence = capability.confidence,
            "registering capability"
        );
        self.capabilities.insert(capability.id.clone(), capability);
        Ok(())
    }

    /// Register an app action under a package. Returns error if at capacity.
    #[instrument(skip(self), fields(package = %package, action_id = %action.id))]
    pub fn register_app_action(
        &mut self,
        package: String,
        action: AppAction,
    ) -> Result<(), GoalError> {
        if let Some(actions) = self.app_actions.get_mut(&package) {
            if actions.len() >= MAX_ACTIONS_PER_APP {
                return Err(GoalError::CapacityExceeded {
                    max: MAX_ACTIONS_PER_APP,
                });
            }
            actions.push(action);
        } else {
            let actions = vec![action];
            self.app_actions.try_insert(package, actions).map_err(|_| {
                GoalError::CapacityExceeded {
                    max: MAX_APP_PACKAGES,
                }
            })?;
        }
        Ok(())
    }

    /// Query: "Can AURA do X?" — find matching capabilities by keyword search.
    ///
    /// Returns matches sorted by relevance (highest first), up to `max_results`.
    #[instrument(skip(self))]
    pub fn find_capabilities(&self, query: &str, max_results: usize) -> Vec<CapabilityMatch> {
        let query_lower = query.to_ascii_lowercase();
        let query_words: Vec<&str> = query_lower.split_whitespace().collect();

        let mut matches: Vec<CapabilityMatch> = self
            .capabilities
            .values()
            .filter_map(|cap| {
                let relevance = Self::compute_relevance(cap, &query_words);
                if relevance > 0.1 {
                    Some(CapabilityMatch {
                        capability: cap.clone(),
                        relevance,
                    })
                } else {
                    None
                }
            })
            .collect();

        // Sort by relevance descending.
        matches.sort_by(|a, b| {
            b.relevance
                .partial_cmp(&a.relevance)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        matches.truncate(max_results);

        tracing::debug!(
            query = %query,
            matches_found = matches.len(),
            "capability search completed"
        );

        matches
    }

    /// Get a specific capability by ID.
    pub fn get_capability(&self, id: &str) -> Option<&Capability> {
        self.capabilities.get(id)
    }

    /// Get all app actions for a specific package.
    pub fn get_app_actions(&self, package: &str) -> Option<&Vec<AppAction>> {
        self.app_actions.get(&package.to_string())
    }

    /// Update capability confidence after an execution outcome using Bayesian
    /// posterior update.
    ///
    /// Uses Beta-Binomial model:
    ///   posterior = (alpha + successes) / (alpha + beta + total)
    ///
    /// where alpha = prior successes, beta = prior failures.
    #[instrument(skip(self))]
    pub fn update_confidence(
        &mut self,
        capability_id: &str,
        succeeded: bool,
        timestamp_ms: u64,
    ) -> Result<f32, GoalError> {
        let cap = self
            .capabilities
            .get_mut(capability_id)
            .ok_or_else(|| GoalError::NoCapability(capability_id.to_string()))?;

        if succeeded {
            cap.success_count = cap.success_count.saturating_add(1);
            cap.last_used = Some(timestamp_ms);
        } else {
            cap.failure_count = cap.failure_count.saturating_add(1);
        }

        // Beta-Binomial posterior.
        let alpha = BAYESIAN_PRIOR_ALPHA + cap.success_count as f32;
        let beta = BAYESIAN_PRIOR_BETA + cap.failure_count as f32;
        let new_confidence = (alpha / (alpha + beta)).clamp(MIN_CONFIDENCE, MAX_CONFIDENCE);

        let old_confidence = cap.confidence;
        cap.confidence = new_confidence;

        tracing::debug!(
            capability_id = %capability_id,
            old_confidence = old_confidence,
            new_confidence = new_confidence,
            success_count = cap.success_count,
            failure_count = cap.failure_count,
            "confidence updated"
        );

        Ok(new_confidence)
    }

    /// Decay confidence for capabilities not used recently.
    ///
    /// Capabilities unused for longer than `stale_threshold_ms` get their
    /// confidence multiplied by `decay_factor`.
    #[instrument(skip(self))]
    pub fn decay_stale_capabilities(
        &mut self,
        now_ms: u64,
        stale_threshold_ms: u64,
        decay_factor: f32,
    ) -> usize {
        let mut decayed_count = 0usize;
        for cap in self.capabilities.values_mut() {
            if let Some(last) = cap.last_used {
                if now_ms.saturating_sub(last) > stale_threshold_ms {
                    cap.confidence = (cap.confidence * decay_factor).max(MIN_CONFIDENCE);
                    decayed_count += 1;
                }
            }
        }
        tracing::debug!(decayed = decayed_count, "stale capability decay pass");
        decayed_count
    }

    /// Total number of registered capabilities.
    #[must_use]
    pub fn capability_count(&self) -> usize {
        self.capabilities.len()
    }

    /// Total number of tracked app packages.
    #[must_use]
    pub fn app_package_count(&self) -> usize {
        self.app_actions.len()
    }

    // -----------------------------------------------------------------------
    // Goal template management
    // -----------------------------------------------------------------------

    /// Register a goal template. Returns error if at capacity.
    #[instrument(skip(self), fields(template_name = %template.name))]
    pub fn register_template(&mut self, template: GoalTemplate) -> Result<(), GoalError> {
        if self.templates.len() >= MAX_TEMPLATES {
            return Err(GoalError::CapacityExceeded { max: MAX_TEMPLATES });
        }

        tracing::debug!(
            name = %template.name,
            kind = ?template.kind,
            "registering goal template"
        );
        self.templates.push(template);
        Ok(())
    }

    /// Register built-in templates for common goal types.
    pub fn register_builtin_templates(&mut self) {
        let builtins = vec![
            GoalTemplate {
                kind: GoalTemplateKind::SendMessage,
                name: "Send Message".to_string(),
                description: "Send a message to a contact via messaging app".to_string(),
                keywords: vec![
                    "send".into(),
                    "message".into(),
                    "text".into(),
                    "tell".into(),
                    "whatsapp".into(),
                    "sms".into(),
                    "telegram".into(),
                    "chat".into(),
                ],
                required_capabilities: vec!["send_message".to_string()],
                params: vec![
                    TemplateParam {
                        name: "recipient".to_string(),
                        description: "Who to send the message to".to_string(),
                        param_type: ParamType::Text,
                        required: true,
                        default_value: None,
                    },
                    TemplateParam {
                        name: "message_body".to_string(),
                        description: "The message content".to_string(),
                        param_type: ParamType::Text,
                        required: true,
                        default_value: None,
                    },
                    TemplateParam {
                        name: "app".to_string(),
                        description: "Which messaging app to use".to_string(),
                        param_type: ParamType::Enum(vec![
                            "whatsapp".into(),
                            "sms".into(),
                            "telegram".into(),
                        ]),
                        required: false,
                        default_value: Some("whatsapp".to_string()),
                    },
                ],
                default_priority: "Medium".to_string(),
                usage_count: 0,
                success_rate: 0.0,
                learned: false,
            },
            GoalTemplate {
                kind: GoalTemplateKind::SetAlarm,
                name: "Set Alarm".to_string(),
                description: "Set an alarm or timer".to_string(),
                keywords: vec![
                    "alarm".into(),
                    "timer".into(),
                    "remind".into(),
                    "wake".into(),
                    "set".into(),
                    "schedule".into(),
                ],
                required_capabilities: vec!["set_alarm".to_string()],
                params: vec![
                    TemplateParam {
                        name: "time".to_string(),
                        description: "When the alarm should go off".to_string(),
                        param_type: ParamType::TimeOfDay,
                        required: true,
                        default_value: None,
                    },
                    TemplateParam {
                        name: "label".to_string(),
                        description: "Label for the alarm".to_string(),
                        param_type: ParamType::Text,
                        required: false,
                        default_value: None,
                    },
                ],
                default_priority: "High".to_string(),
                usage_count: 0,
                success_rate: 0.0,
                learned: false,
            },
            GoalTemplate {
                kind: GoalTemplateKind::SearchWeb,
                name: "Web Search".to_string(),
                description: "Search the web for information".to_string(),
                keywords: vec![
                    "search".into(),
                    "google".into(),
                    "look up".into(),
                    "find".into(),
                    "web".into(),
                    "browse".into(),
                    "query".into(),
                ],
                required_capabilities: vec!["web_search".to_string()],
                params: vec![TemplateParam {
                    name: "query".to_string(),
                    description: "What to search for".to_string(),
                    param_type: ParamType::Text,
                    required: true,
                    default_value: None,
                }],
                default_priority: "Medium".to_string(),
                usage_count: 0,
                success_rate: 0.0,
                learned: false,
            },
            GoalTemplate {
                kind: GoalTemplateKind::NavigateTo,
                name: "Navigate To".to_string(),
                description: "Get directions to a location".to_string(),
                keywords: vec![
                    "navigate".into(),
                    "directions".into(),
                    "maps".into(),
                    "go to".into(),
                    "drive".into(),
                    "route".into(),
                    "location".into(),
                ],
                required_capabilities: vec!["navigate".to_string()],
                params: vec![TemplateParam {
                    name: "destination".to_string(),
                    description: "Where to navigate to".to_string(),
                    param_type: ParamType::Text,
                    required: true,
                    default_value: None,
                }],
                default_priority: "High".to_string(),
                usage_count: 0,
                success_rate: 0.0,
                learned: false,
            },
            GoalTemplate {
                kind: GoalTemplateKind::TakePhoto,
                name: "Take Photo".to_string(),
                description: "Take a photo with the camera".to_string(),
                keywords: vec![
                    "photo".into(),
                    "picture".into(),
                    "camera".into(),
                    "capture".into(),
                    "snap".into(),
                    "selfie".into(),
                ],
                required_capabilities: vec!["take_photo".to_string()],
                params: vec![TemplateParam {
                    name: "camera".to_string(),
                    description: "Which camera to use".to_string(),
                    param_type: ParamType::Enum(vec!["back".into(), "front".into()]),
                    required: false,
                    default_value: Some("back".to_string()),
                }],
                default_priority: "Medium".to_string(),
                usage_count: 0,
                success_rate: 0.0,
                learned: false,
            },
            GoalTemplate {
                kind: GoalTemplateKind::InstallApp,
                name: "Install App".to_string(),
                description: "Install an application from the store".to_string(),
                keywords: vec![
                    "install".into(),
                    "download".into(),
                    "app".into(),
                    "play store".into(),
                    "get".into(),
                    "application".into(),
                ],
                required_capabilities: vec!["install_app".to_string()],
                params: vec![TemplateParam {
                    name: "app_name".to_string(),
                    description: "Name of the app to install".to_string(),
                    param_type: ParamType::Text,
                    required: true,
                    default_value: None,
                }],
                default_priority: "Low".to_string(),
                usage_count: 0,
                success_rate: 0.0,
                learned: false,
            },
        ];

        for t in builtins {
            // Ignore capacity errors for builtins — they should always fit.
            let _ = self.register_template(t);
        }
    }

    // -- Template matching helpers -------------------------------------------

    /// Score how well a template matches a set of intent words.
    ///
    /// Returns 0.0–1.0 based on keyword overlap.
    fn template_match_score(template: &GoalTemplate, intent_words: &[&str]) -> f32 {
        if template.keywords.is_empty() || intent_words.is_empty() {
            return 0.0;
        }
        let hits = template
            .keywords
            .iter()
            .filter(|kw| {
                let kw_lower = kw.to_ascii_lowercase();
                intent_words.iter().any(|w| w.contains(kw_lower.as_str()))
            })
            .count();
        hits as f32 / template.keywords.len() as f32
    }

    /// Extract parameter values from intent text based on a template's definitions.
    ///
    /// Uses a very simple heuristic: if the intent text contains text after a
    /// keyword that looks like a param value, capture it. This is intentionally
    /// simplistic — the full NLU pipeline refines parameters later.
    fn extract_params_from_intent(
        template: &GoalTemplate,
        intent_lower: &str,
    ) -> Vec<(String, String)> {
        let mut extracted = Vec::new();
        for def in &template.params {
            // Try to find the param name mentioned in the intent text and capture what follows.
            let name_lower = def.name.to_ascii_lowercase().replace('_', " ");
            if let Some(pos) = intent_lower.find(&name_lower) {
                let after = &intent_lower[pos + name_lower.len()..];
                let value = after
                    .trim()
                    .split_whitespace()
                    .take(5)
                    .collect::<Vec<_>>()
                    .join(" ");
                if !value.is_empty() {
                    extracted.push((def.name.clone(), value));
                }
            } else if let Some(ref default) = def.default_value {
                extracted.push((def.name.clone(), default.clone()));
            }
        }
        extracted
    }

    /// Validate that a string value matches the expected parameter type.
    ///
    /// Returns `Some(error_message)` if validation fails, or `None` if ok.
    fn validate_param_type(param_type: &ParamType, value: &str) -> Option<String> {
        match param_type {
            ParamType::Text => None, // Any text is valid.
            ParamType::Integer => value
                .parse::<i64>()
                .err()
                .map(|_| format!("expected integer, got '{}'", value)),
            ParamType::Float => value
                .parse::<f64>()
                .err()
                .map(|_| format!("expected float, got '{}'", value)),
            ParamType::Boolean => {
                if matches!(value, "true" | "false" | "yes" | "no" | "1" | "0") {
                    None
                } else {
                    Some(format!("expected boolean, got '{}'", value))
                }
            }
            ParamType::Enum(variants) => {
                if variants.iter().any(|v| v.eq_ignore_ascii_case(value)) {
                    None
                } else {
                    Some(format!("expected one of {:?}, got '{}'", variants, value))
                }
            }
            ParamType::PhoneNumber => {
                let digits: String = value.chars().filter(|c| c.is_ascii_digit()).collect();
                if digits.len() >= 7 {
                    None
                } else {
                    Some(format!("invalid phone number: '{}'", value))
                }
            }
            ParamType::Url => {
                if value.starts_with("http://") || value.starts_with("https://") {
                    None
                } else {
                    Some(format!(
                        "expected URL starting with http(s)://, got '{}'",
                        value
                    ))
                }
            }
            ParamType::TimeOfDay => {
                // Accept HH:MM or H:MM patterns.
                let parts: Vec<&str> = value.split(':').collect();
                if parts.len() == 2
                    && parts[0].parse::<u32>().map_or(false, |h| h < 24)
                    && parts[1].parse::<u32>().map_or(false, |m| m < 60)
                {
                    None
                } else {
                    Some(format!("expected time HH:MM, got '{}'", value))
                }
            }
        }
    }

    /// Match user intent text against registered templates.
    ///
    /// Returns matches sorted by score (highest first), up to `max_results`.
    #[instrument(skip(self))]
    pub fn match_templates(&self, intent: &str, max_results: usize) -> Vec<TemplateMatch> {
        let intent_lower = intent.to_ascii_lowercase();
        let intent_words: Vec<&str> = intent_lower.split_whitespace().collect();

        let all_templates = self.templates.iter().chain(self.learned_templates.iter());

        let mut matches: Vec<TemplateMatch> = all_templates
            .filter_map(|t| {
                let score = Self::template_match_score(t, &intent_words);
                if score > 0.1 {
                    let extracted = Self::extract_params_from_intent(t, &intent_lower);
                    Some(TemplateMatch {
                        template: t.clone(),
                        score,
                        extracted_params: extracted,
                    })
                } else {
                    None
                }
            })
            .collect();

        matches.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        matches.truncate(max_results);

        tracing::debug!(
            intent = %intent,
            matches_found = matches.len(),
            "template matching completed"
        );

        matches
    }

    /// Validate parameters against a template's parameter definitions.
    pub fn validate_template_params(
        template: &GoalTemplate,
        params: &[(String, String)],
    ) -> ParamValidationResult {
        let mut missing = Vec::new();
        let mut invalid = Vec::new();

        for def in &template.params {
            let provided = params.iter().find(|(k, _)| k == &def.name);

            match provided {
                None if def.required && def.default_value.is_none() => {
                    missing.push(def.name.clone());
                }
                Some((_, value)) => {
                    if let Some(err) = Self::validate_param_type(&def.param_type, value) {
                        invalid.push((def.name.clone(), err));
                    }
                }
                _ => {} // Optional or has default — ok.
            }
        }

        ParamValidationResult {
            valid: missing.is_empty() && invalid.is_empty(),
            missing,
            invalid,
        }
    }

    /// Learn a new template from a successful goal completion.
    ///
    /// Extracts the pattern from the goal description and step structure
    /// and registers it as a learned template.
    #[instrument(skip(self))]
    pub fn learn_template_from_completion(
        &mut self,
        description: &str,
        keywords: Vec<String>,
        params: Vec<TemplateParam>,
        required_capabilities: Vec<String>,
    ) -> Result<(), GoalError> {
        if self.learned_templates.len() >= MAX_LEARNED_TEMPLATES {
            // Evict the least-used learned template.
            if let Some(min_idx) = self
                .learned_templates
                .iter()
                .enumerate()
                .min_by_key(|(_, t)| t.usage_count)
                .map(|(i, _)| i)
            {
                self.learned_templates.remove(min_idx);
            }
        }

        let template = GoalTemplate {
            kind: GoalTemplateKind::Custom(description.to_string()),
            name: description.to_string(),
            description: format!("Learned from successful completion: {}", description),
            keywords,
            required_capabilities,
            params,
            default_priority: "Medium".to_string(),
            usage_count: 1,
            success_rate: 1.0,
            learned: true,
        };

        self.learned_templates.push(template);
        tracing::info!(description = %description, "learned new template from completion");
        Ok(())
    }

    /// Record a template usage outcome (for tracking success rate).
    pub fn record_template_usage(&mut self, template_name: &str, succeeded: bool) {
        let find_and_update = |templates: &mut [GoalTemplate]| {
            if let Some(t) = templates.iter_mut().find(|t| t.name == template_name) {
                t.usage_count = t.usage_count.saturating_add(1);
                let total = t.usage_count as f32;
                if succeeded {
                    // Incremental average: new_rate = old_rate + (1 - old_rate) / total
                    t.success_rate += (1.0 - t.success_rate) / total;
                } else {
                    // Incremental average: new_rate = old_rate - old_rate / total
                    t.success_rate -= t.success_rate / total;
                }
                t.success_rate = t.success_rate.clamp(0.0, 1.0);
                return true;
            }
            false
        };

        if !find_and_update(&mut self.templates) {
            find_and_update(&mut self.learned_templates);
        }
    }

    /// Get the total number of registered templates (built-in + learned).
    #[must_use]
    pub fn template_count(&self) -> usize {
        self.templates.len() + self.learned_templates.len()
    }

    /// Get all templates of a specific kind.
    pub fn templates_by_kind(&self, kind: &GoalTemplateKind) -> Vec<&GoalTemplate> {
        self.templates
            .iter()
            .chain(self.learned_templates.iter())
            .filter(|t| &t.kind == kind)
            .collect()
    }

    // -- Private helpers ----------------------------------------------------

    /// Compute relevance score between a capability and query words.
    ///
    /// Combines keyword match ratio with capability confidence.
    fn compute_relevance(cap: &Capability, query_words: &[&str]) -> f32 {
        if query_words.is_empty() {
            return 0.0;
        }

        let cap_text = format!(
            "{} {} {}",
            cap.id.to_ascii_lowercase(),
            cap.name.to_ascii_lowercase(),
            cap.description.to_ascii_lowercase()
        );

        let matched = query_words
            .iter()
            .filter(|w| cap_text.contains(**w))
            .count();

        let word_match_ratio = matched as f32 / query_words.len() as f32;

        // Relevance = 70% text match + 30% confidence.
        word_match_ratio * 0.70 + cap.confidence * 0.30
    }
}

impl Default for GoalRegistry {
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

    fn make_capability(id: &str, name: &str, confidence: f32) -> Capability {
        Capability {
            id: id.to_string(),
            name: name.to_string(),
            description: format!("Capability for {}", name),
            required_permissions: vec!["BIND_ACCESSIBILITY_SERVICE".to_string()],
            supported_apps: vec!["com.example".to_string()],
            confidence,
            last_used: None,
            success_count: 0,
            failure_count: 0,
        }
    }

    #[test]
    fn test_register_and_retrieve_capability() {
        let mut reg = GoalRegistry::new();
        let cap = make_capability("send_msg", "Send Message", 0.8);
        assert!(reg.register_capability(cap).is_ok());
        assert_eq!(reg.capability_count(), 1);

        let found = reg.get_capability("send_msg");
        assert!(found.is_some());
        assert_eq!(found.map(|c| &c.name), Some(&"Send Message".to_string()));
    }

    #[test]
    fn test_find_capabilities_by_query() {
        let mut reg = GoalRegistry::new();
        reg.register_capability(make_capability(
            "send_whatsapp",
            "Send WhatsApp Message",
            0.9,
        ))
        .ok();
        reg.register_capability(make_capability("open_camera", "Open Camera", 0.7))
            .ok();
        reg.register_capability(make_capability("send_email", "Send Email", 0.85))
            .ok();

        let matches = reg.find_capabilities("send message", 10);
        assert!(!matches.is_empty());
        // "send" should match both send_whatsapp and send_email.
        assert!(matches.len() >= 2);
        // Highest relevance first.
        assert!(matches[0].relevance >= matches[1].relevance);
    }

    #[test]
    fn test_bayesian_confidence_update_success() {
        let mut reg = GoalRegistry::new();
        reg.register_capability(make_capability("test_cap", "Test", 0.5))
            .ok();

        let new_conf = reg
            .update_confidence("test_cap", true, 1_000_000)
            .expect("update should succeed");

        // After 1 success: posterior = (2 + 1) / (2 + 1 + 0 + 1) = 3/4 = 0.75.
        // But with prior alpha=2, beta=1, 1 success:
        // alpha' = 2 + 1 = 3, beta' = 1 + 0 = 1, posterior = 3/4 = 0.75
        assert!(new_conf > 0.5, "confidence should increase: {}", new_conf);
        assert!(new_conf < 1.0);
    }

    #[test]
    fn test_bayesian_confidence_update_failure() {
        let mut reg = GoalRegistry::new();
        reg.register_capability(make_capability("test_cap", "Test", 0.8))
            .ok();

        // Multiple failures should decrease confidence.
        for i in 0..5 {
            reg.update_confidence("test_cap", false, 1_000_000 + i).ok();
        }

        let cap = reg.get_capability("test_cap").expect("should exist");
        assert!(
            cap.confidence < 0.5,
            "confidence should drop after 5 failures: {}",
            cap.confidence
        );
        assert!(
            cap.confidence >= MIN_CONFIDENCE,
            "should not drop below floor"
        );
    }

    #[test]
    fn test_confidence_update_missing_capability() {
        let mut reg = GoalRegistry::new();
        let result = reg.update_confidence("nonexistent", true, 1_000);
        assert!(result.is_err());
    }

    #[test]
    fn test_register_app_action() {
        let mut reg = GoalRegistry::new();
        let action = AppAction {
            id: "tap_send".to_string(),
            description: "Tap the send button".to_string(),
            capability_id: "send_whatsapp".to_string(),
            estimated_duration_ms: 500,
            has_etg_path: true,
        };
        assert!(reg
            .register_app_action("com.whatsapp".to_string(), action)
            .is_ok());

        let actions = reg.get_app_actions("com.whatsapp");
        assert!(actions.is_some());
        assert_eq!(actions.map(|a| a.len()), Some(1));
    }

    #[test]
    fn test_decay_stale_capabilities() {
        let mut reg = GoalRegistry::new();
        let mut cap = make_capability("old_cap", "Old Capability", 0.8);
        cap.last_used = Some(1_000);
        reg.register_capability(cap).ok();

        let decayed = reg.decay_stale_capabilities(
            100_000, // now
            50_000,  // stale if unused for >50s
            0.9,     // decay factor
        );

        assert_eq!(decayed, 1);
        let cap = reg.get_capability("old_cap").expect("should exist");
        assert!((cap.confidence - 0.72).abs() < 0.01); // 0.8 * 0.9 = 0.72
    }

    #[test]
    fn test_success_rate_calculation() {
        let mut cap = make_capability("test", "Test", 0.5);
        cap.success_count = 8;
        cap.failure_count = 2;
        assert!((cap.success_rate() - 0.8).abs() < f32::EPSILON);
    }

    // -----------------------------------------------------------------------
    // Template system tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_register_builtin_templates() {
        let mut reg = GoalRegistry::new();
        reg.register_builtin_templates();
        assert_eq!(reg.template_count(), 6, "should register all 6 builtins");

        // Verify each kind is present.
        let kinds = [
            GoalTemplateKind::SendMessage,
            GoalTemplateKind::SetAlarm,
            GoalTemplateKind::SearchWeb,
            GoalTemplateKind::NavigateTo,
            GoalTemplateKind::TakePhoto,
            GoalTemplateKind::InstallApp,
        ];
        for kind in &kinds {
            let found = reg.templates_by_kind(kind);
            assert_eq!(
                found.len(),
                1,
                "should find exactly one template for {:?}",
                kind
            );
        }
    }

    #[test]
    fn test_register_custom_template() {
        let mut reg = GoalRegistry::new();
        let t = GoalTemplate {
            kind: GoalTemplateKind::Custom("order_food".into()),
            name: "Order Food".into(),
            description: "Order food delivery".into(),
            keywords: vec!["order".into(), "food".into(), "delivery".into()],
            required_capabilities: vec!["app_control".into()],
            params: vec![TemplateParam {
                name: "restaurant".into(),
                description: "Restaurant name".into(),
                param_type: ParamType::Text,
                required: true,
                default_value: None,
            }],
            default_priority: "Medium".into(),
            usage_count: 0,
            success_rate: 0.0,
            learned: false,
        };
        assert!(reg.register_template(t).is_ok());
        assert_eq!(reg.template_count(), 1);
    }

    #[test]
    fn test_match_templates_send_message() {
        let mut reg = GoalRegistry::new();
        reg.register_builtin_templates();

        let matches = reg.match_templates("send a text message to John", 5);
        assert!(!matches.is_empty(), "should match at least one template");
        assert_eq!(
            matches[0].template.kind,
            GoalTemplateKind::SendMessage,
            "SendMessage should be the top match for 'send a text message'"
        );
        assert!(matches[0].score > 0.1);
    }

    #[test]
    fn test_match_templates_set_alarm() {
        let mut reg = GoalRegistry::new();
        reg.register_builtin_templates();

        let matches = reg.match_templates("set an alarm for 7 AM", 5);
        assert!(!matches.is_empty(), "should match alarm template");
        // SetAlarm should be a top match — has keywords "set" and "alarm".
        let alarm_match = matches
            .iter()
            .find(|m| m.template.kind == GoalTemplateKind::SetAlarm);
        assert!(
            alarm_match.is_some(),
            "SetAlarm template should appear in matches"
        );
    }

    #[test]
    fn test_match_templates_no_match() {
        let mut reg = GoalRegistry::new();
        reg.register_builtin_templates();

        let matches = reg.match_templates("quantum entanglement physics", 5);
        assert!(
            matches.is_empty(),
            "no template should match unrelated intent"
        );
    }

    #[test]
    fn test_validate_params_all_valid() {
        let template = GoalTemplate {
            kind: GoalTemplateKind::SetAlarm,
            name: "Set Alarm".into(),
            description: "test".into(),
            keywords: vec![],
            required_capabilities: vec![],
            params: vec![
                TemplateParam {
                    name: "time".into(),
                    description: "When".into(),
                    param_type: ParamType::TimeOfDay,
                    required: true,
                    default_value: None,
                },
                TemplateParam {
                    name: "label".into(),
                    description: "Label".into(),
                    param_type: ParamType::Text,
                    required: false,
                    default_value: None,
                },
            ],
            default_priority: "High".into(),
            usage_count: 0,
            success_rate: 0.0,
            learned: false,
        };

        let params = vec![("time".into(), "07:30".into())];
        let result = GoalRegistry::validate_template_params(&template, &params);
        assert!(result.valid, "should be valid with required param provided");
        assert!(result.missing.is_empty());
        assert!(result.invalid.is_empty());
    }

    #[test]
    fn test_validate_params_missing_required() {
        let template = GoalTemplate {
            kind: GoalTemplateKind::SendMessage,
            name: "Send Message".into(),
            description: "test".into(),
            keywords: vec![],
            required_capabilities: vec![],
            params: vec![
                TemplateParam {
                    name: "recipient".into(),
                    description: "Who".into(),
                    param_type: ParamType::Text,
                    required: true,
                    default_value: None,
                },
                TemplateParam {
                    name: "message_body".into(),
                    description: "What".into(),
                    param_type: ParamType::Text,
                    required: true,
                    default_value: None,
                },
            ],
            default_priority: "Medium".into(),
            usage_count: 0,
            success_rate: 0.0,
            learned: false,
        };

        // Provide only recipient, missing message_body.
        let params = vec![("recipient".into(), "Alice".into())];
        let result = GoalRegistry::validate_template_params(&template, &params);
        assert!(!result.valid);
        assert_eq!(result.missing, vec!["message_body"]);
    }

    #[test]
    fn test_validate_params_invalid_type() {
        let template = GoalTemplate {
            kind: GoalTemplateKind::Custom("test".into()),
            name: "Test".into(),
            description: "test".into(),
            keywords: vec![],
            required_capabilities: vec![],
            params: vec![
                TemplateParam {
                    name: "count".into(),
                    description: "Number".into(),
                    param_type: ParamType::Integer,
                    required: true,
                    default_value: None,
                },
                TemplateParam {
                    name: "url".into(),
                    description: "Link".into(),
                    param_type: ParamType::Url,
                    required: true,
                    default_value: None,
                },
            ],
            default_priority: "Low".into(),
            usage_count: 0,
            success_rate: 0.0,
            learned: false,
        };

        let params = vec![
            ("count".into(), "not_a_number".into()),
            ("url".into(), "just-text".into()),
        ];
        let result = GoalRegistry::validate_template_params(&template, &params);
        assert!(!result.valid);
        assert_eq!(result.invalid.len(), 2, "both params should be invalid");
    }

    #[test]
    fn test_validate_param_type_variants() {
        // Boolean
        assert!(GoalRegistry::validate_param_type(&ParamType::Boolean, "true").is_none());
        assert!(GoalRegistry::validate_param_type(&ParamType::Boolean, "yes").is_none());
        assert!(GoalRegistry::validate_param_type(&ParamType::Boolean, "maybe").is_some());

        // Phone number
        assert!(
            GoalRegistry::validate_param_type(&ParamType::PhoneNumber, "+1234567890").is_none()
        );
        assert!(GoalRegistry::validate_param_type(&ParamType::PhoneNumber, "12").is_some());

        // Enum
        let variants = vec!["a".into(), "b".into(), "c".into()];
        assert!(
            GoalRegistry::validate_param_type(&ParamType::Enum(variants.clone()), "A").is_none()
        );
        assert!(GoalRegistry::validate_param_type(&ParamType::Enum(variants), "d").is_some());

        // TimeOfDay
        assert!(GoalRegistry::validate_param_type(&ParamType::TimeOfDay, "07:30").is_none());
        assert!(GoalRegistry::validate_param_type(&ParamType::TimeOfDay, "25:00").is_some());

        // Float
        assert!(GoalRegistry::validate_param_type(&ParamType::Float, "3.14").is_none());
        assert!(GoalRegistry::validate_param_type(&ParamType::Float, "abc").is_some());
    }

    #[test]
    fn test_learn_template_from_completion() {
        let mut reg = GoalRegistry::new();

        let result = reg.learn_template_from_completion(
            "Order Uber to Airport",
            vec!["uber".into(), "ride".into(), "airport".into()],
            vec![TemplateParam {
                name: "destination".into(),
                description: "Where to go".into(),
                param_type: ParamType::Text,
                required: true,
                default_value: None,
            }],
            vec!["app_control".into()],
        );
        assert!(result.is_ok());
        assert_eq!(reg.template_count(), 1);

        // Learned template should be matchable.
        let matches = reg.match_templates("get an uber ride", 5);
        assert!(!matches.is_empty(), "learned template should be matchable");
        assert!(matches[0].template.learned, "should be marked as learned");
    }

    #[test]
    fn test_learned_template_lru_eviction() {
        let mut reg = GoalRegistry::new();

        // Fill up the learned templates to capacity.
        for i in 0..MAX_LEARNED_TEMPLATES {
            reg.learn_template_from_completion(
                &format!("task_{}", i),
                vec![format!("keyword_{}", i)],
                vec![],
                vec![],
            )
            .expect("should succeed");
        }
        assert_eq!(reg.template_count(), MAX_LEARNED_TEMPLATES);

        // Add one more — should evict the least-used (all have usage_count=1, so first added).
        reg.learn_template_from_completion(
            "task_overflow",
            vec!["overflow".into()],
            vec![],
            vec![],
        )
        .expect("should evict and succeed");
        assert_eq!(reg.template_count(), MAX_LEARNED_TEMPLATES);
    }

    #[test]
    fn test_record_template_usage_success_and_failure() {
        let mut reg = GoalRegistry::new();
        reg.register_builtin_templates();

        // Record successes.
        reg.record_template_usage("Send Message", true);
        reg.record_template_usage("Send Message", true);
        reg.record_template_usage("Send Message", false);

        let send_templates = reg.templates_by_kind(&GoalTemplateKind::SendMessage);
        assert_eq!(send_templates.len(), 1);
        let t = send_templates[0];
        assert_eq!(t.usage_count, 3);
        // After 2 successes, 1 failure the rate should be between 0.0 and 1.0.
        assert!(
            t.success_rate > 0.0 && t.success_rate < 1.0,
            "success_rate should be between 0 and 1: {}",
            t.success_rate
        );
    }

    #[test]
    fn test_templates_by_kind_returns_empty_for_unknown() {
        let mut reg = GoalRegistry::new();
        reg.register_builtin_templates();

        let custom = reg.templates_by_kind(&GoalTemplateKind::Custom("nonexistent".into()));
        assert!(
            custom.is_empty(),
            "should return empty for unknown custom kind"
        );
    }
}
