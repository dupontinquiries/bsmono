#!/usr/bin/env bash
# pack-rust.sh — build an OFFLINE Rust toolchain bundle (zip) for sandboxes
# whose network blocks rustup / static.rust-lang.org.
#
# Run this on ANY machine WITH internet (your Mac is fine — the file bytes are
# arch-independent; only the *target triple in the filename* picks the runtime
# arch). It downloads the standalone rustc/cargo/rust-std component tarballs for
# the requested targets, verifies each against its published .sha256, and zips
# them with a self-contained INSTALL.sh that installs the matching toolchain
# inside the target sandbox (the same install.sh machinery rustup uses — which
# is what avoids the "can't find crate for std" trap of using a raw rustc).
#
# Why not bundle rustup-init itself? It can't fetch the toolchain without
# reaching static.rust-lang.org — the very thing the sandbox blocks. So we ship
# the toolchain and skip rustup entirely.
#
# ── usage ────────────────────────────────────────────────────────────────────
#   ./pack-rust.sh                      # default: 1.96.0, x86_64 + aarch64 linux-gnu
#   RUST_VERSION=1.96.0 ./pack-rust.sh  # pin a version
#   TARGETS="x86_64-unknown-linux-gnu" ./pack-rust.sh        # one arch only
#   COMPONENTS="rustc cargo rust-std clippy rustfmt" ./pack-rust.sh  # add tools
#   COMBINED=1 ./pack-rust.sh           # also emit a single zip with both arches
#
# Output: rust-<ver>-<target>.zip per target (upload-friendly), and optionally
# rust-<ver>-offline.zip (combined). Upload the zip matching the sandbox arch.
set -euo pipefail

VERSION="${RUST_VERSION:-1.96.0}"
TARGETS="${TARGETS:-x86_64-unknown-linux-gnu aarch64-unknown-linux-gnu}"
COMPONENTS="${COMPONENTS:-rustc cargo rust-std}"   # minimal set that can compile
DIST="${RUST_DIST:-https://static.rust-lang.org/dist}"
PKG="rust-install"                                  # top folder inside every zip
COMBINED="${COMBINED:-0}"
WORK="$(pwd)/.rust-pack-work"

need(){ command -v "$1" >/dev/null 2>&1 || { echo "ERROR: need '$1' on this machine"; exit 1; }; }
need curl; need tar; need zip; need awk
# sha256 tool differs across OSes (Linux: sha256sum, macOS: shasum -a 256)
sha256_of(){ if command -v sha256sum >/dev/null 2>&1; then sha256sum "$1" | awk '{print $1}';
             else shasum -a 256 "$1" | awk '{print $1}'; fi; }

echo "Rust offline pack — version $VERSION"
echo "targets:    $TARGETS"
echo "components: $COMPONENTS"
echo

rm -rf "$WORK"; mkdir -p "$WORK/$PKG"

for t in $TARGETS; do
  d="$WORK/$PKG/$t"; mkdir -p "$d"
  echo "== $t =="
  for c in $COMPONENTS; do
    f="$c-$VERSION-$t.tar.xz"
    url="$DIST/$f"
    echo "  downloading $f"
    curl -fL --retry 3 -C - -o "$d/$f" "$url"
    # verify against the published sidecar if present (rust dist ships .sha256)
    if curl -fsL -o "$d/$f.sha256" "$url.sha256" 2>/dev/null; then
      want="$(awk '{print $1}' "$d/$f.sha256")"; rm -f "$d/$f.sha256"
      got="$(sha256_of "$d/$f")"
      if [ "$got" = "$want" ]; then echo "    sha256 ok"
      else echo "    !! sha256 MISMATCH ($got != $want) — aborting"; exit 1; fi
    else
      echo "    (no .sha256 sidecar published; skipping verify)"
    fi
  done
done

# --- INSTALL.sh that runs inside the sandbox (POSIX sh) ---
cat > "$WORK/$PKG/INSTALL.sh" <<'INSTALL'
#!/bin/sh
# Offline Rust installer. Detects the host triple, then installs the matching
# rustc/cargo/rust-std from this bundle by running each component's own
# install.sh (so std lands in $PREFIX/lib/rustlib correctly). Run as root, or
# pass a writable prefix:  PREFIX=$HOME/.local sh INSTALL.sh
set -eu
PREFIX="${PREFIX:-/usr/local}"
here="$(cd "$(dirname "$0")" && pwd)"

# host triple detection (mapping adapted from rustup-init.sh)
ostype="$(uname -s)"; cputype="$(uname -m)"; clib="gnu"
case "$ostype" in
  Linux)  if ldd --version 2>&1 | grep -qi musl; then clib="musl"; fi; os="unknown-linux-$clib" ;;
  Darwin) os="apple-darwin" ;;
  *) echo "unsupported OS: $ostype"; exit 1 ;;
esac
case "$cputype" in
  x86_64|x86-64|amd64) cpu="x86_64" ;;
  aarch64|arm64)       cpu="aarch64" ;;
  *) echo "unsupported CPU: $cputype"; exit 1 ;;
esac
triple="$cpu-$os"
echo "host triple : $triple"
echo "install into: $PREFIX"

dir="$here/$triple"
if [ ! -d "$dir" ]; then
  echo "this bundle has no toolchain for '$triple'. it contains:"; ls "$here"
  exit 1
fi

command -v xz >/dev/null 2>&1 || echo "note: if extraction fails, install xz (apt-get install -y xz-utils)"
tmp="$(mktemp -d)"; trap 'rm -rf "$tmp"' EXIT
n=0
for tx in "$dir"/*.tar.xz; do
  [ -e "$tx" ] || continue
  echo "-> installing $(basename "$tx")"
  if ! tar -xf "$tx" -C "$tmp" 2>/dev/null; then xz -dc "$tx" | tar -x -C "$tmp"; fi
  comp="$(basename "$tx" .tar.xz)"
  ( cd "$tmp/$comp" && sh ./install.sh --prefix="$PREFIX" --disable-ldconfig )
  n=$((n+1))
done
[ "$n" -gt 0 ] || { echo "no .tar.xz found in $dir"; exit 1; }

echo
"$PREFIX/bin/rustc" --version
"$PREFIX/bin/cargo" --version
echo "OK — rust installed. ensure '$PREFIX/bin' is on PATH (it usually is)."
INSTALL
chmod +x "$WORK/$PKG/INSTALL.sh"

# manifest for provenance
{
  echo "rust offline bundle"
  echo "version    : $VERSION"
  echo "components : $COMPONENTS"
  echo "targets    : $TARGETS"
  echo "dist       : $DIST"
  echo "packed_utc : $(date -u +%Y-%m-%dT%H:%M:%SZ)"
} > "$WORK/$PKG/manifest.txt"

# --- zip outputs ---
outdir="$(pwd)"
# per-target zips (standalone: each contains INSTALL.sh + its one arch)
for t in $TARGETS; do
  stage="$(mktemp -d)"; mkdir -p "$stage/$PKG/$t"
  cp "$WORK/$PKG/$t"/*.tar.xz "$stage/$PKG/$t/"
  cp "$WORK/$PKG/INSTALL.sh" "$WORK/$PKG/manifest.txt" "$stage/$PKG/"
  ( cd "$stage" && zip -rq "$outdir/rust-$VERSION-$t.zip" "$PKG" )
  rm -rf "$stage"
  echo "wrote rust-$VERSION-$t.zip  ($(du -h "$outdir/rust-$VERSION-$t.zip" | awk '{print $1}'))"
done
# optional combined zip (both arches + one INSTALL.sh)
if [ "$COMBINED" = "1" ]; then
  ( cd "$WORK" && zip -rq "$outdir/rust-$VERSION-offline.zip" "$PKG" )
  echo "wrote rust-$VERSION-offline.zip  ($(du -h "$outdir/rust-$VERSION-offline.zip" | awk '{print $1}'))"
fi

rm -rf "$WORK"
echo
echo "done. upload the zip whose triple matches the sandbox arch."
echo "in the sandbox:  unzip rust-$VERSION-<triple>.zip && $PKG/INSTALL.sh"
