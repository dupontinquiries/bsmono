#!/usr/bin/env bash
# export-bots.sh — build skydribble's assets/bots/ from blackstar fold strips.
#
#   ./tools/export-bots.sh [POSES]
#
# One directory per chassis, one subdirectory per pose (f00 = vehicle,
# f0N = robot). --gltf-merge collapses foldview's per-part nodes into one
# mesh per colour: racer_grit goes from 518 prims to 38, which is the
# difference between a bot costing forty draw calls and forty times a car.
#
# Poses are not free — each is a full model with its own SDF bake at startup.
# 5 is a fold you can read; 9 is smoother and noticeably slower to boot.
set -euo pipefail

POSES="${1:-5}"
HERE="$(cd "$(dirname "$0")" && pwd)"
SKY="$(dirname "$HERE")"
BLACKSTAR="$SKY/../blackstar"
FOLDVIEW="$BLACKSTAR/rust/foldview"
DATA="$BLACKSTAR/engine_src/data"
OUT="$SKY/assets/bots"

# name -> source .iqm. A `jet_` prefix makes the chassis fly (physics treats
# it as never grounded, with lift and nose thrust) instead of drive.
CHASSIS=(
    "blackstar:imported_hoverer_blackstar.iqm"
    "grit:imported_racer_grit_aka_rocket_car.iqm"
    "jet_dawnshade:imported_flyer_dawnshade.iqm"
)

[ -x "$FOLDVIEW/target/release/foldview" ] || ( cd "$FOLDVIEW" && cargo build --release )

for entry in "${CHASSIS[@]}"; do
    name="${entry%%:*}"
    iqm="${entry#*:}"
    if [ ! -f "$DATA/$iqm" ]; then
        echo "skip $name: $iqm not in $DATA"
        continue
    fi
    echo "== $name ($POSES poses) =="
    rm -rf "${OUT:?}/$name"
    for i in $(seq 0 $((POSES - 1))); do
        t=$(awk "BEGIN{printf \"%.4f\", $i/($POSES-1)}")
        "$FOLDVIEW/target/release/foldview" \
            --data "$DATA" --content "$BLACKSTAR/content" \
            --veh "$DATA/$iqm" \
            --gltf "$OUT/$name/$(printf 'f%02d' "$i")" --gltf-merge --t "$t" \
            2>/dev/null | tail -1
    done
done

echo
du -sh "$OUT"/* 2>/dev/null || true
