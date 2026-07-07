#!/usr/bin/env bash
# sandbox-rust.sh — bring the in-sandbox Rust toolchain up from the
# operator's offline bundle (rustup.zip), incl. native build deps.
# The sandbox resets between sessions; run this once per session:
#   bash tools/sandbox-rust.sh [/path/to/rustup.zip]
set -euo pipefail
ZIP="${1:-/mnt/user-data/uploads/rustup.zip}"
[[ -f "$ZIP" ]] || { echo "no bundle at $ZIP — ask the operator to re-upload rustup.zip"; exit 1; }
T=$(mktemp -d)
unzip -q "$ZIP" -d "$T"
INNER=$(find "$T" -name "rust-1.*-x86_64-unknown-linux-gnu.zip" | head -1)
unzip -q "$INNER" -d "$T/tc"
sh "$T/tc/rust-install/INSTALL.sh"
export PATH=/usr/local/bin:$PATH
# native deps for the workspace's sys crates (alsa, glfw, opus):
apt-get install -y -q libasound2-dev pkg-config cmake libwayland-dev \
  libxkbcommon-dev xorg-dev autoconf automake libtool libopus-dev >/dev/null
cargo --version && rustc --version
echo "check:  cargo check -p unendlich-engine --bin unendlich-fullgame --features render,hot-shaders,window,gpu,ui-overlay,audio"
echo "tests:  cargo test -p unendlich-engine --lib --features render,window,gpu,ui-overlay,audio --no-run  (then run deps/unendlich_engine-* with filters)"
