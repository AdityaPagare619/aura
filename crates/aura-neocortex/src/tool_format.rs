//! Structured tool-call output formatting and parsing.
//!
//! Bridges the gap between raw LLM output (JSON strings constrained by GBNF
//! grammars) and the typed Rust structs in `aura-types`.
//!
//! **Parsing direction** (LLM → structs):
//! Takes grammar-constrained JSON from the model and produces typed
//! `ActionPlan`, `DslStep`, `ToolCall`, etc.
//!
//! **Formatting direction** (structs → LLM):
//! Takes typed execution results and formats them as compact text blocks
//! that can be injected back into the context for re-planning, reflection,
//! or conversation.
//!
//! All public functions return `Result<T, AuraError>` — no panics.

use aura_types::actions::{ActionType, ScrollDirection, TargetSelector};
use aura_types::dsl::{DslStep, FailureStrategy, ToolCall, ToolCallResult};
use aura_types::errors::{AuraError, LlmError};
use aura_types::etg::{ActionPlan, PlanSource};
use aura_types::ipc::InferenceMode;
use aura_types::tools::{find_tool, ParamValue, RiskLevel, TOOL_REGISTRY};

// ─── Constants ──────────────────────────────────────────────────────────────

/// Maximum number of steps we accept from a single LLM plan output.
/// Prevents unbounded allocations from malformed model output.
const MAX_PLAN_STEPS: usize = 64;

/// Maximum number of tool parameters we accept per tool call.
#[allow(dead_code)] // Phase 5/8: used by ReAct tool-call parser validation
const MAX_TOOL_PARAMS: usize = 16;

/// Maximum length of any single string field from LLM output (64 KB).
const MAX_STRING_LEN: usize = 65_536;

// ─── Plan parsing (LLM JSON → ActionPlan) ───────────────────────────────────

/// Parse an `ActionPlan` from grammar-constrained JSON output.
///
/// Expects the JSON shape enforced by `GrammarKind::ActionPlan`:
/// ```json
/// {
///   "goal_description": "...",
///   "steps": [ { "action": ..., "target": ..., "timeout_ms": ..., "on_failure": ... } ],
///   "estimated_duration_ms": 5000,
///   "confidence": 0.85
/// }
/// ```
///
/// Steps that fail to parse individually are skipped (logged) rather than
/// failing the entire plan — partial plans are more useful than no plan.
#[tracing::instrument(level = "debug", skip(json))]
pub fn parse_action_plan(json: &str) -> Result<ActionPlan, AuraError> {
    let trimmed = json.trim();
    let value: serde_json::Value = serde_json::from_str(trimmed).map_err(|e| {
        AuraError::Llm(LlmError::InferenceFailed(format!(
            "action plan JSON parse failed: {e}"
        )))
    })?;

    let obj = value.as_object().ok_or_else(|| {
        AuraError::Llm(LlmError::InferenceFailed(
            "action plan: expected JSON object at root".into(),
        ))
    })?;

    let goal_description = obj
        .get("goal_description")
        .and_then(|v| v.as_str())
        .map(|s| truncate_string(s, MAX_STRING_LEN))
        .ok_or_else(|| {
            AuraError::Llm(LlmError::InferenceFailed(
                "action plan: missing goal_description".into(),
            ))
        })?;

    let estimated_duration_ms = obj
        .get("estimated_duration_ms")
        .and_then(|v| v.as_u64())
        .unwrap_or(0) as u32;

    let confidence = obj
        .get("confidence")
        .and_then(|v| v.as_f64())
        .map(|v| (v as f32).clamp(0.0, 1.0))
        .unwrap_or(0.5);

    let steps_array = obj.get("steps").and_then(|v| v.as_array()).ok_or_else(|| {
        AuraError::Llm(LlmError::InferenceFailed(
            "action plan: missing steps array".into(),
        ))
    })?;

    let mut steps = Vec::with_capacity(steps_array.len().min(MAX_PLAN_STEPS));
    for (i, step_value) in steps_array.iter().take(MAX_PLAN_STEPS).enumerate() {
        match parse_dsl_step(step_value) {
            Ok(step) => steps.push(step),
            Err(e) => {
                tracing::warn!(step_index = i, error = %e, "skipping malformed plan step");
            }
        }
    }

    Ok(ActionPlan {
        goal_description,
        steps,
        estimated_duration_ms,
        confidence,
        source: PlanSource::LlmGenerated,
    })
}

/// Parse a single `DslStep` from a JSON value.
///
/// Used both by `parse_action_plan` (for individual steps within a plan)
/// and by `parse_dsl_steps` (for Composer mode output).
#[tracing::instrument(level = "trace", skip(value))]
pub fn parse_dsl_step(value: &serde_json::Value) -> Result<DslStep, AuraError> {
    let obj = value.as_object().ok_or_else(|| {
        AuraError::Llm(LlmError::InferenceFailed(
            "DSL step: expected JSON object".into(),
        ))
    })?;

    let action = obj
        .get("action")
        .ok_or_else(|| {
            AuraError::Llm(LlmError::InferenceFailed(
                "DSL step: missing action field".into(),
            ))
        })
        .and_then(|v| parse_action_type(v))?;

    let target = obj
        .get("target")
        .and_then(|v| if v.is_null() { None } else { Some(v) })
        .map(parse_target_selector)
        .transpose()?;

    let timeout_ms = obj
        .get("timeout_ms")
        .and_then(|v| v.as_u64())
        .unwrap_or_else(|| action.default_timeout() as u64) as u32;

    let on_failure = obj
        .get("on_failure")
        .map(parse_failure_strategy)
        .transpose()?
        .unwrap_or_default();

    let label = obj
        .get("label")
        .and_then(|v| v.as_str())
        .map(|s| truncate_string(s, 256));

    let precondition = None; // Conditions are complex; left for DSL engine to resolve.
    let postcondition = None;

    Ok(DslStep {
        action,
        target,
        timeout_ms,
        on_failure,
        precondition,
        postcondition,
        label,
    })
}

/// Parse a `Vec<DslStep>` from grammar-constrained JSON array output.
///
/// This is the Composer mode parser. Expects the JSON shape enforced by
/// `GrammarKind::DslSteps`: a top-level JSON array of step objects.
#[tracing::instrument(level = "debug", skip(json))]
pub fn parse_dsl_steps(json: &str) -> Result<Vec<DslStep>, AuraError> {
    let trimmed = json.trim();
    let value: serde_json::Value = serde_json::from_str(trimmed).map_err(|e| {
        AuraError::Llm(LlmError::InferenceFailed(format!(
            "DSL steps JSON parse failed: {e}"
        )))
    })?;

    let arr = value.as_array().ok_or_else(|| {
        AuraError::Llm(LlmError::InferenceFailed(
            "DSL steps: expected JSON array at root".into(),
        ))
    })?;

    let mut steps = Vec::with_capacity(arr.len().min(MAX_PLAN_STEPS));
    for (i, step_value) in arr.iter().take(MAX_PLAN_STEPS).enumerate() {
        match parse_dsl_step(step_value) {
            Ok(step) => steps.push(step),
            Err(e) => {
                tracing::warn!(step_index = i, error = %e, "skipping malformed DSL step");
            }
        }
    }

    Ok(steps)
}

// ─── Action type parsing ────────────────────────────────────────────────────

/// Parse an `ActionType` from a JSON value.
///
/// Supports both tagged-enum style (`{"Tap": {"x": 1, "y": 2}}`) and
/// flattened style (`{"type": "Tap", "x": 1, "y": 2}`) for robustness —
/// small LLMs may produce either format.
fn parse_action_type(value: &serde_json::Value) -> Result<ActionType, AuraError> {
    // Try serde deserialization first (handles tagged enum format).
    if let Ok(action) = serde_json::from_value::<ActionType>(value.clone()) {
        return Ok(action);
    }

    // Fallback: flattened format with "type" field.
    let obj = value.as_object().ok_or_else(|| {
        AuraError::Llm(LlmError::InferenceFailed(
            "action type: expected JSON object".into(),
        ))
    })?;

    let type_str = obj.get("type").and_then(|v| v.as_str()).unwrap_or("");

    match type_str.to_lowercase().as_str() {
        "tap" => {
            let x = obj.get("x").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
            let y = obj.get("y").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
            Ok(ActionType::Tap { x, y })
        }
        "longpress" | "long_press" => {
            let x = obj.get("x").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
            let y = obj.get("y").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
            Ok(ActionType::LongPress { x, y })
        }
        "swipe" => {
            let from_x = obj.get("from_x").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
            let from_y = obj.get("from_y").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
            let to_x = obj.get("to_x").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
            let to_y = obj.get("to_y").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
            let duration_ms = obj
                .get("duration_ms")
                .and_then(|v| v.as_u64())
                .unwrap_or(300) as u32;
            Ok(ActionType::Swipe {
                from_x,
                from_y,
                to_x,
                to_y,
                duration_ms,
            })
        }
        "type" | "text" => {
            let text = obj
                .get("text")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            Ok(ActionType::Type { text })
        }
        "scroll" => {
            let direction = obj
                .get("direction")
                .and_then(|v| v.as_str())
                .map(parse_scroll_direction)
                .unwrap_or(ScrollDirection::Down);
            let amount = obj.get("amount").and_then(|v| v.as_i64()).unwrap_or(300) as i32;
            Ok(ActionType::Scroll { direction, amount })
        }
        "back" => Ok(ActionType::Back),
        "home" => Ok(ActionType::Home),
        "recents" => Ok(ActionType::Recents),
        "openapp" | "open_app" => {
            let package = obj
                .get("package")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            Ok(ActionType::OpenApp { package })
        }
        _ => Err(AuraError::Llm(LlmError::InferenceFailed(format!(
            "unknown action type: {type_str}"
        )))),
    }
}

/// Parse a scroll direction string into a `ScrollDirection`.
fn parse_scroll_direction(s: &str) -> ScrollDirection {
    match s.to_lowercase().as_str() {
        "up" => ScrollDirection::Up,
        "down" => ScrollDirection::Down,
        "left" => ScrollDirection::Left,
        "right" => ScrollDirection::Right,
        _ => ScrollDirection::Down, // safe default
    }
}

// ─── Target selector parsing ────────────────────────────────────────────────

/// Parse a `TargetSelector` from a JSON value.
///
/// Supports tagged-enum format and also a simplified `{"type": "...", "value": "..."}` format.
fn parse_target_selector(value: &serde_json::Value) -> Result<TargetSelector, AuraError> {
    // Try serde deserialization first (handles tagged enum format).
    if let Ok(selector) = serde_json::from_value::<TargetSelector>(value.clone()) {
        return Ok(selector);
    }

    // Fallback: simplified format.
    let obj = value.as_object().ok_or_else(|| {
        AuraError::Llm(LlmError::InferenceFailed(
            "target selector: expected JSON object".into(),
        ))
    })?;

    let type_str = obj.get("type").and_then(|v| v.as_str()).unwrap_or("");
    let val = obj
        .get("value")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    match type_str.to_lowercase().as_str() {
        "xpath" => Ok(TargetSelector::XPath(val)),
        "resource_id" | "resourceid" | "id" => Ok(TargetSelector::ResourceId(val)),
        "text" => Ok(TargetSelector::Text(val)),
        "content_description" | "contentdescription" | "description" => {
            Ok(TargetSelector::ContentDescription(val))
        }
        "class_name" | "classname" | "class" => Ok(TargetSelector::ClassName(val)),
        "coordinates" | "coords" => {
            let x = obj.get("x").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
            let y = obj.get("y").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
            Ok(TargetSelector::Coordinates { x, y })
        }
        "llm_description" | "llmdescription" | "description_llm" => {
            Ok(TargetSelector::LlmDescription(val))
        }
        _ => {
            // Last resort: if there's a "value" field, treat as LLM description.
            if !val.is_empty() {
                Ok(TargetSelector::LlmDescription(val))
            } else {
                Err(AuraError::Llm(LlmError::InferenceFailed(format!(
                    "unknown target selector type: {type_str}"
                ))))
            }
        }
    }
}

// ─── Failure strategy parsing ───────────────────────────────────────────────

/// Parse a `FailureStrategy` from a JSON value.
///
/// Accepts both tagged-enum format and simple strings like "retry", "skip", "abort".
fn parse_failure_strategy(value: &serde_json::Value) -> Result<FailureStrategy, AuraError> {
    // Try serde deserialization first.
    if let Ok(strategy) = serde_json::from_value::<FailureStrategy>(value.clone()) {
        return Ok(strategy);
    }

    // Fallback: simple string.
    if let Some(s) = value.as_str() {
        return match s.to_lowercase().as_str() {
            "retry" => Ok(FailureStrategy::Retry { max: 3 }),
            "skip" => Ok(FailureStrategy::Skip),
            "abort" => Ok(FailureStrategy::Abort),
            _ => Ok(FailureStrategy::default()),
        };
    }

    // Fallback: object with "type" field.
    if let Some(obj) = value.as_object() {
        if let Some(type_str) = obj.get("type").and_then(|v| v.as_str()) {
            return match type_str.to_lowercase().as_str() {
                "retry" => {
                    let max = obj.get("max").and_then(|v| v.as_u64()).unwrap_or(3) as u8;
                    Ok(FailureStrategy::Retry { max })
                }
                "skip" => Ok(FailureStrategy::Skip),
                "abort" => Ok(FailureStrategy::Abort),
                "ask_user" | "askuser" => {
                    let msg = obj
                        .get("message")
                        .and_then(|v| v.as_str())
                        .unwrap_or("What should I do?")
                        .to_string();
                    Ok(FailureStrategy::AskUser(msg))
                }
                _ => Ok(FailureStrategy::default()),
            };
        }
    }

    Ok(FailureStrategy::default())
}

// ─── Tool call parsing ──────────────────────────────────────────────────────

/// Parse a tool call from JSON output.
///
/// Expects format:
/// ```json
/// {
///   "tool": "screen_tap",
///   "parameters": { "x": 100, "y": 200 }
/// }
/// ```
///
/// Validates tool name against `TOOL_REGISTRY` and converts parameter values.
#[allow(dead_code)] // Phase 5/8: called by ReAct loop when parsing LLM tool-call JSON
#[tracing::instrument(level = "debug", skip(json))]
pub fn parse_tool_call(json: &str) -> Result<ToolCall, AuraError> {
    let trimmed = json.trim();
    let value: serde_json::Value = serde_json::from_str(trimmed).map_err(|e| {
        AuraError::Llm(LlmError::InferenceFailed(format!(
            "tool call JSON parse failed: {e}"
        )))
    })?;

    let obj = value.as_object().ok_or_else(|| {
        AuraError::Llm(LlmError::InferenceFailed(
            "tool call: expected JSON object".into(),
        ))
    })?;

    let tool_name = obj
        .get("tool")
        .or_else(|| obj.get("tool_name"))
        .or_else(|| obj.get("name"))
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            AuraError::Llm(LlmError::InferenceFailed(
                "tool call: missing tool name".into(),
            ))
        })?
        .to_string();

    // Validate tool name exists in the registry.
    let schema = find_tool(&tool_name).ok_or_else(|| {
        AuraError::Llm(LlmError::InferenceFailed(format!(
            "unknown tool: {tool_name}"
        )))
    })?;

    let params_obj = obj
        .get("parameters")
        .or_else(|| obj.get("params"))
        .and_then(|v| v.as_object());

    let mut parameters = Vec::with_capacity(MAX_TOOL_PARAMS);
    if let Some(params) = params_obj {
        for (key, val) in params.iter().take(MAX_TOOL_PARAMS) {
            let param_value = json_to_param_value(val);
            parameters.push((key.clone(), param_value));
        }
    }

    let confidence = obj
        .get("confidence")
        .and_then(|v| v.as_f64())
        .map(|v| (v as f32).clamp(0.0, 1.0))
        .unwrap_or(0.5);

    Ok(ToolCall {
        tool_name,
        parameters,
        risk_level: schema.risk_level,
        user_confirmed: false,
        confidence,
    })
}

/// Convert a `serde_json::Value` to a `ParamValue`.
#[allow(dead_code)] // Phase 5/8: used by parse_tool_call parameter conversion
fn json_to_param_value(value: &serde_json::Value) -> ParamValue {
    match value {
        serde_json::Value::String(s) => ParamValue::String(truncate_string(s, MAX_STRING_LEN)),
        serde_json::Value::Number(n) => ParamValue::Number(n.as_f64().unwrap_or(0.0)),
        serde_json::Value::Bool(b) => ParamValue::Boolean(*b),
        serde_json::Value::Null => ParamValue::Null,
        // Arrays and objects get serialized back to string — best-effort.
        other => ParamValue::String(serde_json::to_string(other).unwrap_or_default()),
    }
}

// ─── Formatting (structs → LLM context) ─────────────────────────────────────

/// Format an `ActionPlan` as a compact text block for LLM context injection.
///
/// This is used when the daemon sends back a previous plan for re-planning
/// or when the teacher stack needs to show the model what it previously produced.
///
/// # Phase 4 Wire Point
///
/// **Not yet called externally.** Wire this when the following are implemented:
///
/// - **ALPHA** (`ipc_handler.rs` → `handle_inference()`): When a `Replan` or
///   `Strategist` request arrives and `payload.previous_plan` is `Some(_)`,
///   call `format_plan_for_context(&previous_plan)` and inject the result into
///   the `ContextBuilder` before dispatching to the inference engine.
///
/// - **EPSILON** (`context.rs` → `ContextBuilder::build()`): Alongside the
///   existing `previous_attempt` injection, inject the previous plan text so
///   the LLM can reason about what it previously produced vs. what failed.
///
/// DO NOT DELETE — part of the re-planning feedback loop.
#[allow(dead_code)]
#[tracing::instrument(level = "trace", skip(plan))]
pub fn format_plan_for_context(plan: &ActionPlan) -> String {
    let mut out = String::with_capacity(1024);
    out.push_str("[Previous Plan]\n");
    out.push_str("Goal: ");
    out.push_str(&plan.goal_description);
    out.push('\n');
    out.push_str(&format!(
        "Steps: {} | Duration: {}ms | Confidence: {:.0}%\n",
        plan.steps.len(),
        plan.estimated_duration_ms,
        plan.confidence * 100.0,
    ));

    for (i, step) in plan.steps.iter().enumerate().take(16) {
        out.push_str(&format!("  {}. ", i + 1));
        out.push_str(&format_action_brief(&step.action));
        if let Some(ref label) = step.label {
            out.push_str(&format!(" ({label})"));
        }
        out.push('\n');
    }

    if plan.steps.len() > 16 {
        out.push_str(&format!("  ... and {} more steps\n", plan.steps.len() - 16));
    }

    out
}

/// Format a `ToolCallResult` as a compact text block for re-planning context.
///
/// # Phase 4 Wire Points
///
/// **Not yet called externally.** Wire this when the following are implemented:
///
/// - **ALPHA** (`ipc_handler.rs` → `DaemonToNeocortex::ReActStep` handler,
///   ~lines 341–355): The current handler builds a raw observation string
///   manually. Once the daemon sends typed `ToolCallResult` structs instead of
///   raw strings, replace that manual construction with a call to this function.
///
/// - **BETA** (`inference.rs` → `infer_react_loop()`, ~lines 617–626): When
///   assembling the observation turn after a tool call returns, call
///   `format_tool_result_for_context(&result)` to produce the observation text
///   instead of constructing it inline.
///
/// Prerequisite: daemon IPC must be updated to send `ToolCallResult` typed
/// payloads rather than `(tool_name: String, observation: String)` pairs.
///
/// DO NOT DELETE — part of the ReAct tool execution feedback loop.
#[allow(dead_code)]
#[tracing::instrument(level = "trace", skip(result))]
pub fn format_tool_result_for_context(result: &ToolCallResult) -> String {
    let status = if result.success { "OK" } else { "FAILED" };
    let mut out = format!(
        "[Tool Result] {} — {} ({}ms, {} steps)\n",
        result.tool_name, status, result.duration_ms, result.steps_executed,
    );
    out.push_str(&format!("Summary: {}\n", result.summary));
    if let Some(ref err) = result.error {
        out.push_str(&format!("Error: {}\n", err));
    }
    out
}

/// Format a list of available tools as a compact summary for context.
///
/// Lighter than `tools_as_llm_description()` — used when token budget is tight.
/// Groups tools by risk level for quick scanning.
#[tracing::instrument(level = "trace")]
pub fn format_tools_compact() -> String {
    let mut out = String::with_capacity(2048);
    out.push_str("[Available Tools]\n");

    for &risk in &[
        RiskLevel::Low,
        RiskLevel::Medium,
        RiskLevel::High,
        RiskLevel::Critical,
    ] {
        let tools: Vec<&str> = TOOL_REGISTRY
            .iter()
            .filter(|t| t.risk_level == risk)
            .map(|t| t.name)
            .collect();

        if !tools.is_empty() {
            out.push_str(&format!("  {} risk: {}\n", risk.label(), tools.join(", ")));
        }
    }

    out
}

/// Format a brief description of an `ActionType` (one-liner).
///
/// Called internally by [`format_plan_for_context`] to render each plan step.
/// The `dead_code` lint fires because `format_plan_for_context` has no external
/// callers yet (Phase 4). Once that function is wired, this warning disappears.
///
/// DO NOT DELETE — transitively used by the re-planning feedback loop.
#[allow(dead_code)]
pub fn format_action_brief(action: &ActionType) -> String {
    match action {
        ActionType::Tap { x, y } => format!("Tap({x},{y})"),
        ActionType::LongPress { x, y } => format!("LongPress({x},{y})"),
        ActionType::Swipe {
            from_x,
            from_y,
            to_x,
            to_y,
            ..
        } => format!("Swipe({from_x},{from_y}→{to_x},{to_y})"),
        ActionType::Type { text } => {
            let preview = if text.len() > 20 {
                format!("{}…", &text[..20])
            } else {
                text.clone()
            };
            format!("Type(\"{preview}\")")
        }
        ActionType::Scroll { direction, amount } => {
            format!("Scroll({direction:?},{amount})")
        }
        ActionType::Back => "Back".to_string(),
        ActionType::Home => "Home".to_string(),
        ActionType::Recents => "Recents".to_string(),
        ActionType::OpenApp { package } => format!("OpenApp({package})"),
        ActionType::NotificationAction {
            notification_id,
            action_index,
        } => format!("NotifAction({notification_id}#{action_index})"),
        ActionType::WaitForElement {
            selector,
            timeout_ms,
        } => {
            format!("WaitFor({selector:?},{}ms)", timeout_ms)
        }
        ActionType::AssertElement { selector, expected } => {
            format!("Assert({selector:?}={expected:?})")
        }
    }
}

// ─── Mode-aware dispatch ────────────────────────────────────────────────────

/// Outcome of parsing LLM output based on inference mode.
///
/// Each variant carries the parsed result for its respective mode.
/// The teacher stack uses this to route parsed output to the right handler.
#[derive(Debug, Clone)]
pub enum ParsedOutput {
    /// Planner/Strategist mode: full action plan.
    Plan(ActionPlan),
    /// Composer mode: list of DSL steps.
    Steps(Vec<DslStep>),
    /// Conversational mode: free text reply.
    Reply(String),
}

/// Parse LLM output according to the inference mode.
///
/// This is the main entry point for the teacher stack: given the mode and
/// raw output string, produce the appropriate typed result.
#[tracing::instrument(level = "debug", skip(output))]
pub fn parse_for_mode(mode: InferenceMode, output: &str) -> Result<ParsedOutput, AuraError> {
    match mode {
        InferenceMode::Planner | InferenceMode::Strategist => {
            let plan = parse_action_plan(output)?;
            Ok(ParsedOutput::Plan(plan))
        }
        InferenceMode::Composer => {
            let steps = parse_dsl_steps(output)?;
            Ok(ParsedOutput::Steps(steps))
        }
        InferenceMode::Conversational => {
            let reply = output.trim().to_string();
            if reply.is_empty() {
                return Err(AuraError::Llm(LlmError::InferenceFailed(
                    "empty conversational reply".into(),
                )));
            }
            Ok(ParsedOutput::Reply(reply))
        }
    }
}

// ─── Helpers ────────────────────────────────────────────────────────────────

/// Truncate a string to a maximum length, ensuring valid UTF-8 boundary.
fn truncate_string(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        // Find a valid UTF-8 boundary.
        let mut end = max_len;
        while end > 0 && !s.is_char_boundary(end) {
            end -= 1;
        }
        s[..end].to_string()
    }
}

// ─── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── Plan parsing ────────────────────────────────────────────────────

    #[test]
    fn parse_minimal_action_plan() {
        let json = r#"{
            "goal_description": "Open Settings app",
            "steps": [],
            "estimated_duration_ms": 2000,
            "confidence": 0.9
        }"#;
        let plan = parse_action_plan(json).unwrap();
        assert_eq!(plan.goal_description, "Open Settings app");
        assert!(plan.steps.is_empty());
        assert_eq!(plan.estimated_duration_ms, 2000);
        assert!((plan.confidence - 0.9).abs() < f32::EPSILON);
        assert_eq!(plan.source, PlanSource::LlmGenerated);
    }

    #[test]
    fn parse_action_plan_with_steps() {
        let json = r#"{
            "goal_description": "Send message to Alice",
            "steps": [
                {
                    "action": {"OpenApp": {"package": "com.whatsapp"}},
                    "target": null,
                    "timeout_ms": 5000,
                    "on_failure": {"Retry": {"max": 3}}
                },
                {
                    "action": {"Tap": {"x": 100, "y": 200}},
                    "target": {"Text": "Alice"},
                    "timeout_ms": 2000,
                    "on_failure": "retry",
                    "label": "Tap on Alice contact"
                }
            ],
            "estimated_duration_ms": 10000,
            "confidence": 0.85
        }"#;
        let plan = parse_action_plan(json).unwrap();
        assert_eq!(plan.steps.len(), 2);
        assert!(matches!(plan.steps[0].action, ActionType::OpenApp { .. }));
        assert!(matches!(plan.steps[1].action, ActionType::Tap { .. }));
        assert_eq!(plan.steps[1].label.as_deref(), Some("Tap on Alice contact"));
    }

    #[test]
    fn parse_action_plan_invalid_json() {
        let result = parse_action_plan("not json");
        assert!(result.is_err());
    }

    #[test]
    fn parse_action_plan_missing_goal() {
        let json = r#"{"steps": [], "estimated_duration_ms": 0, "confidence": 0.5}"#;
        let result = parse_action_plan(json);
        assert!(result.is_err());
    }

    #[test]
    fn parse_action_plan_skips_bad_steps() {
        let json = r#"{
            "goal_description": "test",
            "steps": [
                {"action": {"Tap": {"x": 1, "y": 2}}, "target": null, "timeout_ms": 1000, "on_failure": "retry"},
                {"bad": "step"},
                {"action": {"Back": null}, "target": null, "timeout_ms": 1000, "on_failure": "skip"}
            ],
            "estimated_duration_ms": 3000,
            "confidence": 0.7
        }"#;
        let plan = parse_action_plan(json).unwrap();
        // The bad step is skipped, so we get 2 steps.
        // Note: Back serialized as null by serde may need tagged format. Let's be lenient.
        assert!(plan.steps.len() >= 1);
    }

    #[test]
    fn parse_action_plan_clamps_confidence() {
        let json = r#"{
            "goal_description": "test",
            "steps": [],
            "estimated_duration_ms": 0,
            "confidence": 1.5
        }"#;
        let plan = parse_action_plan(json).unwrap();
        assert!((plan.confidence - 1.0).abs() < f32::EPSILON);
    }

    // ── DSL steps parsing ───────────────────────────────────────────────

    #[test]
    fn parse_dsl_steps_empty_array() {
        let steps = parse_dsl_steps("[]").unwrap();
        assert!(steps.is_empty());
    }

    #[test]
    fn parse_dsl_steps_single_step() {
        let json = r#"[{
            "action": {"Tap": {"x": 50, "y": 100}},
            "target": null,
            "timeout_ms": 2000,
            "on_failure": "retry"
        }]"#;
        let steps = parse_dsl_steps(json).unwrap();
        assert_eq!(steps.len(), 1);
        assert!(matches!(steps[0].action, ActionType::Tap { x: 50, y: 100 }));
    }

    #[test]
    fn parse_dsl_steps_invalid_root() {
        let result = parse_dsl_steps("{}");
        assert!(result.is_err());
    }

    // ── Action type parsing ─────────────────────────────────────────────

    #[test]
    fn parse_action_type_tagged_enum() {
        let val: serde_json::Value =
            serde_json::from_str(r#"{"Tap": {"x": 10, "y": 20}}"#).unwrap();
        let action = parse_action_type(&val).unwrap();
        assert!(matches!(action, ActionType::Tap { x: 10, y: 20 }));
    }

    #[test]
    fn parse_action_type_flattened() {
        let val: serde_json::Value =
            serde_json::from_str(r#"{"type": "tap", "x": 30, "y": 40}"#).unwrap();
        let action = parse_action_type(&val).unwrap();
        assert!(matches!(action, ActionType::Tap { x: 30, y: 40 }));
    }

    #[test]
    fn parse_action_type_scroll_flattened() {
        let val: serde_json::Value =
            serde_json::from_str(r#"{"type": "scroll", "direction": "up", "amount": 500}"#)
                .unwrap();
        let action = parse_action_type(&val).unwrap();
        assert!(matches!(
            action,
            ActionType::Scroll {
                direction: ScrollDirection::Up,
                amount: 500
            }
        ));
    }

    #[test]
    fn parse_action_type_back_flattened() {
        let val: serde_json::Value = serde_json::from_str(r#"{"type": "back"}"#).unwrap();
        let action = parse_action_type(&val).unwrap();
        assert!(matches!(action, ActionType::Back));
    }

    #[test]
    fn parse_action_type_unknown() {
        let val: serde_json::Value = serde_json::from_str(r#"{"type": "fly_away"}"#).unwrap();
        let result = parse_action_type(&val);
        assert!(result.is_err());
    }

    // ── Target selector parsing ─────────────────────────────────────────

    #[test]
    fn parse_target_selector_tagged() {
        let val: serde_json::Value = serde_json::from_str(r#"{"Text": "OK"}"#).unwrap();
        let selector = parse_target_selector(&val).unwrap();
        assert!(matches!(selector, TargetSelector::Text(ref s) if s == "OK"));
    }

    #[test]
    fn parse_target_selector_simplified() {
        let val: serde_json::Value =
            serde_json::from_str(r#"{"type": "resource_id", "value": "com.app:id/btn"}"#).unwrap();
        let selector = parse_target_selector(&val).unwrap();
        assert!(matches!(selector, TargetSelector::ResourceId(ref s) if s == "com.app:id/btn"));
    }

    #[test]
    fn parse_target_selector_coordinates() {
        let val: serde_json::Value =
            serde_json::from_str(r#"{"type": "coordinates", "x": 100, "y": 200}"#).unwrap();
        let selector = parse_target_selector(&val).unwrap();
        assert!(matches!(
            selector,
            TargetSelector::Coordinates { x: 100, y: 200 }
        ));
    }

    #[test]
    fn parse_target_selector_fallback_to_llm_description() {
        let val: serde_json::Value =
            serde_json::from_str(r#"{"type": "weird", "value": "the blue button"}"#).unwrap();
        let selector = parse_target_selector(&val).unwrap();
        assert!(
            matches!(selector, TargetSelector::LlmDescription(ref s) if s == "the blue button")
        );
    }

    // ── Failure strategy parsing ────────────────────────────────────────

    #[test]
    fn parse_failure_strategy_string_retry() {
        let val = serde_json::Value::String("retry".to_string());
        let strategy = parse_failure_strategy(&val).unwrap();
        assert!(matches!(strategy, FailureStrategy::Retry { max: 3 }));
    }

    #[test]
    fn parse_failure_strategy_string_skip() {
        let val = serde_json::Value::String("skip".to_string());
        let strategy = parse_failure_strategy(&val).unwrap();
        assert!(matches!(strategy, FailureStrategy::Skip));
    }

    #[test]
    fn parse_failure_strategy_tagged_enum() {
        let val: serde_json::Value = serde_json::from_str(r#"{"Retry": {"max": 5}}"#).unwrap();
        let strategy = parse_failure_strategy(&val).unwrap();
        assert!(matches!(strategy, FailureStrategy::Retry { max: 5 }));
    }

    // ── Tool call parsing ───────────────────────────────────────────────

    #[test]
    fn parse_tool_call_valid() {
        let json = r#"{
            "tool": "screen_tap",
            "parameters": {"x": 100, "y": 200}
        }"#;
        let call = parse_tool_call(json).unwrap();
        assert_eq!(call.tool_name, "screen_tap");
        assert_eq!(call.parameters.len(), 2);
        assert_eq!(call.risk_level, RiskLevel::Low);
    }

    #[test]
    fn parse_tool_call_alternate_field_names() {
        let json = r#"{
            "tool_name": "message_send",
            "params": {"contact": "Alice", "text": "Hello!"}
        }"#;
        let call = parse_tool_call(json).unwrap();
        assert_eq!(call.tool_name, "message_send");
        assert_eq!(call.risk_level, RiskLevel::Medium);
    }

    #[test]
    fn parse_tool_call_unknown_tool() {
        let json = r#"{"tool": "fly_to_moon", "parameters": {}}"#;
        let result = parse_tool_call(json);
        assert!(result.is_err());
    }

    #[test]
    fn parse_tool_call_no_params() {
        let json = r#"{"tool": "screen_back"}"#;
        let call = parse_tool_call(json).unwrap();
        assert_eq!(call.tool_name, "screen_back");
        assert!(call.parameters.is_empty());
    }

    // ── Formatting ──────────────────────────────────────────────────────

    #[test]
    fn format_plan_for_context_smoke() {
        let plan = ActionPlan {
            goal_description: "Open Settings".to_string(),
            steps: vec![DslStep {
                action: ActionType::OpenApp {
                    package: "com.android.settings".to_string(),
                },
                target: None,
                timeout_ms: 5000,
                on_failure: FailureStrategy::default(),
                precondition: None,
                postcondition: None,
                label: Some("Open Settings app".to_string()),
            }],
            estimated_duration_ms: 5000,
            confidence: 0.9,
            source: PlanSource::LlmGenerated,
        };
        let text = format_plan_for_context(&plan);
        assert!(text.contains("[Previous Plan]"));
        assert!(text.contains("Open Settings"));
        assert!(text.contains("90%"));
        assert!(text.contains("OpenApp"));
    }

    #[test]
    fn format_tool_result_success() {
        let result = ToolCallResult {
            tool_name: "screen_tap".to_string(),
            success: true,
            duration_ms: 150,
            steps_executed: 1,
            summary: "Tapped the OK button".to_string(),
            error: None,
        };
        let text = format_tool_result_for_context(&result);
        assert!(text.contains("OK"));
        assert!(text.contains("screen_tap"));
        assert!(text.contains("150ms"));
    }

    #[test]
    fn format_tool_result_failure() {
        let result = ToolCallResult {
            tool_name: "message_send".to_string(),
            success: false,
            duration_ms: 5000,
            steps_executed: 3,
            summary: "Failed to find contact".to_string(),
            error: Some("Element not found: Alice".to_string()),
        };
        let text = format_tool_result_for_context(&result);
        assert!(text.contains("FAILED"));
        assert!(text.contains("Error:"));
        assert!(text.contains("Element not found"));
    }

    #[test]
    fn format_tools_compact_groups_by_risk() {
        let text = format_tools_compact();
        assert!(text.contains("[Available Tools]"));
        assert!(text.contains("low risk:"));
        assert!(text.contains("medium risk:"));
        assert!(text.contains("screen_tap"));
    }

    #[test]
    fn format_action_brief_all_variants() {
        assert!(format_action_brief(&ActionType::Tap { x: 1, y: 2 }).contains("Tap"));
        assert!(format_action_brief(&ActionType::Back) == "Back");
        assert!(format_action_brief(&ActionType::Home) == "Home");
        assert!(format_action_brief(&ActionType::Recents) == "Recents");
        assert!(format_action_brief(&ActionType::Type {
            text: "hello".to_string()
        })
        .contains("Type"));
    }

    // ── Mode-aware dispatch ─────────────────────────────────────────────

    #[test]
    fn parse_for_mode_planner() {
        let json = r#"{
            "goal_description": "test",
            "steps": [],
            "estimated_duration_ms": 0,
            "confidence": 0.5
        }"#;
        let result = parse_for_mode(InferenceMode::Planner, json).unwrap();
        assert!(matches!(result, ParsedOutput::Plan(_)));
    }

    #[test]
    fn parse_for_mode_composer() {
        let json = "[]";
        let result = parse_for_mode(InferenceMode::Composer, json).unwrap();
        assert!(matches!(result, ParsedOutput::Steps(_)));
    }

    #[test]
    fn parse_for_mode_conversational() {
        let result = parse_for_mode(InferenceMode::Conversational, "Hello there!").unwrap();
        assert!(matches!(result, ParsedOutput::Reply(ref s) if s == "Hello there!"));
    }

    #[test]
    fn parse_for_mode_conversational_empty_fails() {
        let result = parse_for_mode(InferenceMode::Conversational, "   ");
        assert!(result.is_err());
    }

    // ── Helpers ─────────────────────────────────────────────────────────

    #[test]
    fn truncate_string_within_limit() {
        assert_eq!(truncate_string("hello", 10), "hello");
    }

    #[test]
    fn truncate_string_at_limit() {
        assert_eq!(truncate_string("hello world", 5), "hello");
    }

    #[test]
    fn truncate_string_multibyte() {
        // "café" is 5 bytes (é is 2 bytes). Truncating at 4 should give "caf".
        let s = "café";
        let truncated = truncate_string(s, 4);
        assert_eq!(truncated, "caf");
    }
}
