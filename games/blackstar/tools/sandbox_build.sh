#!/bin/sh
# sandbox_build.sh — configure and build the C engine where there is no GPU
# and no -dev packages.
#
# The CMake configure step fails on a bare container with
#
#     Could not find a GL runtime library (GL/GLESv2/OpenGL)
#
# which is not a missing dependency so much as a missing SYMLINK. Containers
# ship the runtime sonames (`libGL.so.1`, `libSDL2-2.0.so.0`) and omit the
# unversioned `.so` names that `find_library` looks for, because those belong
# to the -dev packages. With no root there is no apt, and the fix does not
# need one: point CMake at a directory of symlinks you own.
#
# This exists because "the CMake edits are unverified" was carried as a
# caveat for most of a working session, and it turned out to be four lines of
# shell rather than a real obstacle. Anything that blocks the build from being
# checked at all is worth more than the thing it is blocking.
#
#   ./tools/sandbox_build.sh          configure + build + ctest
#   ./tools/sandbox_build.sh config   configure only
set -eu

LINKS="${LINKS:-$HOME/.gllink}"
BUILD="${BUILD:-build-sandbox}"

mkdir -p "$LINKS"
for lib in GL GLESv2 EGL GLX; do
    src=$(ls /usr/lib/*/lib$lib.so.* 2>/dev/null | head -1) || true
    [ -n "${src:-}" ] && ln -sf "$src" "$LINKS/lib$lib.so"
done
src=$(ls /usr/lib/*/libSDL2-2.0.so.* 2>/dev/null | head -1) || true
[ -n "${src:-}" ] && ln -sf "$src" "$LINKS/libSDL2.so"

echo "sandbox_build: link farm at $LINKS"
ls "$LINKS"

# CMAKE_LIBRARY_PATH satisfies find_library; the linker flag is needed too,
# because a found absolute path does not put the directory on the link line
# for libraries resolved by name (SDL2 is).
cmake -B "$BUILD" -S . \
    -DCMAKE_LIBRARY_PATH="$LINKS" \
    -DCMAKE_EXE_LINKER_FLAGS="-L$LINKS"

[ "${1:-all}" = "config" ] && exit 0

cmake --build "$BUILD" -j"$(nproc)"

# Two failures are expected on a fresh checkout and are NOT build problems:
#
#   content_validate  five part_group meshes are not in the tree at all
#   model_tuning      engine_src/data holds no imported_*.iqm
#
# The catalogue README says it: ".iqm is built, not stored". The baked header
# and model_tuning.json agree with each other at 1012 entries; it is only the
# INSTALLED models that are missing, and `tools/install_models.py` is what
# puts them there.
( cd "$BUILD" && ctest --output-on-failure ) || true
