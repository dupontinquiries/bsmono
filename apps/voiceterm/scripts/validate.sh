#!/usr/bin/env bash
# scripts/validate.sh — pre-ship validation for web2qml.
#
# Run before publishing a zip. Catches the gap that bit
# v0.1.386 → v0.1.392: brace-check + qt_struct + qml_lint are
# all syntactic, none of them compile. A missing crate dep
# (`dirs` in v0.1.386) made it through 7 ships before the
# user hit `error[E0433]` on hardware.
#
# Order is fastest-first so the cheap checks fail fast:
#   1. Brace balancing across all src/*.rs (<1s)
#   2. qt_struct invariants (<1s)
#   3. qml_lint (<1s)
#   4. cargo check (~10-60s depending on cache)
#
# Exits 0 on success, non-zero on any failure. Prints what
# failed and where.

set -u  # NOT -e — we want to keep going and report all failures
cd "$(dirname "$0")/.."

FAIL=0
log() { printf "\n\033[1;36m== %s ==\033[0m\n" "$1"; }
ok()  { printf "\033[1;32m✓ %s\033[0m\n" "$1"; }
err() { printf "\033[1;31m✗ %s\033[0m\n" "$1"; FAIL=1; }

# --- Step 1: Rust brace balance ---
log "Rust brace check"
if [ -f /tmp/rust_brace_check.py ]; then
    BRACE=/tmp/rust_brace_check.py
elif [ -f scripts/rust_brace_check.py ]; then
    BRACE=scripts/rust_brace_check.py
else
    err "rust_brace_check.py not found (neither /tmp/ nor scripts/)"
    BRACE=""
fi
if [ -n "$BRACE" ]; then
    if python3 "$BRACE" src/*.rs; then
        ok "brace check passed"
    else
        err "brace check FAILED"
    fi
fi

# --- Step 2: qt_struct invariants ---
log "qt_struct invariants"
if python3 scripts/qt_struct_check.py src/main.rs; then
    ok "qt_struct passed"
else
    err "qt_struct FAILED"
fi

# --- Step 3: QML lint ---
log "QML lint"
if python3 scripts/qml_lint.py; then
    ok "qml_lint passed"
else
    err "qml_lint FAILED"
fi

# --- Step 3b: QML compile check (v0.1.851) ---
# qmlcachegen is the real QML compiler — it catches load-fatal errors qml_lint
# misses, above all "Property value set multiple times" (a property/handler bound
# twice on one object), which the engine rejects with a blank screen at startup.
log "QML compile check (qmlcachegen)"
if python3 scripts/qml_check.py; then
    ok "qml_check passed"
else
    err "qml_check FAILED"
fi

# --- Step 4: cargo check ---
# This is the one that catches missing deps, type errors,
# unresolved imports, Cargo.toml inconsistencies. None of
# the syntactic tools above will catch these.
#
# Use the same feature flags as the user's typical build:
#   --features="voice claude"
# That matches their `cargo r --release --features=...` command.
log "cargo check (--features='voice claude')"
if ! command -v cargo >/dev/null 2>&1; then
    err "cargo not in PATH — install rustup first"
else
    if cargo check --features="voice claude" 2>&1 | tail -40; then
        # cargo check returns 0 on success; pipe inverts that,
        # so check PIPESTATUS instead
        if [ "${PIPESTATUS[0]}" -eq 0 ]; then
            ok "cargo check passed"
        else
            err "cargo check FAILED (see output above)"
        fi
    else
        err "cargo check FAILED to run"
    fi
fi

# --- Step 5: golden output check (v0.1.890) ---
# The behavior safety net. Compiles the fixture corpus through
# generate_safe_qml and diffs byte-for-byte against committed goldens —
# catches OUTPUT drift the compile + syntactic checks above can't see, AND
# (since the harness build is exercised here) catches the harness itself
# silently failing to build, which is what let v0.1.884→888 drift unnoticed.
# Staging build is cached under /tmp, so it's cold only on the first run of a
# session (~60s) and ~5s warm. Set WEB2QML_SKIP_GOLDEN=1 to skip during fast
# iteration (prints a visible notice — never silent).
log "golden output check"
if [ "${WEB2QML_SKIP_GOLDEN:-0}" = "1" ]; then
    printf "\033[1;33m• skipped (WEB2QML_SKIP_GOLDEN=1)\033[0m\n"
elif [ ! -f scripts/qml_golden.sh ]; then
    err "qml_golden.sh not found"
else
    if bash scripts/qml_golden.sh > /tmp/golden_gate.log 2>&1; then
        ok "golden output matches"
    else
        err "golden output DIFF or harness build failure (see below)"
        tail -25 /tmp/golden_gate.log
    fi
fi

# --- Optional: cargo test (commented out by default — slower) ---
# Uncomment to run the test suite as part of validation.
# log "cargo test"
# if cargo test --features="voice claude" --lib 2>&1 | tail -20; then
#     if [ "${PIPESTATUS[0]}" -eq 0 ]; then
#         ok "cargo test passed"
#     else
#         err "cargo test FAILED"
#     fi
# fi

# --- Summary ---
echo
if [ $FAIL -eq 0 ]; then
    printf "\033[1;32m═══ ALL VALIDATION PASSED ═══\033[0m\n"
    exit 0
else
    printf "\033[1;31m═══ VALIDATION FAILED — DO NOT SHIP ═══\033[0m\n"
    exit 1
fi
