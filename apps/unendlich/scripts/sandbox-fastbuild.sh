#!/usr/bin/env bash
# Sandbox-only build accelerator. Run once per session (the FS resets).
# Speeds up `cargo test`/`cargo build` (NOT `cargo check` — that doesn't link).
# See docs/TOOLING_fast_linking.md. Never commit these flags to the repo tree:
# the operator's Apple-M2 release build must stay portable + mold-free.
set -u
echo "[fastbuild] installing mold + lld (gold is preinstalled) ..."
apt-get install -y mold lld >/dev/null 2>&1 || echo "[fastbuild] apt unavailable; gold still works"
CARGO_CFG="${CARGO_HOME:-$HOME/.cargo}/config.toml"
mkdir -p "$(dirname "$CARGO_CFG")"
if ! grep -q "fuse-ld=mold" "$CARGO_CFG" 2>/dev/null; then
  LINKER=mold; command -v mold >/dev/null || LINKER=gold
  cat >> "$CARGO_CFG" <<CFG

# [sandbox-fastbuild] faster linking; sandbox-only, do not commit to the repo.
[build]
rustflags = ["-C", "link-arg=-fuse-ld=${LINKER}", "-C", "split-debuginfo=unpacked"]
CFG
  echo "[fastbuild] wrote ${LINKER} + split-debuginfo to ${CARGO_CFG}"
  echo "[fastbuild] note: the NEXT cargo build does a one-time full rebuild (rustflags changed)."
else
  echo "[fastbuild] ${CARGO_CFG} already configured."
fi
echo "[fastbuild] fastest test/run loop (smallest binary; needs libstd.so at runtime):"
echo '  RUSTFLAGS="-C link-arg=-fuse-ld=mold -C prefer-dynamic -C split-debuginfo=unpacked" \'
echo '    cargo test --lib --features "render-core egui"'
echo '  export LD_LIBRARY_PATH="$(rustc --print sysroot)/lib:$LD_LIBRARY_PATH"   # if a bin cant find libstd'
