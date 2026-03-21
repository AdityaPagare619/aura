# AURA v4 — Comprehensive Device Testing Methodology

## Philosophy

**CI tells you if code compiles. Device tells you if code WORKS.**

The AURA codebase has 5 crates, 30+ modules, and 115+ commits of fixes.
Only by running on real Android can we verify:
- Does it start without SIGSEGV?
- Does memory system initialize?
- Do ethics enforce?
- Does Telegram connect?
- Does every module respond?

---

## Testing Layers

### Layer 1: Binary Startup (F001)
**What:** Does aura-daemon start without crashing?
**How:** `./aura-daemon --version` → Exit code must be 0

### Layer 2: Module Health
**What:** Do individual modules initialize?
**How:** Check each module's init function

### Layer 3: Integration
**What:** Do modules talk to each other?
**How:** Telegram → Daemon → Memory → Inference → Response

### Layer 4: End-to-End
**What:** Does the full ReAct loop work?
**How:** "Hey Aura" → Telegram → Response

---

## Module Test Matrix

| Module | File | Test Command | Expected |
|--------|------|-------------|----------|
| Identity | identity/mod.rs | ethics test | Block harmful |
| Memory | memory/mod.rs | memory init | 4 tiers load |
| Inference | neocortex/ | model load | LLM ready |
| Telegram | telegram/mod.rs | bot connect | Polls active |
| Policy | policy/gate.rs | rule check | Deny-by-default |
| ARC | arc/mod.rs | attention | Suggestions |
| Execution | execution/ | planner | Plan → Execute |
| Persistence | persistence/ | vault | Encrypt/decrypt |

---

## Log Capture Strategy

Every test must capture:
1. Command executed
2. stdout
3. stderr  
4. Exit code
5. Timestamp

Format:
```
=== TEST: [name] ===
TIME: $(date)
CMD: [command]
STDOUT:
[output]
STDERR:
[errors]
EXIT: [code]
===
```

---

## Telegram Test Protocol

1. Set env vars
2. Start daemon in background  
3. Send message from Telegram app
4. Capture daemon response
5. Kill daemon
6. Compare sent vs received

---

## Evidence Storage

All evidence saved to:
```
device-test-results/
├── run-YYYY-MM-DD-HHMMSS/
│   ├── summary.txt
│   ├── binary (copy of daemon)
│   ├── build.log
│   ├── startup.log
│   ├── telegram.log
│   ├── memory.log
│   ├── ethics.log
│   └── screenshots/
```

---

## Success Criteria

| Metric | Target |
|--------|--------|
| Startup crash rate | 0% |
| Module init failures | 0% |
| Telegram response time | < 30s |
| Ethics block accuracy | 100% |
| Memory tier load | 4/4 |

---

## Team Responsibilities

| Team | Focus Area |
|------|-----------|
| DevOps | Build verification, CI/CD |
| AI/ML | Inference, model loading |
| Architecture | Module interaction |
| Ethics | Iron Laws enforcement |
| Security | Binary hardening |
| QA | Full E2E testing |

