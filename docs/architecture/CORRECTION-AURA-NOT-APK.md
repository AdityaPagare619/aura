# CORRECTION: AURA Deployment Model

## Clarification (March 30, 2026)

### What AURA IS:
- **Termux-based application** - runs inside Termux
- **Not an APK** - not installed via Play Store or APK file
- **CLI-driven** - terminal-based installation and operation
- **Local-only** - no cloud, works offline

### Installation Method:
```
USER FLOW (CORRECTED):
1. Install Termux (from F-Droid or GitHub)
2. Clone repo OR download scripts
3. Run ./install.sh
4. Enter Telegram token
5. DONE
```

### Not This:
- ❌ Download APK from GitHub
- ❌ Install from Play Store
- ❌ Standard Android app

---

## Updated Architecture Focus

### Primary Installation: Termux-Based
- Auto-detect Termux
- Install via apt (packages.termux.dev)
- Configure automatically
- Manage services

### Supported Devices: Any that run Termux
- Android 10+ (Termux requirement)
- arm64 architecture
- Sufficient storage for model

---

## Meeting Focus Adjustment

All future design discussions should focus on:
1. Termux integration
2. Script-based installation
3. Service management within Termux
4. No Android APK workflows

---

**Correction noted. Continuing with deep analysis adjusted for Termux-only model.**