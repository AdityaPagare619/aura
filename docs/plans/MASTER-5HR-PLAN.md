# AURA v4 — 5-Hour Production Readiness Plan
**CEO Priority: GET AURA RUNNING ON REAL DEVICE**
**Started:** March 21, 2026
**Goal:** AURA responds to "Hey Aura" on real Android

---

## Phase 0: Current State Verification (15 min)
- [x] Binary built: aura-daemon ✅ (SHA256: 6d649c29...ac919)
- [x] GitHub Release: ✅ https://github.com/.../v4.0.0-f001-validated/aura-daemon
- [x] Termux APK: ✅ uploaded to BrowserStack (bs://da238b12...)
- [x] CI status: ✅ All jobs passing (release SUCCESS)
- [x] F001 fix: ✅ lto="thin" + panic="unwind" in place
- [x] Telegram Bot: ✅ Key: 8764736044:AAEAQTbDmsBuMm6HHW5EWjsdfBE47jOs2BI
- [x] Telegram User ID: ✅ 8407946567
- [x] Mobile: ✅ 7875292693
- [ ] **AURA NEVER RUN ON DEVICE** ← THE REAL PROBLEM

---

## Phase 1: BrowserStack App Automate — Get Binary ON Device (45 min)

### 1.1 Start App Automate Session with Termux APK
**Device:** Samsung Galaxy S24, Android 14
**APK:** bs://da238b12e7756cbe140170866fd118ea22b7cb63 (Termux)

### 1.2 Download aura-daemon ONTO Device
Via ADB shell inside App Automate session:
```bash
cd /data/data/com.termux/files/home
wget https://github.com/AdityaPagare619/aura/releases/download/v4.0.0-f001-validated/aura-daemon -O aura-daemon
chmod +x aura-daemon
```

### 1.3 Test 1: Basic Startup (THE F001 TEST)
```bash
./aura-daemon --version
echo "Exit code: $?"
```
- Exit 0 → F001 FIXED ✅
- Exit 139 → SIGSEGV → F001 STILL EXISTS ❌

### 1.4 Test 2: Help Flag
```bash
./aura-daemon --help
```

### 1.5 Test 3: Build Info
```bash
./aura-daemon build-info
```

---

## Phase 2: Telegram Integration — "Hey Aura" Response (60 min)

### 2.1 Verify Telegram Bot Connection
Bot token: `8764736044:AAEAQTbDmsBuMm6HHW5EWjsdfBE47jOs2BI`
User ID: `8407946567`

### 2.2 Check Bot Status
```bash
curl -s "https://api.telegram.org/bot8764736044:AAEAQTbDmsBuMm6HHW5EWjsdfBE47jOs2BI/getMe"
```

### 2.3 Send Test Message
```bash
curl -s "https://api.telegram.org/bot8764736044:AAEAQTbDmsBuMm6HHW5EWjsdfBE47jOs2BI/sendMessage" \
  -d "chat_id=8407946567" \
  -d "text=🧪 AURA v4 smoke test — are you alive?"
```

### 2.4 Configure aura-daemon Telegram Backend
The daemon needs:
- Telegram Bot API token
- User/chat ID for routing
- Termux environment variables

### 2.5 Run aura-daemon with Telegram Backend
```bash
AURA_TELEGRAM_TOKEN=8764736044:AAEAQTbDmsBuMm6HHW5EWjsdfBE47jOs2BI \
AURA_TELEGRAM_CHAT_ID=8407946567 \
AURA_MOBILE=7875292693 \
./aura-daemon
```

### 2.6 Send "Hey Aura" via Telegram
```bash
curl -s "https://api.telegram.org/bot8764736044:AAEAQTbDmsBuMm6HHW5EWjsdfBE47jOs2BI/sendMessage" \
  -d "chat_id=8407946567" \
  -d "text=Hey Aura"
```

### 2.7 Check AURA Response
Monitor daemon logs for "Hey Aura" → response cycle.

---

## Phase 3: Deep Testing — All Systems Check (60 min)

### 3.1 Architecture Validation
- [ ] 4 memory tiers load correctly
- [ ] Ethics layer initializes
- [ ] Trust tier system active
- [ ] Policy gate enforces deny-by-default

### 3.2 Ethics Validation
- [ ] Iron Law 1: LLM=brain verified
- [ ] Iron Law 2: No Theater AGI verified
- [ ] Iron Law 5: No telemetry confirmed
- [ ] Iron Law 7: Anti-sycophancy active

### 3.3 Security Validation
- [ ] Binary has no external network calls (except Telegram)
- [ ] No hardcoded secrets
- [ ] NX bit enabled
- [ ] PIE enabled

### 3.4 Performance Baseline
- [ ] Startup time < 30 seconds
- [ ] Memory usage < 512 MiB
- [ ] First inference < 60 seconds

---

## Phase 4: Report & Document (30 min)

### 4.1 Test Execution Report
Record ALL results in BrowserStack Test Management (PR-2)

### 4.2 F001 Validation Report
Document SIGSEGV test results with evidence

### 4.3 Telegram Integration Report
Document "Hey Aura" test results

### 4.4 CEO Summary
Send comprehensive status report to user

---

## Phase 5: Final Verification (15 min)

### 5.1 Red Team — Try to Break AURA
- [ ] Send harmful request → verify blocked
- [ ] Send jailbreak attempt → verify blocked
- [ ] Send privacy violation request → verify blocked

### 5.2 Smoke Test Pass Criteria
- [ ] AURA starts without crash ✅
- [ ] AURA responds to Telegram ✅
- [ ] Ethics layer active ✅
- [ ] No SIGSEGV on startup ✅

---

## Device Assignments (NO CONFLICTS)

| Team | Device | Purpose |
|------|--------|---------|
| DevOps | BrowserStack Device 1 | Binary deployment |
| AI/ML | BrowserStack Device 2 | Inference testing |
| Architecture | BrowserStack Device 3 | Layer validation |
| Ethics | BrowserStack Device 4 | Iron Laws check |
| Security | BrowserStack Device 5 | Binary analysis |
| QA | BrowserStack Device 6 | Smoke suite |

---

## Telegram Credentials (SECURED)
- Bot Token: `8764736044:AAEAQTbDmsBuMm6HHW5EWjsdfBE47jOs2BI`
- User Chat ID: `8407946567`
- Mobile: `7875292693`

**NOTE:** Treat as environment variables, never hardcode.

---

## Verification Checkpoints

| Time | Milestone | Status |
|------|-----------|--------|
| T+15min | Binary on device | ⏳ |
| T+30min | ./aura-daemon --version works | ⏳ |
| T+45min | F001 test complete | ⏳ |
| T+60min | Telegram connected | ⏳ |
| T+90min | "Hey Aura" works | ⏳ |
| T+3hr | All systems verified | ⏳ |
| T+5hr | AURA PRODUCTION READY | ⏳ |

