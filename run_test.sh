#!/bin/bash
# Steel Capture — Test Harness
#
# Usage:
#   ./run_test.sh              # Native GUI (default)
#   ./run_test.sh --browser    # Browser viz via WebSocket
#   ./run_test.sh --both       # Native GUI + browser viz
#   ./run_test.sh --skip-build # Run without rebuilding

set -e

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
cd "$SCRIPT_DIR"

BINARY="target/release/steel-capture"
WS_PORT=8080
MODE="gui"
SKIP_BUILD=0

for arg in "$@"; do
    case $arg in
        --browser)    MODE="browser" ;;
        --both)       MODE="both" ;;
        --skip-build) SKIP_BUILD=1 ;;
    esac
done

echo "═══════════════════════════════════════════════"
echo "  STEEL CAPTURE — Test Harness"
echo "═══════════════════════════════════════════════"

# ─── Build ──────────────────────────────────────────────
if [ "$SKIP_BUILD" != "1" ]; then
    echo ""
    if [ "$MODE" = "browser" ]; then
        echo "Building (release, no GUI)..."
        cargo build --release --no-default-features 2>&1 | grep -E "(Compiling steel|Finished|error)"
    else
        echo "Building (release, with native GUI)..."
        cargo build --release 2>&1 | grep -E "(Compiling steel|Finished|error)"
    fi
    echo "Build complete."
fi

if [ ! -f "$BINARY" ]; then
    echo "ERROR: Binary not found at $BINARY"
    exit 1
fi

# ─── Start ──────────────────────────────────────────────
echo ""
case $MODE in
    gui)
        echo "Starting simulator + native GUI..."
        echo "  Close the window to exit."
        RUST_LOG=info exec "$BINARY" --simulate
        ;;
    browser)
        if lsof -i :$WS_PORT > /dev/null 2>&1; then
            kill $(lsof -t -i :$WS_PORT) 2>/dev/null || true
            sleep 1
        fi
        echo "Starting simulator + WebSocket on ws://localhost:$WS_PORT..."
        RUST_LOG=info "$BINARY" --simulate --no-gui --ws --ws-addr "0.0.0.0:$WS_PORT" &
        PID=$!
        sleep 1
        echo "Opening http://localhost:$WS_PORT..."
        command -v open > /dev/null 2>&1 && open "http://localhost:$WS_PORT" || \
        command -v xdg-open > /dev/null 2>&1 && xdg-open "http://localhost:$WS_PORT" || \
        echo "Open http://localhost:$WS_PORT in your browser."
        echo "Press Ctrl+C to stop."
        trap "kill $PID 2>/dev/null; exit 0" INT TERM
        wait $PID
        ;;
    both)
        if lsof -i :$WS_PORT > /dev/null 2>&1; then
            kill $(lsof -t -i :$WS_PORT) 2>/dev/null || true
            sleep 1
        fi
        echo "Starting simulator + GUI + WebSocket on ws://localhost:$WS_PORT..."
        echo "  Native window AND http://localhost:$WS_PORT"
        RUST_LOG=info exec "$BINARY" --simulate --ws --ws-addr "0.0.0.0:$WS_PORT"
        ;;
esac
