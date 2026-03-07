# Agent C — Voice & Telegram Bridge Wiring

## Goal

Wire the existing (fully tested) bridge layer into the AURA daemon's startup
and main event loop so that voice and Telegram responses are actually delivered
instead of being logged and dropped.

## Status: IN PROGRESS

## Prior Art

The bridge code is **complete** with 31 passing tests across 4 files:

| File | Lines | Tests | Purpose |
|------|-------|-------|---------|
| `bridge/mod.rs` | 224 | 4 | `InputChannel` trait, `BridgeHandle`, `spawn_bridge()` |
| `bridge/router.rs` | 480 | 8 | `ResponseRouter` — fan-out by `variant_key()` |
| `bridge/voice_bridge.rs` | 267 | 4 | Voice ↔ daemon bridge (`VoiceBridge`) |
| `bridge/telegram_bridge.rs` | 530 | 15 | Telegram ↔ daemon bridge (`TelegramBridge`) |

`lib.rs` already declares `pub mod bridge;`.
`channels.rs` already has `InputSource::Voice`, `InputSource::Telegram { chat_id }`, `variant_key()`.

## Architecture

```text
DaemonChannels::new()
  ├── response_tx  → cloned into LoopSubsystems (handlers send responses)
  └── response_rx  → moved into ResponseRouter (replaces select! branch)

ResponseRouter::register("voice")    → voice_bridge_rx
ResponseRouter::register("telegram") → telegram_bridge_rx
ResponseRouter::spawn()              → RouterHandle (background task)

VoiceBridge::new(engine, cancel)     → spawn_bridge() → BridgeHandle
TelegramBridge::new(config, cancel, queue) → spawn_bridge() → BridgeHandle
```

## Changes

### 1. `startup.rs` — Add bridge/router fields to `SubSystems`

- Add `voice_bridge: Option<BridgeHandle>` and `telegram_bridge: Option<BridgeHandle>`
- Add `response_router: Option<RouterHandle>`
- Initialize all three as `None` in `phase_subsystems_init()` (actual spawning
  happens in `main_loop::run()` because bridges need channel endpoints)
- Update `Debug` impl to include new fields

### 2. `main_loop.rs` — Wire router + bridges into `run()`

In `run()`, after `channels.split()`:

1. **Extract `response_rx`** from `rxs` using `std::mem::replace` with a dummy
   closed channel — the router will own it instead of the select! branch.
2. **Create `ResponseRouter::new(response_rx)`**
3. **Register bridges**: `router.register("voice")` and `router.register("telegram")`
4. **Create bridge instances**:
   - `VoiceBridge::new(VoiceEngine::default(), cancel.clone())`
   - `TelegramBridge::new(TelegramConfig::default(), cancel.clone(), None)`
5. **Spawn bridges** via `spawn_bridge()` with `cmd_tx.clone()` and per-bridge `response_rx`
6. **Spawn router** via `router.spawn()` → `RouterHandle`
7. **Store handles** in `state.subsystems` for health monitoring
8. **Replace `response_rx` select! branch** — the dummy channel will close
   immediately, decrementing `open_channels` by 1 on first iteration. The actual
   routing is now handled by the spawned `ResponseRouter` task. Reduce
   initial `open_channels` from 8 to 7 since response_rx is no longer a
   live channel in the select.

### 3. No changes to bridge internals

The bridge files (`mod.rs`, `router.rs`, `voice_bridge.rs`, `telegram_bridge.rs`)
are NOT modified.

## Constraints

- NO `.unwrap()` — use `Result<T, AuraError>` everywhere
- Use `tracing::{info, warn, error, debug}` for logging
- Add doc comments to all public functions
- Must pass `cargo check`
- Must not regress from 1931 passing tests
