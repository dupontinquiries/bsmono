#!/usr/bin/env bash
# ============================================================
# RustyBoard — sanity-check.sh
#
# Run this when "rusty-board doesn't appear in System Settings"
# or "KWin isn't launching us." Walks the registration chain
# and reports what's wrong.
#
# Doesn't modify anything. Just diagnoses.
# ============================================================
set -uo pipefail  # no -e because we want to keep checking after failures

DESKTOP_NAME="io.rustyboard.Keyboard.desktop"
SYSTEM_DESKTOP="/usr/share/applications/$DESKTOP_NAME"
USER_DESKTOP="$HOME/.local/share/applications/$DESKTOP_NAME"

PASS=()
FAIL=()
WARN=()

ok()    { PASS+=("$1"); echo "[OK]   $1"; }
fail()  { FAIL+=("$1"); echo "[FAIL] $1"; }
warn()  { WARN+=("$1"); echo "[WARN] $1"; }

echo "=== RustyBoard sanity check ==="
echo

# 1. Binary present?
echo "--- 1. Binary ---"
for path in /usr/local/bin/rusty-board $HOME/.local/bin/rusty-board; do
    if [[ -x "$path" ]]; then
        ok "Binary at $path is executable"
        BINARY_PATH="$path"
    fi
done
if [[ -z "${BINARY_PATH:-}" ]]; then
    fail "No rusty-board binary found in /usr/local/bin or ~/.local/bin"
    echo "    Try: cargo b --release && sudo install -m755 target/release/rusty-board /usr/local/bin/"
fi
echo

# 2. .desktop file present and has the magic line?
echo "--- 2. Desktop file registration ---"
for path in "$SYSTEM_DESKTOP" "$USER_DESKTOP"; do
    if [[ -f "$path" ]]; then
        if grep -q '^X-KDE-Wayland-VirtualKeyboard=true' "$path"; then
            ok "$path has X-KDE-Wayland-VirtualKeyboard=true"
        else
            fail "$path EXISTS but is MISSING X-KDE-Wayland-VirtualKeyboard=true"
            echo "    KWin will filter rusty-board out of the virtual-keyboard list."
            echo "    Fix: re-run ./scripts/plasma-setup.sh"
        fi
        # Verify Exec= points to an actual binary
        EXEC_LINE=$(grep '^Exec=' "$path" | head -1 | sed 's/^Exec=//')
        if [[ -n "$EXEC_LINE" ]]; then
            # Take just the binary path (no args).
            EXEC_BIN=$(echo "$EXEC_LINE" | awk '{print $1}')
            if [[ -x "$EXEC_BIN" ]]; then
                ok "Exec= in $path points to $EXEC_BIN (exists)"
            else
                fail "Exec= in $path points to $EXEC_BIN but that file isn't executable"
            fi
        else
            fail "$path has no Exec= line"
        fi
    fi
done
# Shadowing check: if both exist, warn about user-level shadowing.
if [[ -f "$USER_DESKTOP" && -f "$SYSTEM_DESKTOP" ]]; then
    if grep -q '^X-KDE-Wayland-VirtualKeyboard=true' "$SYSTEM_DESKTOP"; then
        if ! grep -q '^X-KDE-Wayland-VirtualKeyboard=true' "$USER_DESKTOP"; then
            fail "User-level $USER_DESKTOP SHADOWS system-level $SYSTEM_DESKTOP"
            echo "    The user-level file lacks the magic line, so KWin uses the WRONG one."
            echo "    Fix: rm '$USER_DESKTOP' && ./scripts/plasma-setup.sh"
        fi
    fi
fi
echo

# 3. kwinrc points to it?
echo "--- 3. kwinrc input-method config ---"
for kwinrc in "$HOME/.config/plasma-mobile/kwinrc" "$HOME/.config/kwinrc"; do
    if [[ -f "$kwinrc" ]]; then
        INPUT_METHOD=$(awk '/^\[Wayland\]/{flag=1; next} /^\[/{flag=0} flag && /^InputMethod=/{print; exit}' "$kwinrc" | sed 's/^InputMethod=//')
        if [[ -n "$INPUT_METHOD" ]]; then
            if [[ "$INPUT_METHOD" == *"rusty-board"* || "$INPUT_METHOD" == *"rustyboard"* ]]; then
                ok "$kwinrc: [Wayland] InputMethod=$INPUT_METHOD"
            else
                warn "$kwinrc: [Wayland] InputMethod=$INPUT_METHOD (not rusty-board)"
                echo "    Maybe you're using a different keyboard, or kwinrc wasn't updated by plasma-setup.sh."
            fi
            # Verify the path it points at exists
            if [[ -f "$INPUT_METHOD" ]]; then
                ok "InputMethod target file exists"
            else
                fail "InputMethod points at $INPUT_METHOD but that file doesn't exist"
            fi
        else
            warn "$kwinrc exists but has no [Wayland] InputMethod= line"
        fi
    fi
done
echo

# 4. KWin running?
echo "--- 4. KWin process ---"
if pgrep -x kwin_wayland >/dev/null; then
    ok "kwin_wayland is running"
elif pgrep -x kwin >/dev/null; then
    ok "kwin is running"
else
    warn "No KWin process found. Are you on a non-KDE session?"
fi
if command -v qdbus6 >/dev/null; then
    if qdbus6 org.kde.KWin /KWin reconfigure >/dev/null 2>&1; then
        ok "qdbus6 can reach KWin"
    else
        warn "qdbus6 present but org.kde.KWin reconfigure call failed"
    fi
elif command -v qdbus >/dev/null; then
    if qdbus org.kde.KWin /KWin reconfigure >/dev/null 2>&1; then
        ok "qdbus can reach KWin"
    else
        warn "qdbus present but org.kde.KWin reconfigure call failed"
    fi
fi
echo

# 5. Log file recent?
echo "--- 5. Log file ---"
LOG_FILE="/run/user/$(id -u)/rusty-board.log"
if [[ -f "$LOG_FILE" ]]; then
    LOG_AGE=$(( $(date +%s) - $(stat -c %Y "$LOG_FILE") ))
    if [[ $LOG_AGE -lt 600 ]]; then
        ok "Log file $LOG_FILE was modified $LOG_AGE seconds ago"
    else
        warn "Log file is $LOG_AGE seconds old — KWin hasn't launched rusty-board recently."
        echo "    Try focusing a text input to trigger keyboard summon."
    fi
    LAST_INIT=$(grep -n "Starting RustyBoard" "$LOG_FILE" | tail -1)
    if [[ -n "$LAST_INIT" ]]; then
        echo "    Last startup: $LAST_INIT"
    fi
else
    warn "No log file at $LOG_FILE — rusty-board has never run or was never launched by KWin."
fi
echo

# Summary
echo "=== Summary ==="
echo "  ${#PASS[@]} OK"
echo "  ${#WARN[@]} WARN"
echo "  ${#FAIL[@]} FAIL"
echo

if [[ ${#FAIL[@]} -gt 0 ]]; then
    echo "Action items:"
    for msg in "${FAIL[@]}"; do
        echo "  - $msg"
    done
    echo
    echo "Quick fix: try ./scripts/fix-registration.sh"
    echo "  Removes the shadowing user-level file and re-runs plasma-setup.sh."
    exit 1
fi

if [[ ${#WARN[@]} -gt 0 ]]; then
    echo "Notes:"
    for msg in "${WARN[@]}"; do
        echo "  - $msg"
    done
fi

echo "Sanity check complete."
