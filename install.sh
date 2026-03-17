#!/usr/bin/env bash
# =============================================================================
# AURA v4 — Enterprise Termux Installer
# =============================================================================
# Usage:
#   bash install.sh [OPTIONS]
#
# Options:
#   --channel stable|nightly   Release channel (default: stable)
#   --model <name>             Override model selection (skips auto-detect)
#   --skip-build               Skip Rust build (use pre-built binary)
#   --skip-model               Skip model download
#   --skip-service             Skip termux-services setup
#   --keep-build-tools         Keep Rust toolchain after build (don't purge ~4 GB)
#   --repair <phase>           Re-run a specific phase only
#                              Phases: preflight|packages|rust|source|model|
#                                      build|purge|config|service|verify
#   --dry-run                  Print actions without executing
#   --update                   Update existing installation
#   --no-color                 Disable color output
#   -h, --help                 Show this help
#
# Environment variables:
#   HF_TOKEN     HuggingFace token for authenticated downloads (optional)
#   AURA_REPO    Override git repo URL (default: https://github.com/AdityaPagare619/aura.git)
#
# Installation phases (all interactive steps front-loaded):
#   Phase 0:   Pre-flight (arch, Termux, Android version)
#   Phase 0.5: Space budget display
#   Phase 1:   Hardware profiling + model auto-selection (interactive)
#   Phase 2:   Telegram bot wizard (interactive)
#   Phase 3:   Vault PIN + user name (interactive)
#              ── ALL INTERACTIVE STEPS DONE — unattended install begins ──
#   Phase 4:   Package installation (pkg install)
#   Phase 5:   Rust toolchain
#   Phase 6:   Source acquisition (git clone + submodules)
#   Phase 7:   Model download (resumable, retry-3, progress)
#   Phase 8:   Build (cargo build --features voice)
#   Phase 9:   Purge build tools (~4 GB saved, unless --keep-build-tools)
#   Phase 10:  Config finalization (full config.toml with all sections)
#   Phase 11:  Service setup (termux-services or .bashrc fallback)
#   Phase 12:  Verification + success banner (with wakelock instructions)
# =============================================================================
set -euo pipefail

# =============================================================================
# CONSTANTS
# =============================================================================

AURA_VERSION="4.0.0-alpha.4"
AURA_REPO="${AURA_REPO:-https://github.com/AdityaPagare619/aura.git}"
AURA_STABLE_TAG="v4.0.0-alpha.3"
AURA_NIGHTLY_TAG="main"

# ── Model registry ────────────────────────────────────────────────────────────
# Tier selection: RAM < 4 GB → 1.5b, 4–6 GB → 4b, 6–10 GB → 8b, ≥10 GB → 14b

MODEL_QWEN3_1_5B_NAME="Qwen3-1.7B-Q8_0.gguf"
MODEL_QWEN3_1_5B_URL="https://huggingface.co/Qwen/Qwen3-1.7B-GGUF/resolve/main/Qwen3-1.7B-Q8_0.gguf"
MODEL_QWEN3_1_5B_SHA256="PLACEHOLDER_UPDATE_AT_RELEASE_TIME_1b"
MODEL_QWEN3_1_5B_SIZE_GB=2
MODEL_QWEN3_1_5B_RAM_MIN_GB=3
MODEL_QWEN3_1_5B_LABEL="Qwen3-1.7B Q8_0 (brainstem / <4 GB RAM)"

MODEL_QWEN3_4B_NAME="Qwen3-4B-Q4_K_M.gguf"
MODEL_QWEN3_4B_URL="https://huggingface.co/Qwen/Qwen3-4B-GGUF/resolve/main/Qwen3-4B-Q4_K_M.gguf"
MODEL_QWEN3_4B_SHA256="PLACEHOLDER_UPDATE_AT_RELEASE_TIME_4b"
MODEL_QWEN3_4B_SIZE_GB=3
MODEL_QWEN3_4B_RAM_MIN_GB=4
MODEL_QWEN3_4B_LABEL="Qwen3-4B Q4_K_M (standard / 4–6 GB RAM)"

MODEL_QWEN3_8B_NAME="Qwen3-8B-Q4_K_M.gguf"
MODEL_QWEN3_8B_URL="https://huggingface.co/Qwen/Qwen3-8B-GGUF/resolve/main/Qwen3-8B-Q4_K_M.gguf"
MODEL_QWEN3_8B_SHA256="PLACEHOLDER_UPDATE_AT_RELEASE_TIME_8b"
MODEL_QWEN3_8B_SIZE_GB=5
MODEL_QWEN3_8B_RAM_MIN_GB=6
MODEL_QWEN3_8B_LABEL="Qwen3-8B Q4_K_M (full / 6–10 GB RAM)"

MODEL_QWEN3_14B_NAME="Qwen3-14B-Q4_K_M.gguf"
MODEL_QWEN3_14B_URL="https://huggingface.co/Qwen/Qwen3-14B-GGUF/resolve/main/Qwen3-14B-Q4_K_M.gguf"
MODEL_QWEN3_14B_SHA256="PLACEHOLDER_UPDATE_AT_RELEASE_TIME_14b"
MODEL_QWEN3_14B_SIZE_GB=9
MODEL_QWEN3_14B_RAM_MIN_GB=10
MODEL_QWEN3_14B_LABEL="Qwen3-14B Q4_K_M (extended / ≥10 GB RAM)"

# ── Paths ─────────────────────────────────────────────────────────────────────
if [ -d "/data/data/com.termux" ]; then
    IS_TERMUX=1
    TERMUX_PREFIX="/data/data/com.termux/files/usr"
    HOME_DIR="/data/data/com.termux/files/home"
else
    IS_TERMUX=0
    TERMUX_PREFIX="/usr"
    HOME_DIR="${HOME:-/home/user}"
fi

AURA_HOME="$HOME_DIR/aura"
AURA_CONFIG_DIR="$HOME_DIR/.config/aura"
AURA_DATA_DIR="$HOME_DIR/.local/share/aura"
AURA_MODELS_DIR="$AURA_DATA_DIR/models"
AURA_LOGS_DIR="$AURA_DATA_DIR/logs"
AURA_DB_DIR="$AURA_DATA_DIR/db"
AURA_CONFIG_FILE="$AURA_CONFIG_DIR/config.toml"
AURA_SOCK="@aura_ipc_v4"
AURA_BIN="$TERMUX_PREFIX/bin/aura-daemon"
AURA_NEOCORTEX_BIN="$TERMUX_PREFIX/bin/aura-neocortex"
AURA_SV_DIR="$TERMUX_PREFIX/var/service/aura-daemon"

# Rust toolchain paths — use existing env vars if set, otherwise default
CARGO_HOME="${CARGO_HOME:-$HOME_DIR/.cargo}"
RUSTUP_HOME="${RUSTUP_HOME:-$HOME_DIR/.rustup}"
export CARGO_HOME RUSTUP_HOME

INSTALL_LOG="${TMPDIR:-/tmp}/aura-install-$(date +%Y%m%d-%H%M%S).log"

# ── Collected during interactive phases ──────────────────────────────────────
COLLECTED_BOT_TOKEN=""
COLLECTED_OWNER_ID=""
COLLECTED_USER_NAME="User"
COLLECTED_PIN_HASH=""
COLLECTED_PIN_SALT=""

# =============================================================================
# FLAGS
# =============================================================================

OPT_CHANNEL="stable"
OPT_MODEL=""           # empty = auto-detect in phase 1
OPT_SKIP_BUILD=0
OPT_SKIP_MODEL=0
OPT_SKIP_SERVICE=0
OPT_KEEP_BUILD_TOOLS=0
OPT_REPAIR=""
OPT_DRY_RUN=0
OPT_UPDATE=0
OPT_NO_COLOR=0

# =============================================================================
# COLORS
# =============================================================================

setup_colors() {
    if [ "${NO_COLOR:-}" = "1" ] || [ "$OPT_NO_COLOR" = "1" ] || ! [ -t 1 ]; then
        RED="" GREEN="" YELLOW="" BLUE="" CYAN="" MAGENTA="" BOLD="" DIM="" RESET=""
    else
        RED="\033[0;31m"
        GREEN="\033[0;32m"
        YELLOW="\033[0;33m"
        BLUE="\033[0;34m"
        CYAN="\033[0;36m"
        MAGENTA="\033[0;35m"
        BOLD="\033[1m"
        DIM="\033[2m"
        RESET="\033[0m"
    fi
}

# =============================================================================
# LOGGING
# =============================================================================

log_header() {
    echo ""
    echo -e "${BOLD}${BLUE}━━━ $1 ━━━${RESET}"
}

log_step() {
    echo -e "${GREEN}  ✓${RESET} $1"
}

log_info() {
    echo -e "${CYAN}  →${RESET} $1"
}

log_input() {
    echo -e "${MAGENTA}  ◆${RESET} $1"
}

warn() {
    echo -e "${YELLOW}  ⚠${RESET}  $1" >&2
}

die() {
    echo "" >&2
    echo -e "${RED}${BOLD}  ✗ ERROR:${RESET} $1" >&2
    local i=2
    while [ $i -le $# ]; do
        eval "local _hint=\${$i}"
        echo -e "${DIM}    Fix: ${_hint}${RESET}" >&2
        i=$(( i + 1 ))
    done
    if [ -n "${INSTALL_LOG:-}" ] && [ -f "${INSTALL_LOG}" ]; then
        echo "" >&2
        echo -e "${DIM}  Full install log: ${INSTALL_LOG}${RESET}" >&2
        echo -e "${DIM}  Share this file when reporting issues.${RESET}" >&2
    fi
    echo "" >&2
    exit 1
}

run() {
    if [ "$OPT_DRY_RUN" = "1" ]; then
        echo -e "${DIM}  [dry-run] $*${RESET}"
    else
        "$@"
    fi
}

confirm() {
    local prompt="$1"
    local default="${2:-y}"
    local answer
    # Read from /dev/tty explicitly to avoid conflicts with exec/tee/pipe
    # redirections that may capture stdin. This ensures interactive prompts
    # work correctly even when stdout is piped through tee for logging.
    if [ "$default" = "y" ]; then
        read -r -p "$(echo -e "${YELLOW}  ?${RESET} $prompt [Y/n]: ")" answer < /dev/tty
        answer="${answer:-y}"
    else
        read -r -p "$(echo -e "${YELLOW}  ?${RESET} $prompt [y/N]: ")" answer < /dev/tty
        answer="${answer:-n}"
    fi
    [[ "$answer" =~ ^[Yy]$ ]]
}

# =============================================================================
# ARGUMENT PARSING
# =============================================================================

parse_args() {
    while [ $# -gt 0 ]; do
        case "$1" in
            --channel)         OPT_CHANNEL="${2:-stable}"; shift 2 ;;
            --model)           OPT_MODEL="${2:-}";         shift 2 ;;
            --skip-build)      OPT_SKIP_BUILD=1;           shift ;;
            --skip-model)      OPT_SKIP_MODEL=1;           shift ;;
            --skip-service)    OPT_SKIP_SERVICE=1;         shift ;;
            --keep-build-tools) OPT_KEEP_BUILD_TOOLS=1;   shift ;;
            --repair)          OPT_REPAIR="${2:-}";        shift 2 ;;
            --dry-run)         OPT_DRY_RUN=1;              shift ;;
            --update)          OPT_UPDATE=1;               shift ;;
            --no-color)        OPT_NO_COLOR=1;             shift ;;
            -h|--help)         show_help; exit 0 ;;
            *) die "Unknown option: $1" "Run with --help for usage" ;;
        esac
    done
}

show_help() {
    cat <<'EOF'
AURA v4 — Enterprise Termux Installer

Usage:
  bash install.sh [OPTIONS]

Options:
  --channel stable|nightly     Release channel (default: stable)
  --model qwen3-1.5b|qwen3-4b|qwen3-8b|qwen3-14b
                               Override model selection (default: auto-detect by RAM)
  --skip-build                 Skip Rust build (use pre-built binary if present)
  --skip-model                 Skip model download
  --skip-service               Skip termux-services autostart setup
  --keep-build-tools           Keep Rust toolchain after build (saves ~4 GB if omitted)
  --repair <phase>             Re-run a specific phase:
                               preflight | packages | rust | source | model |
                               build | purge | config | service | verify
  --dry-run                    Print actions without executing
  --update                     Update existing installation
  --no-color                   Disable color output
  -h, --help                   Show this help

Environment variables:
  HF_TOKEN     HuggingFace token for authenticated model downloads
  AURA_REPO    Override git repository URL

Examples:
  # Standard install (auto-detects model from RAM):
  bash install.sh

  # Force 8B model on a well-spec'd device:
  bash install.sh --model qwen3-8b

  # Update existing install:
  bash install.sh --update

  # Re-run only the model download phase:
  bash install.sh --repair model

  # Preview what would happen:
  bash install.sh --dry-run
EOF
}

# =============================================================================
# PHASE 0: PRE-FLIGHT CHECKS
# =============================================================================

phase_preflight() {
    log_header "Phase 0 · Pre-flight Checks"

    # Architecture
    local arch
    arch=$(uname -m)
    case "$arch" in
        aarch64|arm64)
            log_step "Architecture: $arch (supported)" ;;
        armv7l|armv8l)
            die "ARM32 ($arch) is not supported." \
                "AURA v4 requires a 64-bit ARM device (aarch64). Most Android phones since 2015 are ARM64." ;;
        x86_64)
            warn "Running on x86_64 — desktop/CI build, not a Termux device build" ;;
        *)
            die "Unsupported architecture: $arch" "AURA v4 requires aarch64 (ARM64)." ;;
    esac

    # Termux check
    if [ "$IS_TERMUX" = "1" ]; then
        log_step "Termux environment detected"

        if command -v termux-info &>/dev/null; then
            local tv
            tv=$(termux-info 2>/dev/null | grep "Termux Version" | awk '{print $NF}' || echo "unknown")
            log_info "Termux version: $tv"
        fi

        if [ ! -d "$HOME_DIR/storage" ]; then
            warn "Storage permission not granted to Termux."
            log_info "Run 'termux-setup-storage' and grant permission, then re-run this installer."
            if confirm "Grant storage access now?"; then
                run termux-setup-storage || warn "Could not auto-grant. Please do it manually."
                # Poll for storage directory with timeout — Android can take up to
                # 30 seconds to create the symlinks after granting permission.
                local _wait_elapsed=0
                local _wait_timeout=30
                while [ ! -d "$HOME_DIR/storage" ] && [ $_wait_elapsed -lt $_wait_timeout ]; do
                    printf "\r  Waiting for storage symlinks… (%d/%ds)" "$_wait_elapsed" "$_wait_timeout"
                    sleep 1
                    _wait_elapsed=$((_wait_elapsed + 1))
                done
                echo ""
                if [ -d "$HOME_DIR/storage" ]; then
                    log_step "Storage access granted"
                else
                    warn "Storage directory not created after ${_wait_timeout}s."
                    log_info "AURA can still install without storage access, but some features"
                    log_info "(file sharing, photo analysis) may be limited."
                    log_info "You can grant access later with: termux-setup-storage"
                fi
            fi
        else
            log_step "Storage access granted"
        fi

        # Android API level check
        local api_level=""
        if command -v getprop &>/dev/null; then
            api_level=$(getprop ro.build.version.sdk 2>/dev/null || echo "")
        fi
        if [ -n "$api_level" ] && [ "$api_level" -lt 26 ]; then
            die "Android API level $api_level detected. AURA v4 requires API 26 (Android 8.0) or higher." \
                "Please update your Android version."
        elif [ -n "$api_level" ]; then
            log_step "Android API level: $api_level (≥26 required — OK)"
        fi
    else
        log_info "Not running in Termux — assuming desktop/CI environment"
    fi

    # Network
    if curl --silent --max-time 10 "https://huggingface.co" > /dev/null 2>&1; then
        log_step "Network: HuggingFace reachable"
    else
        warn "Cannot reach huggingface.co — model download will likely fail."
        warn "Check your network connection or VPN settings."
    fi

    log_step "Pre-flight checks passed"
}

# =============================================================================
# PHASE 0.5: SPACE BUDGET DISPLAY
# =============================================================================

phase_space_budget() {
    log_header "Phase 0.5 · Space Budget"

    local total_kb=0
    local total_gb=0
    if [ -f /proc/meminfo ]; then
        total_kb=$(grep MemTotal /proc/meminfo | awk '{print $2}')
        total_gb=$(( total_kb / 1024 / 1024 ))
    fi

    local avail_kb=0
    mkdir -p "$HOME_DIR" 2>/dev/null || true
    avail_kb=$(df -k "$HOME_DIR" 2>/dev/null | awk 'NR==2 {print $4}' || echo "0")
    local avail_gb=$(( avail_kb / 1024 / 1024 ))

    echo ""
    echo -e "${BOLD}  Device Summary${RESET}"
    printf "  %-22s %s\n" "RAM detected:"     "${total_gb} GB"
    printf "  %-22s %s\n" "Storage available:" "${avail_gb} GB"
    echo ""
    echo -e "${BOLD}  Storage Requirements by Model${RESET}"
    echo -e "  ${DIM}──────────────────────────────────────────────────────────────────${RESET}"
    printf "  ${BOLD}%-18s %-12s %-12s %-22s${RESET}\n" "Model" "Model Size" "Min RAM" "Recommended For"
    echo -e "  ${DIM}──────────────────────────────────────────────────────────────────${RESET}"
    printf "  %-18s %-12s %-12s %-22s\n" "Qwen3-1.7B Q8"  "~2 GB"   "3 GB"    "Very low RAM devices"
    printf "  %-18s %-12s %-12s %-22s\n" "Qwen3-4B Q4_K_M" "~3 GB"   "4 GB"    "Budget / mid-range"
    printf "  %-18s %-12s %-12s %-22s\n" "Qwen3-8B Q4_K_M" "~5 GB"   "6 GB"    "Flagship phones"
    printf "  %-18s %-12s %-12s %-22s\n" "Qwen3-14B Q4_K_M" "~9 GB"  "10 GB"   "Tablets / high-RAM"
    echo -e "  ${DIM}──────────────────────────────────────────────────────────────────${RESET}"
    echo ""
    echo -e "  ${DIM}Additional: ~4 GB for Rust toolchain + build (purged after build unless --keep-build-tools)${RESET}"
    echo -e "  ${DIM}Total install: model size + ~0.5 GB binaries/data${RESET}"
    echo ""

    # Warn on low storage
    if [ "$avail_gb" -lt 8 ]; then
        warn "Low storage: ${avail_gb} GB free. Consider freeing space before continuing."
    fi
}

# =============================================================================
# PHASE 1: HARDWARE PROFILING + MODEL SELECTION
# =============================================================================

phase_hardware_and_model() {
    log_header "Phase 1 · Hardware Profiling + Model Selection"

    local total_kb=0
    local total_gb=0
    if [ -f /proc/meminfo ]; then
        total_kb=$(grep MemTotal /proc/meminfo | awk '{print $2}')
        total_gb=$(( total_kb / 1024 / 1024 ))
    fi

    local cpu_count
    cpu_count=$(nproc 2>/dev/null || echo "4")

    local soc_model=""
    if command -v getprop &>/dev/null; then
        soc_model=$(getprop ro.hardware 2>/dev/null || getprop ro.product.board 2>/dev/null || echo "unknown")
    fi

    log_step "RAM:       ${total_gb} GB"
    log_step "CPU cores: ${cpu_count}"
    [ -n "$soc_model" ] && log_step "SoC:       $soc_model"

    # Auto-select model if not overridden by --model flag
    if [ -z "$OPT_MODEL" ]; then
        if [ "$total_gb" -lt 4 ]; then
            OPT_MODEL="qwen3-1.5b"
        elif [ "$total_gb" -lt 6 ]; then
            OPT_MODEL="qwen3-4b"
        elif [ "$total_gb" -lt 10 ]; then
            OPT_MODEL="qwen3-8b"
        else
            OPT_MODEL="qwen3-14b"
        fi
        log_info "Auto-selected model: ${OPT_MODEL} (based on ${total_gb} GB RAM)"
    else
        log_info "Model override: ${OPT_MODEL} (from --model flag)"
    fi

    # Confirm with user
    local model_label
    case "$OPT_MODEL" in
        qwen3-1.5b) model_label="$MODEL_QWEN3_1_5B_LABEL" ;;
        qwen3-4b)   model_label="$MODEL_QWEN3_4B_LABEL"   ;;
        qwen3-14b)  model_label="$MODEL_QWEN3_14B_LABEL"  ;;
        *)          model_label="$MODEL_QWEN3_8B_LABEL"; OPT_MODEL="qwen3-8b" ;;
    esac

    echo ""
    echo -e "${BOLD}  Selected model:${RESET} ${CYAN}${model_label}${RESET}"
    echo ""
    echo -e "  Other options:"
    [ "$OPT_MODEL" != "qwen3-1.5b" ] && echo "    1) qwen3-1.5b — $MODEL_QWEN3_1_5B_LABEL"
    [ "$OPT_MODEL" != "qwen3-4b"   ] && echo "    2) qwen3-4b   — $MODEL_QWEN3_4B_LABEL"
    [ "$OPT_MODEL" != "qwen3-8b"   ] && echo "    3) qwen3-8b   — $MODEL_QWEN3_8B_LABEL"
    [ "$OPT_MODEL" != "qwen3-14b"  ] && echo "    4) qwen3-14b  — $MODEL_QWEN3_14B_LABEL"
    echo ""

    if [ "$OPT_DRY_RUN" != "1" ]; then
        local choice
        read -r -p "$(echo -e "${YELLOW}  ?${RESET} Press Enter to confirm ${OPT_MODEL}, or type 1/2/3/4 to change: ")" choice < /dev/tty
        case "$choice" in
            1) OPT_MODEL="qwen3-1.5b" ;;
            2) OPT_MODEL="qwen3-4b"   ;;
            3) OPT_MODEL="qwen3-8b"   ;;
            4) OPT_MODEL="qwen3-14b"  ;;
            *) : ;; # keep auto-selected
        esac
    fi

    log_step "Model confirmed: ${OPT_MODEL}"
}

# =============================================================================
# PHASE 2: TELEGRAM BOT WIZARD
# =============================================================================

phase_telegram_wizard() {
    log_header "Phase 2 · Telegram Bot Setup"

    # Skip if already configured in an existing config and not --update
    if [ -f "$AURA_CONFIG_FILE" ] && [ "$OPT_UPDATE" != "1" ]; then
        if grep -q 'bot_token = ".\+' "$AURA_CONFIG_FILE" 2>/dev/null; then
            log_info "Telegram already configured in $AURA_CONFIG_FILE — skipping wizard"
            # Read existing values for later use
            COLLECTED_BOT_TOKEN=$(grep 'bot_token' "$AURA_CONFIG_FILE" 2>/dev/null | head -1 | sed 's/.*= "\(.*\)"/\1/' || echo "")
            COLLECTED_OWNER_ID=$(grep 'owner_user_id' "$AURA_CONFIG_FILE" 2>/dev/null | head -1 | sed 's/.*= \([0-9]*\).*/\1/' || echo "0")
            return
        fi
    fi

    if [ "$OPT_DRY_RUN" = "1" ]; then
        log_info "[dry-run] Would run Telegram bot wizard"
        COLLECTED_BOT_TOKEN="DRY_RUN_TOKEN"
        COLLECTED_OWNER_ID="0"
        return
    fi

    echo ""
    echo -e "${BOLD}  AURA communicates exclusively through Telegram.${RESET}"
    echo -e "  You need a Telegram bot token. Here's how to get one:"
    echo ""
    echo -e "  ${BOLD}Step 1:${RESET} Open Telegram and search for ${CYAN}@BotFather${RESET}"
    echo -e "  ${BOLD}Step 2:${RESET} Send ${CYAN}/newbot${RESET} and follow the prompts"
    echo -e "  ${BOLD}Step 3:${RESET} BotFather will give you a token like:"
    echo -e "          ${DIM}1234567890:ABCDefGhIJKlmNoPQRsTUVwxyZ${RESET}"
    echo -e "  ${BOLD}Step 4:${RESET} Paste the token here"
    echo ""

    # Token input + validation loop
    local token=""
    while true; do
        read -r -p "$(echo -e "${YELLOW}  ?${RESET} Paste your Telegram bot token: ")" token < /dev/tty
        token="${token//[[:space:]]/}"   # strip whitespace

        # Format: digits:35-char alphanum
        if [[ ! "$token" =~ ^[0-9]{8,12}:[A-Za-z0-9_-]{35}$ ]]; then
            warn "Token format looks wrong. Expected: 1234567890:ABCDefGhIJKlmNoPQRsTUVwxyZ"
            if ! confirm "Try anyway?" "n"; then
                continue
            fi
        fi

        # Verify token live against Telegram API
        log_info "Verifying token against api.telegram.org ..."
        local api_resp
        api_resp=$(curl --silent --max-time 15 \
            "https://api.telegram.org/bot${token}/getMe" 2>/dev/null || echo "")

        if echo "$api_resp" | grep -q '"ok":true'; then
            local bot_name
            bot_name=$(echo "$api_resp" | grep -o '"username":"[^"]*"' | cut -d'"' -f4 || echo "unknown")
            log_step "Bot verified: @${bot_name}"
            break
        else
            local tg_err
            tg_err=$(echo "$api_resp" | grep -o '"description":"[^"]*"' | cut -d'"' -f4 || echo "no response")
            warn "Token verification failed: $tg_err"
            if ! confirm "Re-enter token?" "y"; then
                die "Cannot proceed without a valid Telegram bot token." \
                    "Create a bot via @BotFather and re-run the installer."
            fi
        fi
    done
    COLLECTED_BOT_TOKEN="$token"

    echo ""
    echo -e "  ${BOLD}Step 5:${RESET} Find your Telegram User ID:"
    echo -e "          Open Telegram → search ${CYAN}@userinfobot${RESET} → send ${CYAN}/start${RESET}"
    echo -e "          It will reply with your numeric ID (e.g. ${DIM}987654321${RESET})"
    echo ""

    local owner_id=""
    while true; do
        read -r -p "$(echo -e "${YELLOW}  ?${RESET} Enter your Telegram User ID (numbers only): ")" owner_id < /dev/tty
        owner_id="${owner_id//[[:space:]]/}"
        if [[ "$owner_id" =~ ^[0-9]{5,12}$ ]]; then
            break
        else
            warn "User ID must be a number (e.g. 987654321). Try again."
        fi
    done
    COLLECTED_OWNER_ID="$owner_id"

    log_step "Telegram bot token captured and verified"
    log_step "Owner Telegram ID: $COLLECTED_OWNER_ID"
}

# =============================================================================
# PHASE 3: VAULT PIN + USER NAME
# =============================================================================

phase_vault_setup() {
    log_header "Phase 3 · Vault & Identity Setup"

    # Skip if already configured and not --update
    if [ -f "$AURA_CONFIG_FILE" ] && [ "$OPT_UPDATE" != "1" ]; then
        if grep -q 'pin_hash = ".\+' "$AURA_CONFIG_FILE" 2>/dev/null; then
            log_info "Vault PIN already configured — skipping"
            COLLECTED_USER_NAME=$(grep 'user_name' "$AURA_CONFIG_FILE" 2>/dev/null | head -1 | sed 's/.*= "\(.*\)"/\1/' || echo "User")
            return
        fi
    fi

    if [ "$OPT_DRY_RUN" = "1" ]; then
        log_info "[dry-run] Would prompt for user name and vault PIN"
        COLLECTED_USER_NAME="User"
        COLLECTED_PIN_HASH="dry_run_hash"
        COLLECTED_PIN_SALT="dry_run_salt"
        return
    fi

    echo ""
    echo -e "${BOLD}  AURA Identity Setup${RESET}"
    echo -e "${DIM}  AURA will use your name to personalize responses.${RESET}"
    echo ""

    read -r -p "$(echo -e "${YELLOW}  ?${RESET} Your name (how AURA addresses you) [User]: ")" user_input < /dev/tty
    COLLECTED_USER_NAME="${user_input:-User}"

    echo ""
    echo -e "${BOLD}  Vault PIN Setup${RESET}"
    echo -e "${DIM}  The vault PIN gates sensitive operations. Minimum 4 characters.${RESET}"
    echo -e "${DIM}  AURA stores a salted SHA-256 hash during install, upgraded to${RESET}"
    echo -e "${DIM}  Argon2id automatically on first daemon start.${RESET}"
    echo ""

    local pin1 pin2
    while true; do
        read -r -s -p "$(echo -e "${YELLOW}  ?${RESET} Set vault PIN (min 4 chars): ")" pin1 < /dev/tty
        echo ""
        if [ ${#pin1} -lt 4 ]; then
            warn "PIN must be at least 4 characters. Try again."
            continue
        fi
        read -r -s -p "$(echo -e "${YELLOW}  ?${RESET} Confirm vault PIN: ")" pin2 < /dev/tty
        echo ""
        if [ "$pin1" = "$pin2" ]; then
            break
        else
            warn "PINs do not match. Try again."
        fi
    done

    # Salted SHA-256 (temporary — daemon upgrades to Argon2id on first start)
    COLLECTED_PIN_SALT=$(head -c 16 /dev/urandom | od -A n -t x1 | tr -d ' \n')
    COLLECTED_PIN_HASH=$(echo -n "${COLLECTED_PIN_SALT}${pin1}" | sha256sum | cut -d' ' -f1)

    log_step "Vault PIN set (sha256 hash stored; Argon2id upgrade on first start)"
    log_step "User name: $COLLECTED_USER_NAME"

    echo ""
    echo -e "${GREEN}${BOLD}  ✓ All interactive setup complete. Starting unattended installation...${RESET}"
    echo ""
    echo -e "  ${DIM}You can now lock your screen. The installer will run unattended.${RESET}"
    echo -e "  ${DIM}Enable Termux wakelock first: swipe down → hold Termux notification → Wakelock${RESET}"
    echo ""
    sleep 2
}

# =============================================================================
# PHASE 4: PACKAGE INSTALLATION
# =============================================================================

phase_packages() {
    log_header "Phase 4 · Package Installation"

    if [ "$IS_TERMUX" != "1" ]; then
        log_info "Skipping package install — not in Termux"
        return
    fi

    log_info "Updating package index..."
    run pkg update -y -o "Dpkg::Options::=--force-confnew" 2>/dev/null || \
        run pkg update -y

    local packages=(
        build-essential
        git
        curl
        openssl
        cmake
        ninja
        libopus
        termux-services
        coreutils
    )

    log_info "Installing packages: ${packages[*]}"
    run pkg install -y "${packages[@]}"

    log_step "Packages installed"
}

# =============================================================================
# PHASE 5: RUST TOOLCHAIN
# =============================================================================

phase_rust() {
    log_header "Phase 5 · Rust Toolchain"

    # Skip Rust toolchain installation when using pre-built binaries
    if [ "$OPT_SKIP_BUILD" = "1" ]; then
        log_info "Skipping Rust toolchain (--skip-build)"
        return
    fi

    # ── Termux environment workarounds ────────────────────────────────────────
    # On Android/Termux, rustup's built-in TLS uses rustls-platform-verifier
    # which calls into Android's native certificate verifier. Termux does NOT
    # provide the standard Android keystore JNI, causing:
    #   panicked at rustls-platform-verifier/src/android.rs:
    #     "Expect rustls-platform-verifier to be initialized"
    #
    # RUSTUP_USE_CURL=1 forces rustup to delegate TLS to the system curl
    # (which Termux ships with working TLS via openssl/rustls).
    #
    # We also set CARGO_HOME / RUSTUP_HOME explicitly because Termux's $HOME
    # (/data/data/com.termux/files/home) differs from the euid-derived home
    # (/data), which causes rustup to error with:
    #   "$HOME differs from euid-obtained home directory"
    if [ "$IS_TERMUX" = "1" ]; then
        export RUSTUP_USE_CURL=1
        export RUSTUP_INIT_SKIP_PATH_CHECK=yes

        # Remove any pkg-installed Rust that conflicts with rustup.
        # Termux's `pkg install rust` puts binaries in $PREFIX/bin which
        # clashes with rustup's toolchain management.
        if command -v rustc &>/dev/null && ! command -v rustup &>/dev/null; then
            log_info "Removing pkg-installed Rust (conflicts with rustup)..."
            run pkg uninstall -y rust 2>/dev/null || true
        fi
    fi

    if command -v rustup &>/dev/null; then
        log_step "rustup already installed: $(rustup --version 2>/dev/null || echo 'unknown')"
        run rustup update nightly-2026-03-01
    else
        log_info "Downloading rustup installer (TLS verified)..."
        local rustup_tmp
        rustup_tmp="$(mktemp)"

        run curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs -o "$rustup_tmp"

        if [ "$OPT_DRY_RUN" != "1" ]; then
            if [ ! -s "$rustup_tmp" ]; then
                rm -f "$rustup_tmp"
                die "rustup download failed (empty file)." "Check network and retry."
            fi
            if ! grep -q 'RUSTUP_UPDATE_ROOT\|rustup-init\|https://static.rust-lang.org' "$rustup_tmp"; then
                rm -f "$rustup_tmp"
                die "rustup installer failed integrity check." \
                    "Possible MITM or CDN corruption. Retry or install manually from https://rustup.rs"
            fi
            log_step "rustup installer content sanity check passed"
            sh "$rustup_tmp" -y --default-toolchain nightly-2026-03-01 --profile minimal
            rm -f "$rustup_tmp"
        fi

        # Source cargo env
        # shellcheck source=/dev/null
        [ -f "$CARGO_HOME/env" ] && source "$CARGO_HOME/env"
        export PATH="$CARGO_HOME/bin:$PATH"
        log_step "rustup installed"
    fi

    if ! command -v cargo &>/dev/null; then
        die "cargo not found after rustup install." \
            "Try: source ~/.cargo/env && cargo --version"
    fi
    log_step "Rust toolchain: $(rustc --version 2>/dev/null || echo 'unknown')"
}

# =============================================================================
# PHASE 6: SOURCE ACQUISITION
# =============================================================================

phase_source() {
    log_header "Phase 6 · Source Acquisition"

    # Skip source acquisition when using pre-built binaries
    if [ "$OPT_SKIP_BUILD" = "1" ]; then
        log_info "Skipping source acquisition (--skip-build)"
        return
    fi

    local target_ref
    case "$OPT_CHANNEL" in
        nightly) target_ref="$AURA_NIGHTLY_TAG" ;;
        *)        target_ref="$AURA_STABLE_TAG"   ;;
    esac

    if [ -d "$AURA_HOME/.git" ]; then
        log_info "Repository already exists at $AURA_HOME"
        run git -C "$AURA_HOME" fetch --tags --prune origin
        run git -C "$AURA_HOME" checkout "$target_ref"
        [ "$target_ref" = "main" ] && run git -C "$AURA_HOME" pull origin main
        log_step "Repository updated to $target_ref"
    else
        log_info "Cloning AURA from $AURA_REPO ..."
        run git clone --depth 1 --branch "$target_ref" "$AURA_REPO" "$AURA_HOME"
        log_step "Repository cloned"
    fi

    log_info "Initializing git submodules (llama.cpp)..."
    run git -C "$AURA_HOME" submodule update --init --recursive --depth 1
    # Verify submodule was populated
    if [ -d "$AURA_HOME/crates/aura-llama-sys/llama.cpp" ] && \
       [ "$(ls -A "$AURA_HOME/crates/aura-llama-sys/llama.cpp" 2>/dev/null | grep -cv '.gitkeep')" -gt 0 ]; then
        log_step "Submodules initialized"
    else
        warn "llama.cpp submodule may not have initialized correctly."
        log_info "Attempting full submodule fetch..."
        run git -C "$AURA_HOME" submodule update --init --recursive --force
        if [ ! -f "$AURA_HOME/crates/aura-llama-sys/llama.cpp/CMakeLists.txt" ]; then
            die "Failed to initialize llama.cpp submodule." \
                "Try: cd $AURA_HOME && git submodule update --init --recursive"
        fi
        log_step "Submodules initialized (full fetch)"
    fi
}

# =============================================================================
# PHASE 7: MODEL DOWNLOAD
# =============================================================================

phase_model() {
    log_header "Phase 7 · Model Download"

    if [ "$OPT_SKIP_MODEL" = "1" ]; then
        log_info "Skipping model download (--skip-model)"
        return
    fi

    local model_name model_url model_sha256 model_size_gb
    case "$OPT_MODEL" in
        qwen3-1.5b)
            model_name="$MODEL_QWEN3_1_5B_NAME"
            model_url="$MODEL_QWEN3_1_5B_URL"
            model_sha256="$MODEL_QWEN3_1_5B_SHA256"
            model_size_gb="$MODEL_QWEN3_1_5B_SIZE_GB"
            ;;
        qwen3-4b)
            model_name="$MODEL_QWEN3_4B_NAME"
            model_url="$MODEL_QWEN3_4B_URL"
            model_sha256="$MODEL_QWEN3_4B_SHA256"
            model_size_gb="$MODEL_QWEN3_4B_SIZE_GB"
            ;;
        qwen3-14b)
            model_name="$MODEL_QWEN3_14B_NAME"
            model_url="$MODEL_QWEN3_14B_URL"
            model_sha256="$MODEL_QWEN3_14B_SHA256"
            model_size_gb="$MODEL_QWEN3_14B_SIZE_GB"
            ;;
        qwen3-8b|*)
            model_name="$MODEL_QWEN3_8B_NAME"
            model_url="$MODEL_QWEN3_8B_URL"
            model_sha256="$MODEL_QWEN3_8B_SHA256"
            model_size_gb="$MODEL_QWEN3_8B_SIZE_GB"
            OPT_MODEL="qwen3-8b"
            ;;
    esac

    local model_path="$AURA_MODELS_DIR/$model_name"

    if [ -f "$model_path" ]; then
        log_info "Model file found: $model_path"
        if verify_checksum "$model_path" "$model_sha256"; then
            log_step "Model verified — skipping download"
            return
        else
            warn "Existing model failed checksum — re-downloading."
        fi
    fi

    # Ensure space
    run mkdir -p "$AURA_MODELS_DIR"
    local avail_kb
    avail_kb=$(df -k "$AURA_MODELS_DIR" 2>/dev/null | awk 'NR==2 {print $4}' || echo "0")
    local avail_gb=$(( avail_kb / 1024 / 1024 ))
    if [ "$avail_gb" -lt "$model_size_gb" ]; then
        die "Insufficient storage for model: ${avail_gb} GB free, ${model_size_gb} GB required." \
            "Free up storage or choose a smaller model with --model qwen3-4b"
    fi

    log_info "Downloading: $model_name (~${model_size_gb} GB)"
    log_info "Source:      $model_url"
    log_info "Destination: $model_path"
    log_info "This will take a while. Download is resumable — re-run if interrupted."

    local curl_args=(
        --fail
        --location
        --continue-at -
        --progress-bar
        --output "$model_path"
    )
    [ -n "${HF_TOKEN:-}" ] && curl_args+=(--header "Authorization: Bearer $HF_TOKEN")

    if [ "$OPT_DRY_RUN" = "1" ]; then
        log_info "[dry-run] Would download: $model_url"
        return
    fi

    local attempt=1
    while [ "$attempt" -le 3 ]; do
        if curl "${curl_args[@]}" "$model_url"; then
            break
        else
            local ec=$?
            [ "$attempt" -lt 3 ] && { warn "Download attempt $attempt failed (exit $ec). Retrying in 10s..."; sleep 10; } || \
                die "Download failed after 3 attempts." \
                    "Check network. Use HF_TOKEN=your_token if rate-limited." \
                    "Re-run with --repair model to retry just this phase."
        fi
        attempt=$(( attempt + 1 ))
    done

    # ── Post-download GGUF validation ──────────────────────────────────────
    if [ ! -f "$model_path" ]; then
        die "Model file missing after download: $model_path"
    fi

    local file_size
    file_size=$(stat -c%s "$model_path" 2>/dev/null || stat -f%z "$model_path" 2>/dev/null || echo 0)
    if [ "$file_size" -lt 104857600 ]; then
        log_info "Downloaded file is only $(( file_size / 1024 )) KB — likely an error page, not a model."
        rm -f "$model_path"
        die "Download produced a tiny file (<100 MB). The URL may have returned a 404 error page." \
            "Verify model URL: $model_url" \
            "If using a private/gated model, set HF_TOKEN=your_token and retry."
    fi

    # Verify GGUF magic bytes (first 4 bytes = 0x47475546 = "GGUF")
    local magic
    magic=$(head -c4 "$model_path" | od -A n -t x1 | tr -d ' \n')
    if [ "$magic" != "47475546" ]; then
        log_info "File does not start with GGUF magic bytes (got: $magic)"
        rm -f "$model_path"
        die "Downloaded file is not a valid GGUF model." \
            "Expected GGUF header, got garbage — likely an HTML error page." \
            "Verify model URL: $model_url"
    fi
    log_info "GGUF magic bytes verified ✓"

    log_info "Verifying checksum..."
    if ! verify_checksum "$model_path" "$model_sha256"; then
        local actual_sha
        actual_sha=$(sha256sum "$model_path" | cut -d' ' -f1)
        rm -f "$model_path"
        die "Checksum mismatch — possible supply-chain attack or corruption!" \
            "Expected: $model_sha256" \
            "Actual:   $actual_sha"
    fi
    log_step "Model downloaded and verified: $model_name"
}

verify_checksum() {
    local file="$1"
    local expected="$2"

    if [[ "$expected" == "" || "$expected" == PLACEHOLDER* ]]; then
        if [[ "$AURA_VERSION" == *alpha* || "$AURA_VERSION" == *beta* ]] || [ "$OPT_CHANNEL" = "nightly" ]; then
            warn "SHA256 verification skipped — alpha/nightly release has placeholder checksums"
            return 0
        else
            die "SHA256 checksum not set (placeholder). This is a packaging error — report to maintainers."
        fi
    fi

    command -v sha256sum &>/dev/null || { warn "sha256sum not found — skipping verification"; return 0; }
    local actual
    actual=$(sha256sum "$file" | cut -d' ' -f1)
    [ "$actual" = "$expected" ]
}

# =============================================================================
# PHASE 8: BUILD
# =============================================================================

phase_build() {
    log_header "Phase 8 · Build"

    if [ "$OPT_SKIP_BUILD" = "1" ]; then
        log_info "Skipping build (--skip-build)"

        if [ -f "$AURA_BIN" ] && [ -f "$AURA_NEOCORTEX_BIN" ]; then
            log_step "Using existing binaries"
            return
        fi

        # Download pre-built from GitHub Releases
        log_info "Downloading pre-built binaries from GitHub Releases..."
        local repo_slug
        repo_slug=$(echo "$AURA_REPO" | sed 's|https://github.com/||;s|\.git$||')
        local release_tag="$AURA_STABLE_TAG"
        local base_url="https://github.com/${repo_slug}/releases/download/${release_tag}"

        for artifact in \
            "aura-daemon-${release_tag}-aarch64-linux-android" \
            "aura-neocortex-${release_tag}-aarch64-linux-android"; do

            local dest
            [[ "$artifact" == aura-daemon-* ]] && dest="$AURA_BIN" || dest="$AURA_NEOCORTEX_BIN"
            local url="${base_url}/${artifact}"
            local sha_url="${url}.sha256"

            log_info "Downloading: $url"
            local att=1
            while [ "$att" -le 3 ]; do
                curl --fail --location --progress-bar --output "$dest" "$url" && break
                local ec=$?
                [ "$att" -lt 3 ] && { warn "Download failed (exit $ec). Retrying..."; sleep 5; } || \
                    die "Failed to download $artifact after 3 attempts." "URL: $url"
                att=$(( att + 1 ))
            done

            local cf
            cf="$(mktemp)"
            if curl --fail --silent --location --output "$cf" "$sha_url" 2>/dev/null; then
                local exp_sha
                exp_sha=$(cut -d' ' -f1 < "$cf")
                rm -f "$cf"
                if command -v sha256sum &>/dev/null; then
                    local act_sha
                    act_sha=$(sha256sum "$dest" | cut -d' ' -f1)
                    [ "$act_sha" != "$exp_sha" ] && {
                        rm -f "$dest"
                        die "Checksum mismatch for $artifact!" \
                            "Expected: $exp_sha" "Actual: $act_sha"
                    }
                    log_step "Checksum verified: $artifact"
                fi
            else
                rm -f "$cf"
                warn "Could not download .sha256 for $artifact — skipping verification"
            fi

            chmod +x "$dest"
            log_step "Downloaded: $dest"
        done
        return
    fi

    [ -d "$AURA_HOME" ] || die "Source directory not found: $AURA_HOME" \
        "Run without --skip-build so source is cloned first"

    # Source cargo env if needed
    # shellcheck source=/dev/null
    [ -f "$CARGO_HOME/env" ] && source "$CARGO_HOME/env" 2>/dev/null || true
    export PATH="$CARGO_HOME/bin:$PATH"

    command -v cargo &>/dev/null || die "cargo not found." "Run: source ~/.cargo/env"

    local cpu_count
    cpu_count=$(nproc 2>/dev/null || echo "2")
    local build_jobs=$(( cpu_count / 2 < 1 ? 1 : cpu_count / 2 ))
    log_info "Build jobs: $build_jobs / $cpu_count CPUs"
    log_info "This takes 10–30 minutes on first build. Keep screen on or enable wakelock."

    [ "$IS_TERMUX" = "1" ] && export RUSTFLAGS="${RUSTFLAGS:-} -C link-arg=-fuse-ld=lld"

    run cargo build --release \
        --manifest-path "$AURA_HOME/Cargo.toml" \
        --package aura-daemon \
        --package aura-neocortex \
        --features "aura-daemon/voice" \
        --jobs "$build_jobs"

    local daemon_bin="$AURA_HOME/target/release/aura-daemon"
    local neocortex_bin="$AURA_HOME/target/release/aura-neocortex"

    [ -f "$daemon_bin" ] || die "Build succeeded but aura-daemon binary not found." \
        "Check cargo output above"

    run cp "$daemon_bin" "$AURA_BIN"
    run cp "$neocortex_bin" "$AURA_NEOCORTEX_BIN"
    run chmod +x "$AURA_BIN" "$AURA_NEOCORTEX_BIN"

    log_step "Binaries installed: $AURA_BIN"
    log_step "Binaries installed: $AURA_NEOCORTEX_BIN"
}

# =============================================================================
# PHASE 9: PURGE BUILD TOOLS
# =============================================================================

phase_purge_build_tools() {
    log_header "Phase 9 · Purge Build Tools"

    if [ "$OPT_KEEP_BUILD_TOOLS" = "1" ]; then
        log_info "Keeping build tools (--keep-build-tools)"
        return
    fi

    if [ "$OPT_SKIP_BUILD" = "1" ]; then
        log_info "Skipping purge — build was skipped (no build tools were installed)"
        return
    fi

    log_info "Purging Rust toolchain and build cache to reclaim storage..."

    local before_kb
    before_kb=$(df -k "$HOME_DIR" 2>/dev/null | awk 'NR==2 {print $3}' || echo "0")

    # Remove rustup + cargo install (toolchain is in $RUSTUP_HOME and $CARGO_HOME)
    if command -v rustup &>/dev/null; then
        run rustup self uninstall -y 2>/dev/null || \
            { run rm -rf "$RUSTUP_HOME" "$CARGO_HOME"; }
        log_step "Rust toolchain removed"
    fi

    # Remove build artifacts (target/ dir is large — ~2 GB+ for AURA)
    if [ -d "$AURA_HOME/target" ]; then
        run rm -rf "$AURA_HOME/target"
        log_step "Build artifacts (target/) removed"
    fi

    local after_kb
    after_kb=$(df -k "$HOME_DIR" 2>/dev/null | awk 'NR==2 {print $3}' || echo "0")
    local freed_mb=$(( (before_kb - after_kb) / 1024 ))
    log_step "Purge complete — freed approximately ${freed_mb} MB"
    log_info "To reinstall Rust: curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh"
}

# =============================================================================
# PHASE 10: CONFIGURATION FINALIZATION
# =============================================================================

phase_config() {
    log_header "Phase 10 · Configuration"

    run mkdir -p "$AURA_CONFIG_DIR" "$AURA_DATA_DIR" "$AURA_MODELS_DIR" \
                 "$AURA_LOGS_DIR" "$AURA_DB_DIR"

    # Determine model path
    local model_name
    case "$OPT_MODEL" in
        qwen3-1.5b) model_name="$MODEL_QWEN3_1_5B_NAME" ;;
        qwen3-4b)   model_name="$MODEL_QWEN3_4B_NAME"   ;;
        qwen3-14b)  model_name="$MODEL_QWEN3_14B_NAME"  ;;
        *)          model_name="$MODEL_QWEN3_8B_NAME"   ;;
    esac
    local model_path="$AURA_MODELS_DIR/$model_name"

    # Thread count: up to 4
    local cpu_count
    cpu_count=$(nproc 2>/dev/null || echo "4")
    local n_threads=$(( cpu_count > 4 ? 4 : cpu_count ))

    # Determine n_ctx by model tier
    local n_ctx=4096
    [ "$OPT_MODEL" = "qwen3-1.5b" ] && n_ctx=2048
    [ "$OPT_MODEL" = "qwen3-14b"  ] && n_ctx=8192

    if [ -f "$AURA_CONFIG_FILE" ] && [ "$OPT_UPDATE" != "1" ]; then
        log_info "Config already exists at $AURA_CONFIG_FILE — NOT overwriting"
        log_info "Delete it to regenerate: rm '$AURA_CONFIG_FILE'"
        return
    fi

    log_info "Writing full config to $AURA_CONFIG_FILE ..."

    if [ "$OPT_DRY_RUN" = "1" ]; then
        log_info "[dry-run] Would write full config.toml"
        return
    fi

    # Safe-escape user name and token for TOML
    local safe_user_name
    safe_user_name=$(printf '%s' "${COLLECTED_USER_NAME:-User}" | sed 's/[\\\"]/\\&/g' | tr -d '\n')

    cat > "$AURA_CONFIG_FILE" <<TOML_EOF
# AURA v4 Configuration
# Generated by install.sh ${AURA_VERSION} on $(date -u +%Y-%m-%dT%H:%M:%SZ)
# Edit freely — install.sh will NOT overwrite this file on re-run (unless --update).
# Full reference: aura-config.example.toml

# =============================================================================
#  § 1. Daemon
# =============================================================================

[daemon]
data_dir                = "${AURA_DATA_DIR}"
log_level               = "info"
checkpoint_interval_s   = 300
rss_warning_mb          = 28
rss_ceiling_mb          = 30

# =============================================================================
#  § 2. Telegram Interface
# =============================================================================

[telegram]
enabled         = true
bot_token       = "${COLLECTED_BOT_TOKEN}"
allowed_chat_ids = [${COLLECTED_OWNER_ID}]
poll_interval_ms = 2000

# =============================================================================
#  § 3. Neocortex — LLM Inference
# =============================================================================

[neocortex]
model_dir              = "${AURA_MODELS_DIR}"
default_model_name     = "${model_name}"
default_model_path     = "${model_path}"
default_n_ctx          = ${n_ctx}
n_threads              = ${n_threads}
max_memory_mb          = 2048
inference_timeout_ms   = 60000

# =============================================================================
#  § 4. LLaMA.cpp Parameters
# =============================================================================

[llama.model]
n_gpu_layers = 0
use_mmap     = true
use_mlock    = false

[llama.context]
n_ctx     = ${n_ctx}
n_batch   = 512
n_threads = ${n_threads}
seed      = 41146

[llama.sampling]
temperature    = 0.6
top_p          = 0.9
top_k          = 40
repeat_penalty = 1.1
max_tokens     = 512

# =============================================================================
#  § 5. Identity
# =============================================================================

[identity]
user_name       = "${safe_user_name}"
assistant_name  = "AURA"
mood_cooldown_ms = 60000
max_mood_delta  = 0.2
trust_hysteresis = 0.05

[identity.ocean]
openness          = 0.85
conscientiousness = 0.75
extraversion      = 0.50
agreeableness     = 0.70
neuroticism       = 0.25

[identity.mood_neutral]
valence   = 0.0
arousal   = 0.0
dominance = 0.5

[identity.relationship_thresholds]
stranger_max     = 0.15
acquaintance_max = 0.35
friend_max       = 0.60
close_friend_max = 0.85

# =============================================================================
#  § 6. Vault
# =============================================================================

[vault]
pin_hash         = "sha256:${COLLECTED_PIN_SALT}:${COLLECTED_PIN_HASH}"
auto_lock_seconds = 0

# =============================================================================
#  § 7. SQLite Storage
# =============================================================================

[sqlite]
db_path           = "${AURA_DB_DIR}/aura.db"
wal_size_limit    = 4194304
max_episodes      = 10000
max_semantic_entries = 5000

# =============================================================================
#  § 8. Amygdala — Event Scoring
# =============================================================================

[amygdala]
instant_threshold   = 0.65
weight_lex          = 0.40
weight_src          = 0.25
weight_time         = 0.20
weight_anom         = 0.15
storm_dedup_size    = 50
storm_rate_limit_ms = 30000
cold_start_events   = 200
cold_start_hours    = 72

# =============================================================================
#  § 9. Execution Engine
# =============================================================================

[execution]
max_steps_normal           = 200
max_steps_safety           = 50
max_steps_power            = 500
rate_limit_actions_per_min = 60
delay_min_ms               = 150
delay_max_ms               = 500

# =============================================================================
#  § 10. Power Management
# =============================================================================

[power]
daily_token_budget      = 50000
conservative_threshold  = 50
low_power_threshold     = 30
critical_threshold      = 15
emergency_threshold     = 5

[power_tiers.charging]
max_inference_calls_per_hour = 120
model_tier                   = "Full8B"
background_scan_interval_s   = 30
proactive_enabled            = true
max_concurrent_goals         = 8

[power_tiers.normal]
max_inference_calls_per_hour = 60
model_tier                   = "Standard4B"
background_scan_interval_s   = 120
proactive_enabled            = true
max_concurrent_goals         = 5

[power_tiers.conserve]
max_inference_calls_per_hour = 20
model_tier                   = "Brainstem1_5B"
background_scan_interval_s   = 600
proactive_enabled            = false
max_concurrent_goals         = 2

[power_tiers.critical]
max_inference_calls_per_hour = 5
model_tier                   = "Brainstem1_5B"
background_scan_interval_s   = 1800
proactive_enabled            = false
max_concurrent_goals         = 1

[power_tiers.emergency]
max_inference_calls_per_hour = 0
model_tier                   = "Brainstem1_5B"
background_scan_interval_s   = 3600
proactive_enabled            = false
max_concurrent_goals         = 0

# =============================================================================
#  § 11. Thermal Management
# =============================================================================

[thermal]
warm_c                    = 40.0
hot_c                     = 45.0
critical_c                = 50.0
hysteresis_c              = 2.0
min_transition_interval_s = 10

# =============================================================================
#  § 12. Retry Policy
# =============================================================================

[retry]
max_retries    = 3
base_delay_ms  = 200
backoff_factor = 2
max_delay_ms   = 10000
jitter_ms      = 50

# =============================================================================
#  § 13. Feature Flags
# =============================================================================

[features]
voice_enabled              = false
proactive_triggers_enabled = true
learning_enabled           = true
sentiment_analysis_enabled = true
multi_language_enabled     = false
debug_mode                 = false
TOML_EOF

    # Restrict config file permissions — contains bot token in plaintext.
    chmod 600 "$AURA_CONFIG_FILE"
    log_step "Configuration written: $AURA_CONFIG_FILE (mode 600)"
}

# =============================================================================
# PHASE 11: SERVICE SETUP
# =============================================================================

phase_service() {
    log_header "Phase 11 · Service Setup"

    if [ "$OPT_SKIP_SERVICE" = "1" ]; then
        log_info "Skipping service setup (--skip-service)"
        return
    fi

    if [ "$IS_TERMUX" != "1" ]; then
        log_info "Skipping service setup — not in Termux"
        return
    fi

    # Attempt termux-services (preferred) — uses runit-style /var/service
    if command -v sv &>/dev/null; then
        log_info "Setting up termux-services (sv/runit)..."

        run mkdir -p "$AURA_SV_DIR"
        run mkdir -p "$AURA_SV_DIR/log"

        # Main run script
        cat > "$AURA_SV_DIR/run" <<'RUN_EOF'
#!/data/data/com.termux/files/usr/bin/bash
exec 2>&1
exec "$PREFIX/bin/aura-daemon"
RUN_EOF
        chmod +x "$AURA_SV_DIR/run"

        # Log run script
        cat > "$AURA_SV_DIR/log/run" <<'LOG_EOF'
#!/data/data/com.termux/files/usr/bin/bash
exec svlogd -tt "$HOME/.local/share/aura/logs"
LOG_EOF
        chmod +x "$AURA_SV_DIR/log/run"

        log_step "termux-services scripts created"
        log_info "Enable with: sv-enable aura-daemon && sv up aura-daemon"
        log_info "Logs at:     $AURA_LOGS_DIR/"
    else
        # Fallback: .bashrc auto-start
        log_info "termux-services not installed — using .bashrc fallback"
        local bashrc="$HOME_DIR/.bashrc"

        if ! grep -q 'aura-daemon' "$bashrc" 2>/dev/null; then
            cat >> "$bashrc" <<'BASHRC_EOF'

# ── AURA v4 auto-start ─────────────────────────────────────────────────────
if [ -z "$AURA_STARTED" ] && command -v aura-daemon &>/dev/null; then
    export AURA_STARTED=1
    echo "[AURA] Starting daemon..."
    nohup aura-daemon >> "$HOME/.local/share/aura/logs/daemon.log" 2>&1 &
fi
# ────────────────────────────────────────────────────────────────────────────
BASHRC_EOF
            log_step ".bashrc auto-start added"
        else
            log_info ".bashrc already contains aura-daemon entry — skipping"
        fi
        log_info "AURA will start automatically on next Termux session."
    fi
}

# =============================================================================
# PHASE 12: VERIFICATION + SUCCESS BANNER
# =============================================================================

phase_verify() {
    log_header "Phase 12 · Verification"

    local all_ok=1

    # Binary check
    if [ -x "$AURA_BIN" ]; then
        log_step "aura-daemon binary: $AURA_BIN"
    else
        warn "aura-daemon binary missing or not executable!"
        all_ok=0
    fi

    if [ -x "$AURA_NEOCORTEX_BIN" ]; then
        log_step "aura-neocortex binary: $AURA_NEOCORTEX_BIN"
    else
        warn "aura-neocortex binary missing or not executable!"
        all_ok=0
    fi

    # Config check
    if [ -f "$AURA_CONFIG_FILE" ]; then
        log_step "Configuration: $AURA_CONFIG_FILE"
    else
        warn "Configuration file missing!"
        all_ok=0
    fi

    # Model check
    if [ "$OPT_SKIP_MODEL" != "1" ]; then
        local model_name
        case "$OPT_MODEL" in
            qwen3-1.5b) model_name="$MODEL_QWEN3_1_5B_NAME" ;;
            qwen3-4b)   model_name="$MODEL_QWEN3_4B_NAME"   ;;
            qwen3-14b)  model_name="$MODEL_QWEN3_14B_NAME"  ;;
            *)          model_name="$MODEL_QWEN3_8B_NAME"   ;;
        esac
        if [ -f "$AURA_MODELS_DIR/$model_name" ]; then
            log_step "Model: $model_name"
        else
            warn "Model file missing: $model_name"
            all_ok=0
        fi
    fi

    # Quick smoke test — just version output
    if [ "$OPT_DRY_RUN" != "1" ] && [ -x "$AURA_BIN" ]; then
        local ver
        ver=$("$AURA_BIN" --version 2>/dev/null || echo "")
        if [ -n "$ver" ]; then
            log_step "Daemon responds: $ver"
        else
            warn "Daemon did not respond to --version"
        fi
    fi

    echo ""
    if [ "$all_ok" = "1" ]; then
        echo -e "${GREEN}${BOLD}"
        echo "  ╔═══════════════════════════════════════════════════════════════════╗"
        echo "  ║                                                                   ║"
        echo "  ║      █████╗ ██╗   ██╗██████╗  █████╗     ██╗   ██╗██╗  ██╗       ║"
        echo "  ║     ██╔══██╗██║   ██║██╔══██╗██╔══██╗    ██║   ██║██║  ██║       ║"
        echo "  ║     ███████║██║   ██║██████╔╝███████║    ██║   ██║███████║       ║"
        echo "  ║     ██╔══██║██║   ██║██╔══██╗██╔══██║    ╚██╗ ██╔╝╚════██║       ║"
        echo "  ║     ██║  ██║╚██████╔╝██║  ██║██║  ██║     ╚████╔╝      ██║       ║"
        echo "  ║     ╚═╝  ╚═╝ ╚═════╝ ╚═╝  ╚═╝╚═╝  ╚═╝      ╚═══╝       ╚═╝       ║"
        echo "  ║                                                                   ║"
        echo "  ║              Installation Complete! Welcome home.                 ║"
        echo "  ║                                                                   ║"
        echo "  ╚═══════════════════════════════════════════════════════════════════╝"
        echo -e "${RESET}"
        echo ""
        echo -e "  ${BOLD}Next steps:${RESET}"
        echo ""
        echo -e "  ${CYAN}1.${RESET} Start AURA manually:"
        echo -e "     ${DIM}aura-daemon${RESET}"
        echo ""
        if command -v sv &>/dev/null 2>/dev/null; then
            echo -e "  ${CYAN}2.${RESET} Or enable as a service (auto-start on Termux boot):"
            echo -e "     ${DIM}sv-enable aura-daemon && sv up aura-daemon${RESET}"
            echo ""
        fi
        echo -e "  ${CYAN}3.${RESET} Open Telegram and message your bot to verify connection."
        echo ""
        echo -e "  ${BOLD}Important:${RESET} Keep Termux running with wakelock to prevent Android"
        echo -e "  from killing the daemon. Swipe down → hold Termux notification → Wakelock."
        echo ""
        echo -e "  ${DIM}Logs:   $AURA_LOGS_DIR/${RESET}"
        echo -e "  ${DIM}Config: $AURA_CONFIG_FILE${RESET}"
        echo -e "  ${DIM}Model:  $AURA_MODELS_DIR/${RESET}"
        echo ""
    else
        echo -e "${RED}${BOLD}"
        echo "  ╔═══════════════════════════════════════════════════════════════════╗"
        echo "  ║       Installation completed with warnings — see above.           ║"
        echo "  ╚═══════════════════════════════════════════════════════════════════╝"
        echo -e "${RESET}"
        echo ""
        echo -e "  Some components may be missing. Re-run with ${CYAN}--repair <phase>${RESET}"
        echo -e "  to fix specific phases, or check the install log:"
        echo -e "  ${DIM}$INSTALL_LOG${RESET}"
        echo ""
    fi
}

# =============================================================================
# REPAIR MODE
# =============================================================================

run_repair() {
    local phase="$1"
    setup_colors
    log_header "Repair Mode · Phase: $phase"

    case "$phase" in
        preflight)   phase_preflight ;;
        packages)    phase_packages ;;
        rust)        phase_rust ;;
        source)      phase_source ;;
        model)       phase_model ;;
        build)       phase_build ;;
        purge)       phase_purge_build_tools ;;
        config)      phase_config ;;
        service)     phase_service ;;
        verify)      phase_verify ;;
        *)
            die "Unknown repair phase: $phase" \
                "Valid phases: preflight|packages|rust|source|model|build|purge|config|service|verify"
            ;;
    esac

    log_step "Repair phase '$phase' complete."
}

# =============================================================================
# MAIN
# =============================================================================

main() {
    parse_args "$@"
    setup_colors

    # ── Repair mode ───────────────────────────────────────────────────────────
    if [ -n "$OPT_REPAIR" ]; then
        run_repair "$OPT_REPAIR"
        exit 0
    fi

    # ── Full install ──────────────────────────────────────────────────────────
    echo ""
    echo -e "${BOLD}${BLUE}"
    echo "  ╔═══════════════════════════════════════════════════════════════════╗"
    echo "  ║                    AURA v4 Installer                              ║"
    echo "  ║                 Enterprise Termux Edition                         ║"
    echo "  ╚═══════════════════════════════════════════════════════════════════╝"
    echo -e "${RESET}"
    echo -e "  ${DIM}Version:  ${AURA_VERSION}${RESET}"
    echo -e "  ${DIM}Channel:  ${OPT_CHANNEL}${RESET}"
    echo -e "  ${DIM}Log:      ${INSTALL_LOG}${RESET}"
    echo ""

    # Tee output to log file (while preserving interactivity via /dev/tty reads)
    exec > >(tee -a "$INSTALL_LOG") 2>&1

    # ── Interactive phases (front-loaded) ─────────────────────────────────────
    phase_preflight
    phase_space_budget
    phase_hardware_and_model
    phase_telegram_wizard
    phase_vault_setup

    # ── Unattended phases ─────────────────────────────────────────────────────
    phase_packages
    phase_rust
    phase_source
    phase_model
    phase_build
    phase_purge_build_tools
    phase_config
    phase_service
    phase_verify
}

main "$@"
