#!/bin/sh
# unwired-check.sh — find capabilities that were built and never called.
#
# Four this session, each found late and by accident:
#   c4-801  parse_tools_reported   — tool-call reject diagnosis, zero callers for 44 bundles
#   c4-819  the encode timer       — measured nothing until c4-830 wired it into the verdict
#   c4-821  swiglu_matvec_q8_0_add — a kernel declared in a file that was not a module at all
#   c4-820  linear_q8_0_add        — one call site of four missed the fused path
#
# The shape is always the same: something is written, its tests pass because they call it directly,
# and nothing in the product does. TESTS ARE NOT CALLERS — that is the whole blind spot, so callers
# inside #[cfg(test)] regions do not count.
#
# Heuristic by design: it reports candidates for a human to judge, not a build failure. A pub fn with
# no caller may be a deliberate API; one with callers may still be dead behind a flag nobody sets.
# What it catches is the case that actually happened four times.
#
# Verified by planting an uncalled function and watching it fire (c4-847), then watching it clear
# when a caller was added. That verification is the whole reason to believe any output here: an
# earlier version reported ZERO candidates and missed the planted case, because `set -e` plus
# `[ ] && echo` killed the loop at the first function that had a caller. It was not finding
# nothing; it was stopping immediately, which reads identically.
#
# NOISE WARNING: on this tree it reports ~728 candidates, which is too many to act on directly. Most
# are genuinely-public APIs, trait-ish helpers and accessors called through paths this grep cannot
# see (macros, generics, re-exports). Use it to CHECK A SPECIFIC function you just wrote —
#   sh tools/unwired-check.sh | grep my_new_function
# — rather than as a list to work through. That narrow use is the one the four real cases needed. A checker never shown
# catching its case is decoration, and this one shipped broken once for exactly that reason.
set -e
cd "$(cd "$(dirname "$0")/.." && pwd)"

FILES=$(ls crates/*/src/*.rs crates/*/src/tabs/*.rs 2>/dev/null)

for f in $FILES; do
    grep -oE '^[[:space:]]*pub fn [a-z_0-9]+' "$f" 2>/dev/null | awk '{print $3}' | sort -u | while read -r fn; do
        [ -n "$fn" ] || continue
        # Names too generic to attribute, or conventional entry points.
        case "$fn" in new|default|fmt|clone|len|is_empty|get|set|name|ui|main|from|into|next|draft) continue;; esac
        n=$(awk -v fn="$fn" '
            FNR==1 { intest=0 }                      # reset per file, or one test module hides the rest
            /^#\[cfg\(test\)\]/ { intest=1 }
            /^}/ { intest=0 }                        # a column-zero brace closes a top-level item
            intest==0 && $0 ~ (fn "\\(") && $0 !~ ("pub fn " fn) { c++ }
            END { print c+0 }' $FILES 2>/dev/null | awk '{s+=$1} END {print s+0}')
        # if/then, NOT `[ ] && echo`. Under set -e a false test makes the statement return
        # non-zero, which kills the subshell running this while loop — so the scan died at the first
        # function that HAD a caller. That is why it once reported zero candidates and missed a
        # planted case: it was not finding nothing, it was stopping immediately.
        if [ "${n:-0}" -eq 0 ]; then echo "$f: pub fn $fn"; fi
    done
done | sort
