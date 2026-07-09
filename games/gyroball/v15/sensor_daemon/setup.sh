#!/bin/sh
# setup.sh — build tilt_sensor on PostmarketOS / Alpine Linux
#
# Run once from the sensor_daemon/ directory:
#   sh setup.sh
#
# What it does:
#   1. Installs build deps via apk (skips any already installed)
#   2. Clones libiio as a submodule if not already present
#   3. Builds libiio statically
#   4. Builds tilt_sensor linked against it
#   5. Falls back to zero-dep sysfs build if anything fails

set -e

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
cd "$SCRIPT_DIR"

echo "=== tilt_sensor setup ==="

# ── 1. Dependencies ────────────────────────────────────────────────────────────
echo "--- Installing build deps (apk) ---"
# libudev-zero is the Alpine/postmarketOS udev replacement
apk add --no-cache \
    gcc make cmake \
    libxml2-dev \
    libudev-zero-dev \
    git \
    || {
        echo "apk failed — trying without libudev-zero-dev (some devices don't need it)"
        apk add --no-cache gcc make cmake libxml2-dev git
    }

# ── 2. Clone libiio submodule if missing ──────────────────────────────────────
if [ ! -f "libiio/CMakeLists.txt" ]; then
    echo "--- Cloning libiio ---"
    # If we're inside a git repo, use submodule; otherwise plain clone
    if git -C .. rev-parse --git-dir >/dev/null 2>&1; then
        git -C .. submodule add https://github.com/analogdevicesinc/libiio.git sensor_daemon/libiio 2>/dev/null || true
        git -C .. submodule update --init --depth 1 sensor_daemon/libiio
    else
        git clone --depth 1 https://github.com/analogdevicesinc/libiio.git libiio
    fi
fi

# ── 3. Build libiio statically ────────────────────────────────────────────────
echo "--- Building libiio (static) ---"
mkdir -p libiio/_build
cd libiio/_build
cmake .. \
    -DBUILD_SHARED_LIBS=OFF \
    -DWITH_TESTS=OFF \
    -DWITH_DOC=OFF \
    -DWITH_MATLAB_BINDINGS=OFF \
    -DWITH_PYTHON_BINDINGS=OFF \
    -DCMAKE_BUILD_TYPE=Release \
    -DHAVE_AVAHI=OFF
make -j"$(nproc)" iio
cd "$SCRIPT_DIR"

# ── 4. Build tilt_sensor ──────────────────────────────────────────────────────
echo "--- Building tilt_sensor ---"
if make submodule; then
    echo ""
    echo "=== Done! ==="
    echo "Run the game:"
    echo "  ./tilt_sensor | python3 ../ball_maze.py YourName"
    exit 0
fi

# ── 5. Sysfs fallback ─────────────────────────────────────────────────────────
echo ""
echo "libiio build failed — falling back to sysfs (no libiio needed)"
make sysfs
echo ""
echo "=== Done (sysfs fallback) ==="
echo "Run the game:"
echo "  ./tilt_sensor_sysfs | python3 ../ball_maze.py YourName"
