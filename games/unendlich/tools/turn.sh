#!/usr/bin/env bash
# turn.sh — one-command per-turn gate+package (token saver).
#   bash tools/turn.sh r192 "short-name"        # gates + zip + patch
# Gates: radar FSM on files newer than .turn-marker, cargo check (bin,
# full features), 200-tick harness, homogeneity probe. Then package.
set -euo pipefail
REL="${1:?release}"; NAME="${2:?name}"
cd "$(dirname "$0")/.."
export PATH=/usr/local/bin:$PATH CARGO_HOME=/home/claude/.cargo
CHANGED=$(find crates tools docs assets -newer .turn-marker -name "*.rs" -o -newer .turn-marker -name "*.comp" 2>/dev/null | head -40 || true)
[ -n "$CHANGED" ] && python3 tools/wiring_radar.py check $CHANGED
timeout 280 cargo check -p unendlich-engine --bin unendlich-fullgame \
  --features render,hot-shaders,window,gpu,ui-overlay,audio 2>&1 | tail -1
timeout 200 cargo build --example headless_run --release -p unendlich-engine \
  --features render,window,gpu,ui-overlay,audio 2>&1 | grep -cE "^error" || true
HR_TICKS=200 timeout 90 ./target/release/examples/headless_run | grep -E "ticks_per|edits_by"
timeout 60 ./target/release/examples/plant_homogeneity_probe | tail -1
OUT=/home/claude/out; rm -f $OUT/*; mkdir -p $OUT
P=$OUT/CHANGES-$REL-CUMULATIVE-$NAME.patch; : > $P
bash tools/diffset.sh >> $P
zip -qr $OUT/unendlich-$REL-$NAME.zip . -x "target/*"
cp $P $OUT/*.zip /mnt/user-data/outputs/
touch .turn-marker
ls /mnt/user-data/outputs/ | grep "$REL"
