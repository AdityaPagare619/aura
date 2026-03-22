# Rollback Procedure

**Document**: `docs/release/ROLLBACK.md`  
**Version**: 4.0.0-stable  
**Date**: 2026-03-22  
**Status**: ACTIVE  
**Owner**: DevOps Release Charter

---

## When to Rollback

Rollback AURA when:
- Daemon crashes immediately on startup (SIGSEGV, panic)
- Boot stages fail and error cannot be resolved
- Telegram connection fails and persists after troubleshooting
- System becomes unresponsive or consumes excessive memory

---

## Rollback Procedure (Termux)

### Step 1: Stop the Daemon

```bash
# Stop any running aura processes
pkill -f aura-daemon
pkill -f aura-neocortex

# Verify processes stopped
pgrep -f aura || echo "No aura processes running"
```

### Step 2: Backup Broken Binary

```bash
# Create backup of current (broken) version
mkdir -p ~/.aura/backups
cp ~/bin/aura-daemon ~/.aura/backups/aura-daemon.broken-$(date +%Y%m%d-%H%M%S)
```

### Step 3: Restore Previous Version

```bash
# List available backups
ls -la ~/.aura/backups/

# Restore from backup (replace with actual filename)
cp ~/.aura/backups/aura-daemon.backup ~/bin/aura-daemon

# Make executable
chmod +x ~/bin/aura-daemon
```

### Step 4: Verify Restoration

```bash
# Check version matches expected
aura-daemon --version

# Start daemon
aura-daemon &

# Check boot logs
tail -f ~/.aura/logs/boot.log
```

### Step 5: Report Incident

```bash
# Collect diagnostic information
cat ~/.aura/logs/boot.log > ~/aura-crash-report-$(date +%Y%m%d).txt

# Report to: https://github.com/adityasnghvi/aura/issues
# Include: boot.log, device info, steps to reproduce
```

---

## Pre-Installation Backup (Recommended)

Before any update, run:

```bash
#!/bin/bash
# backup-aura.sh

BACKUP_DIR=~/.aura/backups
mkdir -p "$BACKUP_DIR"

# Backup daemon
cp ~/bin/aura-daemon "$BACKUP_DIR/aura-daemon.backup"

# Backup config
cp ~/.aura/config.toml "$BACKUP_DIR/config.toml.backup" 2>/dev/null

# Backup checkpoints
tar -czf "$BACKUP_DIR/checkpoints.backup.tar.gz" ~/.aura/checkpoints/

echo "Backup complete: $BACKUP_DIR"
ls -la "$BACKUP_DIR"
```

---

## Automatic Rollback (if install script supports)

```bash
# If using install.sh with rollback support
./install.sh --rollback

# Or explicit version rollback
./install.sh --version 3.5.2
```

---

## Version Verification Checklist

Before updating, verify:

- [ ] Backup created of current version
- [ ] Config backed up
- [ ] Checkpoints backed up
- [ ] Rollback procedure tested on non-production device
- [ ] Release notes reviewed for breaking changes

---

## Related Documents

| Document | Purpose |
|----------|---------|
| `docs/build/CONTRACT.md` | Platform contract |
| `docs/incidents/POSTMORTEM-TEMPLATE.md` | Incident review template |
| `docs/build/FAILURE_TAXONOMY.md` | Failure classification |

---

**END OF DOCUMENT**
