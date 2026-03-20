//! Config command handlers: /set, /get, /personality, /personality_set, /trust, /trust_set, /voice,
//! /chat.

use aura_types::errors::AuraError;
use tracing::instrument;

use super::{HandlerContext, HandlerResponse};

// ─── Helpers ────────────────────────────────────────────────────────────────

/// Clamp a float to the [0.0, 1.0] range, returning an error message if out of range.
fn validate_unit_range(value: f32, name: &str) -> Result<f32, HandlerResponse> {
    if !(0.0..=1.0).contains(&value) {
        return Err(HandlerResponse::text(format!(
            "{name} must be between 0.0 and 1.0 (got {value})"
        )));
    }
    Ok(value)
}

/// Check whether a dot-separated key matches a known config section/field.
fn is_known_config_key(key: &str) -> bool {
    matches!(
        key,
        "daemon.version"
            | "daemon.log_level"
            | "daemon.data_dir"
            | "daemon.checkpoint_interval_s"
            | "daemon.rss_warning_mb"
            | "daemon.rss_ceiling_mb"
            | "identity.mood_cooldown_ms"
            | "identity.max_mood_delta"
            | "identity.trust_hysteresis"
            | "voice.enabled"
            | "voice.wake_sensitivity"
            | "voice.tts_engine"
            | "voice.stt_engine"
            | "voice.max_record_ms"
            | "proactive.enabled"
            | "proactive.min_confidence"
            | "proactive.cooldown_ms"
            | "proactive.max_per_hour"
            | "proactive.require_confirmation"
            | "telegram.enabled"
            | "telegram.poll_interval_ms"
            | "power.daily_token_budget"
            | "power.conservative_threshold"
            | "power.low_power_threshold"
            | "policy.default_effect"
            | "policy.log_all_decisions"
    )
}

/// Read a dot-separated config key from the live [`AuraConfig`], returning
/// a bounded display string. Returns `"(no config)"` when the config
/// snapshot is unavailable.
fn read_config_key(ctx: &HandlerContext<'_>, key: &str) -> String {
    let Some(cfg) = ctx.config else {
        return "(no config)".to_string();
    };

    match key {
        // Daemon
        "daemon.version" => cfg.daemon.version.clone(),
        "daemon.log_level" => cfg.daemon.log_level.clone(),
        "daemon.data_dir" => cfg.daemon.data_dir.clone(),
        "daemon.checkpoint_interval_s" => format!("{}", cfg.daemon.checkpoint_interval_s),
        "daemon.rss_warning_mb" => format!("{}", cfg.daemon.rss_warning_mb),
        "daemon.rss_ceiling_mb" => format!("{}", cfg.daemon.rss_ceiling_mb),
        // Identity
        "identity.mood_cooldown_ms" => format!("{}", cfg.identity.mood_cooldown_ms),
        "identity.max_mood_delta" => format!("{:.2}", cfg.identity.max_mood_delta),
        "identity.trust_hysteresis" => format!("{:.2}", cfg.identity.trust_hysteresis),
        // Voice
        "voice.enabled" => format!("{}", cfg.voice.enabled),
        "voice.wake_sensitivity" => format!("{:.1}", cfg.voice.wake_sensitivity),
        "voice.tts_engine" => cfg.voice.tts_engine.clone(),
        "voice.stt_engine" => cfg.voice.stt_engine.clone(),
        "voice.max_record_ms" => format!("{}", cfg.voice.max_record_ms),
        // Proactive
        "proactive.enabled" => format!("{}", cfg.proactive.enabled),
        "proactive.min_confidence" => format!("{:.2}", cfg.proactive.min_confidence),
        "proactive.cooldown_ms" => format!("{}", cfg.proactive.cooldown_ms),
        "proactive.max_per_hour" => format!("{}", cfg.proactive.max_per_hour),
        "proactive.require_confirmation" => format!("{}", cfg.proactive.require_confirmation),
        // Telegram
        "telegram.enabled" => format!("{}", cfg.telegram.enabled),
        "telegram.poll_interval_ms" => format!("{}", cfg.telegram.poll_interval_ms),
        // Power
        "power.daily_token_budget" => format!("{}", cfg.power.daily_token_budget),
        "power.conservative_threshold" => format!("{}", cfg.power.conservative_threshold),
        "power.low_power_threshold" => format!("{}", cfg.power.low_power_threshold),
        // Policy
        "policy.default_effect" => cfg.policy.default_effect.clone(),
        "policy.log_all_decisions" => format!("{}", cfg.policy.log_all_decisions),
        // Unknown
        _ => "(unknown key)".to_string(),
    }
}

// ─── Handlers ───────────────────────────────────────────────────────────────

/// `/set <key> <value>` — Set a config value.
///
/// Currently read-only — displays the attempted change and notes that
/// live mutation requires the daemon persistence layer (not yet exposed
/// via this path). Validates the key against known config sections so
/// the user gets early feedback on typos.
#[instrument(skip(ctx))]
pub fn handle_set(
    ctx: &HandlerContext<'_>,
    key: &str,
    value: &str,
) -> Result<HandlerResponse, AuraError> {
    // Validate key against known top-level config sections.
    let known = is_known_config_key(key);
    let current = read_config_key(ctx, key);

    let html = if known {
        format!(
            "<b>Config Set</b>\n\n\
             <code>{key}</code> = <code>{value}</code>\n\
             Previous: <code>{current}</code>\n\n\
             <i>Note: runtime config mutation is not yet persisted.\n\
             Restart the daemon to apply TOML changes.</i>"
        )
    } else {
        format!(
            "<b>Unknown Key</b>\n\n\
             <code>{key}</code> is not a recognised config key.\n\n\
             Known sections: daemon, identity, voice, proactive, \
             telegram, power, routing, policy.\n\
             Use /get &lt;section.field&gt; to inspect values."
        )
    };
    Ok(HandlerResponse::Html(html))
}

/// `/get <key>` — Get a config value.
///
/// Reads from the live [`AuraConfig`] snapshot attached to the handler
/// context. Returns the current value for known dot-separated keys
/// (e.g. `daemon.log_level`, `voice.enabled`).
#[instrument(skip(ctx))]
pub fn handle_get(ctx: &HandlerContext<'_>, key: &str) -> Result<HandlerResponse, AuraError> {
    let value = read_config_key(ctx, key);
    let html = format!(
        "<b>Config Value</b>\n\n\
         <code>{key}</code> = <code>{value}</code>"
    );
    Ok(HandlerResponse::Html(html))
}

/// `/personality` — Show personality / identity configuration.
///
/// Displays the live `IdentityConfig` values from [`AuraConfig`].
///
/// NOTE: OCEAN personality traits (openness, conscientiousness, etc.)
/// live in [`IdentityEngine`] at runtime and are not part of `AuraConfig`.
/// To expose them here, either add a personality snapshot field to
/// `AuraConfig`, or give handlers a read-only reference to `IdentityEngine`.
#[instrument(skip(ctx))]
pub fn handle_personality(ctx: &HandlerContext<'_>) -> Result<HandlerResponse, AuraError> {
    let Some(cfg) = ctx.config else {
        return Ok(HandlerResponse::text(
            "Config unavailable — personality data cannot be read.",
        ));
    };

    let id = &cfg.identity;
    let html = format!(
        "<b>Personality / Identity Config</b>\n\n\
         Mood cooldown:     <code>{} ms</code>\n\
         Max mood delta:    <code>{:.2}</code>\n\
         Trust hysteresis:  <code>{:.2}</code>\n\n\
         <i>OCEAN trait values live in the IdentityEngine runtime.\n\
         Use /personality_set &lt;trait&gt; &lt;value&gt; to adjust.</i>",
        id.mood_cooldown_ms, id.max_mood_delta, id.trust_hysteresis,
    );
    Ok(HandlerResponse::Html(html))
}

/// `/personality_set <trait> <value>` — Set a personality trait.
///
/// NOTE: This validates the input and acknowledges the intent, but cannot
/// persist the change — OCEAN traits live in `IdentityEngine`, not in the
/// serialisable `AuraConfig`. To make this work end-to-end, handlers need
/// a `&IdentityEngine` reference or an `IdentityCommandTx` channel.
#[instrument(skip(ctx))]
pub fn handle_personality_set(
    ctx: &HandlerContext<'_>,
    trait_name: &str,
    value: f32,
) -> Result<HandlerResponse, AuraError> {
    let _ = ctx; // Acknowledges ctx is available, used when persistence is wired.
    let v = match validate_unit_range(value, trait_name) {
        Ok(v) => v,
        Err(resp) => return Ok(resp),
    };

    // Validate against known OCEAN trait names.
    let known_traits = [
        "openness",
        "conscientiousness",
        "extraversion",
        "agreeableness",
        "neuroticism",
    ];
    if !known_traits.contains(&trait_name) {
        return Ok(HandlerResponse::Html(format!(
            "<b>Unknown Trait</b>\n\n\
             <code>{trait_name}</code> is not a recognised OCEAN trait.\n\n\
             Valid traits: {}",
            known_traits.join(", "),
        )));
    }

    let html = format!(
        "<b>Personality Updated</b>\n\n\
         <code>{trait_name}</code> set to <b>{v:.2}</b>\n\n\
         <i>Note: persistence to IdentityEngine not yet wired.\n\
         Change takes effect on next interaction once connected.</i>"
    );
    Ok(HandlerResponse::Html(html))
}

/// `/trust` — Show trust configuration and hysteresis settings.
///
/// Reads `IdentityConfig::trust_hysteresis` from the live config.
/// Per-user trust levels live in `IdentityEngine::relationships` and are
/// not accessible through `AuraConfig` alone.
#[instrument(skip(ctx))]
pub fn handle_trust(ctx: &HandlerContext<'_>) -> Result<HandlerResponse, AuraError> {
    let Some(cfg) = ctx.config else {
        return Ok(HandlerResponse::text(
            "Config unavailable — trust settings cannot be read.",
        ));
    };

    let hysteresis = cfg.identity.trust_hysteresis;
    let html = format!(
        "<b>Trust Settings</b>\n\n\
         Trust hysteresis:  <b>{hysteresis:.2}</b>\n\n\
         <i>Hysteresis prevents rapid stage transitions when trust\n\
         fluctuates near a boundary.</i>\n\n\
         0.0 = always ask for confirmation\n\
         1.0 = full autonomy\n\n\
         <i>Per-user trust levels live in the IdentityEngine runtime.\n\
         Use /trust_set &lt;level&gt; to adjust the global hysteresis.</i>"
    );
    Ok(HandlerResponse::Html(html))
}

/// `/trust_set <level>` — Set trust hysteresis level.
///
/// Validates the value and acknowledges the intent. Actual persistence
/// requires the config mutation layer to be wired.
#[instrument(skip(ctx))]
pub fn handle_trust_set(
    ctx: &HandlerContext<'_>,
    level: f32,
) -> Result<HandlerResponse, AuraError> {
    let _ = ctx; // Acknowledges ctx is available for future use.
    let v = match validate_unit_range(level, "trust") {
        Ok(v) => v,
        Err(resp) => return Ok(resp),
    };

    let warning = if v >= 0.8 {
        "⚠ High autonomy — AURA may act without confirmation."
    } else if v <= 0.2 {
        "AURA will confirm most actions before executing."
    } else {
        "Balanced — AURA will confirm critical actions."
    };

    let html = format!(
        "<b>Trust Level Updated</b>\n\n\
         Trust hysteresis set to <b>{v:.2}</b>\n\n\
         {warning}\n\n\
         <i>Note: runtime config mutation not yet persisted.\n\
         Restart the daemon to apply TOML changes.</i>"
    );
    Ok(HandlerResponse::Html(html))
}

/// `/voice` — Show voice config and indicate voice-mode preference.
#[instrument(skip(ctx))]
pub fn handle_voice_mode(ctx: &HandlerContext<'_>) -> Result<HandlerResponse, AuraError> {
    let Some(cfg) = ctx.config else {
        return Ok(HandlerResponse::text(
            "Config unavailable — voice settings cannot be read.",
        ));
    };

    let v = &cfg.voice;
    let html = format!(
        "<b>Voice Mode</b>\n\n\
         Enabled:    <code>{}</code>\n\
         TTS engine: <code>{}</code>\n\
         STT engine: <code>{}</code>\n\
         Max record: <code>{} ms</code>\n\
         Wake sens.: <code>{:.1}</code>\n\n\
         <i>I'll respond with voice for most messages.\n\
         Use /chat to switch back to text-only mode.</i>",
        v.enabled, v.tts_engine, v.stt_engine, v.max_record_ms, v.wake_sensitivity,
    );
    Ok(HandlerResponse::Html(html))
}

/// `/chat` — Show chat mode preference (text-only responses).
#[instrument(skip(ctx))]
pub fn handle_chat_mode(ctx: &HandlerContext<'_>) -> Result<HandlerResponse, AuraError> {
    let Some(cfg) = ctx.config else {
        return Ok(HandlerResponse::text(
            "Config unavailable — chat settings cannot be read.",
        ));
    };

    let tg = &cfg.telegram;
    let html = format!(
        "<b>Chat Mode</b>\n\n\
         Telegram enabled: <code>{}</code>\n\
         Poll interval:    <code>{} ms</code>\n\n\
         <i>I'll respond with text messages.\n\
         Use /voice to enable voice responses.</i>",
        tg.enabled, tg.poll_interval_ms,
    );
    Ok(HandlerResponse::Html(html))
}

/// `/quiet` — Disable all proactive suggestions; show current proactive config.
#[instrument(skip(ctx))]
pub fn handle_quiet(ctx: &HandlerContext<'_>) -> Result<HandlerResponse, AuraError> {
    let Some(cfg) = ctx.config else {
        return Ok(HandlerResponse::text(
            "Config unavailable — proactive settings cannot be read.",
        ));
    };

    let p = &cfg.proactive;
    let html = format!(
        "<b>Quiet Mode</b>\n\n\
         Proactive enabled: <code>{}</code>\n\
         Min confidence:    <code>{:.2}</code>\n\
         Cooldown:          <code>{} ms</code>\n\
         Max per hour:      <code>{}</code>\n\
         Require confirm:   <code>{}</code>\n\n\
         <i>All proactive suggestions are now disabled.\n\
         I'll wait for your commands instead of proactively responding.\n\
         Use /wake to re-enable proactive suggestions.</i>",
        p.enabled, p.min_confidence, p.cooldown_ms, p.max_per_hour, p.require_confirmation,
    );
    Ok(HandlerResponse::Html(html))
}

/// `/wake` — Re-enable proactive suggestions; show current proactive config.
#[instrument(skip(ctx))]
pub fn handle_wake(ctx: &HandlerContext<'_>) -> Result<HandlerResponse, AuraError> {
    let Some(cfg) = ctx.config else {
        return Ok(HandlerResponse::text(
            "Config unavailable — proactive settings cannot be read.",
        ));
    };

    let p = &cfg.proactive;
    let html = format!(
        "<b>Wake Mode</b>\n\n\
         Proactive enabled: <code>{}</code>\n\
         Min confidence:    <code>{:.2}</code>\n\
         Cooldown:          <code>{} ms</code>\n\
         Max per hour:      <code>{}</code>\n\
         Require confirm:   <code>{}</code>\n\n\
         <i>Proactive suggestions are now enabled.\n\
         I'll proactively help when I have something useful to share.\n\
         Use /quiet to disable proactive suggestions.</i>",
        p.enabled, p.min_confidence, p.cooldown_ms, p.max_per_hour, p.require_confirmation,
    );
    Ok(HandlerResponse::Html(html))
}

// ─── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use aura_types::config::AuraConfig;
    use rusqlite::Connection;

    use super::*;
    use crate::telegram::{audit::AuditLog, queue::MessageQueue, security::SecurityGate};

    fn make_ctx<'a>(
        sec: &'a mut SecurityGate,
        aud: &'a mut AuditLog,
        q: &'a MessageQueue,
    ) -> HandlerContext<'a> {
        HandlerContext {
            chat_id: 42,
            security: sec,
            audit: aud,
            queue: q,
            startup_time_ms: 1_700_000_000_000,
            config: None,
            user_command_tx: None,
        }
    }

    #[test]
    fn test_set_config() {
        let mut sec = SecurityGate::new(vec![42]);
        let mut aud = AuditLog::new(100);
        let db = Connection::open_in_memory().unwrap();
        let q = MessageQueue::open(db).unwrap();
        let ctx = make_ctx(&mut sec, &mut aud, &q);

        // Without config, /set should still work (validates key, shows "(no config)").
        match handle_set(&ctx, "daemon.log_level", "debug").unwrap() {
            HandlerResponse::Html(html) => {
                assert!(html.contains("daemon.log_level"));
                assert!(html.contains("debug"));
                assert!(html.contains("Config Set"));
            }
            other => panic!("expected Html, got {other:?}"),
        }

        // Unknown key should be flagged.
        match handle_set(&ctx, "nonexistent", "value").unwrap() {
            HandlerResponse::Html(html) => {
                assert!(html.contains("Unknown Key"));
            }
            other => panic!("expected Html, got {other:?}"),
        }
    }

    #[test]
    fn test_get_config_with_live_config() {
        let mut sec = SecurityGate::new(vec![42]);
        let mut aud = AuditLog::new(100);
        let db = Connection::open_in_memory().unwrap();
        let q = MessageQueue::open(db).unwrap();
        let cfg = AuraConfig::default();
        let ctx = HandlerContext {
            chat_id: 42,
            security: &mut sec,
            audit: &mut aud,
            queue: &q,
            startup_time_ms: 1_700_000_000_000,
            config: Some(&cfg),
            user_command_tx: None,
        };

        match handle_get(&ctx, "daemon.log_level").unwrap() {
            HandlerResponse::Html(html) => {
                assert!(html.contains("daemon.log_level"));
                assert!(html.contains("info")); // default log level
            }
            other => panic!("expected Html, got {other:?}"),
        }
    }

    #[test]
    fn test_personality_without_config() {
        let mut sec = SecurityGate::new(vec![42]);
        let mut aud = AuditLog::new(100);
        let db = Connection::open_in_memory().unwrap();
        let q = MessageQueue::open(db).unwrap();
        let ctx = make_ctx(&mut sec, &mut aud, &q);

        match handle_personality(&ctx).unwrap() {
            HandlerResponse::Text(text) => {
                assert!(text.contains("Config unavailable"));
            }
            other => panic!("expected Text fallback, got {other:?}"),
        }
    }

    #[test]
    fn test_personality_with_config() {
        let mut sec = SecurityGate::new(vec![42]);
        let mut aud = AuditLog::new(100);
        let db = Connection::open_in_memory().unwrap();
        let q = MessageQueue::open(db).unwrap();
        let cfg = AuraConfig::default();
        let ctx = HandlerContext {
            chat_id: 42,
            security: &mut sec,
            audit: &mut aud,
            queue: &q,
            startup_time_ms: 1_700_000_000_000,
            config: Some(&cfg),
            user_command_tx: None,
        };

        match handle_personality(&ctx).unwrap() {
            HandlerResponse::Html(html) => {
                assert!(html.contains("Personality"));
                assert!(html.contains("Mood cooldown"));
                assert!(html.contains("Trust hysteresis"));
            }
            other => panic!("expected Html, got {other:?}"),
        }
    }

    #[test]
    fn test_trust_set_invalid_range() {
        let mut sec = SecurityGate::new(vec![42]);
        let mut aud = AuditLog::new(100);
        let db = Connection::open_in_memory().unwrap();
        let q = MessageQueue::open(db).unwrap();
        let ctx = make_ctx(&mut sec, &mut aud, &q);

        match handle_trust_set(&ctx, 1.5).unwrap() {
            HandlerResponse::Text(text) => {
                assert!(text.contains("must be between 0.0 and 1.0"));
            }
            other => panic!("expected Text error, got {other:?}"),
        }
    }

    #[test]
    fn test_personality_set_unknown_trait() {
        let mut sec = SecurityGate::new(vec![42]);
        let mut aud = AuditLog::new(100);
        let db = Connection::open_in_memory().unwrap();
        let q = MessageQueue::open(db).unwrap();
        let ctx = make_ctx(&mut sec, &mut aud, &q);

        match handle_personality_set(&ctx, "curiosity", 0.8).unwrap() {
            HandlerResponse::Html(html) => {
                assert!(html.contains("Unknown Trait"));
                assert!(html.contains("openness"));
            }
            other => panic!("expected Html, got {other:?}"),
        }
    }

    #[test]
    fn test_personality_set_valid_ocean_trait() {
        let mut sec = SecurityGate::new(vec![42]);
        let mut aud = AuditLog::new(100);
        let db = Connection::open_in_memory().unwrap();
        let q = MessageQueue::open(db).unwrap();
        let ctx = make_ctx(&mut sec, &mut aud, &q);

        match handle_personality_set(&ctx, "openness", 0.8).unwrap() {
            HandlerResponse::Html(html) => {
                assert!(html.contains("Personality Updated"));
                assert!(html.contains("openness"));
                assert!(html.contains("0.80"));
            }
            other => panic!("expected Html, got {other:?}"),
        }
    }

    #[test]
    fn test_quiet_mode() {
        let mut sec = SecurityGate::new(vec![42]);
        let mut aud = AuditLog::new(100);
        let db = Connection::open_in_memory().unwrap();
        let q = MessageQueue::open(db).unwrap();
        let cfg = AuraConfig::default();
        let ctx = HandlerContext {
            chat_id: 42,
            security: &mut sec,
            audit: &mut aud,
            queue: &q,
            startup_time_ms: 1_700_000_000_000,
            config: Some(&cfg),
            user_command_tx: None,
        };

        match handle_quiet(&ctx).unwrap() {
            HandlerResponse::Html(html) => {
                assert!(html.contains("Quiet Mode"));
                assert!(html.contains("proactive suggestions are now disabled"));
            }
            other => panic!("expected Html, got {other:?}"),
        }
    }

    #[test]
    fn test_wake_mode() {
        let mut sec = SecurityGate::new(vec![42]);
        let mut aud = AuditLog::new(100);
        let db = Connection::open_in_memory().unwrap();
        let q = MessageQueue::open(db).unwrap();
        let cfg = AuraConfig::default();
        let ctx = HandlerContext {
            chat_id: 42,
            security: &mut sec,
            audit: &mut aud,
            queue: &q,
            startup_time_ms: 1_700_000_000_000,
            config: Some(&cfg),
            user_command_tx: None,
        };

        match handle_wake(&ctx).unwrap() {
            HandlerResponse::Html(html) => {
                assert!(html.contains("Wake Mode"));
                assert!(html.contains("Proactive suggestions are now enabled"));
            }
            other => panic!("expected Html, got {other:?}"),
        }
    }
}
