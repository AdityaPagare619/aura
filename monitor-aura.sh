#!/usr/bin/env bash

set -e

# Termux detection
if [ -d "/data/data/com.termux/files/usr" ]; then
    export PATH="/data/data/com.termux/files/usr/bin:$PATH"
    IS_TERMUX=1
else
    IS_TERMUX=0
fi

DAEMON_LOG="${AURA_LOG_FILE:-$HOME_DIR/.local/share/aura/logs/daemon.log}"
PID_FILE="${AURA_PID_FILE:-$HOME_DIR/.local/share/aura/aura-daemon.pid}"
LLAMA_PORT=8080
LLAMA_HOST="localhost"
HEALTH_ENDPOINT="http://${LLAMA_HOST}:${LLAMA_PORT}/health"
MODEL_ENDPOINT="http://${LLAMA_HOST}:${LLAMA_PORT}/v1/models"

MEMORY_THRESHOLD_MB=8000
CPU_THRESHOLD_PERCENT=90
RESPONSE_TIMEOUT=5

MODE="detailed"
OUTPUT_JSON=false
WATCH_INTERVAL=5
ALERT_MODE=false

show_usage() {
    cat << EOF
Usage: $(basename "$0") [OPTIONS]

AURA Daemon Health Monitoring Script

OPTIONS:
    --quick           Quick check: only process status
    --detailed        Full health report (default)
    --watch           Continuous monitoring (like top)
    --alert           Check thresholds and alert if exceeded
    --json            Output in JSON format
    -i, --interval    Watch interval in seconds (default: 5)
    -h, --help        Show this help message

EXAMPLES:
    $(basename "$0") --quick
    $(basename "$0") --detailed
    $(basename "$0") --watch --interval 10
    $(basename "$0") --alert --json

EOF
}

parse_args() {
    while [[ $# -gt 0 ]]; do
        case $1 in
            --quick)
                MODE="quick"
                shift
                ;;
            --detailed)
                MODE="detailed"
                shift
                ;;
            --watch)
                MODE="watch"
                shift
                ;;
            --alert)
                ALERT_MODE=true
                MODE="detailed"
                shift
                ;;
            --json)
                OUTPUT_JSON=true
                shift
                ;;
            -i|--interval)
                WATCH_INTERVAL="$2"
                shift 2
                ;;
            -h|--help)
                show_usage
                exit 0
                ;;
            *)
                echo "Unknown option: $1"
                show_usage
                exit 1
                ;;
        esac
    done
}

check_process() {
    local process_name="$1"
    local pid=""
    
    if [[ "$process_name" == "aura-daemon" ]]; then
        if [[ -f "$PID_FILE" ]]; then
            pid=$(cat "$PID_FILE" 2>/dev/null)
            if [[ -n "$pid" ]] && kill -0 "$pid" 2>/dev/null; then
                echo "$pid"
                return 0
            fi
        fi
        pid=$(pgrep -f "aura-daemon" 2>/dev/null | head -1)
    elif [[ "$process_name" == "llama-server" ]]; then
        pid=$(pgrep -f "llama-server" 2>/dev/null | head -1)
    fi
    
    if [[ -n "$pid" ]] && kill -0 "$pid" 2>/dev/null; then
        echo "$pid"
        return 0
    fi
    echo ""
    return 1
}

get_process_stats() {
    local pid="$1"
    if [[ -z "$pid" ]] || ! kill -0 "$pid" 2>/dev/null; then
        echo "0|0|0"
        return
    fi
    
    local stat_file="/proc/$pid/stat"
    if [[ -f "$stat_file" ]]; then
        local stat=$(cat "$stat_file" 2>/dev/null)
        local utime=$(echo "$stat" | awk '{print $14}')
        local stime=$(echo "$stat" | awk '{print $15}')
        local clock_ticks=$(getconf CLK_TCK)
        
        local total_time=$((utime + stime))
        local cpu_time=$((total_time / clock_ticks))
        
        local start_time=$(echo "$stat" | awk '{print $22}')
        local uptime=$(cat /proc/uptime 2>/dev/null | awk '{print $1}')
        local process_start=$((start_time / clock_ticks))
        local process_age=$(echo "$uptime $process_start" | awk '{print $1 - $2}')
        
        local cpu_percent=0
        if [[ $process_age -gt 0 ]]; then
            cpu_percent=$(echo "$cpu_time $process_age" | awk '{printf "%.1f", ($1 / $2) * 100}')
            cpu_percent=$(echo "$cpu_percent" | awk '{if($1>100) print 100; else print $1}')
        fi
    fi
    
    local rss_kb=$(ps -o rss= -p "$pid" 2>/dev/null || echo "0")
    local rss_mb=$((rss_kb / 1024))
    
    echo "${cpu_percent}|${rss_mb}|${pid}"
}

check_port() {
    local host="$1"
    local port="$2"
    local timeout="${3:-2}"
    
    if timeout "$timeout" bash -c "echo >/dev/tcp/$host/$port" 2>/dev/null; then
        return 0
    fi
    return 1
}

get_response_time() {
    local url="$1"
    local timeout="${2:-5}"
    
    local start_time=$(date +%s%N)
    if curl -s --max-time "$timeout" "$url" > /dev/null 2>&1; then
        local end_time=$(date +%s%N)
        local duration=$(( (end_time - start_time) / 1000000 ))
        echo "$duration"
        return 0
    fi
    echo "-1"
    return 1
}

get_health_status() {
    local response=$(curl -s --max-time "$RESPONSE_TIMEOUT" "$HEALTH_ENDPOINT" 2>/dev/null || echo "")
    if [[ -n "$response" ]]; then
        echo "$response"
        return 0
    fi
    echo ""
    return 1
}

get_model_status() {
    local response=$(curl -s --max-time "$RESPONSE_TIMEOUT" "$MODEL_ENDPOINT" 2>/dev/null || echo "")
    if [[ -n "$response" ]]; then
        echo "$response" | grep -o '"id":"[^"]*"' | head -1 | cut -d'"' -f4
        return 0
    fi
    echo ""
    return 1
}

get_log_errors() {
    local lines="${1:-20}"
    if [[ -f "$DAEMON_LOG" ]]; then
        tail -n "$lines" "$DAEMON_LOG" 2>/dev/null | grep -iE "error|failed|exception|critical" | tail -n 10
    else
        echo "Log file not found: $DAEMON_LOG"
    fi
}

check_daemon() {
    local pid=$(check_process "aura-daemon")
    if [[ -n "$pid" ]]; then
        echo "running|$pid"
        return 0
    fi
    echo "stopped|"
    return 1
}

check_llama_server() {
    local pid=$(check_process "llama-server")
    if [[ -n "$pid" ]]; then
        echo "running|$pid"
        return 0
    fi
    echo "stopped|"
    return 1
}

check_port_status() {
    if check_port "$LLAMA_HOST" "$LLAMA_PORT" 2; then
        echo "open"
        return 0
    fi
    echo "closed"
    return 1
}

run_quick_check() {
    local daemon_status=$(check_daemon)
    local llama_status=$(check_llama_server)
    local port_status=$(check_port_status)
    
    if [[ "$OUTPUT_JSON" == "true" ]]; then
        cat << EOF
{
  "timestamp": "$(date -Iseconds)",
  "mode": "quick",
  "daemon": {"status": "${daemon_status%%|*}"},
  "llama_server": {"status": "${llama_status%%|*}"},
  "port_8080": "${port_status}"
}
EOF
    else
        echo "=== AURA Quick Health Check ==="
        echo "Daemon:      ${daemon_status%%|*}"
        echo "Llama-Server: ${llama_status%%|*}"
        echo "Port 8080:   ${port_status}"
        echo "================================"
    fi
}

run_detailed_check() {
    local daemon_info=$(check_daemon)
    local daemon_status="${daemon_info%%|*}"
    local daemon_pid="${daemon_info##*|}"
    
    local llama_info=$(check_llama_server)
    local llama_status="${llama_info%%|*}"
    local llama_pid="${llama_info##*|}"
    
    local port_status=$(check_port_status)
    
    local daemon_stats=$(get_process_stats "$daemon_pid")
    local daemon_cpu="${daemon_stats%%|*}"
    daemon_stats="${daemon_stats##*|}"
    local daemon_rss="${daemon_stats%%|*}"
    
    local llama_stats=$(get_process_stats "$llama_pid")
    local llama_cpu="${llama_stats%%|*}"
    llama_stats="${llama_stats##*|}"
    local llama_rss="${llama_stats%%|*}"
    
    local response_time=-1
    local health_response=""
    local model_name=""
    
    if [[ "$port_status" == "open" ]]; then
        response_time=$(get_response_time "$HEALTH_ENDPOINT" "$RESPONSE_TIMEOUT")
        health_response=$(get_health_status)
        model_name=$(get_model_status)
    fi
    
    local log_errors=$(get_log_errors 50)
    
    if [[ "$OUTPUT_JSON" == "true" ]]; then
        local health_json=$(cat << EOF
{
  "timestamp": "$(date -Iseconds)",
  "mode": "detailed",
  "daemon": {
    "status": "$daemon_status",
    "pid": "$daemon_pid",
    "cpu_percent": "$daemon_cpu",
    "memory_rss_mb": "$daemon_rss"
  },
  "llama_server": {
    "status": "$llama_status",
    "pid": "$llama_pid",
    "cpu_percent": "$llama_cpu",
    "memory_rss_mb": "$llama_rss"
  },
  "port_8080": "$port_status",
  "response_time_ms": "$response_time",
  "health_endpoint": "$health_response",
  "model_loaded": "$model_name",
  "recent_errors": $(echo "$log_errors" | head -5 | jq -R . | jq -s .)
}
EOF
)
        echo "$health_json"
    else
        echo "=========================================="
        echo "       AURA Detailed Health Report       "
        echo "=========================================="
        echo "Timestamp: $(date '+%Y-%m-%d %H:%M:%S')"
        echo ""
        echo "--- Daemon ---"
        echo "Status:    $daemon_status"
        echo "PID:       $daemon_pid"
        echo "CPU:       ${daemon_cpu}%"
        echo "Memory:    ${daemon_rss} MB"
        echo ""
        echo "--- Llama-Server ---"
        echo "Status:    $llama_status"
        echo "PID:       $llama_pid"
        echo "CPU:       ${llama_cpu}%"
        echo "Memory:    ${llama_rss} MB"
        echo ""
        echo "--- Network ---"
        echo "Port 8080: $port_status"
        echo "Response:  ${response_time}ms"
        echo ""
        echo "--- Model ---"
        echo "Loaded:    ${model_name:-N/A}"
        echo ""
        echo "--- Recent Errors ---"
        if [[ -n "$log_errors" ]]; then
            echo "$log_errors" | head -5
        else
            echo "None"
        fi
        echo "=========================================="
    fi
    
    if [[ "$ALERT_MODE" == "true" ]]; then
        check_thresholds "$daemon_rss" "$daemon_cpu" "$llama_rss" "$llama_cpu" "$response_time"
    fi
}

check_thresholds() {
    local daemon_rss=$1 daemon_cpu=$2 llama_rss=$3 llama_cpu=$4 response_time=$5
    local alerts=()
    
    if [[ $daemon_rss -gt $MEMORY_THRESHOLD_MB ]]; then
        alerts+=("Daemon memory ${daemon_rss}MB exceeds threshold ${MEMORY_THRESHOLD_MB}MB")
    fi
    
    if [[ $llama_rss -gt $MEMORY_THRESHOLD_MB ]]; then
        alerts+=("Llama-server memory ${llama_rss}MB exceeds threshold ${MEMORY_THRESHOLD_MB}MB")
    fi
    
    if [[ $(echo "$daemon_cpu > $CPU_THRESHOLD_PERCENT" | bc -l 2>/dev/null || echo "0") -eq 1 ]]; then
        alerts+=("Daemon CPU ${daemon_cpu}% exceeds threshold ${CPU_THRESHOLD_PERCENT}%")
    fi
    
    if [[ $(echo "$llama_cpu > $CPU_THRESHOLD_PERCENT" | bc -l 2>/dev/null || echo "0") -eq 1 ]]; then
        alerts+=("Llama-server CPU ${llama_cpu}% exceeds threshold ${CPU_THRESHOLD_PERCENT}%")
    fi
    
    if [[ $response_time -gt 5000 ]] || [[ $response_time -lt 0 ]]; then
        alerts+=("Slow/No response from health endpoint: ${response_time}ms")
    fi
    
    if [[ ${#alerts[@]} -gt 0 ]]; then
        echo ""
        echo "!!! ALERTS !!!"
        for alert in "${alerts[@]}"; do
            echo "  - $alert"
        done
    fi
}

run_watch_mode() {
    while true; do
        clear
        run_detailed_check
        echo ""
        echo "Press Ctrl+C to stop..."
        sleep "$WATCH_INTERVAL"
    done
}

main() {
    parse_args "$@"
    
    case "$MODE" in
        quick)
            run_quick_check
            ;;
        detailed)
            run_detailed_check
            ;;
        watch)
            run_watch_mode
            ;;
    esac
}

main "$@"
