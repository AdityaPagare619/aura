# AURA v4 — Complete Device Reference (March 31, 2026)

## Device Information
- **Device**: Moto G45 5G
- **Android**: 15 (API 35)
- **Architecture**: aarch64 (ARM64)
- **RAM**: 7 GB
- **SoC**: MediaTek Dimensity 6300

---

## AURA Architecture (226 files, 15+ modules)

### Daemon Core (8,674 lines in main_loop.rs)
- **Event Flow**: Telegram → CommandParser → Amygdala → PolicyGate → Contextor → RouteClassifier
- **Routes**: System1 (fast) or System2 (neocortex: complex inference)
- **IPC**: DaemonToNeocortex → LLM inference → NeocortexToDaemon

### Subsystems (All initialized successfully!)
- **Memory**: 4-tier system (episodic, semantic, archive, workflows)
- **Identity**: Ethics, consent tracking, personality
- **Executor**: Task execution, ETG (Execution Task Graph)
- **Planner**: Enhanced planning with workflow observation
- **Pipeline**: Event parser, command parser, amygdala, contextor
- **Routing**: Route classifier, System1, System2
- **Goals**: BDI scheduler, goal tracker, conflict resolver, goal decomposer, goal registry
- **Screen**: Anti-bot protection
- **Platform**: JNI bridge
- **Extensions**: Capability loader, extension discovery
- **ARC**: Proactive engine, arc manager
- **Telegram**: Polling, queue, bridge
- **Voice**: STT, TTS (disabled for testing)
- **Policy**: 50 hardened safety rules

---

## File Locations on Device

| Item | Path | Notes |
|------|------|-------|
| **Termux Home** | `/data/data/com.termux/files/home/` | Main working directory |
| **Termux Prefix** | `/data/data/com.termux/files/usr/` | System binaries |
| **llama-server** | `$PREFIX/bin/llama-server` | Termux package |
| **aura-daemon** | `~/bin/aura-daemon` | Main agent binary |
| **aura-neocortex** | `~/bin/aura-neocortex` | LLM brain binary |
| **config.toml** | `~/.config/aura/config.toml` | Full configuration |
| **Model** | `~/.local/share/aura/models/tinyllama.gguf` | 638MB LLM model |
| **Database** | `~/.local/share/aura/db/aura.db` | SQLite with WAL |
| **Memory DBs** | `~/.local/share/aura/db/memory/` | 4-tier memory |
| **ETG DB** | `~/.local/share/aura/db/etg.db` | Execution Task Graph |
| **Vault Key** | `~/.local/share/aura/db/vault.key` | Encryption key |
| **Identity WAL** | `~/.local/share/aura/db/identity.wal` | Identity journal |
| **Logs** | `/sdcard/Aura/*.log` | Readable via adb |

---

## Config Structure (aura.toml)

### Required Sections:
```toml
[daemon]
checkpoint_interval_s = 300
rss_warning_mb = 28
rss_ceiling_mb = 30

[amygdala]
instant_threshold = 0.65
weight_lex = 0.40
weight_src = 0.25
weight_time = 0.20
weight_anom = 0.15

[neocortex]
default_n_ctx = 4096      # REQUIRED
n_threads = 4             # REQUIRED
max_memory_mb = 2048      # REQUIRED
inference_timeout_ms = 60000  # REQUIRED
model_dir = "/data/data/com.termux/files/home/.local/share/aura/models"  # REQUIRED

[neocortex.backend]
backend_priority = ["http", "ffi", "stub"]

[neocortex.backend.http]
base_url = "http://localhost:8080"
model_name = "tinyllama"
timeout_secs = 60
health_check = true

[execution]
max_steps_normal = 200
max_steps_safety = 50
max_steps_power = 500

[power]
daily_token_budget = 50000
conservative_threshold = 50
low_power_threshold = 30
critical_threshold = 15
emergency_threshold = 5

[identity]
mood_cooldown_ms = 60000
max_mood_delta = 0.2
trust_hysteresis = 0.05

[sqlite]
db_path = "/data/data/com.termux/files/home/.local/share/aura/db/aura.db"
wal_size_limit = 4194304
max_episodes = 10000
max_semantic_entries = 5000

[telegram]
enabled = true
bot_token = "YOUR_TOKEN"
allowed_chat_ids = [YOUR_ID]
poll_interval_ms = 2000

[screen]
max_tree_depth = 15
snapshot_timeout_ms = 2000
max_nodes = 500
enable_hash_diff = true
```

---

## Known Issues & Fixes

### Issue 1: Neocortex spawn path
- **Error**: `Permission denied (os error 13) path=/data/local/tmp/aura-neocortex`
- **Fix**: `cp ~/bin/aura-neocortex /data/local/tmp/ && chmod +x /data/local/tmp/aura-neocortex`
- **Root Cause**: Hardcoded path in daemon code

### Issue 2: UTF-8 boundary panic
- **Error**: `byte index 1458 is not a char boundary; it is inside '≥'`
- **Location**: `crates/aura-daemon/src/pipeline/entity.rs:231`
- **Cause**: Oversized Telegram message with multi-byte UTF-8 characters
- **Fix**: Need to fix string slicing in entity.rs (use char_indices instead of byte indices)

### Issue 3: Database directory missing
- **Error**: `unable to open database file`
- **Fix**: `mkdir -p ~/.local/share/aura/db`
- **Root Cause**: Directory not created by installer

---

## Startup Sequence

```
1. JNI Load → Android validation
2. Runtime Init → tracing subscriber
3. Database Open → WAL + mmap 4MB
4. State Restore → checkpoint load
5. SubSystems Init → all 15+ modules
6. IPC Bind → abstract socket @aura-daemon
7. Cron Schedule → timer heap
8. Ready → main loop starts
```

**Total startup time**: ~84ms

---

## Telegram Integration

- **Bot**: @AuraTheBegginingBot
- **Owner Chat ID**: 8407946567
- **Allowed Chats**: 1 (owner only)
- **Poll Interval**: 2000ms
- **Trust Level**: 0.5 (initial)

---

## Testing Checklist

- [x] Termux installed
- [x] llama-server running
- [x] Model downloaded (638MB)
- [x] Config created (complete)
- [x] aura-daemon binary present
- [x] aura-neocortex binary present
- [x] Database directory created
- [x] Daemon startup (8 phases, 84ms)
- [x] Telegram engine initialized
- [x] All subsystems initialised
- [ ] Neocortex spawning (needs path fix)
- [ ] Full LLM inference through daemon
- [ ] Screen control testing
- [ ] Memory persistence testing

---

*Last updated: March 31, 2026*
