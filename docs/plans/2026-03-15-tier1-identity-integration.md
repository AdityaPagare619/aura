# Tier 1 Identity Integration — Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add three new context sections to the LLM prompt — `identity_tendencies`, `user_preferences`, and `self_knowledge` — so AURA's neocortex has access to constitutional identity, user configuration, and self-awareness data at inference time.

**Architecture:** Extend the existing 4-hop pipeline (ContextPackage → PromptSlots → build_prompt → system_prompt) with three new Optional fields that thread through all layers. All new fields are `Option<T>` for backward compatibility. The new prompt sections are inserted between the existing Identity (role header) and Personality sections, maintaining the "who I am → what I know about myself → who I serve → how I behave" conceptual flow.

**Tech Stack:** Rust, serde (Serialize/Deserialize), aura-types crate (IPC types), aura-neocortex crate (context assembly + prompt building).

---

## Task 1: Define New Types in `aura-types/src/ipc.rs`

**Files:**
- Modify: `crates/aura-types/src/ipc.rs` (insert after line 411, before `impl ContextPackage`)

**Step 1: Add the `IdentityTendencies` struct**

Insert after the `ContextPackage` struct definition (after line 411) but before `impl ContextPackage` (line 413):

```rust
/// Constitutional first-person principles that define AURA's behavioral identity.
///
/// These are compact, first-person statements that the LLM internalizes as
/// core behavioral guidelines. They are NOT user-editable — they come from
/// the identity subsystem's constitutional layer.
///
/// Tier 1: Identity Integration (Phase 10).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct IdentityTendencies {
    /// Up to 5 first-person constitutional principles.
    /// Example: "I lead with curiosity before judgment."
    pub principles: Vec<String>,
}
```

**Step 2: Add the `UserPreferences` struct**

```rust
/// User-configured preferences that shape AURA's behavior.
///
/// These are explicitly set by the user (not inferred). The LLM uses them
/// to tailor interaction style, proactiveness, and scope.
///
/// SECURITY [SEC-MED-4]: `custom_instructions` is user-authored free text.
/// Must be wrapped in `[UNTRUSTED]` markers when injected into the prompt
/// to prevent indirect prompt injection.
///
/// Tier 1: Identity Integration (Phase 10).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct UserPreferences {
    /// Preferred model tier (e.g. "standard", "advanced", "efficient").
    pub model_preference: Option<String>,
    /// Interaction style (e.g. "concise", "detailed", "casual", "formal").
    pub interaction_style: Option<String>,
    /// How proactive AURA should be (0.0 = reactive only, 1.0 = fully proactive).
    pub proactiveness: Option<f32>,
    /// How much autonomy AURA has (0.0 = always ask, 1.0 = act independently).
    pub autonomy_level: Option<f32>,
    /// Access scope restrictions (e.g. ["contacts", "calendar", "files"]).
    pub access_scope: Vec<String>,
    /// Domain focus areas (e.g. ["productivity", "health", "social"]).
    pub domain_focus: Vec<String>,
    /// Free-text custom instructions from the user.
    /// SECURITY: Must be wrapped in [UNTRUSTED] markers in prompt.
    pub custom_instructions: Option<String>,
}
```

**Step 3: Add the `SelfKnowledge` struct**

```rust
/// Self-knowledge payload — what AURA knows about itself.
///
/// Gives the LLM factual grounding about its own capabilities, limitations,
/// version, and current operational state. Prevents confabulation about
/// what AURA can/cannot do.
///
/// Tier 1: Identity Integration (Phase 10).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SelfKnowledge {
    /// AURA version string (e.g. "4.0.0-alpha").
    pub version: Option<String>,
    /// List of currently available capabilities (e.g. ["screen_reading", "app_control"]).
    pub capabilities: Vec<String>,
    /// Known limitations (e.g. ["no internet access", "cannot make purchases"]).
    pub limitations: Vec<String>,
    /// Current operational mode description.
    pub operational_mode: Option<String>,
}
```

**Step 4: Verify types compile**

Run: `cargo check -p aura-types`
Expected: PASS (structs defined but not yet used)

---

## Task 2: Add New Fields to `ContextPackage`

**Files:**
- Modify: `crates/aura-types/src/ipc.rs:393-411` (ContextPackage struct)
- Modify: `crates/aura-types/src/ipc.rs:424-472` (estimated_size)
- Modify: `crates/aura-types/src/ipc.rs:475-489` (Default impl)

**Step 1: Add fields to ContextPackage struct (after line 410)**

```rust
    /// Constitutional identity tendencies (first-person principles).
    /// `None` if identity data is not available for this request.
    /// Tier 1: Identity Integration.
    pub identity_tendencies: Option<IdentityTendencies>,
    /// User-configured preferences.
    /// `None` if no preferences have been set.
    /// Tier 1: Identity Integration.
    pub user_preferences: Option<UserPreferences>,
    /// Self-knowledge payload — what AURA knows about itself.
    /// `None` if self-knowledge is not available for this request.
    /// Tier 1: Identity Integration.
    pub self_knowledge: Option<SelfKnowledge>,
```

**Step 2: Update `estimated_size()` (after line 469, before closing brace)**

```rust
        // Tier 1 identity fields
        if let Some(ref it) = self.identity_tendencies {
            for p in &it.principles {
                size += p.len();
            }
        }
        if let Some(ref up) = self.user_preferences {
            size += up.model_preference.as_ref().map_or(0, |s| s.len());
            size += up.interaction_style.as_ref().map_or(0, |s| s.len());
            size += 8; // proactiveness + autonomy_level (2 x f32)
            for scope in &up.access_scope {
                size += scope.len();
            }
            for domain in &up.domain_focus {
                size += domain.len();
            }
            size += up.custom_instructions.as_ref().map_or(0, |s| s.len());
        }
        if let Some(ref sk) = self.self_knowledge {
            size += sk.version.as_ref().map_or(0, |s| s.len());
            for cap in &sk.capabilities {
                size += cap.len();
            }
            for lim in &sk.limitations {
                size += lim.len();
            }
            size += sk.operational_mode.as_ref().map_or(0, |s| s.len());
        }
```

**Step 3: Update `Default` impl (add to end of struct literal, before closing brace at line 488)**

```rust
            identity_tendencies: None,
            user_preferences: None,
            self_knowledge: None,
```

**Step 4: Verify**

Run: `cargo check -p aura-types`
Expected: PASS

---

## Task 3: Add New Fields to `PromptSlots` and Section Renderers

**Files:**
- Modify: `crates/aura-neocortex/src/prompts.rs:214-276` (PromptSlots struct)
- Modify: `crates/aura-neocortex/src/prompts.rs` (add new section renderers)
- Modify: `crates/aura-neocortex/src/prompts.rs:618-679` (build_prompt ordering)

**Step 1: Add new fields to PromptSlots (after line 245, before teacher stack extensions)**

```rust
    // ── Tier 1: Identity integration ──
    /// Constitutional first-person principles. `None` if unavailable.
    pub identity_tendencies: Option<IdentityTendencies>,
    /// Rendered user preferences section. `None` if no preferences set.
    pub user_preferences: Option<UserPreferences>,
    /// Self-knowledge payload. `None` if unavailable.
    pub self_knowledge: Option<SelfKnowledge>,
```

Add the necessary import at the top of prompts.rs:
```rust
use aura_types::ipc::{IdentityTendencies, UserPreferences, SelfKnowledge};
```

**Step 2: Add section renderer for self_knowledge (near personality_section around line 540)**

```rust
/// Self-knowledge section — tells the LLM what it knows about itself.
///
/// Prevents confabulation about capabilities and provides factual grounding.
/// Tier 1: Identity Integration.
fn self_knowledge_section(sk: &SelfKnowledge) -> String {
    let mut out = String::from("SELF-KNOWLEDGE:");

    if let Some(ref ver) = sk.version {
        out.push_str(&format!("\n- Version: {}", ver));
    }
    if !sk.capabilities.is_empty() {
        out.push_str(&format!("\n- Capabilities: {}", sk.capabilities.join(", ")));
    }
    if !sk.limitations.is_empty() {
        out.push_str(&format!("\n- Limitations: {}", sk.limitations.join(", ")));
    }
    if let Some(ref mode) = sk.operational_mode {
        out.push_str(&format!("\n- Operational mode: {}", mode));
    }

    out
}
```

**Step 3: Add section renderer for identity_tendencies**

```rust
/// Identity tendencies section — constitutional first-person principles.
///
/// These are injected as compact bullet points so the LLM internalizes
/// them as behavioral guidelines.
/// Tier 1: Identity Integration.
fn identity_tendencies_section(it: &IdentityTendencies) -> String {
    if it.principles.is_empty() {
        return String::new();
    }
    let mut out = String::from("IDENTITY TENDENCIES:");
    for principle in &it.principles {
        out.push_str(&format!("\n- {}", principle));
    }
    out
}
```

**Step 4: Add section renderer for user_preferences**

```rust
/// User preferences section — user-configured behavioral parameters.
///
/// SECURITY [SEC-MED-4]: `custom_instructions` is user-authored free text
/// and MUST be wrapped in [UNTRUSTED] markers to prevent prompt injection.
/// This mirrors the pattern used for screen content (see context_section).
/// Tier 1: Identity Integration.
fn user_preferences_section(up: &UserPreferences) -> String {
    let mut out = String::from("USER PREFERENCES:");

    if let Some(ref style) = up.interaction_style {
        out.push_str(&format!("\n- Interaction style: {}", style));
    }
    if let Some(ref model) = up.model_preference {
        out.push_str(&format!("\n- Preferred model: {}", model));
    }
    if let Some(proactive) = up.proactiveness {
        let label = match proactive {
            p if p >= 0.8 => "highly proactive",
            p if p >= 0.4 => "moderately proactive",
            _ => "reactive (wait for requests)",
        };
        out.push_str(&format!("\n- Proactiveness: {}", label));
    }
    if let Some(autonomy) = up.autonomy_level {
        let label = match autonomy {
            a if a >= 0.8 => "high (act independently)",
            a if a >= 0.4 => "moderate (act, explain later)",
            _ => "low (always ask first)",
        };
        out.push_str(&format!("\n- Autonomy: {}", label));
    }
    if !up.access_scope.is_empty() {
        out.push_str(&format!("\n- Access scope: {}", up.access_scope.join(", ")));
    }
    if !up.domain_focus.is_empty() {
        out.push_str(&format!("\n- Domain focus: {}", up.domain_focus.join(", ")));
    }
    if let Some(ref instructions) = up.custom_instructions {
        out.push_str(&format!(
            "\n- Custom instructions:\n[UNTRUSTED_USER_INSTRUCTIONS_BEGIN]\n{}\n[UNTRUSTED_USER_INSTRUCTIONS_END]",
            instructions
        ));
    }

    out
}
```

**Step 5: Update `build_prompt()` section ordering (lines 618-679)**

Replace the current section assembly with:

```rust
pub fn build_prompt(mode: InferenceMode, slots: &PromptSlots) -> (String, ModeConfig) {
    let mut sections: Vec<String> = Vec::with_capacity(15);

    // 1. Identity (role header)
    sections.push(identity_section(mode).to_string());

    // 2. Self-knowledge (Tier 1) — what AURA knows about itself
    if let Some(ref sk) = slots.self_knowledge {
        let sk_text = self_knowledge_section(sk);
        if !sk_text.is_empty() {
            sections.push(sk_text);
        }
    }

    // 3. Identity tendencies (Tier 1) — constitutional principles
    if let Some(ref it) = slots.identity_tendencies {
        let it_text = identity_tendencies_section(it);
        if !it_text.is_empty() {
            sections.push(it_text);
        }
    }

    // 4. Personality (all modes) — OCEAN + mood + trust + identity_block
    sections.push(personality_section(slots));

    // 5. User preferences (Tier 1) — user-configured behavioral params
    if let Some(ref up) = slots.user_preferences {
        sections.push(user_preferences_section(up));
    }

    // 6. Rules
    sections.push(rules_section(mode).to_string());

    // 7. Output format (if grammar-constrained)
    let format_text = output_format_section(slots.grammar_kind);
    if !format_text.is_empty() {
        sections.push(format_text);
    }

    // 8. Tool descriptions (if available)
    if let Some(ref tools) = slots.tool_descriptions {
        if !tools.is_empty() {
            sections.push(tools_section(tools));
        }
    }

    // 9. Few-shot examples (Layer 3 Tier 2+)
    let examples_text = few_shot_section(&slots.few_shot_examples);
    if !examples_text.is_empty() {
        sections.push(examples_text);
    }

    // 10. Chain-of-thought (Layer 1)
    if slots.force_cot {
        sections.push(cot_section().to_string());
    }

    // 11. DGS template (if present, takes precedence over open-ended)
    if let Some(ref template) = slots.dgs_template {
        sections.push(dgs_template_section(template));
    }

    // 12. Retry context (Layer 3)
    if let (Some(ref prev), Some(ref reason)) = (&slots.previous_attempt, &slots.rejection_reason) {
        sections.push(retry_section(prev, reason));
    }

    // 13. ReAct history (if iterating)
    let react_text = react_history_section(&slots.react_history);
    if !react_text.is_empty() {
        sections.push(react_text);
    }

    // 14. Context block
    sections.push(context_section(mode, slots));

    // 15. Closing instruction
    sections.push(closing_instruction(mode).to_string());

    let prompt = sections.join("\n\n");
    let config = mode_config(mode);
    (prompt, config)
}
```

**Step 6: Also update `build_react_prompt()` in the same file**

The ReAct prompt (line 689) should also include the Tier 1 sections in the same order. Add self_knowledge, identity_tendencies, and user_preferences after the identity section and before rules.

**Step 7: Verify**

Run: `cargo check -p aura-neocortex`
Expected: Will fail — need to thread fields through context.rs first (Task 4).

---

## Task 4: Thread New Fields Through `build_slots_extended()` in `context.rs`

**Files:**
- Modify: `crates/aura-neocortex/src/context.rs:860-902` (PromptSlots struct literal in build_slots_extended)

**Step 1: Add fields to the PromptSlots struct literal (after line 892)**

In the `PromptSlots { ... }` struct literal inside `build_slots_extended()`, add after the `user_state_context` field:

```rust
        // Tier 1: Identity integration
        identity_tendencies: ctx.identity_tendencies.clone(),
        user_preferences: ctx.user_preferences.clone(),
        self_knowledge: ctx.self_knowledge.clone(),
```

**Step 2: Verify**

Run: `cargo check -p aura-neocortex`
Expected: PASS (if all previous tasks done correctly)

---

## Task 5: Update All ContextPackage Construction Sites

Every place that constructs a `ContextPackage { ... }` needs the three new `Option` fields. Since we use `..Default::default()` in most sites, only EXPLICIT construction sites need updating.

**Files:**
- Modify: `crates/aura-neocortex/src/context.rs:991-1039` (test helper `make_context`)
- Modify: `crates/aura-neocortex/src/ipc_handler.rs:1089-1128` (test helper)
- Verify: `crates/aura-daemon/src/daemon_core/react.rs:690` (uses `..Default::default()` — OK)
- Verify: `crates/aura-daemon/src/daemon_core/main_loop.rs:3233` (uses `..ContextPackage::default()` — OK)
- Verify: `crates/aura-daemon/src/ipc/protocol.rs:313` (uses `..Default::default()` — OK)

**Step 1: Update `make_context` in context.rs tests (line 1037-1039)**

Add before the closing brace:
```rust
            identity_tendencies: None,
            user_preferences: None,
            self_knowledge: None,
```

**Step 2: Update test in ipc_handler.rs (line 1126-1127)**

Add before the closing brace:
```rust
            identity_tendencies: None,
            user_preferences: None,
            self_knowledge: None,
```

**Step 3: Verify all construction sites compile**

Run: `cargo check --workspace`
Expected: PASS

---

## Task 6: Run Full Test Suite

**Step 1: Run aura-types tests**

Run: `cargo test -p aura-types`
Expected: All existing tests pass

**Step 2: Run aura-neocortex tests**

Run: `cargo test -p aura-neocortex`
Expected: All existing tests pass (new fields are None, sections are skipped)

**Step 3: Run workspace-wide check**

Run: `cargo check --workspace`
Expected: No errors, no new warnings

---

## Task 7: Add Tests for New Sections

**Files:**
- Modify: `crates/aura-neocortex/src/context.rs` (tests module, after line ~1100)

**Step 1: Add test for identity_tendencies in prompt**

```rust
#[test]
fn identity_tendencies_appear_in_prompt() {
    let mut ctx = make_context(1, 0);
    ctx.identity_tendencies = Some(aura_types::ipc::IdentityTendencies {
        principles: vec![
            "I lead with curiosity before judgment.".into(),
            "I respect user autonomy above all.".into(),
        ],
    });

    let result = assemble_prompt(&ctx, None, None);

    assert!(result.system_prompt.contains("IDENTITY TENDENCIES:"));
    assert!(result.system_prompt.contains("I lead with curiosity before judgment."));
    assert!(result.system_prompt.contains("I respect user autonomy above all."));
}
```

**Step 2: Add test for user_preferences in prompt**

```rust
#[test]
fn user_preferences_appear_in_prompt() {
    let mut ctx = make_context(1, 0);
    ctx.user_preferences = Some(aura_types::ipc::UserPreferences {
        interaction_style: Some("concise".into()),
        proactiveness: Some(0.9),
        autonomy_level: Some(0.2),
        custom_instructions: Some("Always explain your reasoning.".into()),
        ..Default::default()
    });

    let result = assemble_prompt(&ctx, None, None);

    assert!(result.system_prompt.contains("USER PREFERENCES:"));
    assert!(result.system_prompt.contains("Interaction style: concise"));
    assert!(result.system_prompt.contains("highly proactive"));
    assert!(result.system_prompt.contains("low (always ask first)"));
    assert!(result.system_prompt.contains("[UNTRUSTED_USER_INSTRUCTIONS_BEGIN]"));
    assert!(result.system_prompt.contains("Always explain your reasoning."));
    assert!(result.system_prompt.contains("[UNTRUSTED_USER_INSTRUCTIONS_END]"));
}
```

**Step 3: Add test for self_knowledge in prompt**

```rust
#[test]
fn self_knowledge_appears_in_prompt() {
    let mut ctx = make_context(1, 0);
    ctx.self_knowledge = Some(aura_types::ipc::SelfKnowledge {
        version: Some("4.0.0-alpha".into()),
        capabilities: vec!["screen_reading".into(), "app_control".into()],
        limitations: vec!["no internet access".into()],
        operational_mode: Some("standard".into()),
    });

    let result = assemble_prompt(&ctx, None, None);

    assert!(result.system_prompt.contains("SELF-KNOWLEDGE:"));
    assert!(result.system_prompt.contains("Version: 4.0.0-alpha"));
    assert!(result.system_prompt.contains("screen_reading, app_control"));
    assert!(result.system_prompt.contains("no internet access"));
}
```

**Step 4: Add test for section ordering**

```rust
#[test]
fn tier1_sections_ordered_correctly() {
    let mut ctx = make_context(1, 0);
    ctx.identity_tendencies = Some(aura_types::ipc::IdentityTendencies {
        principles: vec!["I am curious.".into()],
    });
    ctx.user_preferences = Some(aura_types::ipc::UserPreferences {
        interaction_style: Some("concise".into()),
        ..Default::default()
    });
    ctx.self_knowledge = Some(aura_types::ipc::SelfKnowledge {
        version: Some("4.0.0".into()),
        ..Default::default()
    });

    let result = assemble_prompt(&ctx, None, None);
    let prompt = &result.system_prompt;

    // Verify ordering: self_knowledge < identity_tendencies < personality < user_preferences < rules
    let sk_pos = prompt.find("SELF-KNOWLEDGE:").expect("self-knowledge missing");
    let it_pos = prompt.find("IDENTITY TENDENCIES:").expect("identity tendencies missing");
    let p_pos = prompt.find("PERSONALITY TRAITS").expect("personality missing");
    let up_pos = prompt.find("USER PREFERENCES:").expect("user preferences missing");

    assert!(sk_pos < it_pos, "self-knowledge must come before identity tendencies");
    assert!(it_pos < p_pos, "identity tendencies must come before personality");
    assert!(p_pos < up_pos, "personality must come before user preferences");
}
```

**Step 5: Add test for None fields (backward compatibility)**

```rust
#[test]
fn tier1_none_fields_produce_no_output() {
    let ctx = make_context(1, 0);
    // All Tier 1 fields are None by default
    let result = assemble_prompt(&ctx, None, None);

    assert!(!result.system_prompt.contains("SELF-KNOWLEDGE:"));
    assert!(!result.system_prompt.contains("IDENTITY TENDENCIES:"));
    assert!(!result.system_prompt.contains("USER PREFERENCES:"));
}
```

**Step 6: Run tests**

Run: `cargo test -p aura-neocortex`
Expected: All tests pass including new ones.

---

## Task 8: Commit

```bash
git add -A
git commit -m "feat(neocortex): add Tier 1 identity integration — identity_tendencies, user_preferences, self_knowledge

Thread three new optional context sections through the full 4-hop pipeline:
ContextPackage → PromptSlots → build_prompt → system_prompt.

New prompt section ordering:
  1. Identity (role header)
  2. Self-knowledge (NEW) — version, capabilities, limitations
  3. Identity tendencies (NEW) — constitutional first-person principles
  4. Personality (existing OCEAN + mood + trust)
  5. User preferences (NEW) — interaction style, proactiveness, autonomy
  6. Rules → Output format → Tools → ... → Context → Closing

Security: custom_instructions wrapped in [UNTRUSTED] markers (SEC-MED-4).
All new fields are Option<T> for backward compatibility."
```

---

## Risks & Mitigations

| Risk | Severity | Mitigation |
|------|----------|------------|
| Token budget pressure (~380 new tokens) | Medium | New sections are conditional (None = 0 tokens). Only populated when data available. |
| `identity_block` collision | Low | Keep existing field — it's the compact JSON for OCEAN/VAD/archetype. New `identity_tendencies` is the constitutional principles layer (different concern). |
| Prompt injection via custom_instructions | High | Wrapped in `[UNTRUSTED_USER_INSTRUCTIONS_BEGIN/END]` markers, matching screen content pattern. |
| PromptSlots struct bloat (25 → 28 fields) | Low | Using full structs (`IdentityTendencies`, `UserPreferences`, `SelfKnowledge`) rather than flat fields. Could sub-struct personality fields later. |
| 64KB IPC size pressure | Low | `estimated_size()` updated. Typical Tier 1 payload is ~500 bytes. |
| Daemon doesn't populate new fields yet | Expected | All fields `Option<T>` with `None` default. Daemon integration is a separate task (Tier 2). |
