//! Tool/Action Schema System for AURA v4.
//!
//! Defines every action AURA can perform as a structured [`ToolSchema`].
//! The LLM references these schemas to know what actions are available,
//! what parameters they take, and what constraints apply.

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Core types
// ---------------------------------------------------------------------------

/// Risk classification for tool invocations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RiskLevel {
    /// Read-only, observation (e.g., read notifications, observe screen).
    Low,
    /// User-facing action (e.g., send message, make call).
    Medium,
    /// Data modification (e.g., delete file, edit contact).
    High,
    /// Financial or irreversible (e.g., payment, account deletion).
    Critical,
}

impl RiskLevel {
    /// Human-readable label for display.
    pub const fn label(&self) -> &'static str {
        match self {
            RiskLevel::Low => "low",
            RiskLevel::Medium => "medium",
            RiskLevel::High => "high",
            RiskLevel::Critical => "critical",
        }
    }

    /// Whether this risk level requires explicit user confirmation.
    pub const fn requires_confirmation(&self) -> bool {
        matches!(self, RiskLevel::High | RiskLevel::Critical)
    }
}

/// Parameter type for tool parameters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ParamType {
    /// Free-form text.
    String,
    /// Numeric value (integer or float).
    Number,
    /// True/false.
    Boolean,
    /// One of a fixed set of values (index into ENUM_VALUES).
    Enum(u8),
    /// App name — validated against installed apps.
    AppName,
    /// Contact name — validated against contacts.
    ContactName,
    /// Date/time expression (parsed from natural language).
    DateTime,
    /// Duration expression ("5 minutes", "1 hour").
    Duration,
    /// File system path.
    FilePath,
    /// Web URL.
    Url,
    /// X/Y screen coordinate pair.
    Coordinate,
    /// Percentage (0–100).
    Percentage,
}

impl ParamType {
    /// Human-readable type name for LLM descriptions.
    pub const fn type_name(&self) -> &'static str {
        match self {
            ParamType::String => "string",
            ParamType::Number => "number",
            ParamType::Boolean => "boolean",
            ParamType::Enum(_) => "enum",
            ParamType::AppName => "app_name",
            ParamType::ContactName => "contact_name",
            ParamType::DateTime => "datetime",
            ParamType::Duration => "duration",
            ParamType::FilePath => "file_path",
            ParamType::Url => "url",
            ParamType::Coordinate => "coordinate",
            ParamType::Percentage => "percentage",
        }
    }
}

/// Definition of a single parameter for a tool.
#[derive(Debug, Clone)]
pub struct ToolParameter {
    pub name: &'static str,
    pub param_type: ParamType,
    pub required: bool,
    pub description: &'static str,
}

/// Schema definition for one tool/action AURA can perform.
#[derive(Debug, Clone)]
pub struct ToolSchema {
    pub name: &'static str,
    pub description: &'static str,
    pub parameters: &'static [ToolParameter],
    /// Whether this tool requires the AccessibilityService.
    pub requires_screen: bool,
    /// Whether this tool requires network connectivity.
    pub requires_network: bool,
    /// Risk classification.
    pub risk_level: RiskLevel,
    /// Estimated execution time in milliseconds.
    pub estimated_duration_ms: u32,
}

// ---------------------------------------------------------------------------
// Enum value sets (referenced by ParamType::Enum index)
// ---------------------------------------------------------------------------

/// Scroll directions.
pub const ENUM_SCROLL_DIRS: &[&str] = &["up", "down", "left", "right"];
/// System settings that can be toggled.
pub const ENUM_SETTINGS: &[&str] = &[
    "wifi",
    "bluetooth",
    "airplane_mode",
    "mobile_data",
    "location",
    "auto_rotate",
    "do_not_disturb",
    "hotspot",
    "nfc",
    "flashlight",
];
/// Volume stream types.
pub const ENUM_VOLUME_STREAMS: &[&str] = &["media", "ring", "notification", "alarm", "system"];
/// Messaging apps.
pub const ENUM_MSG_APPS: &[&str] = &[
    "sms",
    "whatsapp",
    "telegram",
    "signal",
    "messenger",
    "default",
];

/// All enum value sets, indexed by the `u8` in `ParamType::Enum`.
pub const ENUM_VALUE_SETS: &[&[&str]] = &[
    ENUM_SCROLL_DIRS,    // 0
    ENUM_SETTINGS,       // 1
    ENUM_VOLUME_STREAMS, // 2
    ENUM_MSG_APPS,       // 3
];

/// Look up the allowed values for an `Enum(idx)` parameter type.
pub fn enum_values(idx: u8) -> Option<&'static [&'static str]> {
    ENUM_VALUE_SETS.get(idx as usize).copied()
}

// ---------------------------------------------------------------------------
// Tool parameter tables (static, zero-alloc)
// ---------------------------------------------------------------------------

const P_SCREEN_TAP: &[ToolParameter] = &[
    ToolParameter {
        name: "x",
        param_type: ParamType::Number,
        required: false,
        description: "X coordinate to tap",
    },
    ToolParameter {
        name: "y",
        param_type: ParamType::Number,
        required: false,
        description: "Y coordinate to tap",
    },
    ToolParameter {
        name: "element",
        param_type: ParamType::String,
        required: false,
        description: "Element description to tap on",
    },
];

const P_SCREEN_SWIPE: &[ToolParameter] = &[
    ToolParameter {
        name: "from_x",
        param_type: ParamType::Number,
        required: true,
        description: "Start X",
    },
    ToolParameter {
        name: "from_y",
        param_type: ParamType::Number,
        required: true,
        description: "Start Y",
    },
    ToolParameter {
        name: "to_x",
        param_type: ParamType::Number,
        required: true,
        description: "End X",
    },
    ToolParameter {
        name: "to_y",
        param_type: ParamType::Number,
        required: true,
        description: "End Y",
    },
    ToolParameter {
        name: "duration_ms",
        param_type: ParamType::Number,
        required: false,
        description: "Swipe duration in ms",
    },
];

const P_SCREEN_TYPE: &[ToolParameter] = &[ToolParameter {
    name: "text",
    param_type: ParamType::String,
    required: true,
    description: "Text to type",
}];

const P_SCREEN_SCROLL: &[ToolParameter] = &[
    ToolParameter {
        name: "direction",
        param_type: ParamType::Enum(0),
        required: true,
        description: "Scroll direction",
    },
    ToolParameter {
        name: "amount",
        param_type: ParamType::Number,
        required: false,
        description: "Scroll amount in pixels",
    },
];

const P_EMPTY: &[ToolParameter] = &[];

const P_APP_OPEN: &[ToolParameter] = &[ToolParameter {
    name: "app",
    param_type: ParamType::AppName,
    required: true,
    description: "App to open",
}];

const P_APP_SWITCH: &[ToolParameter] = &[ToolParameter {
    name: "app",
    param_type: ParamType::AppName,
    required: false,
    description: "App to switch to (if omitted, cycles recents)",
}];

const P_NOTIFICATION_ACT: &[ToolParameter] = &[
    ToolParameter {
        name: "index",
        param_type: ParamType::Number,
        required: true,
        description: "Notification index",
    },
    ToolParameter {
        name: "action",
        param_type: ParamType::String,
        required: true,
        description: "Action label (e.g. Reply, Mark as read)",
    },
];

const P_MESSAGE_SEND: &[ToolParameter] = &[
    ToolParameter {
        name: "contact",
        param_type: ParamType::ContactName,
        required: true,
        description: "Recipient contact name",
    },
    ToolParameter {
        name: "text",
        param_type: ParamType::String,
        required: true,
        description: "Message text",
    },
    ToolParameter {
        name: "app",
        param_type: ParamType::Enum(3),
        required: false,
        description: "Messaging app to use (default: sms)",
    },
];

const P_CALL_MAKE: &[ToolParameter] = &[ToolParameter {
    name: "contact",
    param_type: ParamType::ContactName,
    required: true,
    description: "Contact to call",
}];

const P_ALARM_SET: &[ToolParameter] = &[
    ToolParameter {
        name: "time",
        param_type: ParamType::DateTime,
        required: true,
        description: "Alarm time",
    },
    ToolParameter {
        name: "label",
        param_type: ParamType::String,
        required: false,
        description: "Alarm label",
    },
    ToolParameter {
        name: "repeat",
        param_type: ParamType::String,
        required: false,
        description: "Repeat days (e.g. 'weekdays', 'Mon,Wed,Fri')",
    },
];

const P_TIMER_SET: &[ToolParameter] = &[
    ToolParameter {
        name: "duration",
        param_type: ParamType::Duration,
        required: true,
        description: "Timer duration",
    },
    ToolParameter {
        name: "label",
        param_type: ParamType::String,
        required: false,
        description: "Timer label",
    },
];

const P_REMINDER_CREATE: &[ToolParameter] = &[
    ToolParameter {
        name: "text",
        param_type: ParamType::String,
        required: true,
        description: "Reminder text",
    },
    ToolParameter {
        name: "time",
        param_type: ParamType::DateTime,
        required: false,
        description: "When to remind",
    },
];

const P_CALENDAR_EVENT: &[ToolParameter] = &[
    ToolParameter {
        name: "title",
        param_type: ParamType::String,
        required: true,
        description: "Event title",
    },
    ToolParameter {
        name: "start_time",
        param_type: ParamType::DateTime,
        required: true,
        description: "Event start time",
    },
    ToolParameter {
        name: "end_time",
        param_type: ParamType::DateTime,
        required: false,
        description: "Event end time",
    },
    ToolParameter {
        name: "location",
        param_type: ParamType::String,
        required: false,
        description: "Event location",
    },
];

const P_SEARCH_WEB: &[ToolParameter] = &[ToolParameter {
    name: "query",
    param_type: ParamType::String,
    required: true,
    description: "Search query",
}];

const P_SEARCH_DEVICE: &[ToolParameter] = &[ToolParameter {
    name: "query",
    param_type: ParamType::String,
    required: true,
    description: "Search query",
}];

const P_SETTINGS_TOGGLE: &[ToolParameter] = &[
    ToolParameter {
        name: "setting",
        param_type: ParamType::Enum(1),
        required: true,
        description: "Setting to toggle",
    },
    ToolParameter {
        name: "state",
        param_type: ParamType::Boolean,
        required: false,
        description: "Desired state (true=on, false=off). If omitted, toggles.",
    },
];

const P_VOLUME_SET: &[ToolParameter] = &[
    ToolParameter {
        name: "level",
        param_type: ParamType::Percentage,
        required: true,
        description: "Volume level (0-100)",
    },
    ToolParameter {
        name: "stream",
        param_type: ParamType::Enum(2),
        required: false,
        description: "Audio stream type",
    },
];

const P_BRIGHTNESS_SET: &[ToolParameter] = &[ToolParameter {
    name: "level",
    param_type: ParamType::Percentage,
    required: true,
    description: "Brightness level (0-100)",
}];

const P_FILE_SHARE: &[ToolParameter] = &[
    ToolParameter {
        name: "file",
        param_type: ParamType::FilePath,
        required: true,
        description: "File to share",
    },
    ToolParameter {
        name: "app",
        param_type: ParamType::AppName,
        required: false,
        description: "App to share via",
    },
];

const P_CLIPBOARD_COPY: &[ToolParameter] = &[ToolParameter {
    name: "text",
    param_type: ParamType::String,
    required: true,
    description: "Text to copy",
}];

const P_WAIT: &[ToolParameter] = &[ToolParameter {
    name: "duration",
    param_type: ParamType::Duration,
    required: true,
    description: "How long to wait",
}];

const P_VERIFY_RESULT: &[ToolParameter] = &[
    ToolParameter {
        name: "expected",
        param_type: ParamType::String,
        required: true,
        description: "Expected text or element on screen",
    },
    ToolParameter {
        name: "timeout_ms",
        param_type: ParamType::Number,
        required: false,
        description: "Max wait time for result",
    },
];

// ---------------------------------------------------------------------------
// Tool Registry
// ---------------------------------------------------------------------------

/// The complete registry of all tools AURA can invoke.
pub static TOOL_REGISTRY: &[ToolSchema] = &[
    ToolSchema {
        name: "screen_tap",
        description: "Tap at coordinates or on a UI element",
        parameters: P_SCREEN_TAP,
        requires_screen: true,
        requires_network: false,
        risk_level: RiskLevel::Low,
        estimated_duration_ms: 500,
    },
    ToolSchema {
        name: "screen_swipe",
        description: "Perform a swipe gesture between two points",
        parameters: P_SCREEN_SWIPE,
        requires_screen: true,
        requires_network: false,
        risk_level: RiskLevel::Low,
        estimated_duration_ms: 800,
    },
    ToolSchema {
        name: "screen_type",
        description: "Type text into the currently focused input field",
        parameters: P_SCREEN_TYPE,
        requires_screen: true,
        requires_network: false,
        risk_level: RiskLevel::Medium,
        estimated_duration_ms: 1000,
    },
    ToolSchema {
        name: "screen_scroll",
        description: "Scroll the screen in a direction",
        parameters: P_SCREEN_SCROLL,
        requires_screen: true,
        requires_network: false,
        risk_level: RiskLevel::Low,
        estimated_duration_ms: 600,
    },
    ToolSchema {
        name: "screen_back",
        description: "Press the back button",
        parameters: P_EMPTY,
        requires_screen: true,
        requires_network: false,
        risk_level: RiskLevel::Low,
        estimated_duration_ms: 300,
    },
    ToolSchema {
        name: "screen_home",
        description: "Press the home button to go to the home screen",
        parameters: P_EMPTY,
        requires_screen: true,
        requires_network: false,
        risk_level: RiskLevel::Low,
        estimated_duration_ms: 300,
    },
    ToolSchema {
        name: "app_open",
        description: "Open an installed app by name",
        parameters: P_APP_OPEN,
        requires_screen: true,
        requires_network: false,
        risk_level: RiskLevel::Low,
        estimated_duration_ms: 2000,
    },
    ToolSchema {
        name: "app_switch",
        description: "Switch to a recently used app",
        parameters: P_APP_SWITCH,
        requires_screen: true,
        requires_network: false,
        risk_level: RiskLevel::Low,
        estimated_duration_ms: 1000,
    },
    ToolSchema {
        name: "notification_read",
        description: "Read current notifications from the notification shade",
        parameters: P_EMPTY,
        requires_screen: false,
        requires_network: false,
        risk_level: RiskLevel::Low,
        estimated_duration_ms: 500,
    },
    ToolSchema {
        name: "notification_act",
        description: "Perform an action on a notification (reply, dismiss, etc.)",
        parameters: P_NOTIFICATION_ACT,
        requires_screen: false,
        requires_network: false,
        risk_level: RiskLevel::Medium,
        estimated_duration_ms: 800,
    },
    ToolSchema {
        name: "message_send",
        description: "Send a text message to a contact via a messaging app",
        parameters: P_MESSAGE_SEND,
        requires_screen: true,
        requires_network: true,
        risk_level: RiskLevel::Medium,
        estimated_duration_ms: 5000,
    },
    ToolSchema {
        name: "call_make",
        description: "Make a phone call to a contact",
        parameters: P_CALL_MAKE,
        requires_screen: true,
        requires_network: true,
        risk_level: RiskLevel::Medium,
        estimated_duration_ms: 3000,
    },
    ToolSchema {
        name: "call_answer",
        description: "Answer an incoming phone call",
        parameters: P_EMPTY,
        requires_screen: true,
        requires_network: false,
        risk_level: RiskLevel::Low,
        estimated_duration_ms: 500,
    },
    ToolSchema {
        name: "call_reject",
        description: "Reject an incoming phone call",
        parameters: P_EMPTY,
        requires_screen: true,
        requires_network: false,
        risk_level: RiskLevel::Low,
        estimated_duration_ms: 500,
    },
    ToolSchema {
        name: "alarm_set",
        description: "Set an alarm at a specified time",
        parameters: P_ALARM_SET,
        requires_screen: true,
        requires_network: false,
        risk_level: RiskLevel::Low,
        estimated_duration_ms: 3000,
    },
    ToolSchema {
        name: "timer_set",
        description: "Set a countdown timer for a specified duration",
        parameters: P_TIMER_SET,
        requires_screen: true,
        requires_network: false,
        risk_level: RiskLevel::Low,
        estimated_duration_ms: 2000,
    },
    ToolSchema {
        name: "reminder_create",
        description: "Create a reminder with optional time",
        parameters: P_REMINDER_CREATE,
        requires_screen: true,
        requires_network: false,
        risk_level: RiskLevel::Low,
        estimated_duration_ms: 3000,
    },
    ToolSchema {
        name: "calendar_event",
        description: "Create a calendar event with title, time, and optional location",
        parameters: P_CALENDAR_EVENT,
        requires_screen: true,
        requires_network: false,
        risk_level: RiskLevel::Medium,
        estimated_duration_ms: 4000,
    },
    ToolSchema {
        name: "search_web",
        description: "Search the web using the default browser",
        parameters: P_SEARCH_WEB,
        requires_screen: true,
        requires_network: true,
        risk_level: RiskLevel::Low,
        estimated_duration_ms: 3000,
    },
    ToolSchema {
        name: "search_device",
        description: "Search for files, apps, or content on the device",
        parameters: P_SEARCH_DEVICE,
        requires_screen: false,
        requires_network: false,
        risk_level: RiskLevel::Low,
        estimated_duration_ms: 1000,
    },
    ToolSchema {
        name: "settings_toggle",
        description: "Toggle a system setting (wifi, bluetooth, airplane mode, etc.)",
        parameters: P_SETTINGS_TOGGLE,
        requires_screen: false,
        requires_network: false,
        risk_level: RiskLevel::Medium,
        estimated_duration_ms: 1000,
    },
    ToolSchema {
        name: "volume_set",
        description: "Set the device volume level for a specific audio stream",
        parameters: P_VOLUME_SET,
        requires_screen: false,
        requires_network: false,
        risk_level: RiskLevel::Low,
        estimated_duration_ms: 300,
    },
    ToolSchema {
        name: "brightness_set",
        description: "Set the screen brightness level",
        parameters: P_BRIGHTNESS_SET,
        requires_screen: false,
        requires_network: false,
        risk_level: RiskLevel::Low,
        estimated_duration_ms: 300,
    },
    ToolSchema {
        name: "screenshot_take",
        description: "Take a screenshot of the current screen",
        parameters: P_EMPTY,
        requires_screen: true,
        requires_network: false,
        risk_level: RiskLevel::Low,
        estimated_duration_ms: 500,
    },
    ToolSchema {
        name: "file_share",
        description: "Share a file via a specified app",
        parameters: P_FILE_SHARE,
        requires_screen: true,
        requires_network: false,
        risk_level: RiskLevel::Medium,
        estimated_duration_ms: 3000,
    },
    ToolSchema {
        name: "clipboard_copy",
        description: "Copy text to the system clipboard",
        parameters: P_CLIPBOARD_COPY,
        requires_screen: false,
        requires_network: false,
        risk_level: RiskLevel::Low,
        estimated_duration_ms: 100,
    },
    ToolSchema {
        name: "clipboard_paste",
        description: "Paste text from the system clipboard into the focused field",
        parameters: P_EMPTY,
        requires_screen: true,
        requires_network: false,
        risk_level: RiskLevel::Medium,
        estimated_duration_ms: 500,
    },
    ToolSchema {
        name: "wait",
        description: "Wait for a specified duration before continuing",
        parameters: P_WAIT,
        requires_screen: false,
        requires_network: false,
        risk_level: RiskLevel::Low,
        estimated_duration_ms: 0, // variable
    },
    ToolSchema {
        name: "observe_screen",
        description: "Read and describe the current screen state (visible elements, text, layout)",
        parameters: P_EMPTY,
        requires_screen: true,
        requires_network: false,
        risk_level: RiskLevel::Low,
        estimated_duration_ms: 800,
    },
    ToolSchema {
        name: "verify_result",
        description: "Check if an expected result or element is visible on screen",
        parameters: P_VERIFY_RESULT,
        requires_screen: true,
        requires_network: false,
        risk_level: RiskLevel::Low,
        estimated_duration_ms: 1000,
    },
];

// ---------------------------------------------------------------------------
// LLM description generation
// ---------------------------------------------------------------------------

/// Generate a human-readable tool description for the LLM system prompt.
///
/// The output format is designed for maximum clarity to an LLM:
/// ```text
/// ## Available Tools
///
/// ### screen_tap
/// Tap at coordinates or on a UI element
/// Parameters:
///   - x (number, optional): X coordinate to tap
///   - y (number, optional): Y coordinate to tap
///   - element (string, optional): Element description to tap on
/// Requires: screen
/// Risk: low
/// ```
pub fn tools_as_llm_description() -> String {
    let mut out = String::with_capacity(8192);
    out.push_str("## Available Tools\n\n");
    out.push_str("You may invoke the following tools. ");
    out.push_str("Provide parameters as a JSON object.\n\n");

    for tool in TOOL_REGISTRY {
        out.push_str("### ");
        out.push_str(tool.name);
        out.push('\n');
        out.push_str(tool.description);
        out.push('\n');

        if !tool.parameters.is_empty() {
            out.push_str("Parameters:\n");
            for p in tool.parameters {
                out.push_str("  - ");
                out.push_str(p.name);
                out.push_str(" (");
                out.push_str(p.param_type.type_name());
                if let ParamType::Enum(idx) = p.param_type {
                    if let Some(vals) = enum_values(idx) {
                        out.push_str(": ");
                        for (i, v) in vals.iter().enumerate() {
                            if i > 0 {
                                out.push('|');
                            }
                            out.push_str(v);
                        }
                    }
                }
                out.push_str(", ");
                out.push_str(if p.required { "required" } else { "optional" });
                out.push_str("): ");
                out.push_str(p.description);
                out.push('\n');
            }
        }

        let mut reqs = Vec::new();
        if tool.requires_screen {
            reqs.push("screen");
        }
        if tool.requires_network {
            reqs.push("network");
        }
        if !reqs.is_empty() {
            out.push_str("Requires: ");
            out.push_str(&reqs.join(", "));
            out.push('\n');
        }
        out.push_str("Risk: ");
        out.push_str(tool.risk_level.label());
        out.push_str("\n\n");
    }

    out
}

/// Find a tool schema by name.
pub fn find_tool(name: &str) -> Option<&'static ToolSchema> {
    TOOL_REGISTRY.iter().find(|t| t.name == name)
}

/// Return tool names that match a given risk level.
pub fn tools_by_risk(level: RiskLevel) -> Vec<&'static str> {
    TOOL_REGISTRY
        .iter()
        .filter(|t| t.risk_level == level)
        .map(|t| t.name)
        .collect()
}

/// A resolved tool invocation with concrete parameter values.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolInvocation {
    pub tool_name: String,
    /// Bounded at runtime to MAX_TOOL_PARAMETERS entries — enforced by consumer.
    pub parameters: Vec<(String, ParamValue)>,
}

/// Concrete parameter value after parsing and validation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ParamValue {
    String(String),
    Number(f64),
    Boolean(bool),
    DateTime(u64), // unix timestamp ms
    Duration(u64), // duration in ms
    Coordinate(i32, i32),
    Null,
}

impl ParamValue {
    /// Get as string, if it is one.
    pub fn as_str(&self) -> Option<&str> {
        match self {
            ParamValue::String(s) => Some(s),
            _ => None,
        }
    }

    /// Get as number, coercing if possible.
    pub fn as_number(&self) -> Option<f64> {
        match self {
            ParamValue::Number(n) => Some(*n),
            _ => None,
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
    fn test_registry_has_30_tools() {
        assert_eq!(TOOL_REGISTRY.len(), 30);
    }

    #[test]
    fn test_all_tool_names_unique() {
        let mut names: Vec<&str> = TOOL_REGISTRY.iter().map(|t| t.name).collect();
        names.sort();
        let before = names.len();
        names.dedup();
        assert_eq!(names.len(), before, "duplicate tool names found");
    }

    #[test]
    fn test_find_tool() {
        assert!(find_tool("screen_tap").is_some());
        assert!(find_tool("message_send").is_some());
        assert!(find_tool("nonexistent").is_none());
    }

    #[test]
    fn test_risk_level_confirmation() {
        assert!(!RiskLevel::Low.requires_confirmation());
        assert!(!RiskLevel::Medium.requires_confirmation());
        assert!(RiskLevel::High.requires_confirmation());
        assert!(RiskLevel::Critical.requires_confirmation());
    }

    #[test]
    fn test_tools_as_llm_description_contains_all_tools() {
        let desc = tools_as_llm_description();
        for tool in TOOL_REGISTRY {
            assert!(desc.contains(tool.name), "missing tool: {}", tool.name);
        }
    }

    #[test]
    fn test_enum_values_lookup() {
        assert_eq!(enum_values(0), Some(ENUM_SCROLL_DIRS));
        assert_eq!(enum_values(1), Some(ENUM_SETTINGS));
        assert!(enum_values(99).is_none());
    }

    #[test]
    fn test_tools_by_risk() {
        let low = tools_by_risk(RiskLevel::Low);
        assert!(low.contains(&"screen_tap"));
        assert!(low.contains(&"observe_screen"));
        assert!(!low.contains(&"message_send"));
    }

    #[test]
    fn test_param_value_accessors() {
        let s = ParamValue::String("hello".to_string());
        assert_eq!(s.as_str(), Some("hello"));
        assert!(s.as_number().is_none());

        let n = ParamValue::Number(42.0);
        assert_eq!(n.as_number(), Some(42.0));
        assert!(n.as_str().is_none());
    }

    #[test]
    fn test_tool_schema_screen_requirements() {
        let tap = find_tool("screen_tap").unwrap();
        assert!(tap.requires_screen);
        assert!(!tap.requires_network);

        let msg = find_tool("message_send").unwrap();
        assert!(msg.requires_screen);
        assert!(msg.requires_network);

        let clip = find_tool("clipboard_copy").unwrap();
        assert!(!clip.requires_screen);
    }

    #[test]
    fn test_message_send_has_required_params() {
        let tool = find_tool("message_send").unwrap();
        let required: Vec<&str> = tool
            .parameters
            .iter()
            .filter(|p| p.required)
            .map(|p| p.name)
            .collect();
        assert!(required.contains(&"contact"));
        assert!(required.contains(&"text"));
    }
}
