# AURA v4 Boot Stages

**Document**: `docs/runtime/BOOT-STAGES.md`  
**Version**: 4.0.0-stable  
**Date**: 2026-03-22  
**Status**: ACTIVE  
**Owner**: Runtime Platform Charter

---

## Overview

AURA v4 boots in **5 sequential stages** with a total budget of **<20 seconds** (per CONTRACT.md Section 4.1). Each stage validates the platform contract and initializes a specific subsystem.

```
┌─────────────────────────────────────────────────────────────────┐
│                    AURA BOOT STAGES                            │
├─────────────────────────────────────────────────────────────────┤
│                                                                  │
│  Stage 1        Stage 2        Stage 3        Stage 4    Stage 5│
│  PRE-FLIGHT     CONFIG         HTTP           TELEGRAM   READY │
│  (Environment)  (Loading)      (Backend)      (MTProto)  (Loop)│
│                                                                  │
│  ┌─────────┐   ┌─────────┐   ┌─────────┐   ┌─────────┐ ┌─────┐ │
│  │SDK Check│   │Dir Init │   │curl init│   │MTProto  │ │Event│ │
│  │Arch Vfy │   │Load CFG │   │HTTP Pool│   │session  │ │Loop │ │
│  │Termux   │   │Validate │   │TLS setup│   │bot auth │ │Start│ │
│  │RAM/Disk │   │DB setup │   │Connect  │   │Updates  │ │Ready│ │
│  └────┬────┘   └────┬────┘   └────┬────┘   └────┬────┘ └──┬──┘ │
│       │              │              │              │         │   │
│       ▼              ▼              ▼              ▼         ▼   │
│  0-2 sec        2-5 sec        5-10 sec      10-18 sec  18-20s │
│                                                                  │
└─────────────────────────────────────────────────────────────────┘
```

---

## Stage 1: PRE-FLIGHT (Environment Validation)

**Duration Budget**: < 2 seconds  
**Source**: `crates/aura-daemon/src/startup/env_check.rs`  
**Contract Reference**: CONTRACT.md Section 2

### 1.1 Android API Level Check

```rust
// Read system API level
let sdk_int = android.os.Build.VERSION.SDK_INT;

assert!(sdk_int >= 24, "AURA requires Android 7.0 (API 24) or higher");
assert!(sdk_int <= 35, "AURA has not been tested on Android 16+");
```

**Failure Code**: F002-OS_VERSION

### 1.2 Architecture Verification

```rust
// Verify arm64-v8a
let cpu_abi = android.os.Build.CPU_ABI;
assert!(cpu_abi == "arm64-v8a", "AURA requires 64-bit ARM");
```

**Supported**: `arm64-v8a`  
**Not Supported**: ARMv7, x86, x86_64, RISC-V

**Failure Code**: F003-ABI_CONTRACT

### 1.3 Termux Environment Check

```rust
// Verify Termux is present
let prefix = env::var("PREFIX").unwrap_or_default();
assert!(prefix.contains("com.termux"), "Termux environment required");

// Verify pkg command available
let pkg_version = Command::new("pkg").arg("version").output()?;
assert!(pkg_version.status.success(), "pkg command unavailable");
```

**Failure Code**: F002-ENV_MISSING

### 1.4 Memory Validation

```rust
// Read /proc/meminfo
let mem_available = read_meminfo()?;  // kB
let ram_gb = mem_available / 1024 / 1024;

assert!(ram_gb >= 3, "Minimum 3GB RAM required, found {}GB", ram_gb);
```

**Failure Code**: F005-MEMORY_CONTRACT

### 1.5 Storage Validation

```rust
// Check available storage
let storage_free = df_h_output()?;  // GB
assert!(storage_free >= 2, "Minimum 2GB free storage required");
```

**Failure Code**: F002-STORAGE_CONTRACT

### 1.6 bionic libc Verification

```rust
// Verify bionic is present (not glibc or musl)
let libc_path = Path::new("/system/lib64/libc.so");
assert!(libc_path.exists(), "bionic libc not found");
```

**Failure Code**: F003-ABI_CONTRACT

---

## Stage 2: CONFIG (Configuration Loading)

**Duration Budget**: < 3 seconds  
**Source**: `crates/aura-daemon/src/startup/config.rs`  
**Contract Reference**: CONTRACT.md Section 3

### 2.1 Directory Initialization

```rust
// Create ~/.aura/ if not exists
let aura_dir = PathBuf::from(env::var("HOME")?).join(".aura");
aura_dir.create_dir_all()?;

// Create subdirectories
["logs", "checkpoints", "models", "crash-reports"]
    .iter()
    .for_each(|subdir| (aura_dir.join(subdir).create_dir_all()));
```

### 2.2 Configuration Loading

```rust
// Load ~/.aura/config.toml
let config = if config_path.exists() {
    Config::load_from_file(&config_path)?
} else {
    Config::default()  // First-run defaults
};

// Validate required fields
assert!(config.telegram_bot_token.is_some(), "Telegram bot token required");
assert!(config.model_path.is_some(), "LLM model path required");
```

### 2.3 Database Initialization

```rust
// Open or create SQLite database
let db_path = aura_dir.join("aura.db");
let db = Connection::open(&db_path)?;

// Initialize WAL mode for performance
db.execute_batch(
    "PRAGMA journal_mode=WAL;
     PRAGMA synchronous=NORMAL;
     PRAGMA cache_size=-64000;  // 64MB
     PRAGMA mmap_size=4194304;"  // 4MB
)?;

// Create tables on first run
create_tables_if_missing(&db)?;
```

### 2.4 Checkpoint Restore

```rust
// Load last daemon state if exists
let checkpoint_path = aura_dir.join("checkpoints").join("daemon.state.bin");
if checkpoint_path.exists() {
    let state: DaemonState = bincode::decode_from_file(&checkpoint_path)?;
    restore_subsystems_from_checkpoint(state)?;
} else {
    log::info!("First run: starting with empty state");
}
```

**Failure Mode**: Non-fatal. If checkpoint is corrupt, start with empty state.

---

## Stage 3: HTTP (Backend Initialization)

**Duration Budget**: < 5 seconds  
**Source**: `crates/aura-daemon/src/startup/http.rs`  
**Contract Reference**: CONTRACT.md Section 2.5

### 3.1 curl Initialization

```rust
// Verify curl is available with OpenSSL
let curl_version = Command::new("curl").arg("--version").output()?;
let output = String::from_utf8_lossy(&curl_version.stdout);
assert!(output.contains("OpenSSL"), "curl requires OpenSSL support");

// Initialize HTTP client pool
let http_client = Client::builder()
    .pool_max_idle_per_host(4)
    .pool_idle_timeout(Duration::from_secs(300))
    .build()?;
```

### 3.2 Dependency Verification

```rust
// Verify all required packages per CONTRACT.md
let deps = [
    ("curl", "7.68.0"),
    ("git", "2.30.0"),
    ("bash", "5.0"),
    ("openssl", "1.1.1"),
];

for (cmd, min_version) in deps {
    let output = Command::new(cmd).arg("--version").output()?;
    let version = parse_version(&output)?;
    assert!(version >= min_version, "{} version {} < {}", cmd, version, min_version);
}
```

**Failure Code**: F002-DEPENDENCY_MISSING

### 3.3 TLS Configuration

```rust
// Configure HTTPS settings
let tls_config = rustls::ClientConfig::builder()
    .with_safe_defaults()
    .with_root_cert_store(root_store)
    .with_no_client_auth();
```

---

## Stage 4: TELEGRAM (MTProto Session)

**Duration Budget**: < 8 seconds  
**Source**: `crates/aura-daemon/src/startup/telegram.rs`  
**Contract Reference**: CONTRACT.md Section 3.3

### 4.1 MTProto Session Creation

```rust
// Initialize Telegram session
let session = MTProtoSession::new(
    config.telegram_api_id,
    config.telegram_api_hash,
    config.telegram_bot_token,
)?;
```

### 4.2 Bot Authentication

```rust
// Authenticate as bot
let bot_info = session.invoke(&GetMeRequest).await?;
log::info!("Authenticated as: @{}", bot_info.username);

// Verify bot has not been disabled
assert!(!bot_info.is_deleted, "Bot has been deleted");
assert!(!bot_info.is_bot, "Authentication failed");
```

**Failure Code**: F007-RUNTIME_CRASH (if token invalid)

### 4.3 Update Stream Initialization

```rust
// Start receiving updates
let update_stream = session.start_updates_stream(UpdateKind::Message)?;

log::info!(
    "Telegram MTProto session established (dc: {}, server: {})",
    session.dc_id(),
    session.server()
);
```

### 4.4 Connection Verification

```rust
// Send test ping to verify connection
session.invoke(&PingRequest { data: [0u8; 32] }).await?;
log::info!("Telegram connection verified");
```

**Failure Code**: F002-NETWORK (if Telegram blocked)

---

## Stage 5: READY (Main Event Loop)

**Duration Budget**: < 2 seconds  
**Source**: `crates/aura-daemon/src/daemon_core/main_loop.rs`  
**Contract Reference**: CONTRACT.md Section 4.1

### 5.1 Subsystem Finalization

```rust
// Finalize all subsystems
subsystems::finalize()?;

// Start cron/scheduler
scheduler::start_consolidation_timers()?;
scheduler::start_checkpoint_timer()?;

// Enter main event loop
log::info!("AURA v4.0 READY - accepting requests");
daemon_state = DaemonState::Running;
```

### 5.2 Main Event Loop

```rust
// tokio::select! over 7 channels
loop {
    tokio::select! {
        // Telegram updates
        Some(update) = telegram_rx.recv() => {
            handle_telegram_update(update).await?;
        }

        // Accessibility events
        Some(event) = a11y_rx.recv() => {
            handle_a11y_event(event).await?;
        }

        // CLI commands
        Some(cmd) = cli_rx.recv() => {
            handle_cli_command(cmd).await?;
        }

        // Scheduled tasks
        Some(task) = scheduler_rx.recv() => {
            execute_scheduled_task(task).await?;
        }

        // IPC from neocortex
        Some(response) = ipc_rx.recv() => {
            handle_inference_response(response).await?;
        }

        // Checkpoint timer
        _ = checkpoint_timer.tick() => {
            save_checkpoint().await?;
        }

        // Shutdown signal
        _ = shutdown_rx.recv() => {
            graceful_shutdown().await?;
            break;
        }
    }
}
```

### 5.3 Ready State Notification

```rust
// Post to OutcomeBus: system ready
outcome_bus::publish(SystemEvent::DaemonReady {
    version: env!("CARGO_PKG_VERSION"),
    uptime_ms: start_time.elapsed().as_millis() as u64,
    boot_stages_passed: 5,
});

// Register with Android as foreground service
// (keeps daemon alive in background)
```

---

## Boot Log Format

Every boot attempt produces a structured log:

```json
{
  "version": "4.0.0",
  "boot_id": "uuid-v4",
  "timestamp": "2026-03-22T10:30:00.000Z",
  "device": {
    "os_version": "Android 15 (API 35)",
    "architecture": "aarch64",
    "device_model": "Moto G45 5G",
    "ram_mb": 8192,
    "storage_free_gb": 45
  },
  "stages": [
    {
      "name": "PRE-FLIGHT",
      "duration_ms": 1234,
      "status": "PASS",
      "checks": ["api_level_ok", "arch_ok", "termux_present", "bionic_present", "ram_ok", "storage_ok"]
    },
    {
      "name": "CONFIG",
      "duration_ms": 567,
      "status": "PASS",
      "checks": ["dirs_created", "config_loaded", "db_initialized", "checkpoint_restored"]
    },
    {
      "name": "HTTP",
      "duration_ms": 890,
      "status": "PASS",
      "checks": ["curl_ok", "deps_verified", "tls_configured"]
    },
    {
      "name": "TELEGRAM",
      "duration_ms": 3456,
      "status": "PASS",
      "checks": ["session_created", "bot_authenticated", "updates_started", "connection_verified"]
    },
    {
      "name": "READY",
      "duration_ms": 123,
      "status": "PASS",
      "checks": ["subsystems_finalized", "scheduler_started", "main_loop_entered"]
    }
  ],
  "overall_status": "PASS",
  "total_duration_ms": 6270,
  "contract_version": "1.0"
}
```

---

## Error Handling

| Stage | On Failure | Failure Code | Recovery |
|-------|-----------|-------------|----------|
| PRE-FLIGHT | Exit with clear error | F002, F003, F005 | User must fix environment |
| CONFIG | Exit with config error | F007 | User must fix config |
| HTTP | Warn, continue offline | F002 | Install missing deps |
| TELEGRAM | Retry 3x, then degraded | F002-NETWORK | Operate without Telegram |
| READY | Exit with crash report | F007 | Check logs, restart |

---

## Related Documents

| Document | Purpose |
|----------|---------|
| `docs/build/CONTRACT.md` | Platform contract specification |
| `docs/build/FAILURE_TAXONOMY.md` | Failure classification codes |
| `docs/workflows/AURA-BOOT-SEQUENCE.md` | Detailed 8-phase boot trace |

---

**END OF DOCUMENT**
