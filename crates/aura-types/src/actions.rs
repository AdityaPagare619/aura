use serde::{Deserialize, Serialize};

/// Every action AURA can perform on the device screen.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ActionType {
    Tap {
        x: i32,
        y: i32,
    },
    LongPress {
        x: i32,
        y: i32,
    },
    Swipe {
        from_x: i32,
        from_y: i32,
        to_x: i32,
        to_y: i32,
        duration_ms: u32,
    },
    Type {
        text: String,
    },
    Scroll {
        direction: ScrollDirection,
        amount: i32,
    },
    Back,
    Home,
    Recents,
    OpenApp {
        package: String,
    },
    NotificationAction {
        notification_id: u32,
        action_index: u32,
    },
    WaitForElement {
        selector: TargetSelector,
        timeout_ms: u32,
    },
    AssertElement {
        selector: TargetSelector,
        expected: ElementAssertion,
    },
}

/// Scroll direction for Scroll actions.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum ScrollDirection {
    Up,
    Down,
    Left,
    Right,
}

/// How to locate a target UI element.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum TargetSelector {
    XPath(String),
    ResourceId(String),
    Text(String),
    ContentDescription(String),
    ClassName(String),
    Position {
        index: u32,
        parent_selector: Box<TargetSelector>,
    },
    Coordinates {
        x: i32,
        y: i32,
    },
    /// Natural-language description for LLM-based element matching.
    LlmDescription(String),
}

/// Assertion to verify against a target element.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ElementAssertion {
    Exists,
    NotExists,
    TextEquals(String),
    TextContains(String),
    IsEnabled,
    IsChecked,
}

/// Result returned after executing an action.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionResult {
    pub success: bool,
    pub duration_ms: u32,
    pub error: Option<String>,
    pub screen_changed: bool,
    pub matched_element: Option<String>,
}

/// Default timeout constants per action type (in milliseconds).
pub struct ActionTimeout;

impl ActionTimeout {
    pub const TAP: u32 = 2_000;
    pub const TYPE: u32 = 5_000;
    pub const SWIPE: u32 = 3_000;
    pub const SCROLL: u32 = 2_000;
    pub const WAIT: u32 = 30_000;
    pub const ASSERT: u32 = 3_000;
    pub const BACK: u32 = 1_500;
    pub const HOME: u32 = 1_500;
    pub const RECENTS: u32 = 1_500;
    pub const OPEN_APP: u32 = 10_000;
    pub const NOTIFICATION: u32 = 3_000;
    pub const LONG_PRESS: u32 = 3_000;
}

impl ActionType {
    /// Returns the default timeout for this action type.
    #[must_use]
    pub fn default_timeout(&self) -> u32 {
        match self {
            ActionType::Tap { .. } => ActionTimeout::TAP,
            ActionType::LongPress { .. } => ActionTimeout::LONG_PRESS,
            ActionType::Swipe { .. } => ActionTimeout::SWIPE,
            ActionType::Type { .. } => ActionTimeout::TYPE,
            ActionType::Scroll { .. } => ActionTimeout::SCROLL,
            ActionType::Back => ActionTimeout::BACK,
            ActionType::Home => ActionTimeout::HOME,
            ActionType::Recents => ActionTimeout::RECENTS,
            ActionType::OpenApp { .. } => ActionTimeout::OPEN_APP,
            ActionType::NotificationAction { .. } => ActionTimeout::NOTIFICATION,
            ActionType::WaitForElement { timeout_ms, .. } => *timeout_ms,
            ActionType::AssertElement { .. } => ActionTimeout::ASSERT,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_action_default_timeouts() {
        let tap = ActionType::Tap { x: 100, y: 200 };
        assert_eq!(tap.default_timeout(), 2_000);

        let open = ActionType::OpenApp {
            package: "com.example".to_string(),
        };
        assert_eq!(open.default_timeout(), 10_000);

        let wait = ActionType::WaitForElement {
            selector: TargetSelector::Text("OK".to_string()),
            timeout_ms: 15_000,
        };
        assert_eq!(wait.default_timeout(), 15_000);
    }

    #[test]
    fn test_target_selector_nested_position() {
        let selector = TargetSelector::Position {
            index: 2,
            parent_selector: Box::new(TargetSelector::ResourceId(
                "com.example:id/list".to_string(),
            )),
        };
        if let TargetSelector::Position {
            index,
            parent_selector,
        } = &selector
        {
            assert_eq!(*index, 2);
            assert!(matches!(
                parent_selector.as_ref(),
                TargetSelector::ResourceId(_)
            ));
        } else {
            panic!("expected Position variant");
        }
    }

    #[test]
    fn test_action_result_defaults() {
        let result = ActionResult {
            success: true,
            duration_ms: 150,
            error: None,
            screen_changed: true,
            matched_element: Some("btn_ok".to_string()),
        };
        assert!(result.success);
        assert!(result.error.is_none());
        assert!(result.screen_changed);
    }

    #[test]
    fn test_action_type_serialization_roundtrip() {
        let action = ActionType::Swipe {
            from_x: 100,
            from_y: 500,
            to_x: 100,
            to_y: 100,
            duration_ms: 300,
        };
        let json = serde_json::to_string(&action).unwrap();
        let deser: ActionType = serde_json::from_str(&json).unwrap();
        assert_eq!(action, deser);
    }
}
