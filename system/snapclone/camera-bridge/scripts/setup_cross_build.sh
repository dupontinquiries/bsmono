#!/bin/sh
# setup_cross_build.sh — one-shot cross-build setup for Fedora Asahi → FP5 (postmarketOS/musl)
#
# Run this ONCE on your Fedora Asahi machine, then use build.sh for day-to-day builds.
#
# Usage:
#   sh scripts/setup_cross_build.sh
#
# Required:
#   PHONE_IP  — IP address of your FP5 (set in env or edit below)
#   PHONE_USER — SSH user on the phone (default: user)

set -e

# ── Config — edit these or set as environment variables ──────────────────────
PHONE_IP="${PHONE_IP:-}"
PHONE_USER="${PHONE_USER:-user}"
PHONE_SSH_PORT="${PHONE_SSH_PORT:-22}"
SYSROOT="$HOME/.cache/fp5-sysroot"
TARGET="aarch64-unknown-linux-musl"
CARGO_CONFIG=".cargo/config.toml"

GREEN='\033[1;32m'; YELLOW='\033[1;33m'; RED='\033[1;31m'; NC='\033[0m'
ok()   { printf "${GREEN}[ok]${NC}   %s\n" "$*"; }
warn() { printf "${YELLOW}[warn]${NC} %s\n" "$*"; }
fail() { printf "${RED}[fail]${NC} %s\n" "$*" >&2; exit 1; }
info() { printf "[....] %s\n" "$*"; }

# ── 0. Locate rustup — don't require it to be in sudo PATH ───────────────────
# rustup installs to ~/.cargo/bin which isn't in the sudo PATH.
# Find it in the user's home directory directly.
RUSTUP=""
for candidate in \
    "$HOME/.cargo/bin/rustup" \
    "$HOME/.local/bin/rustup" \
    "$(which rustup 2>/dev/null)"
do
    if [ -x "$candidate" ]; then
        RUSTUP="$candidate"
        break
    fi
done

if [ -z "$RUSTUP" ]; then
    fail "rustup not found. Install it: curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh"
fi
ok "rustup found: $RUSTUP"

CARGO="${RUSTUP%rustup}cargo"
ok "cargo: $CARGO"

# ── 1. Install musl-gcc (needs sudo/root, doesn't need rustup) ───────────────
if ! command -v musl-gcc > /dev/null 2>&1; then
    info "Installing musl-gcc..."
    sudo dnf install -y musl-gcc musl-libc-static musl-devel
fi
ok "musl-gcc: $(which musl-gcc)"

# ── 2. Add the musl Rust target (as the normal user, not sudo) ───────────────
info "Adding Rust target: $TARGET"
"$RUSTUP" target add "$TARGET"
ok "Rust target added: $TARGET"

# ── 3. Create sysroot directory before rsyncing into it ──────────────────────
mkdir -p "$SYSROOT/usr/lib" "$SYSROOT/usr/include"
ok "Sysroot directory: $SYSROOT"

# ── 4. Sync GStreamer headers + pkg-config from the phone ────────────────────
if [ -z "$PHONE_IP" ]; then
    warn "PHONE_IP not set — skipping sysroot sync."
    warn "Set PHONE_IP=<phone-ip> and re-run, OR build on the phone directly."
    warn "Without the sysroot, GStreamer crates won't find their C headers."
    warn ""
    warn "Example:"
    warn "  PHONE_IP=192.168.2.214 sh scripts/setup_cross_build.sh"
else
    info "Syncing sysroot from ${PHONE_USER}@${PHONE_IP}..."
    SSH="ssh -p $PHONE_SSH_PORT -o StrictHostKeyChecking=no"
    RSYNC="rsync -az --ignore-missing-args -e \"$SSH\""

    # Headers
    eval "$RSYNC \
        \"${PHONE_USER}@${PHONE_IP}:/usr/include/gstreamer-1.0\" \
        \"${PHONE_USER}@${PHONE_IP}:/usr/include/glib-2.0\" \
        \"${PHONE_USER}@${PHONE_IP}:/usr/include/gobject-introspection-1.0\" \
        \"${PHONE_USER}@${PHONE_IP}:/usr/lib/glib-2.0/include\" \
        \"$SYSROOT/usr/include/\""

    # pkg-config .pc files
    eval "$RSYNC \
        \"${PHONE_USER}@${PHONE_IP}:/usr/lib/pkgconfig/\" \
        \"${PHONE_USER}@${PHONE_IP}:/usr/share/pkgconfig/\" \
        \"$SYSROOT/usr/lib/pkgconfig/\""

    # Shared libraries (for linking)
    eval "$RSYNC \
        \"${PHONE_USER}@${PHONE_IP}:/usr/lib/libgstreamer-1.0.so*\" \
        \"${PHONE_USER}@${PHONE_IP}:/usr/lib/libgstapp-1.0.so*\" \
        \"${PHONE_USER}@${PHONE_IP}:/usr/lib/libgstvideo-1.0.so*\" \
        \"${PHONE_USER}@${PHONE_IP}:/usr/lib/libglib-2.0.so*\" \
        \"${PHONE_USER}@${PHONE_IP}:/usr/lib/libgobject-2.0.so*\" \
        \"${PHONE_USER}@${PHONE_IP}:/usr/lib/libgio-2.0.so*\" \
        \"${PHONE_USER}@${PHONE_IP}:/usr/lib/libpipewire-0.3.so*\" \
        \"$SYSROOT/usr/lib/\""

    ok "Sysroot synced: $SYSROOT"
fi

# ── 5. Write .cargo/config.toml ──────────────────────────────────────────────
mkdir -p ".cargo"

cat > "$CARGO_CONFIG" << TOML
[target.${TARGET}]
linker = "musl-gcc"

[env]
PKG_CONFIG_SYSROOT_DIR = "${SYSROOT}"
PKG_CONFIG_PATH        = "${SYSROOT}/usr/lib/pkgconfig:${SYSROOT}/usr/share/pkgconfig"
PKG_CONFIG_ALLOW_CROSS = "1"
TOML

ok "Wrote $CARGO_CONFIG"

# ── 6. Write the build + deploy script ───────────────────────────────────────
cat > "scripts/build.sh" << BUILD
#!/bin/sh
# build.sh — build for FP5 and optionally deploy
#
# Usage:
#   sh scripts/build.sh              # build only
#   sh scripts/build.sh --deploy     # build + rsync to phone + restart services
#   PHONE_IP=x.x.x.x sh scripts/build.sh --deploy

TARGET="${TARGET}"
PHONE_IP="\${PHONE_IP:-${PHONE_IP}}"
PHONE_USER="\${PHONE_USER:-${PHONE_USER}}"
PHONE_SSH_PORT="\${PHONE_SSH_PORT:-${PHONE_SSH_PORT}}"
SYSROOT="${SYSROOT}"

set -e

GREEN='\033[1;32m'; YELLOW='\033[1;33m'; NC='\033[0m'
ok()   { printf "\${GREEN}[ok]\${NC}   %s\n" "\$*"; }
info() { printf "[....] %s\n" "\$*"; }
warn() { printf "\${YELLOW}[warn]\${NC} %s\n" "\$*"; }

DEPLOY=0
for arg in "\$@"; do
    case "\$arg" in --deploy|-d) DEPLOY=1 ;; esac
done

export PKG_CONFIG_ALLOW_CROSS=1
export PKG_CONFIG_SYSROOT_DIR="\$SYSROOT"
export PKG_CONFIG_PATH="\$SYSROOT/usr/lib/pkgconfig:\$SYSROOT/usr/share/pkgconfig"
export TARGET_CC=musl-gcc

info "Building for \$TARGET..."
START=\$(date +%s)

"${CARGO}" build --release --target "\$TARGET" \\
    --bin camera-bridge \\
    --bin camera-view

END=\$(date +%s)
ok "Build complete in \$((END-START))s"
ls -lh "target/\$TARGET/release/camera-bridge" \\
        "target/\$TARGET/release/camera-view"

if [ "\$DEPLOY" = "1" ]; then
    if [ -z "\$PHONE_IP" ]; then
        warn "PHONE_IP not set — skipping deploy"
        warn "Usage: PHONE_IP=192.168.x.x sh scripts/build.sh --deploy"
        exit 0
    fi
    info "Deploying to \${PHONE_USER}@\${PHONE_IP}..."
    rsync -avz --progress \\
        -e "ssh -p \$PHONE_SSH_PORT" \\
        "target/\$TARGET/release/camera-bridge" \\
        "target/\$TARGET/release/camera-view" \\
        "\${PHONE_USER}@\${PHONE_IP}:\$HOME/.local/bin/"
    ssh -p "\$PHONE_SSH_PORT" "\${PHONE_USER}@\${PHONE_IP}" \\
        'systemctl --user restart camera-bridge camera-view 2>/dev/null; echo "restarted"'
    ok "Deployed and restarted"
fi
BUILD

chmod +x scripts/build.sh
ok "Wrote scripts/build.sh"

# ── 7. Summary ───────────────────────────────────────────────────────────────
echo ""
ok "Setup complete!"
echo ""
echo "Next steps:"
echo ""
if [ -z "$PHONE_IP" ]; then
    echo "  1. Sync the sysroot (needed for GStreamer headers):"
    echo "     PHONE_IP=<your-phone-ip> sh scripts/setup_cross_build.sh"
    echo ""
fi
echo "  Build only:"
echo "    sh scripts/build.sh"
echo ""
echo "  Build + deploy to phone:"
echo "    PHONE_IP=192.168.x.x sh scripts/build.sh --deploy"
echo ""
echo "  Or set PHONE_IP permanently:"
echo "    echo 'export PHONE_IP=192.168.x.x' >> ~/.bashrc"
echo "    sh scripts/build.sh --deploy"
