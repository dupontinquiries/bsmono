#!/bin/sh
# Build tideshell + comfortgov for aarch64 in the emulated Alpine container
# (see Containerfile.aarch64), from the source tarball pack-src.sh makes.
# Output: ./out/tideshell, ./out/comfortgov. Needs podman with qemu-user
# binfmt (Fedora/Debian: `apt install qemu-user-static` or `podman machine`
# on macOS, which ships it). Not verified here (no podman in the sandbox).
set -eu
HERE=$(cd "$(dirname "$0")" && pwd)
TAR=${1:-$HERE/../../../tideshell-src.tar.gz}
[ -f "$TAR" ] || { echo "no source tarball at $TAR (run pack-src.sh)"; exit 1; }
mkdir -p out cache
podman image exists tideshell-aarch64 || podman build --platform linux/arm64 -t tideshell-aarch64 -f "$HERE/Containerfile.aarch64" "$HERE"
podman run --rm --platform linux/arm64 \
  -v "$TAR:/src.tar.gz:ro,z" -v "$PWD/out:/out:z" -v "$PWD/cache:/root/.cargo/registry:z" \
  tideshell-aarch64 sh -c '
    set -e; mkdir -p /src && tar -xzf /src.tar.gz -C /src
    cd /src/apps/tideshell/egui-frontend
    cargo build --release --locked --no-default-features --features layer,ei --bin tideshell
    cp target/release/tideshell /out/
    cd /src/apps/tideshell/settings && cargo build --release --locked --bin tideshell-settings && cp target/release/tideshell-settings /out/
    cd /src/system/comfortgov && cargo build --release && cp target/release/comfortgov /out/
    file /out/tideshell || true'
echo "built: $(ls -la out/)"
