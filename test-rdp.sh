#!/usr/bin/env bash
# cosmic-ext-rdp-server manual testing script
# Run from a COSMIC desktop terminal (needs Wayland + D-Bus session)
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SERVER="$SCRIPT_DIR/target/release/cosmic-ext-rdp-server"
SETTINGS="$SCRIPT_DIR/target/release/cosmic-ext-rdp-settings"
PORT="${1:-3389}"

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
CYAN='\033[0;36m'
NC='\033[0m'

info()  { echo -e "${CYAN}[INFO]${NC} $*"; }
ok()    { echo -e "${GREEN}[PASS]${NC} $*"; }
warn()  { echo -e "${YELLOW}[WARN]${NC} $*"; }
fail()  { echo -e "${RED}[FAIL]${NC} $*"; }

check_prereqs() {
    echo "============================================"
    echo " cosmic-ext-rdp-server Test Suite"
    echo "============================================"
    echo ""

    info "Checking prerequisites..."

    if [[ -z "${WAYLAND_DISPLAY:-}" ]]; then
        fail "WAYLAND_DISPLAY not set. Run this from a COSMIC desktop terminal."
        exit 1
    fi
    ok "Wayland session: $WAYLAND_DISPLAY"

    if pgrep -x cosmic-comp > /dev/null 2>&1; then
        ok "cosmic-comp is running"
    else
        warn "cosmic-comp not running (live capture may not work)"
    fi

    if [[ -n "${DBUS_SESSION_BUS_ADDRESS:-}" ]]; then
        ok "D-Bus session bus available"
    else
        fail "DBUS_SESSION_BUS_ADDRESS not set"
        exit 1
    fi

    if pactl info > /dev/null 2>&1; then
        ok "PipeWire/PulseAudio available"
    else
        warn "PipeWire not available (audio forwarding will fail)"
    fi

    if [[ ! -x "$SERVER" ]]; then
        fail "Server binary not found: $SERVER"
        echo "  Run: nix develop --command cargo build --release"
        exit 1
    fi
    ok "Server binary: $SERVER"

    if command -v xfreerdp > /dev/null 2>&1; then
        ok "xfreerdp client available"
    elif command -v wlfreerdp > /dev/null 2>&1; then
        ok "wlfreerdp client available"
    else
        warn "No FreeRDP client found (install freerdp3 to test)"
    fi
    echo ""
}

test_static() {
    echo "============================================"
    echo " Test 1: Static Display Mode"
    echo "============================================"
    info "Starting server on port $PORT with static blue screen..."

    "$SERVER" --static-display --port "$PORT" &
    local pid=$!
    sleep 2

    if kill -0 "$pid" 2>/dev/null; then
        ok "Server started (PID $pid)"
    else
        fail "Server failed to start"
        return 1
    fi

    echo ""
    info "Connect with an RDP client now:"
    echo ""
    echo "  xfreerdp /v:localhost /port:$PORT /cert:ignore"
    echo "  -- or --"
    echo "  wlfreerdp /v:localhost /port:$PORT /cert:ignore"
    echo ""
    echo "  You should see a blue screen."
    echo ""
    read -rp "Press Enter when done testing (server will stop)..."

    kill "$pid" 2>/dev/null && wait "$pid" 2>/dev/null || true
    ok "Static display test complete"
    echo ""
}

test_live() {
    echo "============================================"
    echo " Test 2: Live Screen Capture"
    echo "============================================"
    info "Starting server on port $PORT with live capture..."
    info "The ScreenCast portal will ask for permission."

    "$SERVER" --port "$PORT" &
    local pid=$!
    sleep 2

    if kill -0 "$pid" 2>/dev/null; then
        ok "Server started (PID $pid)"
    else
        fail "Server failed to start (check logs above)"
        return 1
    fi

    echo ""
    info "Connect with an RDP client:"
    echo ""
    echo "  xfreerdp /v:localhost /port:$PORT /cert:ignore"
    echo ""
    echo "  You should see your live desktop."
    echo "  Try:"
    echo "    - Moving windows around (verify smooth capture)"
    echo "    - Moving the mouse in the RDP window (verify input injection)"
    echo "    - Typing text (verify keyboard input)"
    echo "    - Copying text locally, then pasting in RDP (verify clipboard)"
    echo ""
    read -rp "Press Enter when done testing (server will stop)..."

    kill "$pid" 2>/dev/null && wait "$pid" 2>/dev/null || true
    ok "Live capture test complete"
    echo ""
}

test_config() {
    echo "============================================"
    echo " Test 3: Config File"
    echo "============================================"
    local config_dir="${XDG_CONFIG_HOME:-$HOME/.config}/cosmic-ext-rdp-server"
    local config_file="$config_dir/config.toml"

    if [[ -f "$config_file" ]]; then
        info "Existing config: $config_file"
        cat "$config_file"
    else
        info "No config file yet. Creating example..."
        mkdir -p "$config_dir"
        cat > "$config_file" << 'TOML'
bind = "0.0.0.0:3389"
static_display = false

[auth]
enable = false

[capture]
fps = 30
channel_capacity = 4
multi_monitor = false

[encode]
encoder = "auto"
preset = "ultrafast"
bitrate = 10000000

[clipboard]
enable = true

[audio]
enable = true
sample_rate = 44100
channels = 2
TOML
        ok "Config written to $config_file"
    fi

    info "Starting server with config file..."
    "$SERVER" --config "$config_file" &
    local pid=$!
    sleep 2

    if kill -0 "$pid" 2>/dev/null; then
        ok "Server started from config (PID $pid)"
    else
        fail "Server failed with config"
        return 1
    fi

    echo ""
    read -rp "Press Enter to stop..."
    kill "$pid" 2>/dev/null && wait "$pid" 2>/dev/null || true
    ok "Config test complete"
    echo ""
}

test_dbus() {
    echo "============================================"
    echo " Test 4: D-Bus Interface"
    echo "============================================"
    info "Starting server for D-Bus testing..."

    "$SERVER" --static-display --port "$PORT" &
    local pid=$!
    sleep 2

    if ! kill -0 "$pid" 2>/dev/null; then
        fail "Server failed to start"
        return 1
    fi

    info "Introspecting D-Bus interface..."
    if busctl --user introspect io.github.olafkfreund.CosmicExtRdpServer /io/github/olafkfreund/CosmicExtRdpServer 2>/dev/null; then
        ok "D-Bus interface accessible"
    else
        fail "D-Bus interface not found"
    fi

    info "Checking properties..."
    busctl --user get-property io.github.olafkfreund.CosmicExtRdpServer /io/github/olafkfreund/CosmicExtRdpServer io.github.olafkfreund.CosmicExtRdpServer Running 2>/dev/null && ok "Running property works" || fail "Running property failed"
    busctl --user get-property io.github.olafkfreund.CosmicExtRdpServer /io/github/olafkfreund/CosmicExtRdpServer io.github.olafkfreund.CosmicExtRdpServer BoundAddress 2>/dev/null && ok "BoundAddress property works" || fail "BoundAddress property failed"

    info "Testing Reload method..."
    busctl --user call io.github.olafkfreund.CosmicExtRdpServer /io/github/olafkfreund/CosmicExtRdpServer io.github.olafkfreund.CosmicExtRdpServer Reload 2>/dev/null && ok "Reload works" || fail "Reload failed"

    info "Testing Stop method..."
    busctl --user call io.github.olafkfreund.CosmicExtRdpServer /io/github/olafkfreund/CosmicExtRdpServer io.github.olafkfreund.CosmicExtRdpServer Stop 2>/dev/null && ok "Stop works" || fail "Stop failed"

    sleep 1
    if kill -0 "$pid" 2>/dev/null; then
        warn "Server still running after Stop, killing..."
        kill "$pid" 2>/dev/null && wait "$pid" 2>/dev/null || true
    else
        ok "Server stopped via D-Bus"
    fi
    echo ""
}

test_settings_gui() {
    echo "============================================"
    echo " Test 5: Settings GUI"
    echo "============================================"
    if [[ ! -x "$SETTINGS" ]]; then
        warn "Settings binary not found: $SETTINGS"
        return 0
    fi

    info "Launching cosmic-ext-rdp-settings..."
    info "(Close the window when done testing)"
    "$SETTINGS" 2>&1 || true
    ok "Settings GUI test complete"
    echo ""
}

# --- Main ---
check_prereqs

echo "Which tests to run?"
echo "  1) Static display (blue screen)"
echo "  2) Live capture (needs COSMIC session)"
echo "  3) Config file loading"
echo "  4) D-Bus interface"
echo "  5) Settings GUI"
echo "  a) All tests"
echo "  q) Quit"
echo ""
read -rp "Choice [1-5/a/q]: " choice

case "$choice" in
    1) test_static ;;
    2) test_live ;;
    3) test_config ;;
    4) test_dbus ;;
    5) test_settings_gui ;;
    a)
        test_static
        test_dbus
        test_config
        test_live
        test_settings_gui
        ;;
    q) echo "Bye!" ;;
    *) echo "Invalid choice" ;;
esac

echo ""
echo "============================================"
echo " Testing complete!"
echo "============================================"
