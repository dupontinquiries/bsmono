#!/usr/bin/env bash
# dandere2x benchmark kit — builds the CLI (GPU feature only, no GUI deps),
# verifies the GPU kernels against the CPU reference on THIS machine, runs
# every benchmark, and packs a report. Send back report/*.tar.gz.
#
#   ./run_benchmarks.sh                    # everything, synthetic clips
#   CLIP=/path/to/real.mkv ./run_benchmarks.sh   # also analyze real footage
#   FRAMES=48 ./run_benchmarks.sh          # longer end-to-end runs (default 24)
#   QUICK=1 ./run_benchmarks.sh            # skip the end-to-end upscale runs
#
# Needs: cargo (stable), ffmpeg/ffprobe, a Vulkan ICD (vulkaninfo --summary
# should list Honeykrisp on Asahi). Nothing outside this directory.

set -u
KIT="$(cd "$(dirname "$0")" && pwd)"
cd "$KIT"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$KIT/target}"
export D2X_RRDB_X2="$KIT/apps/dandere2x/models/RealESRGAN_x2.safetensors"
FRAMES="${FRAMES:-24}"
STAMP="$(date +%Y%m%d-%H%M%S)"
REP="$KIT/report/$STAMP"
mkdir -p "$REP"
LOG="$REP/report.txt"
BIN="$CARGO_TARGET_DIR/release/dandere2x"
MODEL_X2="$KIT/apps/dandere2x/models/RealESRGAN_x2.safetensors"
MODEL_FAST="$KIT/apps/dandere2x/models/realesr-animevideov3.safetensors"

say()  { printf '\n=== %s\n' "$*" | tee -a "$LOG"; }
run()  { printf '$ %s\n' "$*" | tee -a "$LOG"; "$@" 2>&1 | tee -a "$LOG"; }
runt() { local name="$1"; shift; printf '$ %s\n' "$*" | tee -a "$LOG"; local t0=$(date +%s.%N); "$@" > "$REP/$name.log" 2>&1; local rc=$?; local t1=$(date +%s.%N); printf '   -> rc=%s  %.1fs  (%s.log)\n' "$rc" "$(echo "$t1 - $t0" | bc)" "$name" | tee -a "$LOG"; grep -E 'done:|frames:|memory:|upscaler:|error|Error' "$REP/$name.log" | head -8 | sed 's/^/   /' | tee -a "$LOG"; }

say "dandere2x benchmark kit — $STAMP"
{
  echo "host: $(uname -a)"
  echo "cpus: $(nproc)"
  grep -E 'MemTotal|MemAvailable' /proc/meminfo
  command -v rustc >/dev/null && rustc -V && cargo -V
  command -v ffmpeg >/dev/null && ffmpeg -version | head -1
  command -v vulkaninfo >/dev/null && vulkaninfo --summary 2>/dev/null | grep -E 'deviceName|driverName|apiVersion' | head -6
  ls /usr/share/vulkan/icd.d/ 2>/dev/null | tr '\n' ' '; echo
  ls -la "$MODEL_X2" "$MODEL_FAST" 2>&1
} | tee -a "$LOG"

say "build (release, gpu feature only)"
( cd apps/dandere2x && cargo build --release -p dandere2x --no-default-features --features gpu 2>&1 | tail -3 ) | tee -a "$LOG"
[ -x "$BIN" ] || { echo "build failed; see above" | tee -a "$LOG"; exit 1; }

say "probe"
run "$BIN" probe
SOFT=0; "$BIN" probe 2>/dev/null | head -1 | grep -q SOFTWARE && SOFT=1
if [ "$SOFT" = 1 ]; then
  echo "!! software adapter: numbers below are for a CPU rasterizer. Reducing sizes so the kit still completes (smoke test only)." | tee -a "$LOG"
  BENCH_TILE="--tile 48"; ITERS=2; BUDGETS="2G"; INFLIGHTS="1"; FIXED_TILES="48 96"; CLIP_SIZE="320x180"; AFRAMES=12; QUICK=1
else
  BENCH_TILE=""; ITERS=6; BUDGETS="8G"; INFLIGHTS="1"; FIXED_TILES="512 688"; CLIP_SIZE="1920x1080"; AFRAMES=48
fi

say "model costs"
run "$BIN" model "$MODEL_X2"
run "$BIN" model "$MODEL_FAST"

say "selftest: GPU kernels vs CPU reference on THIS device (both kernels, both models)"
for k in rt2f rt2h rt2c8h; do
  run "$BIN" selftest --model "$MODEL_FAST" --tile 40 --kernel "$k"
  run "$BIN" selftest --model "$MODEL_X2" --tile 48 --kernel "$k"
done

say "core + gpu test suites (CPU exactness vs numpy references; GPU bit-exact incl. 3 sets in flight)"
( cd apps/dandere2x && cargo test --release -p d2x-core -p d2x-wgpu -- --test-threads=1 2>&1 | grep -E 'test result|FAILED|panicked|PASS|FAIL —' ) | tee -a "$LOG"

say "CPU hot paths (per-call ms, one core)"
( cd apps/dandere2x && cargo test --release -p d2x-core --test bench_hot -- --ignored --nocapture 2>&1 | grep -E ' ms$' ) | tee -a "$LOG"

say "GPU kernel bench: rt2f (1022 last run, current default) vs rt2h / rt2c8h (hoisted addressing, single code path) vs rt2c8 (977) vs rt2c8fq"
for vb in $BUDGETS; do
  for inf in $INFLIGHTS; do
    run "$BIN" bench --model "$MODEL_X2" --vram-budget "$vb" --inflight "$inf" --iters "$ITERS" $BENCH_TILE
  done
done
say "GPU kernel bench: fixed tiles ($FIXED_TILES) at inflight 1"
for t in $FIXED_TILES; do
  run "$BIN" bench --model "$MODEL_X2" --tile "$t" --inflight 1 --iters "$ITERS"
done
say "GPU kernel bench: compact model"
run "$BIN" bench --model "$MODEL_FAST" --vram-budget 8G --inflight 1 --iters "$ITERS" $BENCH_TILE

say "synthetic clips ($CLIP_SIZE)"
CLIPS="$REP/clips"; mkdir -p "$CLIPS"
ENC="-c:v libx264 -preset ultrafast -crf 18 -pix_fmt yuv420p"
if ffmpeg -hide_banner -encoders 2>/dev/null | grep -q libsvtav1; then ENC="-c:v libsvtav1 -preset 10 -crf 30 -g 48 -pix_fmt yuv420p10le"; fi
ffmpeg -hide_banner -loglevel error -y -f lavfi -i "gradients=size=$CLIP_SIZE:rate=24:duration=3:speed=0.01:nb_colors=4,drawbox=x='iw*0.05+t*iw*0.15':y=ih*0.2:w=iw*0.2:h=ih*0.35:c=red:t=fill,drawbox=x=iw*0.62:y='ih*0.1+t*ih*0.14':w=iw*0.1:h=ih*0.18:c=white@0.9:t=fill,noise=alls=4:allf=t" $ENC "$CLIPS/partial.mkv"
ffmpeg -hide_banner -loglevel error -y -f lavfi -i "testsrc2=size=$CLIP_SIZE:rate=24:duration=3" $ENC "$CLIPS/motion.mkv"
ls -la "$CLIPS" | tee -a "$LOG"

say "analysis: reuse fraction and main-network share by quality (no GPU)"
for c in partial motion; do
  for q in 97 93 90; do
    printf 'clip=%s q=%s: ' "$c" "$q" | tee -a "$LOG"
    "$BIN" analyze -i "$CLIPS/$c.mkv" --frames "$AFRAMES" --quality "$q" --cascade default --flat-bypass 2>&1 | tail -1 | tee -a "$LOG"
  done
done
if [ -n "${CLIP:-}" ] && [ -f "$CLIP" ]; then
  say "analysis: real footage $CLIP (first 300 frames)"
  for q in 97 93 90; do
    printf 'q=%s: ' "$q" | tee -a "$LOG"
    "$BIN" analyze -i "$CLIP" --frames 300 --quality "$q" --cascade default --flat-bypass 2>&1 | tail -1 | tee -a "$LOG"
  done
  "$BIN" analyze -i "$CLIP" --frames 300 --quality 93 --cascade default --flat-bypass > "$REP/analyze-real-q93.log" 2>&1
fi

if [ -z "${QUICK:-}" ]; then
  say "end-to-end upscale throughput ($FRAMES frames, 1080p -> 4K)"
  OUT="$REP/out"; mkdir -p "$OUT"
  ENCARGS="--codec x264 --preset 12 --crf 30"
  if ffmpeg -hide_banner -encoders 2>/dev/null | grep -q libsvtav1; then ENCARGS="--codec av1 --preset 12 --crf 35"; fi
  runt e2e-x2-rt2            "$BIN" upscale -i "$CLIPS/partial.mkv" -o "$OUT/x2-rt2.mkv" --model "$MODEL_X2" --frames "$FRAMES" --quality 93 --cascade default --flat-bypass --vram-budget 8G --inflight 1 --kernel rt2 $ENCARGS
  runt e2e-x2-wino           "$BIN" upscale -i "$CLIPS/partial.mkv" -o "$OUT/x2-wino.mkv" --model "$MODEL_X2" --frames "$FRAMES" --quality 93 --cascade default --flat-bypass --vram-budget 8G --inflight 1 --kernel wino $ENCARGS
  runt e2e-x2-nocascade      "$BIN" upscale -i "$CLIPS/partial.mkv" -o "$OUT/x2-nocascade.mkv" --model "$MODEL_X2" --frames "$FRAMES" --quality 93 --vram-budget 8G --inflight 1 $ENCARGS
  runt e2e-x2-motion         "$BIN" upscale -i "$CLIPS/motion.mkv"  -o "$OUT/x2-motion.mkv" --model "$MODEL_X2" --frames "$FRAMES" --quality 93 --cascade default --flat-bypass --vram-budget 8G --inflight 1 $ENCARGS
  runt e2e-x2-j2             "$BIN" upscale -i "$CLIPS/partial.mkv" -o "$OUT/x2-j2.mkv" --model "$MODEL_X2" --frames "$FRAMES" --quality 93 --cascade default --flat-bypass --vram-budget 8G --inflight 1 -j 2 $ENCARGS
  runt e2e-fast              "$BIN" upscale -i "$CLIPS/partial.mkv" -o "$OUT/fast.mkv" --model "$MODEL_FAST" --frames "$FRAMES" --quality 93 --vram-budget 8G --inflight 1 $ENCARGS
  if [ -n "${CLIP:-}" ] && [ -f "$CLIP" ]; then
    runt e2e-real-x2         "$BIN" upscale -i "$CLIP" -o "$OUT/real-x2.mkv" --model "$MODEL_X2" --frames "$FRAMES" --quality 93 --cascade default --flat-bypass --vram-budget 8G --inflight 1 $ENCARGS
  fi
  say "per-frame lines from the x2 rt2 run (A = analysis, U = upscale, M = merge, ms)"
  grep -E '^frame ' "$REP/e2e-x2-rt2.log" | head -12 | tee -a "$LOG"
  rm -rf "$OUT"
fi

rm -rf "$CLIPS"
say "done — report in $REP"
tar czf "$KIT/report/d2x-report-$STAMP.tar.gz" -C "$KIT/report" "$STAMP"
echo "send back: $KIT/report/d2x-report-$STAMP.tar.gz"
