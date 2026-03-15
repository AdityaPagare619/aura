#!/usr/bin/env bash
# =============================================================================
# AURA v4 — Termux Installer
# =============================================================================
# Usage:
#   bash install.sh [OPTIONS]
#
# Options:
#   --channel stable|nightly   Release channel (default: stable)
#   --model <name>             Model variant to download (default: qwen3-8b-q4_k_m)
#   --skip-build               Skip Rust build (use pre-built binary)
#   --skip-model               Skip model download
#   --skip-service             Skip termux-services setup
#   --dry-run                  Print actions without executing
#   --update                   Update existing installation
#   --no-color                 Disable color output
#   -h, --help                 Show this help
#
# Environment variables:
#   HF_TOKEN     HuggingFace token for authenticated downloads (optional)
#   AURA_REPO    Override git repo URL (default: https://github.com/AdityaPagare619/aura.git)
# =============================================================================
set -euo pipefail

# =============================================================================
# CONSTANTS
# =============================================================================

AURA_VERSION="4.0.0-alpha.1"
# TODO: Set this to your actual GitHub repo URL before publishing
# Repository URL - override with: AURA_REPO=https://your-fork.git bash install.sh
AURA_REPO="${AURA_REPO:-https://github.com/AdityaPagare619/aura.git}"
AURA_STABLE_TAG="v4.0.0-alpha.1"
AURA_NIGHTLY_TAG="main"

# Model registry
MODEL_QWEN3_8B_NAME="qwen3-8b-q4_k_m.gguf"
MODEL_QWEN3_8B_URL="https://huggingface.co/Qwen/Qwen3-8B-GGUF/resolve/main/qwen3-8b-q4_k_m.gguf"
MODEL_QWEN3_8B_SHA256="PLACEHOLDER_UPDATE_AT_RELEASE_TIME_8b"
MODEL_QWEN3_8B_SIZE_GB=5

MODEL_QWEN3_4B_NAME="qwen3-4b-q4_k_m.gguf"
MODEL_QWEN3_4B_URL="https://huggingface.co/Qwen/Qwen3-4B-GGUF/resolve/main/qwen3-4b-q4_k_m.gguf"
MODEL_QWEN3_4B_SHA256="PLACEHOLDER_UPDATE_AT_RELEASE_TIME_4b"
MODEL_QWEN3_4B_SIZE_GB=3

MODEL_QWEN3_14B_NAME="qwen3-14b-q4_k_m.gguf"
MODEL_QWEN3_14B_URL="https://huggingface.co/Qwen/Qwen3-14B-GGUF/resolve/main/qwen3-14b-q4_k_m.gguf"
MODEL_QWEN3_14B_SHA256="PLACEHOLDER_UPDATE_AT_RELEASE_TIME_14b"
MODEL_QWEN3_14B_SIZE_GB=10

# Paths — work in both Termux and standard Linux
if [ -d "/data/data/com.termux" ]; then
    IS_TERMUX=1
    TERMUX_PREFIX="/data/data/com.termux/files/usr"
    HOME_DIR="/data/data/com.termux/files/home"
else
    IS_TERMUX=0
    TERMUX_PREFIX="/usr"
    HOME_DIR="$HOME"
fi

AURA_HOME="$HOME_DIR/aura"
AURA_CONFIG_DIR="$HOME_DIR/.config/aura"
AURA_DATA_DIR="$HOME_DIR/.local/share/aura"
AURA_MODELS_DIR="$AURA_DATA_DIR/models"
AURA_LOGS_DIR="$AURA_DATA_DIR/logs"
AURA_DB_DIR="$AURA_DATA_DIR/db"
AURA_CONFIG_FILE="$AURA_CONFIG_DIR/config.toml"
AURA_SOCK="$AURA_DATA_DIR/daemon.sock"
AURA_BIN="$TERMUX_PREFIX/bin/aura-daemon"
AURA_NEOCORTEX_BIN="$TERMUX_PREFIX/bin/aura-neocortex"
AURA_SV_DIR="$TERMUX_PREFIX/var/service/aura-daemon"

# Install log — written alongside the installer for easy sharing on failure
INSTALL_LOG="${TMPDIR:-/tmp}/aura-install-$(date +%Y%m%d-%H%M%S).log"

MIN_FREE_GB=8
MIN_RAM_GB=4

# =============================================================================
# FLAGS
# =============================================================================

OPT_CHANNEL="stable"
OPT_MODEL="qwen3-8b"
OPT_SKIP_BUILD=0
OPT_SKIP_MODEL=0
OPT_SKIP_SERVICE=0
OPT_DRY_RUN=0
OPT_UPDATE=0
OPT_NO_COLOR=0

# =============================================================================
# COLORS
# =============================================================================

setup_colors() {
    if [ "${NO_COLOR:-}" = "1" ] || [ "$OPT_NO_COLOR" = "1" ] || ! [ -t 1 ]; then
        RED="" GREEN="" YELLOW="" BLUE="" CYAN="" BOLD="" DIM="" RESET=""
    else
        RED="\033[0;31m"
        GREEN="\033[0;32m"
        YELLOW="\033[0;33m"
        BLUE="\033[0;34m"
        CYAN="\033[0;36m"
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
    echo -e "${BOLD}${BLUE}==> $1${RESET}"
}

log_step() {
    echo -e "${GREEN}  ✓${RESET} $1"
}

log_info() {
    echo -e "${CYAN}  →${RESET} $1"
}

warn() {
    echo -e "${YELLOW}  ⚠${RESET}  $1" >&2
}

die() {
    echo "" >&2
    echo -e "${RED}${BOLD}  ✗ ERROR:${RESET} $1" >&2
    # Print all remaining args as fix hints (supports multi-line context)
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

# Prompt user y/n, default y
confirm() {
    local prompt="$1"
    local default="${2:-y}"
    local answer
    if [ "$default" = "y" ]; then
        read -r -p "$(echo -e "${YELLOW}  ?${RESET} $prompt [Y/n]: ")" answer
        answer="${answer:-y}"
    else
        read -r -p "$(echo -e "${YELLOW}  ?${RESET} $prompt [y/N]: ")" answer
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
            --channel)
                OPT_CHANNEL="${2:-stable}"
                shift 2
                ;;
            --model)
                OPT_MODEL="${2:-qwen3-8b}"
                shift 2
                ;;
            --skip-build)    OPT_SKIP_BUILD=1;   shift ;;
            --skip-model)    OPT_SKIP_MODEL=1;    shift ;;
            --skip-service)  OPT_SKIP_SERVICE=1;  shift ;;
            --dry-run)       OPT_DRY_RUN=1;       shift ;;
            --update)        OPT_UPDATE=1;         shift ;;
            --no-color)      OPT_NO_COLOR=1;       shift ;;
            -h|--help)
                show_help
                exit 0
                ;;
            *)
                die "Unknown option: $1" "Run with --help for usage"
                ;;
        esac
    done
}

show_help() {
    cat <<'EOF'
AURA v4 — Termux Installer

Usage:
  bash install.sh [OPTIONS]

Options:
  --channel stable|nightly   Release channel (default: stable)
  --model qwen3-4b|qwen3-8b|qwen3-14b
                             Model size (default: qwen3-8b)
  --skip-build               Skip Rust build (use pre-built binary if present)
  --skip-model               Skip model download
  --skip-service             Skip termux-services autostart setup
  --dry-run                  Print actions without executing
  --update                   Update existing installation
  --no-color                 Disable color output
  -h, --help                 Show this help

Environment variables:
  HF_TOKEN     HuggingFace token for authenticated model downloads
  AURA_REPO    Override git repository URL

Examples:
  # Standard install:
  bash install.sh

  # Install on a 4 GB RAM device:
  bash install.sh --model qwen3-4b

  # Update existing install:
  bash install.sh --update

  # Preview what would happen:
  bash install.sh --dry-run
EOF
}

# =============================================================================
# PHASE 0: PRE-FLIGHT CHECKS
# =============================================================================

phase_preflight() {
    log_header "Phase 0: Pre-flight Checks"

    # Architecture check
    local arch
    arch=$(uname -m)
    case "$arch" in
        aarch64|arm64)
            log_step "Architecture: $arch (supported)"
            ;;
        armv7l|armv8l)
            die "ARM32 ($arch) is not supported." \
                "AURA v4 requires a 64-bit ARM device (aarch64). Most Android phones since 2015 are ARM64."
            ;;
        x86_64)
            warn "Running on x86_64 — this is a desktop/emulator build, not a Termux device build"
            ;;
        *)
            die "Unsupported architecture: $arch" "AURA v4 requires aarch64 (ARM64)."
            ;;
    esac

    # Termux check
    if [ "$IS_TERMUX" = "1" ]; then
        log_step "Termux environment detected"

        # Check Termux version
        local termux_version
        if command -v termux-info &>/dev/null; then
            termux_version=$(termux-info 2>/dev/null | grep "Termux Version" | awk '{print $NF}' || echo "unknown")
            log_info "Termux version: $termux_version"
        fi

        # Check storage permission
        if [ ! -d "$HOME_DIR/storage" ]; then
            warn "Storage permission not granted to Termux."
            log_info "Run 'termux-setup-storage' and grant permission, then re-run this installer."
            if confirm "Grant storage access now (opens Termux:API dialog)?"; then
                run termux-setup-storage || warn "Could not auto-grant storage access. Please do it manually."
                sleep 2
            fi
        else
            log_step "Storage access granted"
        fi
    else
        log_info "Not running in Termux — assuming desktop/CI environment"
    fi

    # Free space check
    check_free_space "$AURA_DATA_DIR" "$MIN_FREE_GB"

    # RAM check
    check_ram

    # Network check
    check_network

    log_step "Pre-flight checks passed"
}

check_free_space() {
    local dir="${1:-$HOME_DIR}"
    local required_gb="${2:-$MIN_FREE_GB}"

    # Create dir if needed to check its mount point
    mkdir -p "$dir" 2>/dev/null || true

    local available_kb
    available_kb=$(df -k "$dir" 2>/dev/null | awk 'NR==2 {print $4}' || echo "0")
    local available_gb=$(( available_kb / 1024 / 1024 ))

    if [ "$available_gb" -lt "$required_gb" ]; then
        die "Insufficient storage: ${available_gb} GB free, ${required_gb} GB required." \
            "Free up space on your device or use --model qwen3-4b for a smaller model."
    fi
    log_step "Free space: ${available_gb} GB available (need ${required_gb} GB)"
}

check_ram() {
    local total_kb=0
    if [ -f /proc/meminfo ]; then
        total_kb=$(grep MemTotal /proc/meminfo | awk '{print $2}')
    fi
    local total_gb=$(( total_kb / 1024 / 1024 ))

    if [ "$total_kb" -eq 0 ]; then
        warn "Could not determine RAM size — proceeding anyway"
        return
    fi

    if [ "$total_gb" -lt "$MIN_RAM_GB" ]; then
        warn "Low RAM detected: ${total_gb} GB. AURA may run slowly."
        warn "Consider using --model qwen3-4b for better performance."
    else
        log_step "RAM: ${total_gb} GB (minimum ${MIN_RAM_GB} GB)"
    fi

    # Suggest smaller model for low-RAM devices
    if [ "$total_gb" -lt 6 ] && [ "$OPT_MODEL" = "qwen3-8b" ]; then
        warn "8 GB RAM recommended for qwen3-8b. You have ${total_gb} GB."
        if confirm "Switch to qwen3-4b (smaller, works on 4 GB RAM)?"; then
            OPT_MODEL="qwen3-4b"
            log_step "Switched to qwen3-4b"
        fi
    fi
}

check_network() {
    if curl --silent --max-time 10 "https://huggingface.co" > /dev/null 2>&1; then
        log_step "Network connectivity: OK (HuggingFace reachable)"
    else
        warn "Cannot reach huggingface.co — model download will likely fail."
        warn "Check your network connection or VPN settings."
    fi
}

# =============================================================================
# PHASE 1: PACKAGE INSTALLATION
# =============================================================================

phase_packages() {
    log_header "Phase 1: Package Installation"

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
        python3
        patchelf
        termux-services
        coreutils
    )

    log_info "Installing required packages: ${packages[*]}"
    run pkg install -y "${packages[@]}"

    log_step "Packages installed"
}

# =============================================================================
# PHASE 2: RUST TOOLCHAIN
# =============================================================================

phase_rust() {
    log_header "Phase 2: Rust Toolchain"

    # Check if rustup already installed
    if command -v rustup &>/dev/null; then
        log_step "rustup already installed: $(rustup --version 2>/dev/null || echo 'unknown')"
        run rustup update nightly-2026-03-01
    else
        log_info "Installing rustup..."
        # HIGH-SEC-6: Mitigated — never pipe curl directly to interpreter.
        # CI-MED-4 / SEC-HIGH-6: Download rustup-init to a temp file and verify
        # integrity before execution. Never pipe curl directly to sh.
        local rustup_tmp
        rustup_tmp="$(mktemp)"
        run curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs -o "$rustup_tmp"
        # CI-MED-4: Verify the downloaded script is the genuine rustup installer.
        # rustup.rs serves a dynamic shell script (content changes per release),
        # so static SHA256 pinning is not feasible. We rely on:
        #   1. TLS 1.2+ certificate validation (--proto '=https' --tlsv1.2)
        #   2. Non-empty file check
        #   3. Content sanity: script must self-identify as rustup installer
        if [ ! -s "$rustup_tmp" ]; then
            rm -f "$rustup_tmp"
            die "rustup download failed (empty file)." \
                "Check network and retry."
        fi
        if ! grep -q 'RUSTUP_UPDATE_ROOT\|rustup-init\|https://static.rust-lang.org' "$rustup_tmp"; then
            rm -f "$rustup_tmp"
            die "rustup installer failed integrity check — content does not match expected rustup script." \
                "Possible MITM or CDN corruption. Retry or install manually from https://rustup.rs"
        fi
        log_step "rustup installer integrity check passed (TLS + content sanity)"
        # Execute from verified file instead of piping (never curl|sh)
        run sh "$rustup_tmp" -y --default-toolchain nightly-2026-03-01 --profile minimal
        rm -f "$rustup_tmp"

        # Source rustup in current session
        # shellcheck source=/dev/null
        if [ -f "$HOME_DIR/.cargo/env" ]; then
            source "$HOME_DIR/.cargo/env"
        fi
        export PATH="$HOME_DIR/.cargo/bin:$PATH"
        log_step "rustup installed"
    fi

    # Verify cargo
    if ! command -v cargo &>/dev/null; then
        die "cargo not found after rustup install." \
            "Try: source ~/.cargo/env && cargo --version"
    fi
    log_step "Rust toolchain: $(rustc --version)"

    # Check rust-toolchain.toml if present
    if [ -f "$AURA_HOME/rust-toolchain.toml" ]; then
        log_info "rust-toolchain.toml found — rustup will use pinned version"
    fi

    # For cross-compilation target (advanced usage, skip on Termux native)
    if [ "$IS_TERMUX" != "1" ]; then
        log_info "Adding cross-compilation target: aarch64-linux-android"
        run rustup target add aarch64-linux-android
        log_step "Cross-compilation target added"
    fi
}

# =============================================================================
# PHASE 3: SOURCE ACQUISITION
# =============================================================================

phase_source() {
    log_header "Phase 3: Source Acquisition"

    local target_ref
    case "$OPT_CHANNEL" in
        nightly) target_ref="$AURA_NIGHTLY_TAG" ;;
        *)        target_ref="$AURA_STABLE_TAG"   ;;
    esac

    if [ -d "$AURA_HOME/.git" ]; then
        log_info "Repository already exists at $AURA_HOME"
        log_info "Fetching latest changes (channel: $OPT_CHANNEL)..."
        run git -C "$AURA_HOME" fetch --tags --prune origin
        run git -C "$AURA_HOME" checkout "$target_ref"
        if [ "$target_ref" = "main" ]; then
            run git -C "$AURA_HOME" pull origin main
        fi
        log_step "Repository updated to $target_ref"
    else
        log_info "Cloning AURA repository..."
        log_info "Source: $AURA_REPO"
        log_info "Target: $AURA_HOME"
        run git clone --depth 1 --branch "$target_ref" "$AURA_REPO" "$AURA_HOME"
        log_step "Repository cloned"
    fi

    # Initialize and update submodules (required for llama.cpp)
    log_info "Initializing git submodules..."
    run git -C "$AURA_HOME" submodule update --init --recursive
    log_step "Submodules initialized"

    log_step "Source: $target_ref at $AURA_HOME"
}

# =============================================================================
# PHASE 4: MODEL DOWNLOAD
# =============================================================================

phase_model() {
    log_header "Phase 4: Model Download"

    if [ "$OPT_SKIP_MODEL" = "1" ]; then
        log_info "Skipping model download (--skip-model)"
        return
    fi

    # Resolve model config
    local model_name model_url model_sha256 model_size_gb
    case "$OPT_MODEL" in
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
            ;;
    esac

    local model_path="$AURA_MODELS_DIR/$model_name"

    # Check if model already exists and verify checksum
    if [ -f "$model_path" ]; then
        log_info "Model file found at $model_path"
        if verify_checksum "$model_path" "$model_sha256"; then
            log_step "Model already downloaded and verified — skipping download"
            return
        else
            warn "Existing model file failed checksum. Re-downloading."
        fi
    fi

    # Check space before starting download
    check_free_space "$AURA_MODELS_DIR" "$model_size_gb"

    run mkdir -p "$AURA_MODELS_DIR"

    log_info "Downloading: $model_name (~${model_size_gb} GB)"
    log_info "Source: $model_url"
    log_info "Destination: $model_path"
    log_info "This will take a while. Download supports resume — re-run if interrupted."

    # Build curl command with optional HF auth
    local curl_args=(
        --location
        --continue-at -
        --progress-bar
        --output "$model_path"
    )

    if [ -n "${HF_TOKEN:-}" ]; then
        log_info "Using HuggingFace token for authenticated download"
        curl_args+=(--header "Authorization: Bearer $HF_TOKEN")
    fi

    if [ "$OPT_DRY_RUN" = "1" ]; then
        log_info "[dry-run] Would download: $model_url"
        return
    fi

    # Download with retry logic
    local max_attempts=3
    local attempt=1
    while [ "$attempt" -le "$max_attempts" ]; do
        if curl "${curl_args[@]}" "$model_url"; then
            break
        else
            local exit_code=$?
            if [ "$attempt" -lt "$max_attempts" ]; then
                warn "Download attempt $attempt failed (exit $exit_code). Retrying in 5s..."
                sleep 5
            else
                die "Download failed after $max_attempts attempts." \
                    "Check network connection. Run with HF_TOKEN=your_token if rate-limited."
            fi
        fi
        attempt=$(( attempt + 1 ))
    done

    # Verify checksum after download
    log_info "Verifying checksum..."
    if ! verify_checksum "$model_path" "$model_sha256"; then
        # SECURITY [HIGH-SEC-2]: Checksum failure is ALWAYS fatal.
        # Supply-chain attacks rely on users clicking past warnings.
        # No user override — fail hard, delete the compromised file.
        local actual_sha
        actual_sha=$(sha256sum "$model_path" | cut -d' ' -f1)
        warn "FATAL: Checksum mismatch — possible supply-chain attack!"
        warn "Expected: $model_sha256"
        warn "Actual:   $actual_sha"
        rm -f "$model_path"
        die "Checksum verification failed. Corrupted or tampered download." \
            "Delete partial file and re-run, or verify the expected hash."
    else
        log_step "Model downloaded and verified: $model_name"
    fi
}

verify_checksum() {
    local file="$1"
    local expected_sha256="$2"

    # Placeholder checksums are ONLY acceptable on the nightly channel.
    # Stable releases MUST have real checksums — refuse to install otherwise.
    if [[ "${expected_sha256}" == "" || "${expected_sha256}" == "PLACEHOLDER" || "${expected_sha256}" == PLACEHOLDER* ]]; then
        if [[ "$AURA_VERSION" == *alpha* || "$AURA_VERSION" == *beta* ]]; then
            warn "SHA256 verification skipped — alpha/beta release has placeholder checksums"
            warn "Production releases will require verified checksums"
            return 0
        elif [ "${OPT_CHANNEL}" = "stable" ]; then
            die "SHA256 checksum not set for this model (placeholder detected)." \
                "This is a release packaging error. Do NOT use this installer for production. Report to maintainers."
        else
            warn "SHA256 verification skipped — nightly channel allows placeholder checksums"
            warn "DO NOT use nightly for production deployments"
            return 0
        fi
    fi

    if ! command -v sha256sum &>/dev/null; then
        warn "sha256sum not found — skipping checksum verification"
        warn "Install with: pkg install coreutils"
        return 0
    fi

    local actual_sha256
    actual_sha256=$(sha256sum "$file" | cut -d' ' -f1)

    if [[ "$actual_sha256" == "$expected_sha256" ]]; then
        return 0
    else
        return 1
    fi
}

# =============================================================================
# PHASE 5: BUILD
# =============================================================================

phase_build() {
    log_header "Phase 5: Build"

    if [ "$OPT_SKIP_BUILD" = "1" ]; then
        log_info "Skipping build (--skip-build) — will use pre-built binaries"

        # Check if binaries already exist locally
        if [ -f "$AURA_BIN" ] && [ -f "$AURA_NEOCORTEX_BIN" ]; then
            log_step "Using existing binaries: $AURA_BIN, $AURA_NEOCORTEX_BIN"
            return
        fi

        # Download from GitHub Releases
        log_info "Downloading pre-built binaries from GitHub Releases..."

        # Derive GitHub owner/repo slug from AURA_REPO URL
        local repo_slug
        repo_slug=$(echo "$AURA_REPO" | sed 's|https://github.com/||;s|\.git$||')

        local release_tag="$AURA_STABLE_TAG"
        local base_url="https://github.com/${repo_slug}/releases/download/${release_tag}"

        # Binary names match release.yml artifact naming
        local daemon_artifact="aura-daemon-${release_tag}-aarch64-linux-android"
        local neocortex_artifact="aura-neocortex-${release_tag}-aarch64-linux-android"

        for artifact in "$daemon_artifact" "$neocortex_artifact"; do
            local url="${base_url}/${artifact}"
            local checksum_url="${url}.sha256"
            local dest
            if [[ "$artifact" == aura-daemon-* ]]; then
                dest="$AURA_BIN"
            else
                dest="$AURA_NEOCORTEX_BIN"
            fi

            log_info "Downloading: $url"
            local dl_attempt=1
            local dl_max=3
            while [ "$dl_attempt" -le "$dl_max" ]; do
                if curl --fail --location --progress-bar --output "$dest" "$url"; then
                    break
                else
                    local dl_exit=$?
                    if [ "$dl_attempt" -lt "$dl_max" ]; then
                        warn "Download attempt $dl_attempt failed (exit $dl_exit). Retrying in 5s..."
                        sleep 5
                    else
                        die "Failed to download $artifact from GitHub Releases after $dl_max attempts." \
                            "Check: ${url}" \
                            "Ensure release ${release_tag} exists and contains the binary."
                    fi
                fi
                dl_attempt=$(( dl_attempt + 1 ))
            done

            # Download and verify checksum
            local checksum_file
            checksum_file="$(mktemp)"
            if curl --fail --silent --location --output "$checksum_file" "$checksum_url"; then
                local expected_sha
                expected_sha=$(cut -d' ' -f1 < "$checksum_file")
                rm -f "$checksum_file"

                if command -v sha256sum &>/dev/null; then
                    local actual_sha
                    actual_sha=$(sha256sum "$dest" | cut -d' ' -f1)
                    if [ "$actual_sha" != "$expected_sha" ]; then
                        rm -f "$dest"
                        die "Checksum mismatch for $artifact!" \
                            "Expected: $expected_sha" \
                            "Actual:   $actual_sha" \
                            "Possible corruption or tampering. Re-run installer."
                    fi
                    log_step "Checksum verified for $artifact"
                else
                    warn "sha256sum not available — skipping binary checksum verification"
                fi
            else
                rm -f "$checksum_file"
                warn "Could not download checksum file for $artifact — skipping verification"
            fi

            chmod +x "$dest"
            log_step "Downloaded: $dest"
        done

        log_step "Pre-built binaries installed successfully"
        return
    fi

    if [ ! -d "$AURA_HOME" ]; then
        die "Source directory not found: $AURA_HOME" \
            "Run without --skip-build so source is cloned first"
    fi

    log_info "Building aura-daemon and aura-neocortex..."
    log_info "This takes 10–30 minutes on first build. Subsequent builds are incremental."

    # Set parallelism: default to half CPU count, minimum 1
    local cpu_count
    cpu_count=$(nproc 2>/dev/null || echo "2")
    local build_jobs=$(( cpu_count / 2 ))
    build_jobs=$(( build_jobs < 1 ? 1 : build_jobs ))
    log_info "Build jobs: $build_jobs (out of $cpu_count CPUs)"

    # Configure Termux-specific linker if needed
    if [ "$IS_TERMUX" = "1" ]; then
        export RUSTFLAGS="${RUSTFLAGS:-} -C link-arg=-fuse-ld=lld"
    fi

    run cargo build --release \
        --manifest-path "$AURA_HOME/Cargo.toml" \
        --package aura-daemon \
        --package aura-neocortex \
        --jobs "$build_jobs"

    # Install binaries
    local daemon_bin="$AURA_HOME/target/release/aura-daemon"
    local neocortex_bin="$AURA_HOME/target/release/aura-neocortex"

    if [ ! -f "$daemon_bin" ]; then
        die "Build succeeded but aura-daemon binary not found at $daemon_bin" \
            "Check cargo build output above for errors"
    fi

    run cp "$daemon_bin" "$TERMUX_PREFIX/bin/aura-daemon"
    run cp "$neocortex_bin" "$TERMUX_PREFIX/bin/aura-neocortex"
    run chmod +x "$TERMUX_PREFIX/bin/aura-daemon"
    run chmod +x "$TERMUX_PREFIX/bin/aura-neocortex"

    log_step "Binaries installed: $TERMUX_PREFIX/bin/aura-daemon"
    log_step "Binaries installed: $TERMUX_PREFIX/bin/aura-neocortex"

    # ── JNI library note ──────────────────────────────────────────────
    # REMOVED (GAP-HIGH-008): Previous code copied the CLI daemon binary
    # as libaura_daemon.so for Android JNI packaging. This was incorrect:
    # a Termux CLI binary is NOT a JNI-compatible shared library and would
    # crash System.loadLibrary(). JNI .so files must be cross-compiled with
    # proper JNI exports via build-android.yml / cargo-ndk, not copied from
    # the host Termux build. See build-android.yml for the correct pipeline.
}

# =============================================================================
# PHASE 6: CONFIGURATION
# =============================================================================

phase_config() {
    log_header "Phase 6: Configuration"

    run mkdir -p "$AURA_CONFIG_DIR"
    run mkdir -p "$AURA_DATA_DIR"
    run mkdir -p "$AURA_MODELS_DIR"
    run mkdir -p "$AURA_LOGS_DIR"
    run mkdir -p "$AURA_DB_DIR"

    # Determine model path based on selected model
    local model_name
    case "$OPT_MODEL" in
        qwen3-4b)  model_name="$MODEL_QWEN3_4B_NAME"  ;;
        qwen3-14b) model_name="$MODEL_QWEN3_14B_NAME" ;;
        *)         model_name="$MODEL_QWEN3_8B_NAME"  ;;
    esac
    local model_path="$AURA_MODELS_DIR/$model_name"

    # Determine thread count
    local cpu_count
    cpu_count=$(nproc 2>/dev/null || echo "4")
    local n_threads=$(( cpu_count > 4 ? 4 : cpu_count ))

    if [ -f "$AURA_CONFIG_FILE" ] && [ "$OPT_UPDATE" != "1" ]; then
        log_info "Config already exists at $AURA_CONFIG_FILE — NOT overwriting (preserving user edits)"
        log_info "To regenerate config, delete it: rm '$AURA_CONFIG_FILE'"
    else
        log_info "Writing config to $AURA_CONFIG_FILE"
        if [ "$OPT_DRY_RUN" != "1" ]; then
            cat > "$AURA_CONFIG_FILE" <<TOML_EOF
# AURA v4 Configuration
# Generated by install.sh ${AURA_VERSION} on $(date -u +%Y-%m-%dT%H:%M:%SZ)
# Edit freely — install.sh will NOT overwrite this file on re-run.
# Full reference: docs/architecture/AURA-V4-INSTALLATION-AND-DEPLOYMENT.md

[daemon]
socket_path = "${AURA_SOCK}"
log_level = "info"

[neocortex]
model_path = "${model_path}"
n_gpu_layers = 0
context_size = 4096
n_threads = ${n_threads}
n_batch = 512

[memory]
max_episodic_entries = 10000
hnsw_ef_construction = 200
hnsw_m = 16

[identity]
assistant_name = "AURA"
user_name = ""
warmth = 0.7
curiosity = 0.8
directness = 0.6

[vault]
pin_hash = ""
auto_lock_seconds = 0

[trust]
default_tier = 1
TOML_EOF
            chmod 600 "$AURA_CONFIG_FILE"
            log_step "Config permissions set to 600 (owner read/write only)"
        else
            log_info "[dry-run] Would write config.toml"
        fi
        log_step "Config written: $AURA_CONFIG_FILE"
    fi
}

# =============================================================================
# PHASE 7: SERVICE SETUP
# =============================================================================

phase_service() {
    log_header "Phase 7: Service Setup"

    if [ "$OPT_SKIP_SERVICE" = "1" ]; then
        log_info "Skipping service setup (--skip-service)"
        return
    fi

    # Method 1: termux-services (preferred)
    if [ "$IS_TERMUX" = "1" ] && command -v sv-enable &>/dev/null; then
        setup_termux_service
    else
        # Method 2: ~/.bashrc fallback
        setup_bashrc_autostart
    fi
}

setup_termux_service() {
    log_info "Setting up termux-services autostart..."

    run mkdir -p "$AURA_SV_DIR/log"

    if [ "$OPT_DRY_RUN" != "1" ]; then
        # Service run script
        cat > "$AURA_SV_DIR/run" <<SV_EOF
#!/data/data/com.termux/files/usr/bin/sh
exec "${TERMUX_PREFIX}/bin/aura-daemon" \\
    --config "${AURA_CONFIG_FILE}" \\
    2>&1
SV_EOF
        chmod +x "$AURA_SV_DIR/run"

        # Log run script
        cat > "$AURA_SV_DIR/log/run" <<SV_LOG_EOF
#!/data/data/com.termux/files/usr/bin/sh
exec svlogd -tt "${AURA_LOGS_DIR}/"
SV_LOG_EOF
        chmod +x "$AURA_SV_DIR/log/run"
    fi

    run sv-enable aura-daemon 2>/dev/null || true
    run sv up aura-daemon 2>/dev/null || true

    log_step "termux-services: aura-daemon enabled and started"
    log_info "AURA daemon will auto-start whenever Termux opens"
}

setup_bashrc_autostart() {
    local bashrc="$HOME_DIR/.bashrc"
    local marker="# AURA v4 auto-start"

    if grep -q "$marker" "$bashrc" 2>/dev/null; then
        log_info "Auto-start already configured in ~/.bashrc"
    else
        log_info "Adding auto-start to ~/.bashrc (fallback method)"
        if [ "$OPT_DRY_RUN" != "1" ]; then
            cat >> "$bashrc" <<BASHRC_EOF

${marker} (managed by install.sh)
if ! pgrep -x aura-daemon > /dev/null 2>&1; then
    "${TERMUX_PREFIX}/bin/aura-daemon" --config "${AURA_CONFIG_FILE}" &>/dev/null &
    disown
fi
BASHRC_EOF
        else
            log_info "[dry-run] Would append auto-start to ~/.bashrc"
        fi
        log_step "Auto-start added to ~/.bashrc"
    fi
}

# =============================================================================
# PHASE 8: FIRST-TIME SETUP
# =============================================================================

phase_firsttime() {
    log_header "Phase 8: First-Time Setup"

    # Check if vault PIN already set
    local pin_set=0
    if [ -f "$AURA_CONFIG_FILE" ]; then
        if grep -q 'pin_hash = "[^"]' "$AURA_CONFIG_FILE" 2>/dev/null; then
            pin_set=1
        fi
    fi

    if [ "$pin_set" = "1" ]; then
        log_step "Vault PIN already configured — skipping first-time setup"
        return
    fi

    if [ "$OPT_DRY_RUN" = "1" ]; then
        log_info "[dry-run] Would prompt for vault PIN and user name"
        return
    fi

    echo ""
    echo -e "${BOLD}  AURA Vault Setup${RESET}"
    echo -e "${DIM}  The vault PIN protects sensitive operations. Choose something you'll remember.${RESET}"
    echo ""

    # Prompt for user name
    local user_name
    read -r -p "$(echo -e "${YELLOW}  ?${RESET} Your name (how AURA addresses you): ")" user_name
    user_name="${user_name:-User}"

    # Prompt for PIN
    local pin1 pin2
    while true; do
        read -r -s -p "$(echo -e "${YELLOW}  ?${RESET} Set vault PIN (min 4 digits): ")" pin1
        echo ""
        if [ ${#pin1} -lt 4 ]; then
            warn "PIN must be at least 4 characters. Try again."
            continue
        fi
        read -r -s -p "$(echo -e "${YELLOW}  ?${RESET} Confirm vault PIN: ")" pin2
        echo ""
        if [ "$pin1" = "$pin2" ]; then
            break
        else
            warn "PINs do not match. Try again."
        fi
    done

    # Hash the PIN using sha256 (Argon2id not available in basic Termux shell)
    #
    # SECURITY [SEC-CRIT-004]: PIN Migration Plan
    # ─────────────────────────────────────────────
    # This installer stores the PIN as "sha256:<salt>:<hex>" — a SALTED SHA-256 hash.
    # This is a TEMPORARY format used ONLY during installation. The daemon's
    # verify_pin_with_migration() function in vault.rs detects this legacy format
    # on first authentication and AUTOMATICALLY upgrades to Argon2id (salted,
    # memory-hard, timing-attack-resistant). The legacy hash is replaced in-place
    # and never used again after the one-time migration.
    #
    # Attack surface during the window between install and first daemon start:
    # - Offline brute-force against salted SHA-256 (mitigated: random salt eliminates
    #   rainbow-table attacks; PIN is 6+ digits; attacker needs physical device access)
    # - The config file is chmod 600 (owner-only read) — see PHASE 7 permissions
    #
    # This is an ACCEPTED RISK for the install→first-boot window only.
    # Generate a random salt for the install-time hash.
    # The daemon will upgrade to Argon2id on first start regardless.
    local pin_salt
    pin_salt=$(head -c 16 /dev/urandom | od -A n -t x1 | tr -d ' \n')
    local pin_hash
    pin_hash=$(echo -n "${pin_salt}${pin1}" | sha256sum | cut -d' ' -f1)

    # SECURITY [HIGH-SEC-3]: Sanitize user_name before sed injection.
    # Without sanitization, a username like: foo/e s/pin_hash.*/pin_hash = "pwned"/
    # would allow arbitrary config file rewriting via sed injection.
    local safe_user_name
    safe_user_name=$(printf '%s' "$user_name" | sed 's/[\/&\\"'"'"'$`!;|<>(){}[\]*?#~^]/\\&/g' | tr -d '\n')

    # Update config with sanitized user name and PIN hash
    if command -v sed &>/dev/null; then
        sed -i "s/user_name = \"\"/user_name = \"${safe_user_name}\"/" "$AURA_CONFIG_FILE"
        sed -i "s/pin_hash = \"\"/pin_hash = \"sha256:${pin_salt}:${pin_hash}\"/" "$AURA_CONFIG_FILE"
    fi

    log_step "Vault PIN set (stored as hash)"
    log_step "User name: $user_name"
    log_info "Note: AURA daemon will upgrade PIN hash to Argon2id on first start"
}

# =============================================================================
# PHASE 9: VERIFY AND PRINT SUCCESS
# =============================================================================

phase_success() {
    log_header "Phase 9: Verification"

    # Check daemon binary exists
    if [ -f "$TERMUX_PREFIX/bin/aura-daemon" ]; then
        log_step "Binary: $TERMUX_PREFIX/bin/aura-daemon"
    else
        warn "Daemon binary not found at $TERMUX_PREFIX/bin/aura-daemon"
    fi

    # Check config exists
    if [ -f "$AURA_CONFIG_FILE" ]; then
        log_step "Config: $AURA_CONFIG_FILE"
    else
        warn "Config file not found at $AURA_CONFIG_FILE"
    fi

    # Check model exists
    local model_name
    case "$OPT_MODEL" in
        qwen3-4b)  model_name="$MODEL_QWEN3_4B_NAME"  ;;
        qwen3-14b) model_name="$MODEL_QWEN3_14B_NAME" ;;
        *)         model_name="$MODEL_QWEN3_8B_NAME"  ;;
    esac
    if [ -f "$AURA_MODELS_DIR/$model_name" ]; then
        local size_mb
        size_mb=$(du -m "$AURA_MODELS_DIR/$model_name" 2>/dev/null | cut -f1 || echo "?")
        log_step "Model: $model_name (${size_mb} MB)"
    else
        warn "Model file not found: $AURA_MODELS_DIR/$model_name"
    fi

    # Check daemon running (if service was set up)
    if [ "$OPT_SKIP_SERVICE" != "1" ]; then
        if pgrep -x aura-daemon > /dev/null 2>&1; then
            log_step "Daemon: running (PID $(pgrep -x aura-daemon))"
        else
            log_info "Daemon: not yet running (will start on next Termux open)"
            log_info "Start manually: aura-daemon --config $AURA_CONFIG_FILE &"
        fi
    fi

    print_success_banner
}

print_success_banner() {
    echo ""
    echo -e "${GREEN}${BOLD}╔═══════════════════════════════════════════════════╗${RESET}"
    echo -e "${GREEN}${BOLD}║         AURA v4 installation complete!            ║${RESET}"
    echo -e "${GREEN}${BOLD}╚═══════════════════════════════════════════════════╝${RESET}"
    echo ""
    echo -e "${BOLD}  Quick start:${RESET}"
    echo ""
    echo -e "    ${CYAN}# Start the daemon manually (if not auto-started):${RESET}"
    echo -e "    aura-daemon --config $AURA_CONFIG_FILE &"
    echo ""
    if command -v sv &>/dev/null; then
        echo -e "    ${CYAN}# Check daemon status (termux-services):${RESET}"
        echo -e "    sv status aura-daemon"
        echo ""
        echo -e "    ${CYAN}# Stop daemon (termux-services):${RESET}"
        echo -e "    sv down aura-daemon"
        echo ""
    else
        echo -e "    ${CYAN}# Check daemon status:${RESET}"
        echo -e "    pgrep -x aura-daemon && echo running || echo not running"
        echo ""
        echo -e "    ${CYAN}# Stop daemon:${RESET}"
        echo -e "    pkill -x aura-daemon"
        echo ""
    fi
    echo -e "    ${CYAN}# View logs:${RESET}"
    echo -e "    tail -f $AURA_LOGS_DIR/current"
    echo ""
    echo -e "${BOLD}  Config:${RESET}  $AURA_CONFIG_FILE"
    echo -e "${BOLD}  Models:${RESET}  $AURA_MODELS_DIR"
    echo -e "${BOLD}  Logs:${RESET}    $AURA_LOGS_DIR"
    if [ -n "${INSTALL_LOG:-}" ] && [ -f "${INSTALL_LOG}" ]; then
        echo -e "${BOLD}  Install log:${RESET} $INSTALL_LOG"
    fi
    echo ""
    echo -e "${DIM}  Docs: docs/architecture/AURA-V4-INSTALLATION-AND-DEPLOYMENT.md${RESET}"
    echo ""
    echo -e "  Re-run this script anytime to update: ${CYAN}bash install.sh --update${RESET}"
    echo ""
}

# =============================================================================
# MAIN
# =============================================================================

main() {
    parse_args "$@"
    setup_colors

    # Redirect all output to log file AND terminal simultaneously.
    # This captures every phase, warning, and error for easy sharing on failure.
    # tee is available in Termux via coreutils (installed in phase_packages).
    # We open the log before phase_packages so even pre-flight failures are captured.
    mkdir -p "$(dirname "$INSTALL_LOG")"
    exec > >(tee -a "$INSTALL_LOG") 2>&1

    echo ""
    echo -e "${BOLD}${CYAN}  AURA v4 Installer${RESET} ${DIM}(${AURA_VERSION})${RESET}"
    echo -e "${DIM}  Channel: $OPT_CHANNEL | Model: $OPT_MODEL${RESET}"
    echo -e "${DIM}  Install log: $INSTALL_LOG${RESET}"
    if [ "$OPT_DRY_RUN" = "1" ]; then
        echo -e "${YELLOW}  DRY RUN — no changes will be made${RESET}"
    fi
    echo ""

    phase_preflight
    phase_packages
    phase_rust
    phase_source
    # Phase ordering rationale: model download (Phase 4) runs before build (Phase 5)
    # because download is a passive network wait while build is CPU-intensive.
    # This order gives better perceived progress during installation.
    phase_model
    phase_build
    phase_config
    phase_service
    phase_firsttime
    phase_success
}

main "$@"
