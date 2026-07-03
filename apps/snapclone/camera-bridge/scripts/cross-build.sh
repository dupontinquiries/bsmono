#!/bin/sh
# cross-build.sh — cross-compile camera-bridge from Fedora Asahi (aarch64/glibc)
#                  to FairPhone 5 (aarch64/musl / postmarketOS)
#
# Both hosts are aarch64, so no cross-compiler is needed.
# The only difference is libc: Fedora uses glibc, postmarketOS uses musl.
# A glibc binary will NOT run on musl — we must link against musl.
#
# Strategy: use musl-cross or a pre-built musl toolchain, point Cargo at it.
#
# PREREQUISITES (run once on your Fedora Asahi machine):
#   sudo dnf install -y musl-gcc    # provides musl-gcc wrapper
#   # OR if musl-gcc isn't packaged:
#   sudo dnf install -y gcc
#   # Then install musl manually — see MANUAL MUSL section below
#
# USAGE:
#   sh scripts/cross-build.sh              # build both binaries
#   sh scripts/cross-build.sh --install    # build + rsync to device
#
# CONFIGURATION (edit these):
DEVICE_HOST="user@192.168.1.xxx"   # SSH target — set to your phone's IP
DEVICE_BIN_DIR="/home/user/.local/bin"
DEVICE_SSH_PORT=22
#
# ─────────────────────────────────────────────────────────────────────────────

set -e
GREEN='\033[1;32m'; YELLOW='\033[1;33m'; RED='\033[1;31m'; NC='\033[0m'
ok()   { printf "${GREEN}[ok]${NC}  %s\n" "$*"; }
warn() { printf "${YELLOW}[warn]${NC} %s\n" "$*"; }
fail() { printf "${RED}[fail]${NC} %s\n" "$*" >&2; exit 1; }
info() { printf "[....] %s\n" "$*"; }

INSTALL=0
for arg in "$@"; do
    case "$arg" in --install|-i) INSTALL=1 ;; esac
done

# ── 1. Locate musl toolchain ──────────────────────────────────────────────────
# We need a C linker that links against musl, not glibc.
# musl-gcc on Fedora wraps the system gcc with a musl sysroot.

find_linker() {
    # Priority: musl-gcc → aarch64-linux-musl-gcc → clang with musl sysroot
    for candidate in \
        musl-gcc \
        aarch64-linux-musl-gcc \
        aarch64-unknown-linux-musl-gcc
    do
        if command -v "$candidate" > /dev/null 2>&1; then
            echo "$candidate"
            return 0
        fi
    done
    return 1
}

LINKER=$(find_linker || true)

if [ -z "$LINKER" ]; then
    warn "No musl linker found. Installing musl-gcc..."
    
    # Try Fedora package first
    if command -v dnf > /dev/null 2>&1; then
        if sudo dnf install -y musl-gcc musl-libc-static 2>/dev/null; then
            LINKER=$(find_linker)
        fi
    fi
    
    # If still not found, offer manual install instructions
    if [ -z "$LINKER" ]; then
        cat << 'EOF'

musl-gcc not found and could not be installed automatically.

Option A — Fedora package (preferred):
  sudo dnf install -y musl-gcc musl-libc-static

Option B — Build musl from source:
  MUSL_VER=1.2.5
  curl -O https://musl.libc.org/releases/musl-${MUSL_VER}.tar.gz
  tar xf musl-${MUSL_VER}.tar.gz && cd musl-${MUSL_VER}
  ./configure --prefix=/usr/local/musl
  make -j$(nproc) && sudo make install
  sudo ln -sf /usr/local/musl/bin/musl-gcc /usr/local/bin/musl-gcc
  cd ..

Option C — Use a Docker/Podman container with alpine:
  podman run --rm -v $PWD:/src -w /src alpine sh -c \
    'apk add rust cargo pkgconf gstreamer-dev && cargo build --release'
  # Then rsync the binary

After installing, re-run this script.
EOF
        exit 1
    fi
fi

ok "Linker: $LINKER ($(which $LINKER))"

# ── 2. Rust target ────────────────────────────────────────────────────────────
TARGET="aarch64-unknown-linux-musl"

if ! rustup target list --installed | grep -q "$TARGET"; then
    info "Installing Rust target: $TARGET"
    rustup target add "$TARGET"
fi
ok "Rust target: $TARGET"

# ── 3. GStreamer / pkg-config for the musl target ────────────────────────────
# GStreamer is a C library. We need pkg-config to find the correct headers
# and libraries for the musl target, not the host glibc ones.
#
# Best approach: get headers from the device via rsync, set PKG_CONFIG_SYSROOT.
#
# If you have a postmarketOS sysroot available (e.g. via pmbootstrap or rsync
# from the device), set SYSROOT below. Otherwise we use a Docker/Podman build.

SYSROOT="${CAMERA_BRIDGE_SYSROOT:-}"  # export this env var to skip auto-setup

setup_sysroot() {
    SYSROOT_DIR="$HOME/.cache/fp5-sysroot"
    
    if [ -d "$SYSROOT_DIR/usr/include/gstreamer-1.0" ]; then
        ok "Sysroot cached at $SYSROOT_DIR"
        echo "$SYSROOT_DIR"
        return 0
    fi
    
    info "Fetching sysroot from device via rsync (this takes ~2 min, only once)..."
    info "Target: $DEVICE_HOST"
    
    mkdir -p "$SYSROOT_DIR"
    
    # Sync just the headers and libraries we need
    for dir in \
        /usr/include \
        /usr/lib/pkgconfig \
        /usr/lib/libgstreamer* \
        /usr/lib/libgst* \
        /usr/lib/libglib* \
        /usr/lib/libgobject* \
        /usr/lib/libgio* \
        /usr/lib/libpipewire* \
        /lib/ld-musl-aarch64.so.1
    do
        rsync -az --ignore-missing-args \
            -e "ssh -p $DEVICE_SSH_PORT" \
            "${DEVICE_HOST}:${dir}" \
            "${SYSROOT_DIR}$(dirname $dir)/" 2>/dev/null || true
    done
    
    ok "Sysroot synced to $SYSROOT_DIR"
    echo "$SYSROOT_DIR"
}

if [ -z "$SYSROOT" ] && [ "$INSTALL" = "1" ] || [ -n "$DEVICE_HOST" ]; then
    SYSROOT=$(setup_sysroot)
fi

# ── 4. Write .cargo/config.toml ───────────────────────────────────────────────
mkdir -p .cargo
cat > .cargo/config.toml << TOML
[target.aarch64-unknown-linux-musl]
linker = "${LINKER}"

[env]
# Tell pkg-config to look in the musl sysroot for library metadata.
PKG_CONFIG_SYSROOT_DIR = "${SYSROOT:-/}"
PKG_CONFIG_PATH = "${SYSROOT:-}/usr/lib/pkgconfig:${SYSROOT:-}/usr/share/pkgconfig"
PKG_CONFIG_ALLOW_CROSS = "1"
TOML

ok "Wrote .cargo/config.toml"

# ── 5. Environment for pkg-config ─────────────────────────────────────────────
export PKG_CONFIG_ALLOW_CROSS=1
export PKG_CONFIG_SYSROOT_DIR="${SYSROOT:-/}"
export PKG_CONFIG_PATH="${SYSROOT:-}/usr/lib/pkgconfig:${SYSROOT:-}/usr/share/pkgconfig"
export TARGET_CC="$LINKER"

# GStreamer crates use pkg-config. If the sysroot has .pc files, this works.
# If not, we fall back to static linking hints.

# ── 6. Build ──────────────────────────────────────────────────────────────────
info "Building for $TARGET..."
BUILD_START=$(date +%s)

cargo build \
    --release \
    --target "$TARGET" \
    --bin camera-bridge \
    --bin camera-view

BUILD_END=$(date +%s)
BUILD_SECS=$((BUILD_END - BUILD_START))
ok "Build complete in ${BUILD_SECS}s"

ls -lh \
    "target/${TARGET}/release/camera-bridge" \
    "target/${TARGET}/release/camera-view"

# ── 7. Install to device ──────────────────────────────────────────────────────
if [ "$INSTALL" = "1" ]; then
    if [ "$DEVICE_HOST" = "user@192.168.1.xxx" ]; then
        warn "DEVICE_HOST not configured — edit cross-build.sh before using --install"
        exit 0
    fi
    
    info "Installing to ${DEVICE_HOST}:${DEVICE_BIN_DIR}/"
    
    rsync -avz --progress \
        -e "ssh -p $DEVICE_SSH_PORT" \
        "target/${TARGET}/release/camera-bridge" \
        "target/${TARGET}/release/camera-view" \
        "${DEVICE_HOST}:${DEVICE_BIN_DIR}/"
    
    ok "Installed. Restarting services on device..."
    ssh -p "$DEVICE_SSH_PORT" "$DEVICE_HOST" \
        'systemctl --user restart camera-bridge camera-view 2>/dev/null; echo "Services restarted"'
    
    ok "Done. Check status:"
    info "  ssh ${DEVICE_HOST} journalctl --user -u camera-bridge -n 20"
fi
