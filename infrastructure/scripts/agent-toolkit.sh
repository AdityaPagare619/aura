#!/bin/bash
# AURA Agent Toolkit — Shell wrapper
# Provides convenient CLI access to the Python toolkit

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PYTHON_SCRIPT="$SCRIPT_DIR/agent_toolkit"

# Ensure Python 3
if ! command -v python3 &> /dev/null; then
    echo "Error: python3 not found"
    exit 1
fi

# Add script directory to Python path
export PYTHONPATH="$PYTHONPATH:$SCRIPT_DIR"

# Parse command
case "${1:-}" in
    ci)
        shift
        python3 -m agent_toolkit.ci "$@"
        ;;
    browserstack|bs)
        shift
        python3 -m agent_toolkit.browserstack "$@"
        ;;
    classify|taxonomy)
        shift
        python3 -m agent_toolkit.taxonomy "$@"
        ;;
    "")
        echo "AURA Agent Toolkit"
        echo ""
        echo "Usage:"
        echo "  $0 ci <command>       GitHub Actions CI control"
        echo "  $0 browserstack <cmd> BrowserStack device testing"
        echo "  $0 classify <text>    Classify a failure"
        echo ""
        echo "Examples:"
        echo "  $0 ci trigger --workflow aura-android-validate.yml"
        echo "  $0 ci status --workflow aura-android-validate.yml"
        echo "  $0 bs list-devices"
        echo "  $0 classify SIGSEGV signal 11"
        ;;
    *)
        echo "Unknown command: $1"
        echo "Run '$0' for usage"
        exit 1
        ;;
esac
