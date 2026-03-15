//! GBNF grammar definitions for constrained LLM output.
//!
//! Layer 0 of the teacher structure stack: grammar-constrained generation.
//! Each `InferenceMode` gets a GBNF grammar that ensures the model produces
//! syntactically valid output. Conversational mode uses no grammar (free text).
//!
//! Phase 3: this entire module is wired when `aura_llama_sys` exposes the
//! grammar sampling API (`llama_sampling_grammar`). Until then all items are
//! intentionally dead scaffolding.
#![allow(dead_code)]
//!
//! GBNF is the grammar format used by llama.cpp for constraining token
//! generation. It is a BNF-like DSL that defines the shape of valid outputs.
//! The model can only generate tokens that match the grammar at each step.

use aura_types::ipc::InferenceMode;

// ─── Grammar kind ───────────────────────────────────────────────────────────

/// Which grammar to apply during generation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GrammarKind {
    /// Structured JSON action plan (Planner/Strategist).
    ActionPlan,
    /// Structured JSON DSL steps array (Composer).
    DslSteps,
    /// Chain-of-thought wrapper: thinking block + action block.
    ChainOfThought,
    /// Brainstem reflection verdict (Layer 4 cross-model check).
    ReflectionVerdict,
    /// Confidence self-assessment (Layer 2 calibration).
    ConfidenceAssessment,
    /// Free text — no grammar constraint (Conversational).
    FreeText,
}

impl GrammarKind {
    /// Select the appropriate grammar for an inference mode.
    ///
    /// Returns `None` for `Conversational` — that mode uses free-text generation
    /// with no grammar constraint. All structured modes return `Some(grammar)`.
    ///
    /// Callers that previously matched against `GrammarKind::FreeText` should
    /// instead handle the `None` case to skip grammar application entirely.
    #[must_use]
    pub fn for_mode(mode: InferenceMode) -> Option<Self> {
        match mode {
            InferenceMode::Planner | InferenceMode::Strategist => Some(GrammarKind::ActionPlan),
            InferenceMode::Composer => Some(GrammarKind::DslSteps),
            // Conversational produces free text — no grammar constraint.
            InferenceMode::Conversational => None,
        }
    }

    /// Whether this grammar kind actually constrains output.
    #[must_use]
    pub fn is_constrained(&self) -> bool {
        !matches!(self, GrammarKind::FreeText)
    }
}

// ─── Compiled grammar ───────────────────────────────────────────────────────

/// A compiled GBNF grammar ready to pass to llama.cpp.
///
/// Contains both the raw GBNF string (for debugging/logging) and
/// the compiled grammar pointer (on Android). On host stubs, the
/// pointer is null and grammar enforcement is skipped.
#[derive(Debug, Clone)]
pub struct CompiledGrammar {
    /// The GBNF source string.
    pub source: String,
    /// Which kind of grammar this is.
    pub kind: GrammarKind,
}

impl CompiledGrammar {
    /// Create a compiled grammar from a GBNF source string.
    #[must_use]
    pub fn new(kind: GrammarKind, source: String) -> Self {
        Self { source, kind }
    }

    /// Whether this grammar actually constrains output.
    #[must_use]
    pub fn is_constrained(&self) -> bool {
        self.kind.is_constrained()
    }
}

// ─── Grammar compilation ────────────────────────────────────────────────────

/// Compile a grammar for the given kind.
///
/// Returns `None` for `FreeText` (no grammar needed).
/// Returns `Some(CompiledGrammar)` for all constrained modes.
#[tracing::instrument(level = "debug")]
pub fn compile_grammar(kind: GrammarKind) -> Option<CompiledGrammar> {
    if !kind.is_constrained() {
        return None;
    }

    let source = match kind {
        GrammarKind::ActionPlan => action_plan_grammar(),
        GrammarKind::DslSteps => dsl_steps_grammar(),
        GrammarKind::ChainOfThought => chain_of_thought_grammar(),
        GrammarKind::ReflectionVerdict => reflection_verdict_grammar(),
        GrammarKind::ConfidenceAssessment => confidence_assessment_grammar(),
        GrammarKind::FreeText => unreachable!("filtered above"),
    };

    Some(CompiledGrammar::new(kind, source))
}

/// Compile a grammar for an inference mode.
///
/// Returns `None` for `Conversational` (no grammar needed) and for any mode
/// that maps to `None` via `GrammarKind::for_mode`. Delegates to
/// `compile_grammar` for all structured modes.
#[tracing::instrument(level = "debug")]
pub fn grammar_for_mode(mode: InferenceMode) -> Option<CompiledGrammar> {
    compile_grammar(GrammarKind::for_mode(mode)?)
}

// ─── GBNF grammar definitions ──────────────────────────────────────────────
//
// These produce GBNF strings compatible with llama.cpp's grammar sampler.
// GBNF syntax reference:
//   rule-name ::= expression
//   Terminals: "literal" or [a-z] character classes
//   Alternation: expr1 | expr2
//   Repetition: expr* (0+), expr+ (1+), expr? (0 or 1)
//   Grouping: ( expr )
//
// We keep grammars relatively loose to avoid over-constraining the model
// while still ensuring valid JSON structure and required fields.

/// GBNF grammar for ActionPlan JSON output.
///
/// Enforces:
/// - Top-level JSON object with required fields
/// - `goal_description`: string
/// - `steps`: array of step objects
/// - `estimated_duration_ms`: number
/// - `confidence`: number between 0 and 1
///
/// Each step object must have: action, target (nullable), timeout_ms,
/// on_failure, label (nullable).
fn action_plan_grammar() -> String {
    r#"
root ::= "{" ws plan-body ws "}"

plan-body ::= goal-field "," ws steps-field "," ws duration-field "," ws confidence-field

goal-field ::= "\"goal_description\"" ws ":" ws string
steps-field ::= "\"steps\"" ws ":" ws "[" ws step-list? ws "]"
duration-field ::= "\"estimated_duration_ms\"" ws ":" ws integer
confidence-field ::= "\"confidence\"" ws ":" ws number

step-list ::= step ( "," ws step )*
step ::= "{" ws step-body ws "}"
step-body ::= action-field "," ws target-field "," ws timeout-field "," ws failure-field ( "," ws label-field )?  ( "," ws precondition-field )? ( "," ws postcondition-field )?

action-field ::= "\"action\"" ws ":" ws action-value
target-field ::= "\"target\"" ws ":" ws ( "null" | target-value )
timeout-field ::= "\"timeout_ms\"" ws ":" ws integer
failure-field ::= "\"on_failure\"" ws ":" ws failure-value
label-field ::= "\"label\"" ws ":" ws ( "null" | string )
precondition-field ::= "\"precondition\"" ws ":" ws ( "null" | json-value )
postcondition-field ::= "\"postcondition\"" ws ":" ws ( "null" | json-value )

action-value ::= json-object
target-value ::= json-object
failure-value ::= json-object | string

json-value ::= string | integer | number | "null" | "true" | "false" | json-object | json-array
json-object ::= "{" ws ( json-pair ( "," ws json-pair )* )? ws "}"
json-pair ::= string ws ":" ws json-value
json-array ::= "[" ws ( json-value ( "," ws json-value )* )? ws "]"

string ::= "\"" char* "\""
char ::= [^"\\] | "\\" escape-char
escape-char ::= ["\\/bfnrt] | "u" hex hex hex hex
hex ::= [0-9a-fA-F]

integer ::= "0" | [1-9] [0-9]*
number ::= integer ( "." [0-9]+ )? ( [eE] [+-]? [0-9]+ )?

ws ::= [ \t\n\r]*
"#
    .trim()
    .to_string()
}

/// GBNF grammar for DSL steps array output (Composer mode).
///
/// Enforces a JSON array of DslStep objects.
fn dsl_steps_grammar() -> String {
    r#"
root ::= "[" ws step-list? ws "]"

step-list ::= step ( "," ws step )*
step ::= "{" ws step-body ws "}"
step-body ::= action-field "," ws target-field "," ws timeout-field "," ws failure-field ( "," ws label-field )? ( "," ws precondition-field )? ( "," ws postcondition-field )?

action-field ::= "\"action\"" ws ":" ws action-value
target-field ::= "\"target\"" ws ":" ws ( "null" | target-value )
timeout-field ::= "\"timeout_ms\"" ws ":" ws integer
failure-field ::= "\"on_failure\"" ws ":" ws failure-value
label-field ::= "\"label\"" ws ":" ws ( "null" | string )
precondition-field ::= "\"precondition\"" ws ":" ws ( "null" | json-value )
postcondition-field ::= "\"postcondition\"" ws ":" ws ( "null" | json-value )

action-value ::= json-object | string
target-value ::= json-object
failure-value ::= json-object | string

json-value ::= string | integer | number | "null" | "true" | "false" | json-object | json-array
json-object ::= "{" ws ( json-pair ( "," ws json-pair )* )? ws "}"
json-pair ::= string ws ":" ws json-value
json-array ::= "[" ws ( json-value ( "," ws json-value )* )? ws "]"

string ::= "\"" char* "\""
char ::= [^"\\] | "\\" escape-char
escape-char ::= ["\\/bfnrt] | "u" hex hex hex hex
hex ::= [0-9a-fA-F]

integer ::= "0" | [1-9] [0-9]*
number ::= integer ( "." [0-9]+ )? ( [eE] [+-]? [0-9]+ )?

ws ::= [ \t\n\r]*
"#
    .trim()
    .to_string()
}

/// GBNF grammar for chain-of-thought output (Layer 1).
///
/// Forces the model to produce structured thinking before action:
/// ```json
/// {
///   "thinking": "step by step reasoning...",
///   "action": { ... the actual response ... }
/// }
/// ```
fn chain_of_thought_grammar() -> String {
    r#"
root ::= "{" ws thinking-field "," ws action-field ws "}"

thinking-field ::= "\"thinking\"" ws ":" ws string
action-field ::= "\"action\"" ws ":" ws json-value

json-value ::= string | integer | number | "null" | "true" | "false" | json-object | json-array
json-object ::= "{" ws ( json-pair ( "," ws json-pair )* )? ws "}"
json-pair ::= string ws ":" ws json-value
json-array ::= "[" ws ( json-value ( "," ws json-value )* )? ws "]"

string ::= "\"" char* "\""
char ::= [^"\\] | "\\" escape-char
escape-char ::= ["\\/bfnrt] | "u" hex hex hex hex
hex ::= [0-9a-fA-F]

integer ::= "0" | [1-9] [0-9]*
number ::= integer ( "." [0-9]+ )? ( [eE] [+-]? [0-9]+ )?

ws ::= [ \t\n\r]*
"#
    .trim()
    .to_string()
}

/// GBNF grammar for reflection verdict (Layer 4 — Brainstem check).
///
/// The smallest model produces a structured verdict:
/// ```json
/// {
///   "safe": true|false,
///   "correct": true|false,
///   "concerns": ["optional list of concerns"],
///   "verdict": "approve"|"flag"|"reject"
/// }
/// ```
fn reflection_verdict_grammar() -> String {
    r#"
root ::= "{" ws safe-field "," ws correct-field "," ws concerns-field "," ws verdict-field ws "}"

safe-field ::= "\"safe\"" ws ":" ws boolean
correct-field ::= "\"correct\"" ws ":" ws boolean
concerns-field ::= "\"concerns\"" ws ":" ws "[" ws concern-list? ws "]"
verdict-field ::= "\"verdict\"" ws ":" ws verdict-value

concern-list ::= string ( "," ws string )*
verdict-value ::= "\"approve\"" | "\"flag\"" | "\"reject\""
boolean ::= "true" | "false"

string ::= "\"" char* "\""
char ::= [^"\\] | "\\" escape-char
escape-char ::= ["\\/bfnrt] | "u" hex hex hex hex
hex ::= [0-9a-fA-F]

ws ::= [ \t\n\r]*
"#
    .trim()
    .to_string()
}

/// GBNF grammar for confidence self-assessment (Layer 2).
///
/// Used when the teacher stack requests the model to assess its own
/// confidence in a response:
/// ```json
/// {
///   "confidence": 0.85,
///   "reasoning": "explanation of certainty level",
///   "uncertain_aspects": ["list of uncertain parts"]
/// }
/// ```
fn confidence_assessment_grammar() -> String {
    r#"
root ::= "{" ws confidence-field "," ws reasoning-field "," ws uncertain-field ws "}"

confidence-field ::= "\"confidence\"" ws ":" ws number
reasoning-field ::= "\"reasoning\"" ws ":" ws string
uncertain-field ::= "\"uncertain_aspects\"" ws ":" ws "[" ws aspect-list? ws "]"

aspect-list ::= string ( "," ws string )*

string ::= "\"" char* "\""
char ::= [^"\\] | "\\" escape-char
escape-char ::= ["\\/bfnrt] | "u" hex hex hex hex
hex ::= [0-9a-fA-F]

integer ::= "0" | [1-9] [0-9]*
number ::= integer ( "." [0-9]+ )? ( [eE] [+-]? [0-9]+ )?

ws ::= [ \t\n\r]*
"#
    .trim()
    .to_string()
}

// ─── Error types ────────────────────────────────────────────────────────────

/// Error returned by `validate_output` when structured output does not match
/// the expected grammar contract.
///
/// Variants carry enough detail for the inference engine to decide whether to
/// retry, cascade, or surface an error code to the daemon.
#[derive(Debug, Clone, PartialEq)]
pub enum GrammarError {
    /// The output was empty (or only whitespace).
    EmptyOutput,
    /// The output is missing a required top-level field.
    MissingField {
        /// Name of the expected JSON key that was absent.
        field: &'static str,
    },
    /// The output has the wrong top-level structure (e.g. object instead of array).
    InvalidStructure {
        /// Human-readable description of the structural mismatch.
        reason: String,
    },
}

impl std::fmt::Display for GrammarError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GrammarError::EmptyOutput => write!(f, "grammar error: output is empty"),
            GrammarError::MissingField { field } => {
                write!(f, "grammar error: missing required field \"{field}\"")
            },
            GrammarError::InvalidStructure { reason } => {
                write!(f, "grammar error: invalid structure — {reason}")
            },
        }
    }
}

/// Error returned by structured output parsers (`ReflectionVerdict::parse`,
/// `ChainOfThoughtOutput::parse`).
///
/// LLM output is untrusted — every variant avoids panicking and instead
/// returns a specific, actionable error so callers can log and handle it.
#[derive(Debug, Clone, PartialEq)]
pub enum ParseError {
    /// The raw string was not valid JSON.
    InvalidJson {
        /// The serde_json error message.
        detail: String,
    },
    /// The JSON was valid but not an object at the top level.
    NotAnObject,
    /// A required field was absent from the JSON object.
    MissingField {
        /// Name of the absent JSON key.
        field: &'static str,
    },
    /// A field was present but had an unexpected type or value.
    InvalidField {
        /// Name of the JSON key.
        field: &'static str,
        /// Description of what was wrong.
        detail: String,
    },
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ParseError::InvalidJson { detail } => {
                write!(f, "parse error: invalid JSON — {detail}")
            },
            ParseError::NotAnObject => {
                write!(f, "parse error: expected a JSON object at the top level")
            },
            ParseError::MissingField { field } => {
                write!(f, "parse error: missing required field \"{field}\"")
            },
            ParseError::InvalidField { field, detail } => {
                write!(f, "parse error: field \"{field}\" — {detail}")
            },
        }
    }
}

// ─── Grammar validation helpers ─────────────────────────────────────────────

/// Validate that a generated string matches the expected grammar structure.
///
/// This is a lightweight post-generation check — the grammar sampler should
/// have already enforced structure during generation, but we double-check on
/// the parsing side as a safety net against truncated or malformed output.
///
/// Returns `Ok(())` if the output satisfies the structural contract for `kind`.
/// Returns `Err(GrammarError)` with a specific error if it does not, so callers
/// can decide whether to retry or surface an error code to the daemon.
#[tracing::instrument(level = "trace", skip(output))]
pub fn validate_output(kind: GrammarKind, output: &str) -> Result<(), GrammarError> {
    let trimmed = output.trim();

    if trimmed.is_empty() {
        return Err(GrammarError::EmptyOutput);
    }

    match kind {
        GrammarKind::ActionPlan => {
            if !trimmed.starts_with('{') || !trimmed.ends_with('}') {
                return Err(GrammarError::InvalidStructure {
                    reason: "expected a JSON object (\"{ ... }\")".to_string(),
                });
            }
            if !trimmed.contains("\"goal_description\"") {
                return Err(GrammarError::MissingField {
                    field: "goal_description",
                });
            }
            if !trimmed.contains("\"steps\"") {
                return Err(GrammarError::MissingField { field: "steps" });
            }
            Ok(())
        },
        GrammarKind::DslSteps => {
            if !trimmed.starts_with('[') || !trimmed.ends_with(']') {
                return Err(GrammarError::InvalidStructure {
                    reason: "expected a JSON array (\"[ ... ]\")".to_string(),
                });
            }
            Ok(())
        },
        GrammarKind::ChainOfThought => {
            if !trimmed.starts_with('{') || !trimmed.ends_with('}') {
                return Err(GrammarError::InvalidStructure {
                    reason: "expected a JSON object (\"{ ... }\")".to_string(),
                });
            }
            if !trimmed.contains("\"thinking\"") {
                return Err(GrammarError::MissingField { field: "thinking" });
            }
            if !trimmed.contains("\"action\"") {
                return Err(GrammarError::MissingField { field: "action" });
            }
            Ok(())
        },
        GrammarKind::ReflectionVerdict => {
            if !trimmed.starts_with('{') || !trimmed.ends_with('}') {
                return Err(GrammarError::InvalidStructure {
                    reason: "expected a JSON object (\"{ ... }\")".to_string(),
                });
            }
            if !trimmed.contains("\"verdict\"") {
                return Err(GrammarError::MissingField { field: "verdict" });
            }
            Ok(())
        },
        GrammarKind::ConfidenceAssessment => {
            if !trimmed.starts_with('{') || !trimmed.ends_with('}') {
                return Err(GrammarError::InvalidStructure {
                    reason: "expected a JSON object (\"{ ... }\")".to_string(),
                });
            }
            if !trimmed.contains("\"confidence\"") {
                return Err(GrammarError::MissingField {
                    field: "confidence",
                });
            }
            Ok(())
        },
        GrammarKind::FreeText => {
            // Any non-empty string is valid free text.
            Ok(())
        },
    }
}

// ─── Reflection verdict parsing ─────────────────────────────────────────────

/// Parsed reflection verdict from the Brainstem model (Layer 4).
#[derive(Debug, Clone, PartialEq)]
pub struct ReflectionVerdict {
    pub safe: bool,
    pub correct: bool,
    pub concerns: Vec<String>,
    pub verdict: VerdictOutcome,
    /// Primary reason / first concern summarising the verdict (empty if concerns is empty).
    pub reason: String,
    /// Brainstem's self-reported confidence in this verdict (0.0–1.0).
    /// Extracted from `"confidence"` field if present; defaults to `1.0` when absent.
    pub confidence: f32,
}

/// Outcome of a reflection verdict.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerdictOutcome {
    Approve,
    Flag,
    Reject,
}

impl ReflectionVerdict {
    /// Parse a reflection verdict from JSON output.
    ///
    /// Returns `Err(ParseError)` on any structural or type failure so the
    /// inference engine can log a specific error instead of silently discarding.
    ///
    /// **No panics** — all access to untrusted JSON is guarded.
    pub fn parse(json: &str) -> Result<Self, ParseError> {
        let trimmed = json.trim();

        let value: serde_json::Value =
            serde_json::from_str(trimmed).map_err(|e| ParseError::InvalidJson {
                detail: e.to_string(),
            })?;

        let obj = value.as_object().ok_or(ParseError::NotAnObject)?;

        let safe = obj
            .get("safe")
            .ok_or(ParseError::MissingField { field: "safe" })?
            .as_bool()
            .ok_or(ParseError::InvalidField {
                field: "safe",
                detail: "expected boolean".to_string(),
            })?;

        let correct = obj
            .get("correct")
            .ok_or(ParseError::MissingField { field: "correct" })?
            .as_bool()
            .ok_or(ParseError::InvalidField {
                field: "correct",
                detail: "expected boolean".to_string(),
            })?;

        let concerns: Vec<String> = obj
            .get("concerns")
            .ok_or(ParseError::MissingField { field: "concerns" })?
            .as_array()
            .ok_or(ParseError::InvalidField {
                field: "concerns",
                detail: "expected array".to_string(),
            })?
            .iter()
            .enumerate()
            .map(|(i, v)| {
                v.as_str()
                    .map(String::from)
                    .ok_or(ParseError::InvalidField {
                        field: "concerns",
                        detail: format!("element {i} is not a string"),
                    })
            })
            .collect::<Result<Vec<_>, _>>()?;

        let verdict_str = obj
            .get("verdict")
            .ok_or(ParseError::MissingField { field: "verdict" })?
            .as_str()
            .ok_or(ParseError::InvalidField {
                field: "verdict",
                detail: "expected string".to_string(),
            })?;

        let verdict = match verdict_str {
            "approve" => VerdictOutcome::Approve,
            "flag" => VerdictOutcome::Flag,
            "reject" => VerdictOutcome::Reject,
            other => {
                return Err(ParseError::InvalidField {
                    field: "verdict",
                    detail: format!("unknown value \"{other}\"; expected approve|flag|reject"),
                })
            },
        };

        // Optional confidence field — default to 1.0 if absent (grammar enforces
        // the four required fields; confidence is an extension for richer verdicts).
        let confidence = obj
            .get("confidence")
            .and_then(|v| v.as_f64())
            .map(|v| (v as f32).clamp(0.0, 1.0))
            .unwrap_or(1.0);

        // Primary reason: first concern, or empty string.
        let reason = concerns.first().cloned().unwrap_or_default();

        Ok(ReflectionVerdict {
            safe,
            correct,
            concerns,
            verdict,
            reason,
            confidence,
        })
    }

    /// Whether this verdict allows the response to proceed.
    #[must_use]
    pub fn is_approved(&self) -> bool {
        self.verdict == VerdictOutcome::Approve
    }

    /// Whether this verdict requires a retry with additional context.
    #[must_use]
    pub fn needs_retry(&self) -> bool {
        matches!(self.verdict, VerdictOutcome::Flag | VerdictOutcome::Reject)
    }

    /// Whether this verdict demands that the response be discarded entirely.
    ///
    /// A rejected response is not retried — it is surfaced as an error to the
    /// daemon. This differs from `needs_retry()` which covers Flag too.
    #[must_use]
    pub fn should_reject(&self) -> bool {
        self.verdict == VerdictOutcome::Reject
    }
}

// ─── Chain-of-thought parsing ───────────────────────────────────────────────

/// Parsed chain-of-thought output (Layer 1).
#[derive(Debug, Clone)]
pub struct ChainOfThoughtOutput {
    /// The model's step-by-step reasoning.
    pub thinking: String,
    /// The action/response produced after reasoning.
    pub action: String,
}

impl ChainOfThoughtOutput {
    /// Parse a chain-of-thought JSON output.
    ///
    /// Returns `Err(ParseError)` on any structural failure — LLM output is
    /// untrusted and a fallback that silently swallows malformed CoT would
    /// hide bugs at the grammar-sampler or prompt level. Callers that need
    /// graceful degradation should handle the error explicitly.
    ///
    /// **No panics** — all access to untrusted JSON is guarded.
    pub fn parse(json: &str) -> Result<Self, ParseError> {
        let trimmed = json.trim();

        let value: serde_json::Value =
            serde_json::from_str(trimmed).map_err(|e| ParseError::InvalidJson {
                detail: e.to_string(),
            })?;

        let obj = value.as_object().ok_or(ParseError::NotAnObject)?;

        let thinking = obj
            .get("thinking")
            .ok_or(ParseError::MissingField { field: "thinking" })?
            .as_str()
            .ok_or(ParseError::InvalidField {
                field: "thinking",
                detail: "expected string".to_string(),
            })?
            .to_string();

        let action_val = obj
            .get("action")
            .ok_or(ParseError::MissingField { field: "action" })?;

        // `action` may be a plain string or a nested JSON object (e.g., an
        // ActionPlan). Serialize nested objects back to a JSON string so callers
        // always receive a `String` regardless of the inner shape.
        let action = if action_val.is_string() {
            action_val
                .as_str()
                .expect("is_string() guarantees as_str() succeeds")
                .to_string()
        } else {
            serde_json::to_string(action_val).map_err(|e| ParseError::InvalidField {
                field: "action",
                detail: format!("could not re-serialize nested value: {e}"),
            })?
        };

        Ok(ChainOfThoughtOutput { thinking, action })
    }
}

// ─── Confidence assessment parsing ──────────────────────────────────────────

/// Parsed confidence self-assessment (Layer 2).
#[derive(Debug, Clone)]
pub struct ConfidenceAssessment {
    /// Self-reported confidence score (0.0 - 1.0).
    pub confidence: f32,
    /// Model's reasoning about its certainty.
    pub reasoning: String,
    /// Aspects the model is uncertain about.
    pub uncertain_aspects: Vec<String>,
}

impl ConfidenceAssessment {
    /// Parse a confidence assessment from JSON output.
    ///
    /// Returns a default low-confidence assessment if parsing fails.
    pub fn parse(json: &str) -> Self {
        let trimmed = json.trim();

        if let Ok(value) = serde_json::from_str::<serde_json::Value>(trimmed) {
            if let Some(obj) = value.as_object() {
                let confidence = obj
                    .get("confidence")
                    .and_then(|v| v.as_f64())
                    .map(|v| (v as f32).clamp(0.0, 1.0))
                    .unwrap_or(0.0);

                let reasoning = obj
                    .get("reasoning")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();

                let uncertain_aspects = obj
                    .get("uncertain_aspects")
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| v.as_str().map(String::from))
                            .collect()
                    })
                    .unwrap_or_default();

                return ConfidenceAssessment {
                    confidence,
                    reasoning,
                    uncertain_aspects,
                };
            }
        }

        // Fallback: low confidence, parse failed.
        ConfidenceAssessment {
            confidence: 0.0,
            reasoning: "failed to parse confidence assessment".to_string(),
            uncertain_aspects: vec!["entire response".to_string()],
        }
    }
}

// ─── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grammar_kind_for_modes() {
        assert_eq!(
            GrammarKind::for_mode(InferenceMode::Planner),
            Some(GrammarKind::ActionPlan)
        );
        assert_eq!(
            GrammarKind::for_mode(InferenceMode::Strategist),
            Some(GrammarKind::ActionPlan)
        );
        assert_eq!(
            GrammarKind::for_mode(InferenceMode::Composer),
            Some(GrammarKind::DslSteps)
        );
        // Conversational returns None — no grammar constraint.
        assert_eq!(GrammarKind::for_mode(InferenceMode::Conversational), None);
    }

    #[test]
    fn free_text_is_not_constrained() {
        assert!(!GrammarKind::FreeText.is_constrained());
        assert!(GrammarKind::ActionPlan.is_constrained());
        assert!(GrammarKind::DslSteps.is_constrained());
        assert!(GrammarKind::ChainOfThought.is_constrained());
        assert!(GrammarKind::ReflectionVerdict.is_constrained());
        assert!(GrammarKind::ConfidenceAssessment.is_constrained());
    }

    #[test]
    fn compile_grammar_returns_none_for_free_text() {
        assert!(compile_grammar(GrammarKind::FreeText).is_none());
    }

    #[test]
    fn compile_grammar_returns_some_for_constrained() {
        let grammars = [
            GrammarKind::ActionPlan,
            GrammarKind::DslSteps,
            GrammarKind::ChainOfThought,
            GrammarKind::ReflectionVerdict,
            GrammarKind::ConfidenceAssessment,
        ];
        for kind in grammars {
            let grammar = compile_grammar(kind);
            assert!(grammar.is_some(), "expected grammar for {kind:?}");
            let g = grammar.unwrap();
            assert!(!g.source.is_empty());
            assert!(g.is_constrained());
        }
    }

    #[test]
    fn grammar_for_conversational_mode_is_none() {
        assert!(grammar_for_mode(InferenceMode::Conversational).is_none());
    }

    #[test]
    fn grammar_for_planner_mode_has_plan_rules() {
        let g = grammar_for_mode(InferenceMode::Planner).unwrap();
        assert!(g.source.contains("goal_description"));
        assert!(g.source.contains("steps"));
        assert!(g.source.contains("confidence"));
    }

    #[test]
    fn grammar_for_composer_mode_has_step_rules() {
        let g = grammar_for_mode(InferenceMode::Composer).unwrap();
        assert!(g.source.contains("action"));
        assert!(g.source.contains("timeout_ms"));
        assert!(g.source.contains("on_failure"));
    }

    #[test]
    fn action_plan_grammar_is_valid_gbnf() {
        let source = action_plan_grammar();
        // Verify basic GBNF structure: has root rule, uses ::=
        assert!(source.contains("root ::="));
        assert!(source.contains("ws ::="));
        assert!(source.contains("string ::="));
    }

    #[test]
    fn dsl_steps_grammar_starts_with_array() {
        let source = dsl_steps_grammar();
        // DSL steps root should start with a JSON array.
        assert!(source.contains("root ::= \"[\""));
    }

    #[test]
    fn chain_of_thought_grammar_has_thinking_field() {
        let source = chain_of_thought_grammar();
        assert!(source.contains("thinking"));
        assert!(source.contains("action"));
    }

    #[test]
    fn reflection_verdict_grammar_has_verdict_values() {
        let source = reflection_verdict_grammar();
        assert!(source.contains("approve"));
        assert!(source.contains("flag"));
        assert!(source.contains("reject"));
    }

    #[test]
    fn validate_action_plan_output() {
        let valid = r#"{"goal_description": "test", "steps": [], "estimated_duration_ms": 1000, "confidence": 0.9}"#;
        assert!(validate_output(GrammarKind::ActionPlan, valid).is_ok());

        let invalid = "just some text";
        assert!(validate_output(GrammarKind::ActionPlan, invalid).is_err());
    }

    #[test]
    fn validate_dsl_steps_output() {
        let valid = r#"[{"action": {"Tap": {"x": 1, "y": 2}}, "target": null, "timeout_ms": 1000, "on_failure": {"Retry": {"max": 3}}}]"#;
        assert!(validate_output(GrammarKind::DslSteps, valid).is_ok());

        let empty_array = "[]";
        assert!(validate_output(GrammarKind::DslSteps, empty_array).is_ok());

        let invalid = "not an array";
        assert!(validate_output(GrammarKind::DslSteps, invalid).is_err());
    }

    #[test]
    fn validate_cot_output() {
        let valid = r#"{"thinking": "step by step", "action": "do something"}"#;
        assert!(validate_output(GrammarKind::ChainOfThought, valid).is_ok());

        let missing_thinking = r#"{"action": "do something"}"#;
        assert!(validate_output(GrammarKind::ChainOfThought, missing_thinking).is_err());
    }

    #[test]
    fn validate_free_text() {
        assert!(validate_output(GrammarKind::FreeText, "any text").is_ok());
        assert!(validate_output(GrammarKind::FreeText, "").is_err());
        assert!(validate_output(GrammarKind::FreeText, "   ").is_err());
    }

    #[test]
    fn parse_reflection_verdict_approve() {
        let json = r#"{"safe": true, "correct": true, "concerns": [], "verdict": "approve"}"#;
        let verdict = ReflectionVerdict::parse(json).unwrap();
        assert!(verdict.safe);
        assert!(verdict.correct);
        assert!(verdict.concerns.is_empty());
        assert_eq!(verdict.verdict, VerdictOutcome::Approve);
        assert!(verdict.is_approved());
        assert!(!verdict.needs_retry());
    }

    #[test]
    fn parse_reflection_verdict_flag() {
        let json = r#"{"safe": true, "correct": false, "concerns": ["incorrect selector", "may tap wrong element"], "verdict": "flag"}"#;
        let verdict = ReflectionVerdict::parse(json).unwrap();
        assert!(verdict.safe);
        assert!(!verdict.correct);
        assert_eq!(verdict.concerns.len(), 2);
        assert_eq!(verdict.verdict, VerdictOutcome::Flag);
        assert!(!verdict.is_approved());
        assert!(verdict.needs_retry());
    }

    #[test]
    fn parse_reflection_verdict_reject() {
        let json = r#"{"safe": false, "correct": false, "concerns": ["unsafe action"], "verdict": "reject"}"#;
        let verdict = ReflectionVerdict::parse(json).unwrap();
        assert!(!verdict.safe);
        assert_eq!(verdict.verdict, VerdictOutcome::Reject);
        assert!(verdict.needs_retry());
        assert!(verdict.should_reject());
        assert!(!verdict.is_approved());
        assert_eq!(verdict.reason, "unsafe action");
    }

    #[test]
    fn parse_reflection_verdict_invalid_json() {
        assert!(ReflectionVerdict::parse("not json").is_err());
        assert!(ReflectionVerdict::parse("{}").is_err());
        assert!(ReflectionVerdict::parse(r#"{"safe": true}"#).is_err());
    }

    #[test]
    fn parse_chain_of_thought_valid() {
        let json = r#"{"thinking": "First I need to open settings, then find WiFi", "action": "open settings"}"#;
        let cot = ChainOfThoughtOutput::parse(json).unwrap();
        assert!(cot.thinking.contains("open settings"));
        assert_eq!(cot.action, "open settings");
    }

    #[test]
    fn parse_chain_of_thought_nested_action() {
        let json = r#"{"thinking": "planning steps", "action": {"goal": "test", "steps": []}}"#;
        let cot = ChainOfThoughtOutput::parse(json).unwrap();
        assert!(cot.thinking.contains("planning"));
        assert!(cot.action.contains("goal"));
    }

    #[test]
    fn parse_chain_of_thought_invalid_returns_err() {
        // Non-JSON input must return Err, not silently fall back.
        assert!(
            ChainOfThoughtOutput::parse("just a plain response without CoT structure").is_err()
        );
        // Valid JSON but not an object.
        assert!(ChainOfThoughtOutput::parse("[1, 2, 3]").is_err());
        // Object missing `thinking` field.
        assert!(ChainOfThoughtOutput::parse(r#"{"action": "do it"}"#).is_err());
        // Object missing `action` field.
        assert!(ChainOfThoughtOutput::parse(r#"{"thinking": "hmm"}"#).is_err());
    }

    #[test]
    fn parse_confidence_assessment_valid() {
        let json = r#"{"confidence": 0.85, "reasoning": "I am fairly certain", "uncertain_aspects": ["selector accuracy"]}"#;
        let ca = ConfidenceAssessment::parse(json);
        assert!((ca.confidence - 0.85).abs() < f32::EPSILON);
        assert!(ca.reasoning.contains("fairly certain"));
        assert_eq!(ca.uncertain_aspects.len(), 1);
    }

    #[test]
    fn parse_confidence_assessment_clamps() {
        let json = r#"{"confidence": 1.5, "reasoning": "over-confident", "uncertain_aspects": []}"#;
        let ca = ConfidenceAssessment::parse(json);
        assert!((ca.confidence - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn parse_confidence_assessment_fallback() {
        let ca = ConfidenceAssessment::parse("invalid");
        assert!((ca.confidence - 0.0).abs() < f32::EPSILON);
        assert!(ca.reasoning.contains("failed"));
        assert!(!ca.uncertain_aspects.is_empty());
    }
}
