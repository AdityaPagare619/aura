# AURA v4 — Termux Constraints Research Report
## Date: March 31, 2026

---

## Executive Summary

This report documents critical constraints for running AURA on Android via Termux, based on web research and real device testing.

---

## 1. File Permissions on Android

### Problem
Termux runs as user `u0_a487`, but `/data/local/tmp/` is owned by `shell` user.

**Impact:** 
- Termux can READ files in `/data/local/tmp/`
- Termux CANNOT modify permissions (chmod) on files in `/data/local/tmp/`
- Termux CANNOT execute files owned by other users in some Android versions

### Solution
```rust
// WRONG: Hardcoded path to permission-restricted location
const ANDROID_NEOCORTEX_PATH: &str = "/data/local/tmp/aura-neocortex";

// RIGHT: Check user-accessible paths first
// 1. $PREFIX/bin/aura-neocortex (Termux system bin)
// 2. $HOME/bin/aura-neocortex (user bin)
// 3. $HOME/.local/bin/aura-neocortex (XDG bin)
// 4. /data/local/tmp/aura-neocortex (legacy, last resort)
```

### Source
- Reddit: "i can cd to /data/local/tmp, but cant ls. it says permission denied"
- Termux GitHub Issue #4486: "Permission denied on /storage/emulated/0/"

---

## 2. Android OOM Killer

### Problem
Android aggressively kills background processes to free memory.

**Impact:**
- Termux processes get killed when screen is off
- Long-running daemons don't survive
- Even with wakelock, Android 12+ introduces phantom process killer

### Research Findings
1. **Phantom Process Killer (Android 12+):** Kills child processes that consume too much CPU
2. **Wakelock:** Helps but doesn't guarantee survival
3. **Foreground Service:** Required for guaranteed persistence

### Solution
```bash
# In Termux, use wakelock:
termux-wake-lock

# Also ensure Termux notification is enabled
# (notification absence causes service termination - Issue #4657)
```

### Source
- Termux Issue #2366: Android 12 Phantom Processes Killed
- Termux Issue #4657: Termux service stops if no notification
- Sagar Tamang's "The Persistence Protocol" blog (2026-02-07)

---

## 3. LLM Memory Constraints

### Problem
Running LLMs on Android requires significant RAM.

**Device Tiers:**
| RAM | Model | Status |
|-----|-------|--------|
| 2GB | TinyLlama 1.1B Q4 | ✅ Works |
| 4GB | Phi-2 2.7B Q4 | ✅ Works |
| 7GB | Qwen3-4B Q4_K_M | ✅ Works (our device) |
| 8GB+ | Qwen3-8B Q4_K_M | ✅ Works |

### Research Findings
1. Vulkan backend uses MORE RAM than CPU-only
2. `--cache-ram` can be set to 0 to disable prompt cache (saves memory)
3. Context size (`n_ctx`) significantly impacts memory usage

### Source
- llama.cpp Issue #7351: Android/Termux higher RAM usage with Vulkan
- Reddit: "Running a local LLM on Android with Termux" (2026-03-06)

---

## 4. IPC Communication

### Problem
Daemon and neocortex need to communicate via IPC.

**Options:**
1. Abstract Unix sockets (works in Termux)
2. Named pipes (limited support)
3. TCP localhost (always works)

### Research Findings
- Abstract Unix sockets work in Termux (see wantguns.dev blog)
- Socket path `@aura-daemon` is valid for abstract sockets
- TCP localhost:8080 is simpler but less secure

### Current Implementation
```rust
// spawn.rs creates abstract socket address
IPC bind: prepared abstract socket address address="@aura-daemon"
```

### Source
- wantguns.dev: "IPC between Termux and Other Android Apps using ZMQ"
- Stack Overflow: "Android - How to connect to abstract socket"

---

## 5. Hardcoded Path Anti-Pattern

### Problem
The code had hardcoded paths that don't work across all devices.

### Enterprise Principle Violated
**P4:** "If a system requires manual fixing repeatedly, the system design is incomplete."

### Solution
Check multiple paths in priority order:
```rust
fn resolve_neocortex_path() -> PathBuf {
    // 1. Env var override
    // 2. $PREFIX/bin/ (Termux - always works)
    // 3. $HOME/bin/ (user install)
    // 4. $HOME/.local/bin/ (XDG)
    // 5. Default path (may have permission issues)
}
```

**Already implemented:** We fixed this in spawn.rs (edit applied earlier).

---

## 6. Termux Environment Detection

### Problem
How to detect if running in Termux?

### Solution
```rust
fn is_termux() -> bool {
    std::env::var("PREFIX")
        .map(|p| p.contains("com.termux"))
        .unwrap_or(false)
}
```

### Confirmed Paths
- `$PREFIX` = `/data/data/com.termux/files/usr`
- `$HOME` = `/data/data/com.termux/files/home`
- `$TMPDIR` = `/data/data/com.termux/files/usr/tmp`

---

## 7. Recommendations

### Code Changes Needed
1. ✅ DONE: Fix neocortex path resolution (check multiple paths)
2. TODO: Add config option for neocortex_path
3. TODO: Auto-detect Termux environment
4. TODO: Handle OOM gracefully (restart daemon automatically)
5. TODO: Add `termux-wake-lock` to install.sh

### Documentation Needed
1. Document all paths and permissions
2. Document wakelock requirements
3. Document device compatibility matrix

---

## Sources

1. Termux GitHub Issues (permission, OOM, phantom processes)
2. llama.cpp GitHub Issues (Android memory)
3. Reddit r/termux (permission issues)
4. Reddit r/LocalLLaMA (running LLMs on Android)
5. wantguns.dev (IPC on Termux)
6. Sagar Tamang blog (persistence protocol)

---

*Last updated: March 31, 2026*
