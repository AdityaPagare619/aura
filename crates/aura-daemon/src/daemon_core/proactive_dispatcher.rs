//! Proactive delivery dispatcher.
//!
//! Bridges the gap between intelligence systems (goal tracker, health monitor,
//! memory consolidator, social tracker, routine scheduler, context detector)
//! and the LLM neocortex that generates the user-facing messages.
//!
//! # Architecture
//!
//! The daemon detects trigger conditions and packages raw **typed** state as a
//! [`DaemonToNeocortex::ProactiveContext`] IPC message. The LLM (neocortex)
//! receives typed structured data, reasons about the user's context (OCEAN, VAD,
//! relationship stage, memories), and generates an appropriate natural-language
//! message. The daemon delivers the reply via Telegram through the normal
//! `ConversationReply` inbound path.
//!
//! **Daemon never writes message text — that is the LLM's job.**
//! **Daemon never encodes intent as format strings — use typed IPC variants.**
//!
//! # 6 proactive systems
//!
//! | System | Status | IPC variant |
//! |--------|--------|-------------|
//! | Social gaps | wired | `SocialGap` |
//! | Goal overdue | wired | `GoalOverdue` |
//! | Goal stalls | wired | `GoalStalled` |
//! | Health alerts | NOT YET wired — dispatch call site missing | `HealthAlert` |
//! | Memory insights | NOT YET wired — dispatch call site missing | `MemoryInsight` |
//! | Trigger rules | NOT YET wired — dispatch call site missing | `TriggerRuleFired` |
//!
//! # Usage in main_loop
//!
//! ```ignore
//! let trigger = ProactiveTrigger::GoalStalled { ... };
//! let ipc_msg = trigger_to_ipc(&trigger);
//! state.last_system2_source = Some(InputSource::Direct);
//! if let Err(e) = subs.neocortex.send(&ipc_msg).await {
//!     tracing::warn!("failed to dispatch proactive trigger: {e}");
//! }
//! ```

use aura_types::ipc::{
    ContextPackage, DaemonToNeocortex, IdentityTendencies, InferenceMode,
    ProactiveTrigger as IpcProactiveTrigger, SelfKnowledge, UserStateSignals,
};

use crate::arc::life_arc::ProactiveTrigger as ArcProactiveTrigger;

// ---------------------------------------------------------------------------
// Trigger types — raw daemon-detected conditions (daemon-internal only)
//
// These are the daemon's internal representation of proactive conditions.
// They are NOT the IPC types — those live in `aura_types::ipc::ProactiveTrigger`.
// The `trigger_to_ipc` function maps these to the typed IPC variants.
// ---------------------------------------------------------------------------

/// A proactive trigger detected by the daemon (daemon-internal type).
///
/// Contains only raw, factual data. The conversion to
/// [`aura_types::ipc::ProactiveTrigger`] (via [`trigger_to_ipc`]) strips
/// daemon-internal fields and produces the IPC-serialisable form.
///
/// The LLM (neocortex) decides what to say, how urgent it sounds, and how to
/// frame it for the user's current context — never Rust code.
#[derive(Debug, Clone)]
pub enum ProactiveTrigger {
    /// A goal has been stalled for N days without progress.
    GoalStalled {
        goal_id: u64,
        goal_title: String,
        stalled_days: u32,
        progress_at_stall: f32,
    },
    /// Health metric (system or user) crossed a threshold.
    HealthAlert {
        metric: String,
        value: f32,
        threshold: f32,
        direction: AlertDirection,
    },
    /// Memory consolidation identified a recurring pattern worth surfacing.
    MemoryInsight {
        pattern_summary: String,
        relevance_score: f32,
        occurrence_count: u32,
    },
    /// A social relationship has gone silent past the configured threshold.
    RelationshipNudge {
        contact_id: u64,
        days_since_contact: u32,
        urgency: f32,
    },
    /// Routine deviation detected: user usually does X at this time but hasn't.
    RoutineDeviation {
        routine_name: String,
        expected_at: String,
        deviation_minutes: i32,
    },
    /// A goal has overrun its deadline.
    GoalOverdue {
        goal_id: u64,
        goal_title: String,
        overdue_ms: u64,
    },
}

/// Direction a metric is moving relative to its threshold.
#[derive(Debug, Clone, Copy)]
pub enum AlertDirection {
    /// Metric is rising toward or above threshold.
    Rising,
    /// Metric is falling toward or below threshold.
    Falling,
}

impl std::fmt::Display for AlertDirection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AlertDirection::Rising => write!(f, "rising"),
            AlertDirection::Falling => write!(f, "falling"),
        }
    }
}

// ---------------------------------------------------------------------------
// Core conversion: local trigger → DaemonToNeocortex::ProactiveContext
//
// Architecture law: NEVER encode intent or intelligence as format strings.
// Map raw daemon-internal data to typed IpcProactiveTrigger variants and let
// the LLM (neocortex) generate all user-facing language from the typed data.
// ---------------------------------------------------------------------------

/// Wraps a proactive trigger into a [`DaemonToNeocortex::ProactiveContext`] IPC
/// message carrying typed structured data.
///
/// The neocortex receives the typed trigger and a full [`ContextPackage`]
/// (OCEAN, VAD, relationship stage, memories), reasons about the user's
/// situation, and generates a natural, context-appropriate message. No Rust
/// code writes the message text — that is the LLM's exclusive domain.
///
/// # Arguments
///
/// * `trigger` — the condition detected by the daemon (daemon-internal type)
pub fn trigger_to_ipc(trigger: &ProactiveTrigger) -> DaemonToNeocortex {
    let ipc_trigger = to_ipc_trigger(trigger);

    // Build a minimal ContextPackage. Callers with richer context (OCEAN, VAD,
    // memories, relationship stage) should pass a pre-populated package; the
    // default is sufficient for correctness on day zero.
    let mut ctx = ContextPackage::default();
    ctx.inference_mode = InferenceMode::Conversational;
    ctx.user_state = UserStateSignals::default(); // assume user can receive the message
    ctx.token_budget = 512; // keep proactive messages short

    // Tier 1: Identity Core fields for proactive messages.
    // Constitutional tendencies ensure proactive messages align with AURA's character.
    ctx.identity_tendencies = Some(IdentityTendencies::constitutional());
    // Self-knowledge grounds the LLM in what AURA can/cannot do.
    ctx.self_knowledge = Some(SelfKnowledge::for_mode("conversational"));
    // user_preferences: None — proactive dispatcher doesn't have access to UserProfile.
    // TODO(tier-1): Thread UserProfile into trigger_to_ipc() for personalized proactives.

    DaemonToNeocortex::ProactiveContext {
        trigger: ipc_trigger,
        context: ctx,
    }
}

/// Maps a daemon-internal [`ProactiveTrigger`] to the IPC-serialisable
/// [`IpcProactiveTrigger`] (i.e. `aura_types::ipc::ProactiveTrigger`).
///
/// # Architectural note
///
/// Every arm must produce a typed variant — never a format string. The LLM
/// receives the variant's structured fields and generates language from them.
fn to_ipc_trigger(trigger: &ProactiveTrigger) -> IpcProactiveTrigger {
    match trigger {
        // ── System 3: Goal stalls (wired) ───────────────────────────────────
        ProactiveTrigger::GoalStalled {
            goal_id,
            goal_title,
            stalled_days,
            progress_at_stall: _, // progress is daemon-internal; not needed by LLM here
        } => IpcProactiveTrigger::GoalStalled {
            goal_id: *goal_id,
            title: goal_title.clone(),
            stalled_days: *stalled_days,
        },

        // ── System 2: Goal overdue (wired) ──────────────────────────────────
        ProactiveTrigger::GoalOverdue {
            goal_id,
            goal_title,
            overdue_ms,
        } => IpcProactiveTrigger::GoalOverdue {
            goal_id: *goal_id,
            title: goal_title.clone(),
            // Convert milliseconds to whole days for the LLM (simpler reasoning).
            overdue_days: (*overdue_ms / 86_400_000).max(1) as u32,
        },

        // ── System 1: Social gaps (wired) ────────────────────────────────────
        //
        // TODO(agent-1): `RelationshipNudge` only carries `contact_id`, not the
        // contact's display name. A contact-name resolution service needs to be
        // wired into the social gap detection path so the LLM receives a human-
        // readable name instead of a numeric ID. Until then we use the ID as a
        // string placeholder — the LLM can still generate a useful nudge message.
        ProactiveTrigger::RelationshipNudge {
            contact_id,
            days_since_contact,
            urgency: _, // urgency gates dispatch via should_dispatch(); not needed by LLM
        } => IpcProactiveTrigger::SocialGap {
            contact_name: format!("contact:{contact_id}"),
            days_since_contact: *days_since_contact,
        },

        // ── System 4: Health alerts (NOT YET wired — conversion correct) ────
        //
        // Dispatch call site still needed in main_loop.rs (health monitor integration).
        // Data needed: metric name, current value, threshold from HealthMonitor.
        ProactiveTrigger::HealthAlert {
            metric,
            value,
            threshold,
            direction: _, // direction is daemon context; LLM infers from value vs threshold
        } => IpcProactiveTrigger::HealthAlert {
            metric: metric.clone(),
            value: *value,
            threshold: *threshold,
        },

        // ── System 5: Memory insights (NOT YET wired — conversion correct) ──
        //
        // Dispatch call site still needed in main_loop.rs (memory consolidation integration).
        // Data needed: pattern_summary from MemoryConsolidator after dreaming/compaction runs.
        ProactiveTrigger::MemoryInsight {
            pattern_summary,
            relevance_score: _, // gates dispatch via should_dispatch(); not needed by LLM
            occurrence_count: _, // ditto
        } => IpcProactiveTrigger::MemoryInsight {
            summary: pattern_summary.clone(),
        },

        // ── System 6: Trigger rule evaluation (NOT YET wired — conversion correct) ─
        //
        // Dispatch call site still needed in main_loop.rs (trigger rule engine integration).
        // Data needed: rule_name and a factual description of what fired, from the
        // routine-deviation / contextual-trigger subsystem.
        ProactiveTrigger::RoutineDeviation {
            routine_name,
            expected_at,
            deviation_minutes,
        } => {
            let abs_mins = deviation_minutes.unsigned_abs();
            let direction = if *deviation_minutes > 0 {
                "late"
            } else {
                "early"
            };
            IpcProactiveTrigger::TriggerRuleFired {
                rule_name: routine_name.clone(),
                // Factual description assembled from typed fields — this is data
                // assembly, not intelligence generation.  The LLM decides how to
                // phrase this for the user.
                description: format!("expected at {expected_at}, {abs_mins} minutes {direction}"),
            }
        },
    }
}

// ---------------------------------------------------------------------------
// Arc trigger conversion — life_arc::ProactiveTrigger → IPC
// ---------------------------------------------------------------------------

/// Convert a `life_arc::ProactiveTrigger` into a
/// [`DaemonToNeocortex::ProactiveContext`] IPC message.
///
/// Life-arc triggers carry rich factual context (`context_for_llm`) gathered
/// by the arc subsystem. That context is forwarded verbatim as the
/// `TriggerRuleFired::description` field so the neocortex can reason about
/// the user's arc health without any intermediate format strings.
///
/// The 24-hour dedup is already enforced by each arc's `last_trigger_ms`
/// field before `collect_triggers` is called, so this function does not
/// apply an additional gate.
pub fn arc_trigger_to_ipc(arc_trigger: &ArcProactiveTrigger) -> DaemonToNeocortex {
    let ipc_trigger = IpcProactiveTrigger::TriggerRuleFired {
        rule_name: format!("life_arc:{}", arc_trigger.arc_type.as_str()),
        // Raw factual context assembled by the arc — NOT advice. The LLM
        // decides what (if anything) to say to the user.
        description: arc_trigger.context_for_llm.clone(),
    };

    let mut ctx = ContextPackage::default();
    ctx.inference_mode = InferenceMode::Conversational;
    ctx.user_state = UserStateSignals::default();
    ctx.token_budget = 512;

    DaemonToNeocortex::ProactiveContext {
        trigger: ipc_trigger,
        context: ctx,
    }
}

// ---------------------------------------------------------------------------
// Guard helpers — prevent trigger spam
// ---------------------------------------------------------------------------

/// Returns `true` if a proactive trigger should be dispatched based on
/// an urgency/relevance threshold. Prevents low-signal triggers from
/// flooding the user.
///
/// Callers can tune per-trigger minimum urgency in the cron handlers.
pub fn should_dispatch(trigger: &ProactiveTrigger) -> bool {
    match trigger {
        ProactiveTrigger::GoalStalled { stalled_days, .. } => *stalled_days >= 3,
        ProactiveTrigger::HealthAlert {
            value,
            threshold,
            direction,
            ..
        } => match direction {
            AlertDirection::Rising => *value >= *threshold * 1.1,
            AlertDirection::Falling => *value <= *threshold * 0.9,
        },
        ProactiveTrigger::MemoryInsight {
            relevance_score,
            occurrence_count,
            ..
        } => *relevance_score >= 0.6 && *occurrence_count >= 2,
        ProactiveTrigger::RelationshipNudge { urgency, .. } => *urgency >= 1.2,
        ProactiveTrigger::RoutineDeviation {
            deviation_minutes, ..
        } => deviation_minutes.unsigned_abs() >= 30,
        ProactiveTrigger::GoalOverdue { .. } => true, // always notify on overdue
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_goal_stalled_produces_proactive_context() {
        let trigger = ProactiveTrigger::GoalStalled {
            goal_id: 42,
            goal_title: "Learn Rust".to_string(),
            stalled_days: 5,
            progress_at_stall: 0.35,
        };
        let msg = trigger_to_ipc(&trigger);
        match msg {
            DaemonToNeocortex::ProactiveContext {
                trigger: ipc_trigger,
                context,
            } => {
                assert_eq!(context.inference_mode, InferenceMode::Conversational);
                assert_eq!(context.token_budget, 512);
                match ipc_trigger {
                    IpcProactiveTrigger::GoalStalled {
                        goal_id,
                        title,
                        stalled_days,
                    } => {
                        assert_eq!(goal_id, 42);
                        assert_eq!(title, "Learn Rust");
                        assert_eq!(stalled_days, 5);
                    },
                    other => panic!("expected GoalStalled, got {other:?}"),
                }
            },
            other => panic!("expected ProactiveContext, got {other:?}"),
        }
    }

    #[test]
    fn test_goal_overdue_converts_ms_to_days() {
        let trigger = ProactiveTrigger::GoalOverdue {
            goal_id: 7,
            goal_title: "Ship feature".to_string(),
            overdue_ms: 3 * 86_400_000, // 3 days
        };
        let msg = trigger_to_ipc(&trigger);
        match msg {
            DaemonToNeocortex::ProactiveContext {
                trigger: ipc_trigger,
                ..
            } => match ipc_trigger {
                IpcProactiveTrigger::GoalOverdue { overdue_days, .. } => {
                    assert_eq!(overdue_days, 3);
                },
                other => panic!("expected GoalOverdue, got {other:?}"),
            },
            other => panic!("expected ProactiveContext, got {other:?}"),
        }
    }

    #[test]
    fn test_social_gap_produces_typed_variant() {
        let trigger = ProactiveTrigger::RelationshipNudge {
            contact_id: 99,
            days_since_contact: 14,
            urgency: 1.5,
        };
        let msg = trigger_to_ipc(&trigger);
        match msg {
            DaemonToNeocortex::ProactiveContext {
                trigger: ipc_trigger,
                ..
            } => {
                match ipc_trigger {
                    IpcProactiveTrigger::SocialGap {
                        contact_name,
                        days_since_contact,
                    } => {
                        // contact_name is the placeholder until name resolution is wired
                        assert!(contact_name.contains("99"));
                        assert_eq!(days_since_contact, 14);
                    },
                    other => panic!("expected SocialGap, got {other:?}"),
                }
            },
            other => panic!("expected ProactiveContext, got {other:?}"),
        }
    }

    #[test]
    fn test_health_alert_typed_variant() {
        let trigger = ProactiveTrigger::HealthAlert {
            metric: "error_rate".to_string(),
            value: 0.85,
            threshold: 0.70,
            direction: AlertDirection::Rising,
        };
        let msg = trigger_to_ipc(&trigger);
        match msg {
            DaemonToNeocortex::ProactiveContext {
                trigger: ipc_trigger,
                ..
            } => match ipc_trigger {
                IpcProactiveTrigger::HealthAlert {
                    metric,
                    value,
                    threshold,
                } => {
                    assert_eq!(metric, "error_rate");
                    assert!((value - 0.85).abs() < f32::EPSILON);
                    assert!((threshold - 0.70).abs() < f32::EPSILON);
                },
                other => panic!("expected HealthAlert, got {other:?}"),
            },
            other => panic!("expected ProactiveContext, got {other:?}"),
        }
    }

    #[test]
    fn test_memory_insight_typed_variant() {
        let trigger = ProactiveTrigger::MemoryInsight {
            pattern_summary: "You often avoid tasks after 9pm".to_string(),
            relevance_score: 0.8,
            occurrence_count: 5,
        };
        let msg = trigger_to_ipc(&trigger);
        match msg {
            DaemonToNeocortex::ProactiveContext {
                trigger: ipc_trigger,
                ..
            } => match ipc_trigger {
                IpcProactiveTrigger::MemoryInsight { summary } => {
                    assert!(summary.contains("9pm"));
                },
                other => panic!("expected MemoryInsight, got {other:?}"),
            },
            other => panic!("expected ProactiveContext, got {other:?}"),
        }
    }

    #[test]
    fn test_routine_deviation_maps_to_trigger_rule_fired() {
        let trigger = ProactiveTrigger::RoutineDeviation {
            routine_name: "morning_run".to_string(),
            expected_at: "07:00".to_string(),
            deviation_minutes: 45,
        };
        let msg = trigger_to_ipc(&trigger);
        match msg {
            DaemonToNeocortex::ProactiveContext {
                trigger: ipc_trigger,
                ..
            } => match ipc_trigger {
                IpcProactiveTrigger::TriggerRuleFired {
                    rule_name,
                    description,
                } => {
                    assert_eq!(rule_name, "morning_run");
                    assert!(description.contains("45"));
                    assert!(description.contains("late"));
                },
                other => panic!("expected TriggerRuleFired, got {other:?}"),
            },
            other => panic!("expected ProactiveContext, got {other:?}"),
        }
    }

    #[test]
    fn test_should_dispatch_goal_stalled_threshold() {
        let short_stall = ProactiveTrigger::GoalStalled {
            goal_id: 1,
            goal_title: "Test".to_string(),
            stalled_days: 2,
            progress_at_stall: 0.5,
        };
        assert!(!should_dispatch(&short_stall), "2 days should not dispatch");

        let long_stall = ProactiveTrigger::GoalStalled {
            goal_id: 1,
            goal_title: "Test".to_string(),
            stalled_days: 3,
            progress_at_stall: 0.5,
        };
        assert!(should_dispatch(&long_stall), "3 days should dispatch");
    }

    #[test]
    fn test_should_dispatch_health_alert_direction() {
        let ok = ProactiveTrigger::HealthAlert {
            metric: "x".into(),
            value: 0.70,
            threshold: 0.70,
            direction: AlertDirection::Rising,
        };
        assert!(
            !should_dispatch(&ok),
            "at-threshold rising should not dispatch"
        );

        let over = ProactiveTrigger::HealthAlert {
            metric: "x".into(),
            value: 0.78,
            threshold: 0.70,
            direction: AlertDirection::Rising,
        };
        assert!(should_dispatch(&over), "10% over threshold should dispatch");
    }

    #[test]
    fn test_relationship_nudge_urgency_gate() {
        let low = ProactiveTrigger::RelationshipNudge {
            contact_id: 1,
            days_since_contact: 7,
            urgency: 1.0,
        };
        assert!(!should_dispatch(&low));

        let high = ProactiveTrigger::RelationshipNudge {
            contact_id: 1,
            days_since_contact: 14,
            urgency: 1.5,
        };
        assert!(should_dispatch(&high));
    }

    #[test]
    fn test_goal_overdue_always_dispatches() {
        let overdue = ProactiveTrigger::GoalOverdue {
            goal_id: 99,
            goal_title: "Ship feature".to_string(),
            overdue_ms: 3_600_000,
        };
        assert!(should_dispatch(&overdue));
    }

    /// Verify that trigger_to_ipc never returns DaemonToNeocortex::Converse.
    /// Converse would be Theater AGI — encoding trigger intent as format strings.
    #[test]
    fn test_trigger_to_ipc_never_returns_converse() {
        let triggers = vec![
            ProactiveTrigger::GoalStalled {
                goal_id: 1,
                goal_title: "T".into(),
                stalled_days: 3,
                progress_at_stall: 0.5,
            },
            ProactiveTrigger::GoalOverdue {
                goal_id: 2,
                goal_title: "T".into(),
                overdue_ms: 86_400_000,
            },
            ProactiveTrigger::RelationshipNudge {
                contact_id: 3,
                days_since_contact: 10,
                urgency: 1.5,
            },
            ProactiveTrigger::HealthAlert {
                metric: "m".into(),
                value: 0.9,
                threshold: 0.7,
                direction: AlertDirection::Rising,
            },
            ProactiveTrigger::MemoryInsight {
                pattern_summary: "p".into(),
                relevance_score: 0.8,
                occurrence_count: 3,
            },
            ProactiveTrigger::RoutineDeviation {
                routine_name: "r".into(),
                expected_at: "08:00".into(),
                deviation_minutes: 30,
            },
        ];
        for t in &triggers {
            let msg = trigger_to_ipc(t);
            assert!(
                matches!(msg, DaemonToNeocortex::ProactiveContext { .. }),
                "trigger_to_ipc must return ProactiveContext, not Converse or any other variant"
            );
        }
    }
}
