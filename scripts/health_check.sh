#!/data/data/com.termux/files/usr/bin/bash
# ─────────────────────────────────────────────────────────────────────────────
# AURA Health Check Script
# ─────────────────────────────────────────────────────────────────────────────
# Simple JSON health status for the AURA daemon.
# Run manually or via cron/termux-boot for monitoring.
#
# Usage: ./health_check.sh
# Output: JSON to stdout
#
# This replaces the over-engineered Prometheus/Grafana/Alertmanager stack
# with a single lightweight script appropriate for a personal AGI app.
# ─────────────────────────────────────────────────────────────────────────────

set -euo pipefail

AURA_DATA_DIR="${AURA_DATA_DIR:-/data/local/tmp/aura}"
LOG_FILE="${AURA_DATA_DIR}/logs/aura.log"
DB_FILE="${AURA_DATA_DIR}/aura.db"
PID_FILE="${AURA_DATA_DIR}/aura-daemon.pid"

# ── Check daemon process ──────────────────────────────────────────────────
daemon_running="false"
daemon_pid=""
if [ -f "$PID_FILE" ]; then
    daemon_pid=$(cat "$PID_FILE" 2>/dev/null || echo "")
    if [ -n "$daemon_pid" ] && kill -0 "$daemon_pid" 2>/dev/null; then
        daemon_running="true"
    fi
fi

# ── Check database ────────────────────────────────────────────────────────
db_exists="false"
db_size_bytes=0
if [ -f "$DB_FILE" ]; then
    db_exists="true"
    db_size_bytes=$(stat -c%s "$DB_FILE" 2>/dev/null || echo 0)
fi

# ── Check disk space ──────────────────────────────────────────────────────
disk_free_mb=$(df -m "$AURA_DATA_DIR" 2>/dev/null | awk 'NR==2 {print $4}' || echo 0)

# ── Check memory ──────────────────────────────────────────────────────────
mem_available_mb=$(grep MemAvailable /proc/meminfo 2>/dev/null | awk '{print int($2/1024)}' || echo 0)

# ── Check battery ─────────────────────────────────────────────────────────
battery_level=$(cat /sys/class/power_supply/battery/capacity 2>/dev/null || echo -1)
battery_status=$(cat /sys/class/power_supply/battery/status 2>/dev/null || echo "Unknown")

# ── Check log file ────────────────────────────────────────────────────────
log_lines=0
last_error=""
if [ -f "$LOG_FILE" ]; then
    log_lines=$(wc -l < "$LOG_FILE" 2>/dev/null || echo 0)
    last_error=$(grep -i "ERROR\|FATAL\|PANIC" "$LOG_FILE" 2>/dev/null | tail -1 || echo "")
fi

# ── Determine overall health ──────────────────────────────────────────────
health="healthy"
issues="[]"
issue_list=""

if [ "$daemon_running" = "false" ]; then
    health="unhealthy"
    issue_list="${issue_list}\"daemon not running\","
fi

if [ "$disk_free_mb" -lt 100 ] 2>/dev/null; then
    health="degraded"
    issue_list="${issue_list}\"disk space low (${disk_free_mb}MB)\","
fi

if [ "$mem_available_mb" -lt 200 ] 2>/dev/null; then
    health="degraded"
    issue_list="${issue_list}\"memory low (${mem_available_mb}MB)\","
fi

if [ "$battery_level" -ge 0 ] && [ "$battery_level" -lt 15 ] 2>/dev/null; then
    health="degraded"
    issue_list="${issue_list}\"battery critical (${battery_level}%)\","
fi

# Remove trailing comma and wrap in array
if [ -n "$issue_list" ]; then
    issues="[${issue_list%,}]"
fi

# ── Output JSON ───────────────────────────────────────────────────────────
cat <<EOF
{
  "timestamp": "$(date -u +%Y-%m-%dT%H:%M:%SZ)",
  "health": "${health}",
  "daemon": {
    "running": ${daemon_running},
    "pid": "${daemon_pid}"
  },
  "database": {
    "exists": ${db_exists},
    "size_bytes": ${db_size_bytes}
  },
  "resources": {
    "disk_free_mb": ${disk_free_mb},
    "memory_available_mb": ${mem_available_mb},
    "battery_level": ${battery_level},
    "battery_status": "${battery_status}"
  },
  "logs": {
    "total_lines": ${log_lines},
    "last_error": "${last_error}"
  },
  "issues": ${issues}
}
EOF
