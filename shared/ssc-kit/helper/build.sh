#!/bin/sh
# Build the IMU helper for the Fairphone 5 (aarch64 musl, as the phone's postmarketOS) in the tideshell-aarch64
# container: a libssc with a selectable sample rate, and `sscstream` linked against it.
#   usage: helper/build.sh <out-dir>      then copy <out-dir>/libssc.so.2 to ~/.local/lib/ssc/ and
#                                         <out-dir>/sscstream to ~/.local/bin/sscstream.bin on the phone
# libssc (https://codeberg.org/DylanVanAssche/libssc) is cloned at build time; the commit this patch was made and
# measured against is in LIBSSC_REV.
set -e
OUT=${1:?out dir}; HERE=$(cd "$(dirname "$0")" && pwd); mkdir -p "$OUT"
podman run --rm -v "$HERE":/h:ro,z -v "$OUT":/out:z localhost/tideshell-aarch64 sh -ec '
  apk add --no-cache git meson ninja gcc musl-dev pkgconf glib-dev libqrtr-glib-dev protobuf-c-dev libqmi-dev python3 python3-dev >/dev/null
  cd /tmp && git clone -q https://codeberg.org/DylanVanAssche/libssc src && cd src
  git checkout -q $(cat /h/LIBSSC_REV)
  git apply /h/libssc-sample-rate.patch
  meson setup build >/dev/null && ninja -C build >/dev/null
  cp build/src/libssc.so.2 /out/
  gcc -O2 -Wall /h/sscstream.c -o /out/sscstream -Ibuild/src -Isrc $(pkg-config --cflags glib-2.0 gio-2.0 gobject-2.0) -Lbuild/src -lssc $(pkg-config --libs glib-2.0 gio-2.0 gobject-2.0)
'
