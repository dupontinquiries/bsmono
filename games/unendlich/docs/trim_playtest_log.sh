#!/usr/bin/env bash
# trim_playtest_log.sh
#
# Compresses an unendlich-world playtest log to its signal:
#   - keeps every WARN/ERROR line verbatim
#   - keeps startup banner (everything before the first periodic FPS line)
#   - keeps shutdown footer (everything after the last periodic FPS line)
#   - keeps non-periodic INFO lines (block edits, mode switches, captures,
#     "Lava sync", anything that's not the per-second FPS spam)
#   - decimates the periodic per-second blocks: keeps the first sample,
#     the last sample, and one every N seconds in between (N=10 by default)
#   - keeps any periodic block whose FPS dropped below a threshold
#     (default 50) or whose p99 spiked above a threshold (default 80ms),
#     since those are the interesting frames
#
# The per-second block in unendlich-world looks like:
#   FPS:  XX  frame: ...
#     streaming: ...
#     tick breakdown: ...
#     input: ...
#     climate: ...
# (5 INFO lines, sometimes 6 if there's a "Lava sync" mixed in.)
#
# Usage:
#   ./trim_playtest_log.sh INPUT.log OUTPUT.log [--every N] [--fps-floor F] [--p99-ceiling MS]
#
# Defaults: --every 10 --fps-floor 50 --p99-ceiling 80

set -euo pipefail

EVERY=10
FPS_FLOOR=50
P99_CEILING=80

POSITIONAL=()
while [[ $# -gt 0 ]]; do
    case "$1" in
        --every)        EVERY="$2"; shift 2 ;;
        --fps-floor)    FPS_FLOOR="$2"; shift 2 ;;
        --p99-ceiling)  P99_CEILING="$2"; shift 2 ;;
        --help|-h)
            sed -n '2,/^set -euo/p' "$0" | sed -n '/^#/p' | sed 's/^# \?//'
            exit 0 ;;
        *)              POSITIONAL+=("$1"); shift ;;
    esac
done

if [[ ${#POSITIONAL[@]} -ne 2 ]]; then
    echo "usage: $0 INPUT.log OUTPUT.log [--every N] [--fps-floor F] [--p99-ceiling MS]" >&2
    exit 1
fi

IN="${POSITIONAL[0]}"
OUT="${POSITIONAL[1]}"

if [[ ! -f "$IN" ]]; then
    echo "input file not found: $IN" >&2
    exit 1
fi

awk -v EVERY="$EVERY" -v FPS_FLOOR="$FPS_FLOOR" -v P99_CEILING="$P99_CEILING" '
BEGIN {
    block_count = 0
    in_block = 0
    block_keep = 0
    block_buf_n = 0
    seen_first_block = 0
}

# Buffer flushing
function flush_block(    i) {
    if (block_keep) {
        for (i = 0; i < block_buf_n; i++) print block_buf[i]
    }
    block_buf_n = 0
    block_keep = 0
    in_block = 0
}

# WARN and ERROR are always kept, immediately
/\] (WARN|ERROR) / {
    if (in_block) flush_block()
    print
    next
}

# Detect the start of a periodic FPS block. The line looks like:
#   [date INFO  unendlich_world] FPS:  XX  frame: p50=...
/^\[[^]]*\] INFO  unendlich_world\] FPS: / {
    # If we were already inside a block, flush the previous one first.
    if (in_block) flush_block()

    in_block = 1
    block_buf_n = 0
    block_buf[block_buf_n++] = $0
    block_count++

    # Parse the FPS and p99 to decide if this block is "interesting"
    fps = 0
    p99 = 0
    if (match($0, /FPS: +[0-9]+/)) {
        fps_str = substr($0, RSTART, RLENGTH)
        gsub(/[^0-9]/, "", fps_str)
        fps = fps_str + 0
    }
    if (match($0, /p99=[0-9.]+ms/)) {
        p99_str = substr($0, RSTART+4, RLENGTH-6)
        p99 = p99_str + 0
    }

    # Keep the first block always (gives a baseline), and any block that
    # crosses the alarm thresholds, and every Nth block otherwise.
    keep = 0
    if (!seen_first_block) { keep = 1; seen_first_block = 1 }
    if (fps > 0 && fps < FPS_FLOOR) keep = 1
    if (p99 > P99_CEILING) keep = 1
    if (block_count % EVERY == 1) keep = 1
    block_keep = keep
    next
}

# Continuation lines of a periodic block: indented with "  ".
# These are the streaming/tick/input/climate lines.
in_block && /^\[[^]]*\] INFO  unendlich_world\]   / {
    block_buf[block_buf_n++] = $0
    next
}

# Anything else inside a block ends the block. Flush, then process the
# current line outside-block.
in_block {
    flush_block()
}

# Outside-block lines (startup banner, shutdown footer, "Mouse captured",
# "Broke OakLeaves", "Lava sync", etc.) are kept verbatim.
{
    print
}

END {
    if (in_block) flush_block()
}
' "$IN" > "$OUT"

# Report sizes
in_lines=$(wc -l < "$IN")
out_lines=$(wc -l < "$OUT")
in_bytes=$(wc -c < "$IN")
out_bytes=$(wc -c < "$OUT")
ratio=$(awk -v a="$out_bytes" -v b="$in_bytes" 'BEGIN { printf "%.1f", (a/b)*100 }')

echo "input:  $in_lines lines, $in_bytes bytes" >&2
echo "output: $out_lines lines, $out_bytes bytes" >&2
echo "ratio:  ${ratio}% (kept blocks at every=${EVERY}, fps<${FPS_FLOOR}, p99>${P99_CEILING}ms)" >&2
