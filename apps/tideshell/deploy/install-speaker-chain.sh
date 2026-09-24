#!/bin/sh
# install-speaker-chain.sh — build bankstown as a valid LV2 bundle, check the
# LADSPA stages exist, and install tideshell's speaker sink.
#
#   ./install-speaker-chain.sh            build, verify, install, restart
#   ./install-speaker-chain.sh --check    verify only, change nothing
#   ./install-speaker-chain.sh --uninstall
#
# WHY NOT THE UPSTREAM SNIPPET
#
# The install script that circulates with bankstown produces a bundle lilv
# cannot load, in three ways, and the symptom is not an error — it is a
# filter chain that silently never appears, so the sink is missing and audio
# goes nowhere:
#
#   1. It copies `libbankstown.so` unrenamed. manifest.ttl declares
#      `lv2:binary <bankstown.so>`, so nothing resolves. Upstream's own
#      Makefile installs it renamed; this follows the Makefile.
#   2. It copies `bankstown.ttl` AS manifest.ttl, and only when manifest.ttl
#      already exists — a condition on the wrong file. The bundle ends up
#      with no manifest and no bankstown.ttl.
#   3. Its WirePlumber rule is 0.4 Lua in main.lua.d, which 0.5 does not
#      read, and it matches `...HiFi__Speaker__Sink` where the real node is
#      `...HiFi__Speaker__sink`. Lowercase s.
#
# This script does not write WirePlumber rules at all. tideshell's sink is an
# ordinary PipeWire sink; choose it with `wpctl set-default` once, which
# WirePlumber then remembers.

set -eu

SUDO=${SUDO-sudo}
PREFIX="$HOME/.lv2/bankstown.lv2"
CONF="${XDG_CONFIG_HOME:-$HOME/.config}/pipewire/pipewire.conf.d/tideshell-speaker.conf"
SRC="${SRC:-$HOME/src/bankstown}"
CHECK_ONLY=0

for a in "$@"; do
    case "$a" in
        --check) CHECK_ONLY=1 ;;
        --uninstall)
            rm -rf "$PREFIX" "$CONF"
            echo "removed $PREFIX and $CONF"
            echo "restart pipewire: systemctl --user restart pipewire pipewire-pulse wireplumber"
            exit 0
            ;;
        -h|--help) sed -n '2,8p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    esac
done

say()  { printf '\033[1;32m[chain]\033[0m %s\n' "$*"; }
warn() { printf '\033[1;33m[warn ]\033[0m %s\n' "$*"; }
die()  { printf '\033[1;31m[error]\033[0m %s\n' "$*" >&2; exit 1; }

export LV2_PATH="$HOME/.lv2:/usr/lib/lv2:/usr/local/lib/lv2"

# ── verify ───────────────────────────────────────────────────────────────────
verify() {
    ok=0
    printf '\n'
    say "LV2 bundle at $PREFIX"
    for f in bankstown.so bankstown.ttl manifest.ttl; do
        if [ -f "$PREFIX/$f" ]; then
            printf '   present  %s\n' "$f"
        else
            printf '   MISSING  %s\n' "$f"
            ok=1
        fi
    done
    # The failure this catches: a bundle whose manifest names a binary that
    # is not there under that name.
    if [ -f "$PREFIX/manifest.ttl" ]; then
        want=$(sed -n 's/.*lv2:binary[[:space:]]*<\([^>]*\)>.*/\1/p' "$PREFIX/manifest.ttl" | head -1)
        if [ -n "$want" ] && [ ! -f "$PREFIX/$want" ]; then
            printf '   BROKEN   manifest names <%s>, which is not in the bundle\n' "$want"
            ok=1
        fi
    fi
    say "does lilv see it"
    if command -v lv2ls >/dev/null 2>&1; then
        if lv2ls 2>/dev/null | grep -q chadmed.au/bankstown; then
            printf '   yes      %s\n' "$(lv2ls | grep chadmed.au/bankstown)"
        else
            printf '   NO       lv2ls does not list it (the bundle is not loadable)\n'
            ok=1
        fi
    else
        warn "lv2ls not installed (apk add lilv-utils) — cannot confirm"
    fi
    say "LADSPA stages"
    if command -v listplugins >/dev/null 2>&1; then
        for want in sc4 fastLookaheadLimiter; do
            if listplugins 2>/dev/null | grep -qi "$want"; then
                printf '   present  %s\n' "$want"
            else
                printf '   MISSING  %s (apk add swh-plugins)\n' "$want"
                ok=1
            fi
        done
    else
        warn "listplugins not installed (apk add ladspa-sdk swh-plugins) — cannot confirm"
    fi
    say "the sink"
    if command -v pw-cli >/dev/null 2>&1 && pw-cli ls Node 2>/dev/null | grep -q tideshell-speaker; then
        printf '   running  tideshell-speaker\n'
    else
        printf '   not up   (install the config and restart pipewire)\n'
        ok=1
    fi
    return $ok
}

if [ "$CHECK_ONLY" = 1 ]; then
    verify && say "everything is in place" || die "see the MISSING/BROKEN lines above"
    exit 0
fi

# ── build ────────────────────────────────────────────────────────────────────
command -v cargo >/dev/null 2>&1 || die "cargo not found (apk add cargo)"
if [ ! -d "$SRC" ]; then
    say "cloning bankstown into $SRC"
    mkdir -p "$(dirname "$SRC")"
    git clone https://github.com/chadmed/bankstown.git "$SRC"
fi
say "building"
( cd "$SRC" && cargo build --release )

[ -f "$SRC/target/release/libbankstown.so" ] || die "build produced no libbankstown.so"

# ── install the bundle, the way upstream's Makefile does ─────────────────────
say "installing the bundle"
mkdir -p "$PREFIX"
install -m0755 "$SRC/target/release/libbankstown.so" "$PREFIX/bankstown.so"   # renamed
install -m0644 "$SRC/bankstown.ttl" "$PREFIX/bankstown.ttl"
install -m0644 "$SRC/manifest.ttl" "$PREFIX/manifest.ttl"                     # the real one

# ── the sink ─────────────────────────────────────────────────────────────────
if [ -n "${TIDESHELL:-}" ] && [ -x "$TIDESHELL" ]; then
    say "writing $CONF from tideshell"
    mkdir -p "$(dirname "$CONF")"
    "$TIDESHELL" speaker-config > "$CONF"
else
    warn "set TIDESHELL=/path/to/tideshell to write $CONF; or move the warmth slider once and tideshell writes it itself"
fi

say "restarting pipewire"
systemctl --user restart pipewire pipewire-pulse wireplumber || warn "could not restart; do it by hand"
sleep 2

verify || warn "not everything is in place yet — see above"
printf '\n'
say "to use it:  wpctl status | grep tideshell-speaker  then  wpctl set-default <id>"
