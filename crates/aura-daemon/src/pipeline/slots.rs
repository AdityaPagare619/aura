//! Slot filling system for AURA v4's NLP pipeline.
//!
//! Once the parser identifies an intent, the slot filler determines which
//! parameters are still missing and either:
//! 1. Infers them from context (recent conversation, working memory)
//! 2. Generates a natural clarification question for the user

use aura_types::tools::{find_tool, ParamType, ToolSchema};
use tracing::{debug, instrument, trace};

use super::entity::{Entity, EntityType};

// Cap on recent entities retained in conversation context.
const MAX_RECENT_ENTITIES: usize = 10;

// ---------------------------------------------------------------------------
// Slot types
// ---------------------------------------------------------------------------

/// State of a single slot in the filling process.
#[derive(Debug, Clone)]
pub enum SlotState {
    /// Slot has a filled value.
    Filled(String),
    /// Slot was inferred from context (lower confidence).
    Inferred(String),
    /// Slot is missing and required.
    Missing,
    /// Slot is optional and not provided.
    Optional,
}

impl SlotState {
    /// Whether this slot has any value (filled or inferred).
    pub fn has_value(&self) -> bool {
        matches!(self, SlotState::Filled(_) | SlotState::Inferred(_))
    }

    /// Get the value if filled or inferred.
    pub fn value(&self) -> Option<&str> {
        match self {
            SlotState::Filled(v) | SlotState::Inferred(v) => Some(v),
            _ => None,
        }
    }
}

/// A slot with its definition and current state.
#[derive(Debug, Clone)]
pub struct Slot {
    pub name: String,
    pub param_type: ParamType,
    pub required: bool,
    pub description: String,
    pub state: SlotState,
}

/// Result of the slot-filling process.
#[derive(Debug, Clone)]
pub struct SlotFillingResult {
    /// All slots with their states.
    pub slots: Vec<Slot>,
    /// Whether all required slots are filled.
    pub complete: bool,
    /// Names of missing required slots.
    pub missing: Vec<String>,
    /// A natural clarification question (if slots are missing).
    pub clarification: Option<String>,
}

// ---------------------------------------------------------------------------
// Context for inference
// ---------------------------------------------------------------------------

/// Conversational context for slot inference.
///
/// The slot filler uses this to resolve pronouns and implicit references.
/// E.g., "call him" → resolve "him" to the most recent contact.
#[derive(Debug, Clone, Default)]
pub struct ConversationContext {
    /// Most recently mentioned contact name.
    pub last_contact: Option<String>,
    /// Most recently mentioned app name.
    pub last_app: Option<String>,
    /// Most recently mentioned time.
    pub last_time: Option<String>,
    /// Most recently mentioned text/query.
    pub last_text: Option<String>,
    /// Recent entity mentions (last 10).
    pub recent_entities: Vec<Entity>,
}

impl ConversationContext {
    /// Update context with newly extracted entities.
    pub fn update(&mut self, entities: &[Entity]) {
        for entity in entities {
            match entity.entity_type {
                EntityType::Contact => {
                    self.last_contact = Some(entity.value.clone());
                }
                EntityType::App => {
                    self.last_app = Some(entity.value.clone());
                }
                EntityType::Time => {
                    self.last_time = Some(entity.value.clone());
                }
                _ => {}
            }
            self.recent_entities.push(entity.clone());
        }

        // Keep only last MAX_RECENT_ENTITIES entities.
        if self.recent_entities.len() > MAX_RECENT_ENTITIES {
            let drain = self.recent_entities.len() - MAX_RECENT_ENTITIES;
            self.recent_entities.drain(..drain);
        }
    }
}

// ---------------------------------------------------------------------------
// Slot filler
// ---------------------------------------------------------------------------

/// Fills parameter slots for a given tool using extracted entities and context.
pub struct SlotFiller;

impl SlotFiller {
    /// Fill slots for a tool using extracted entities and conversation context.
    ///
    /// Algorithm:
    /// 1. Build slot list from tool schema
    /// 2. Match entities to slots by type compatibility
    /// 3. Attempt context inference for missing required slots
    /// 4. Generate clarification for any remaining missing required slots
    #[instrument(skip(entities, context), fields(tool = tool_name, entity_count = entities.len()))]
    pub fn fill(
        tool_name: &str,
        entities: &[Entity],
        context: &ConversationContext,
    ) -> SlotFillingResult {
        let tool = match find_tool(tool_name) {
            Some(t) => t,
            None => {
                debug!(tool = tool_name, "tool not found in registry");
                return SlotFillingResult {
                    slots: Vec::new(),
                    complete: false,
                    missing: vec![tool_name.to_string()],
                    clarification: Some(format!("Unknown tool: {}", tool_name)),
                };
            }
        };

        let mut slots = build_slots(tool);

        // Step 1: Match entities to slots by type compatibility.
        for entity in entities {
            match_entity_to_slots(entity, &mut slots);
        }

        // Step 2: Context inference for missing required slots.
        infer_from_context(&mut slots, context);

        // Step 3: Determine completeness and generate clarification.
        let missing: Vec<String> = slots
            .iter()
            .filter(|s| s.required && !s.state.has_value())
            .map(|s| s.name.clone())
            .collect();

        let complete = missing.is_empty();
        let clarification = if complete {
            None
        } else {
            Some(generate_clarification(tool, &missing))
        };

        trace!(
            complete = complete,
            missing_count = missing.len(),
            "slot filling complete"
        );

        SlotFillingResult {
            slots,
            complete,
            missing,
            clarification,
        }
    }

    /// Quick check: does this tool have all required slots fillable from the
    /// given entities (without context)?
    pub fn can_fill_required(tool_name: &str, entities: &[Entity]) -> bool {
        let ctx = ConversationContext::default();
        let result = Self::fill(tool_name, entities, &ctx);
        result.complete
    }
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Build the initial slot list from a tool's parameter definitions.
fn build_slots(tool: &ToolSchema) -> Vec<Slot> {
    tool.parameters
        .iter()
        .map(|p| Slot {
            name: p.name.to_string(),
            param_type: p.param_type,
            required: p.required,
            description: p.description.to_string(),
            state: if p.required {
                SlotState::Missing
            } else {
                SlotState::Optional
            },
        })
        .collect()
}

/// Try to match an entity to an unfilled slot by type compatibility.
fn match_entity_to_slots(entity: &Entity, slots: &mut [Slot]) {
    for slot in slots.iter_mut() {
        if slot.state.has_value() {
            continue; // Already filled.
        }

        if is_type_compatible(&entity.entity_type, &slot.param_type) {
            trace!(
                entity = ?entity.entity_type,
                slot = slot.name,
                value = entity.value,
                "matched entity to slot"
            );
            slot.state = SlotState::Filled(entity.value.clone());
            return; // Each entity fills at most one slot.
        }
    }
}

/// Check if an entity type is compatible with a parameter type.
fn is_type_compatible(entity_type: &EntityType, param_type: &ParamType) -> bool {
    matches!(
        (entity_type, param_type),
        (EntityType::Contact, ParamType::ContactName)
            | (EntityType::App, ParamType::AppName)
            | (EntityType::Time, ParamType::DateTime)
            | (EntityType::Duration, ParamType::Duration)
            | (EntityType::Number, ParamType::Number)
            | (EntityType::Number, ParamType::Percentage)
            | (EntityType::Url, ParamType::Url)
            | (EntityType::Setting, ParamType::Enum(_))
            // String is a catch-all for any entity type.
            | (_, ParamType::String)
    )
}

/// Try to infer missing required slot values from conversation context.
fn infer_from_context(slots: &mut [Slot], context: &ConversationContext) {
    for slot in slots.iter_mut() {
        if slot.state.has_value() || !slot.required {
            continue;
        }

        let inferred = match slot.param_type {
            ParamType::ContactName => context.last_contact.as_deref(),
            ParamType::AppName => context.last_app.as_deref(),
            ParamType::DateTime => context.last_time.as_deref(),
            _ => None,
        };

        if let Some(value) = inferred {
            trace!(
                slot = slot.name,
                value = value,
                "inferred slot from context"
            );
            slot.state = SlotState::Inferred(value.to_string());
        }
    }
}

/// Generate a natural clarification question for missing slots.
fn generate_clarification(tool: &ToolSchema, missing: &[String]) -> String {
    if missing.len() == 1 {
        let slot_name = &missing[0];
        // Find the parameter description.
        let desc = tool
            .parameters
            .iter()
            .find(|p| p.name == slot_name.as_str())
            .map(|p| p.description)
            .unwrap_or("value");

        match slot_name.as_str() {
            "contact" => "Who would you like to contact?".to_string(),
            "app" => "Which app should I use?".to_string(),
            "text" => "What would you like to say?".to_string(),
            "time" | "start_time" => "What time?".to_string(),
            "duration" => "For how long?".to_string(),
            "query" => "What would you like to search for?".to_string(),
            "setting" => "Which setting would you like to change?".to_string(),
            "level" => "What level? (0-100)".to_string(),
            "title" => "What should I call it?".to_string(),
            _ => format!("What {} should I use?", desc),
        }
    } else {
        let names: Vec<&str> = missing.iter().map(|s| s.as_str()).collect();
        let last = names
            .last()
            .expect("names is non-empty: inside else branch where missing.len() > 1");
        if names.len() == 2 {
            format!("I need the {} and the {}.", names[0], last)
        } else {
            let init = &names[..names.len() - 1];
            format!("I need the {}, and the {}.", init.join(", "), last)
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn make_entity(etype: EntityType, value: &str) -> Entity {
        Entity {
            entity_type: etype,
            raw: value.to_string(),
            value: value.to_string(),
            span_start: 0,
            span_end: value.len(),
            confidence: 0.9,
        }
    }

    #[test]
    fn test_fill_message_send_complete() {
        let entities = vec![
            make_entity(EntityType::Contact, "Alice"),
            make_entity(EntityType::Unknown, "Hello there"),
        ];
        let ctx = ConversationContext::default();
        let result = SlotFiller::fill("message_send", &entities, &ctx);
        // Contact is filled, text gets "Hello there" via String catch-all.
        assert!(
            result
                .slots
                .iter()
                .any(|s| s.name == "contact" && s.state.has_value()),
            "contact should be filled"
        );
    }

    #[test]
    fn test_fill_message_send_missing_contact() {
        let entities = vec![make_entity(EntityType::Unknown, "Hello there")];
        let ctx = ConversationContext::default();
        let result = SlotFiller::fill("message_send", &entities, &ctx);
        assert!(!result.complete);
        assert!(result.missing.contains(&"contact".to_string()));
        assert!(result.clarification.is_some());
    }

    #[test]
    fn test_fill_with_context_inference() {
        let entities = vec![make_entity(EntityType::Unknown, "call him")];
        let mut ctx = ConversationContext::default();
        ctx.last_contact = Some("Bob".to_string());
        let result = SlotFiller::fill("call_make", &entities, &ctx);
        let contact_slot = result.slots.iter().find(|s| s.name == "contact").unwrap();
        assert!(
            matches!(&contact_slot.state, SlotState::Inferred(v) if v == "Bob"),
            "should infer contact from context"
        );
    }

    #[test]
    fn test_fill_alarm_set() {
        let entities = vec![make_entity(EntityType::Time, "07:00")];
        let ctx = ConversationContext::default();
        let result = SlotFiller::fill("alarm_set", &entities, &ctx);
        assert!(
            result.complete,
            "alarm_set should be complete with just time"
        );
    }

    #[test]
    fn test_fill_unknown_tool() {
        let result = SlotFiller::fill("nonexistent_tool", &[], &ConversationContext::default());
        assert!(!result.complete);
        assert!(result.clarification.as_ref().unwrap().contains("Unknown"));
    }

    #[test]
    fn test_clarification_single_slot() {
        let entities: Vec<Entity> = vec![];
        let ctx = ConversationContext::default();
        let result = SlotFiller::fill("call_make", &entities, &ctx);
        let q = result.clarification.unwrap();
        assert!(
            q.contains("contact") || q.contains("Who"),
            "should ask about contact: {}",
            q
        );
    }

    #[test]
    fn test_context_update() {
        let mut ctx = ConversationContext::default();
        let entities = vec![
            make_entity(EntityType::Contact, "Alice"),
            make_entity(EntityType::App, "WhatsApp"),
        ];
        ctx.update(&entities);
        assert_eq!(ctx.last_contact.as_deref(), Some("Alice"));
        assert_eq!(ctx.last_app.as_deref(), Some("WhatsApp"));
    }

    #[test]
    fn test_slot_state_has_value() {
        assert!(SlotState::Filled("x".to_string()).has_value());
        assert!(SlotState::Inferred("x".to_string()).has_value());
        assert!(!SlotState::Missing.has_value());
        assert!(!SlotState::Optional.has_value());
    }

    #[test]
    fn test_can_fill_required() {
        let entities = vec![make_entity(EntityType::Time, "07:00")];
        assert!(SlotFiller::can_fill_required("alarm_set", &entities));

        let empty: Vec<Entity> = vec![];
        assert!(!SlotFiller::can_fill_required("call_make", &empty));
    }
}
