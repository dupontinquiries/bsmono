#!/usr/bin/env bash
# Every gate in this crate, to log.log, with cargo's warning noise suppressed.
#
# Warnings are dropped rather than silenced at the source: `cargo build` prints them once at the
# top and they bury the gate output, but they are real and belong in a `cargo clippy` run of their
# own rather than being turned off with an allow.
set -uo pipefail
LOG="${1:-log.log}"
: > "$LOG"

say() { printf '\n=== %s ===\n' "$1" | tee -a "$LOG"; }

# BUILD FIRST, QUIETLY. --message-format=short keeps errors readable; 2>/dev/null drops warnings.
# If this fails, nothing below is meaningful, so it stops.
printf 'building (warnings suppressed; run `cargo build --release --bins` alone to see them)\n' | tee -a "$LOG"
if ! cargo build --release --bins --message-format=short 2>/dev/null; then
    printf 'BUILD FAILED — rerun `cargo build --release --bins` to see why\n' | tee -a "$LOG"
    exit 1
fi

BIN=target/release/kernel-validate

# ---- the 57 kernel-validate gates -------------------------------------------------------------
# Ordered cheap-to-expensive so a broken environment shows up in seconds rather than minutes.
GATES_FAST="arch memplan store rubric quanttables quant-survey cpuops moe-cpu"
GATES_CORE="bytes q4k q4resident q6k q8direct quantref matvec qkv gemm-spec"
GATES_ARCH="gemma qwen2 qwen3 llama4-kqnorm mla-block swa branch kvtree"
GATES_MOE="iq1s iq1s-kernel moe-fusion moe-raw moe-repin moe-stream moe-prefetch passfuse"
# the hybrid (Gated DeltaNet) family, the fused-kernel rewrite and native IQ4_XS — added 2026-10-04;
# fused-v2 takes a GGUF for its whole-model half (the fixture gen-all.sh writes)
GATES_HYBRID="deltanet-load prefill-parity rope-kv deltanet-carry iq4xs"
GATES_PERF="autotune autotune-cache submit stream-slots residency perf prefill prefill-attn"
GATES_SYS="vram mmap zerocopy loadpath worker"
GATES_VISION="vit-block vae-conv vae-resblock dit-block dit-primitives dit-conditioning"

for g in $GATES_FAST $GATES_CORE $GATES_ARCH $GATES_MOE $GATES_HYBRID $GATES_PERF $GATES_SYS $GATES_VISION; do
    say "--gate-$g"
    timeout 600 "$BIN" "--gate-$g" 2>&1 | tee -a "$LOG"
done
say "--gate-fused-v2 /tmp/tiny-qwen3.gguf"
timeout 600 "$BIN" --gate-fused-v2 /tmp/tiny-qwen3.gguf 2>&1 | tee -a "$LOG"

# ---- standalone gate binaries ------------------------------------------------------------------
# gate-policy and gate-rowstats are pure CPU. gate-bidir-attn and gate-rnd1 need a GPU; gate-rnd1
# additionally needs an RND1 checkpoint and will say so rather than fail obscurely.
for b in gate-policy gate-rowstats gate-bidir-attn gate-rnd1; do
    say "$b"
    timeout 600 "target/release/$b" 2>&1 | tee -a "$LOG"
done

# ---- summary ----------------------------------------------------------------------------------
say "SUMMARY"
{
    printf 'FAIL lines: %s\n' "$(grep -acE '^\s*\[ *FAIL|: FAIL' "$LOG")"
    printf '\nBASELINE FROM A LAVAPIPE RUN — 7 FAIL lines. Compare against yours:\n'
    printf '  --gate-q4k         2 tie-straddles: quotients landing exactly on .5 that hardware\n'
    printf '                     rcp cannot reproduce. Documented, not a regression.\n'
    printf '  --gate-q4resident  pre-existing; also prints a 70B sizing line that is informational\n'
    printf '  --gate-qkv         UNCLASSIFIED on lavapipe\n'
    printf '  --gate-qwen3       UNCLASSIFIED ("see failing checks above")\n'
    printf '  --gate-moe-raw     UNCLASSIFIED\n'
    printf '  --gate-submit      UNCLASSIFIED; likely lavapipe timing, since the submit governor\n'
    printf '                     tunes against completion callbacks this driver does not deliver\n'
    printf '  gate-bidir-attn    column 1 at t=2100 only. Columns 2 and 3 pass at 5e-8 and 4e-6,\n'
    printf '                     which acquits the tiled kernel: a real defect would show against\n'
    printf '                     the f64 reference, not only against its naive twin.\n'
    printf '\nI did not verify the four UNCLASSIFIED ones are hardware-specific. They may be real\n'
    printf 'failures that predate this run. Treat a match with this list as "no worse than the\n'
    printf 'baseline", not as "expected".\n'
    printf '\ngate-rnd1 needs an RND1 checkpoint and declines rather than failing.\n'
} | tee -a "$LOG"
