#!/bin/sh
# get-to-gui-builder.sh -- from a fresh checkout to gitmind's GUI, on THIS machine.
#
# gitmind builds and deploys the apps; depot is where packages live and are
# served (crate/RPM/Nix/toolchain mirrors, and the fleet's own builds). So the
# old "build gitmind in its folder, open the GUI" now starts one step earlier:
#
#   1. check the platform and the build tools;
#   2. ask where things go (answers remembered for the next run);
#   3. build + install depot, `depot init` its data directory, start it;
#   4. build + install gitmind-egui, say how to point it at depot, launch it.
#
# Every step is safe to rerun. Options:
#   --yes           accept every default (the remembered answers, else built-ins)
#   --dry-run       print what would be done; change nothing
#   --no-depot      skip depot (build + launch gitmind-egui only)
#   --no-launch     do not start the GUI at the end
#   --depot-root D  --bin-dir D  --bind ADDR  --run-depot service|background|no
#   -h, --help
set -eu

REPO=$(cd "$(dirname "$0")" && pwd)
CONF="${XDG_CONFIG_HOME:-$HOME/.config}/get-to-gui-builder.conf"
YES=0 DRY=0 DO_DEPOT=1 LAUNCH=1
DEPOT_ROOT_ARG="" BIN_DIR_ARG="" BIND_ARG="" RUN_ARG=""

while [ $# -gt 0 ]; do
    case "$1" in
        --yes) YES=1 ;;
        --dry-run) DRY=1 ;;
        --no-depot) DO_DEPOT=0 ;;
        --no-launch) LAUNCH=0 ;;
        --depot-root) DEPOT_ROOT_ARG=$2; shift ;;
        --bin-dir) BIN_DIR_ARG=$2; shift ;;
        --bind) BIND_ARG=$2; shift ;;
        --run-depot) RUN_ARG=$2; shift ;;
        -h|--help) sed -n '2,23p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
        *) echo "unknown option: $1 (see --help)" >&2; exit 2 ;;
    esac
    shift
done

say()  { printf '\n\033[1m%s\033[0m\n' "$*"; }
note() { printf '  %s\n' "$*"; }
run()  { if [ "$DRY" = 1 ]; then printf '  [dry-run] %s\n' "$*"; else "$@"; fi; }

# ask VAR "question" "default": the answer, or the default under --yes / no tty.
ask() {
    _var=$1 _q=$2 _def=$3
    if [ "$YES" = 1 ] || [ ! -t 0 ]; then
        eval "$_var=\$_def"
        note "$_q $_def"
        return
    fi
    printf '  %s [%s] ' "$_q" "$_def"
    read -r _ans || _ans=""
    [ -n "$_ans" ] || _ans=$_def
    eval "$_var=\$_ans"
}

# ── 1. the platform and the tools ────────────────────────────────────────────
say "1. This machine"
OS=$(uname -s); ARCH=$(uname -m); DISTRO=unknown
[ -r /etc/os-release ] && DISTRO=$(. /etc/os-release && echo "${ID:-unknown}")
note "$OS $ARCH ($DISTRO), repository at $REPO"

# The line to install what a failed build is missing, for this distro.
deps_hint() {
    case "$DISTRO" in
        fedora*|rhel*|centos*) echo "sudo dnf install gcc pkgconf-pkg-config dbus-devel alsa-lib-devel libxkbcommon-devel wayland-devel openssl-devel" ;;
        debian*|ubuntu*|pop*|linuxmint*) echo "sudo apt install build-essential pkg-config libdbus-1-dev libasound2-dev libxkbcommon-dev libwayland-dev libssl-dev" ;;
        alpine*|postmarketos*) echo "sudo apk add build-base pkgconf dbus-dev alsa-lib-dev libxkbcommon-dev wayland-dev openssl-dev" ;;
        arch*|manjaro*) echo "sudo pacman -S base-devel pkgconf dbus alsa-lib libxkbcommon wayland openssl" ;;
        *) if [ "$OS" = Darwin ]; then echo "xcode-select --install; brew install pkg-config"; else echo "(install a C compiler, pkg-config, and the dbus / alsa / xkbcommon / wayland development packages)"; fi ;;
    esac
}

missing=""
command -v cargo >/dev/null 2>&1 || missing="$missing cargo"
command -v cc >/dev/null 2>&1 || command -v gcc >/dev/null 2>&1 || command -v clang >/dev/null 2>&1 || missing="$missing a-C-compiler"
[ "$OS" = Darwin ] || command -v pkg-config >/dev/null 2>&1 || command -v pkgconf >/dev/null 2>&1 || missing="$missing pkg-config"
if [ -n "$missing" ]; then
    note "missing:$missing"
    command -v cargo >/dev/null 2>&1 || note "Rust: https://rustup.rs (then open a new shell)"
    note "build tools: $(deps_hint)"
    [ "$DRY" = 1 ] || exit 1
fi

# A dependency's own rust-toolchain file must not pick the compiler (hound
# pins 1.40, dashmap 1.65/1.85): pin every nested rustc to this checkout's.
if [ -z "${RUSTUP_TOOLCHAIN:-}" ] && command -v rustup >/dev/null 2>&1; then
    tc=$(cd "$REPO" && rustup show active-toolchain 2>/dev/null | awk 'NR==1 {print $1}')
    [ -n "$tc" ] && export RUSTUP_TOOLCHAIN="$tc" && note "toolchain pinned: $tc"
fi

# ── 2. where things go ───────────────────────────────────────────────────────
say "2. Where things go"
[ -r "$CONF" ] && . "$CONF" && note "(defaults from your last run: $CONF)"
: "${SAVED_DEPOT_ROOT:=${XDG_DATA_HOME:-$HOME/.local/share}/depot}"
: "${SAVED_BIN_DIR:=$HOME/.local/bin}"
: "${SAVED_BIND:=0.0.0.0:8000}"
if command -v systemctl >/dev/null 2>&1 && systemctl --user show-environment >/dev/null 2>&1; then
    : "${SAVED_RUN:=service}"
else
    : "${SAVED_RUN:=background}"
fi

BIN_DIR=${BIN_DIR_ARG:-}; [ -n "$BIN_DIR" ] || ask BIN_DIR "Install the binaries to" "$SAVED_BIN_DIR"
if [ "$DO_DEPOT" = 1 ]; then
    DEPOT_ROOT=${DEPOT_ROOT_ARG:-}; [ -n "$DEPOT_ROOT" ] || ask DEPOT_ROOT "depot's data (mirrors and fleet packages -- can grow large) in" "$SAVED_DEPOT_ROOT"
    BIND=${BIND_ARG:-}; [ -n "$BIND" ] || ask BIND "depot listens on" "$SAVED_BIND"
    RUN_DEPOT=${RUN_ARG:-}; [ -n "$RUN_DEPOT" ] || ask RUN_DEPOT "Run depot as: service (starts at login) | background (now only) | no" "$SAVED_RUN"
else
    DEPOT_ROOT=$SAVED_DEPOT_ROOT BIND=$SAVED_BIND RUN_DEPOT=no
fi
case "$RUN_DEPOT" in service|background|no) ;; *) echo "run depot: service, background or no -- not '$RUN_DEPOT'" >&2; exit 2 ;; esac

if [ "$DRY" = 0 ]; then
    mkdir -p "$(dirname "$CONF")"
    {
        echo "# get-to-gui-builder.sh answers, used as defaults next time"
        echo "SAVED_DEPOT_ROOT='$DEPOT_ROOT'"
        echo "SAVED_BIN_DIR='$BIN_DIR'"
        echo "SAVED_BIND='$BIND'"
        echo "SAVED_RUN='$RUN_DEPOT'"
    } > "$CONF"
fi
run mkdir -p "$BIN_DIR"

build() { # build <dir> <cargo args...>: cargo build, with the install line if it fails
    _dir=$1; shift
    note "cargo build $* (in ${_dir#"$REPO"/})"
    if [ "$DRY" = 1 ]; then printf '  [dry-run] (cd %s && cargo build %s)\n' "$_dir" "$*"; return 0; fi
    if ! (cd "$_dir" && cargo build "$@"); then
        say "The build failed."
        note "If a system library was missing, this installs the usual set:"
        note "  $(deps_hint)"
        exit 1
    fi
}

# ── 3. depot ─────────────────────────────────────────────────────────────────
if [ "$DO_DEPOT" = 1 ]; then
    say "3. depot"
    build "$REPO/apps/depot" --release
    run install -m 755 "$REPO/apps/depot/target/release/depot" "$BIN_DIR/depot"
    if [ -f "$DEPOT_ROOT/depot.toml" ]; then
        note "already initialised: $DEPOT_ROOT"
    else
        run mkdir -p "$DEPOT_ROOT"
        run "$BIN_DIR/depot" init --root "$DEPOT_ROOT"
    fi
    # A fresh depot refuses writes until an admin token exists -- and pairing
    # gitmind (so it can publish builds into depot) needs them. Made once; the
    # secret stays in its 0600 file rather than in this terminal's scrollback.
    if [ -f "$DEPOT_ROOT/keys/admin.token" ]; then
        note "writes enabled (admin token: $DEPOT_ROOT/keys/admin.token)"
    else
        ask WRITES "Enable writes now (needed to pair gitmind)? yes/no" "yes"
        if [ "$WRITES" = yes ]; then
            if [ "$DRY" = 1 ]; then printf '  [dry-run] %s admin-token --new --root %s\n' "$BIN_DIR/depot" "$DEPOT_ROOT"
            else "$BIN_DIR/depot" admin-token --new --root "$DEPOT_ROOT" >/dev/null; fi
            note "writes enabled; the admin token is in $DEPOT_ROOT/keys/admin.token (0600) -- it approves pairings"
        else
            note "writes stay off; later: depot admin-token --new --root $DEPOT_ROOT"
        fi
    fi
    case "$RUN_DEPOT" in
        service)
            unit="${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user/depot.service"
            note "user service: $unit"
            if [ "$DRY" = 0 ]; then
                mkdir -p "$(dirname "$unit")"
                cat > "$unit" <<EOF
[Unit]
Description=depot -- offline package mirror and fleet repository
After=network-online.target

[Service]
Environment=DEPOT_ROOT=$DEPOT_ROOT
ExecStart=$BIN_DIR/depot serve --bind $BIND
Restart=on-failure

[Install]
WantedBy=default.target
EOF
            fi
            run systemctl --user daemon-reload
            run systemctl --user enable --now depot.service
            run systemctl --user restart depot.service
            ;;
        background)
            note "started in the background (until logout); log: $DEPOT_ROOT/depot.log"
            if [ "$DRY" = 1 ]; then printf '  [dry-run] DEPOT_ROOT=%s nohup %s serve --bind %s &\n' "$DEPOT_ROOT" "$BIN_DIR/depot" "$BIND"
            else DEPOT_ROOT="$DEPOT_ROOT" nohup "$BIN_DIR/depot" serve --bind "$BIND" >> "$DEPOT_ROOT/depot.log" 2>&1 & fi
            ;;
        no) note "not started: DEPOT_ROOT=$DEPOT_ROOT $BIN_DIR/depot serve --bind $BIND" ;;
    esac
fi

# ── 4. gitmind ───────────────────────────────────────────────────────────────
say "4. gitmind"
build "$REPO/apps/gitmind" --release -p gitmind-egui
run install -m 755 "$REPO/apps/gitmind/target/release/gitmind-egui" "$BIN_DIR/gitmind-egui"

say "Done"
port=${BIND##*:}
if [ "$DO_DEPOT" = 1 ] && [ "$RUN_DEPOT" != no ]; then
    note "depot: http://$(hostname 2>/dev/null || echo localhost):$port/  (manual at /manual)"
    note "in gitmind: pair with that depot (MANUAL.md section 7) so builds can be published to it"
fi
case ":$PATH:" in *":$BIN_DIR:"*) ;; *) note "add $BIN_DIR to your PATH to run depot / gitmind-egui by name" ;; esac
if [ "$LAUNCH" = 1 ]; then
    note "starting gitmind-egui…"
    if [ "$DRY" = 1 ]; then printf '  [dry-run] %s\n' "$BIN_DIR/gitmind-egui"; else exec "$BIN_DIR/gitmind-egui"; fi
fi
