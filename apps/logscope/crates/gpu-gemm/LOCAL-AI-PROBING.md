# LOCAL-AI-PROBING.md — a runbook for the model babysitting this GPU

You are the local model driving performance work on the wgpu engine. A human started you and is not
watching. This file is your whole brief: what to run, in what order, when to stop, and the three
things that can break the machine.

Read the rules first. They are short and they are the part that matters.

---

## Rules

1. **One variable per run.** Two knobs changed at once produces a number nobody can use. If you want
   to test two things, that is two runs with two tags.
2. **Every run gets a tag and a record.** `perf-loop/` already does this: `./loop.sh bench <tag> K=V`
   writes a JSON. Never report a number you did not write down.
3. **Correctness outranks speed, always.** If `--gate-matvec` or `--gate-bytes` gains a FAIL line, the
   change is discarded no matter what the tok/s says. A faster wrong kernel is a bug.
4. **A number within ±3% of baseline is noise.** Re-run once. Two noise results mean the hypothesis is
   dead, not the run.
5. **Never bench a busy GPU.** `./loop.sh status`, and `./loop.sh quiet` if anything is up. `bench.sh`
   refuses anyway; do not pass `FORCE=1` to get around it.
6. **Stop and write to the ledger when you are stuck.** An absence record — what you looked for, what
   you did not find, the command that re-checks it — is a real result. See "Recording nothing" below.
7. **Do not edit kernels on a hunch.** Everything below is either a flag or a measurement. Writing
   WGSL is the human's job unless a hypothesis has already been measured and named.

---

## The three hazards

Read these before running anything. They are the ways this work damages something.

**1. The subgroup mat-vec can hang the GPU.** On Asahi/Honeykrisp the subgroup kernel is correct at
small shapes and can hang the firmware at a vocabulary-sized projection; the watchdog then kills the
GPU and the machine needs a reboot. This is why `--gate-sgladder` exists and why it probes exactly one
width per run. **Never set `LOGSCOPE_MATVEC_SG=1` to "just try it" at full scale.** Use the ladder.

**2. Raising the memory budget can OOM the machine.** `HK_SYSMEM` and `LOGSCOPE_MAX_GPU_GB` override
conservative limits. On a 192 GB box that is usually fine and occasionally is not. Raise in steps, never
to the physical maximum, and never while something else large is running.

**3. Long sustained runs heat the box.** A ladder or a sweep left looping overnight is a thermal load.
Finish a sweep, report, and stop; do not invent an endless loop.

If the machine stops responding mid-probe, that is data, not failure. The ladder writes its note before
each attempt precisely so a hang is informative. When the human reboots, the first thing you do is read
`~/.cache/logscope/sgladder-<adapter>.txt` and report what it says.

---

## Session start, every time

```sh
cd apps/logscope
./loop.sh status                 # what is on the Ultra right now
./loop.sh quiet                  # stop ollama / llama-server / optiq if any are up
./loop.sh sync && ./loop.sh build
./loop.sh smoke                  # ~20 s on the 1B: proves the pipe works
./loop.sh bench baseline         # the reference every later compare uses
```

If `smoke` fails, stop and report. Do not proceed to experiments on a broken pipe.

The two numbers that matter in every JSON: **engine/ref** (this engine vs llama.cpp Metal on the same
GGUF) and **% of roofline** (`745 GiB/s` divided by the active bytes per token). Quote both.

---

## The experiment queue

Work down it. Each entry says what to run, what "it worked" looks like, and when to stop. Do not skip
ahead: a later experiment measured on an unstable earlier state is wasted.

### P0 — where is the time actually going

```sh
./loop.sh bench baseline
```

Read `decode_ctx512` in the JSON: `sync_ms` (waiting for the GPU) vs `setup_ms` (CPU building bind
groups and dims buffers) vs `other_ms`.

* `sync_ms` ≈ the whole per-token time → the GPU is the bottleneck. Go to P2, P3, P5.
* `setup_ms` > 15% of the token → the CPU is the bottleneck. Go to P4.

Report the split before running anything else. Everything after this is chosen by it.

### P1 — the subgroup ladder (the biggest single lever, and the dangerous one)

Only if `subgroups` is reported available. One rung per run:

```sh
./target/release/kernel-validate --gate-sgladder     # repeat until it says finished or blocked
```

After each run it prints `before` and `after` verdicts. Keep going until it says the ladder is finished
or blocked. Then:

```sh
./loop.sh bench e1-matvec-sg
./loop.sh compare baseline e1-matvec-sg
```

Expect: `decode-vs-prefill.md` says the m=1 path is latency-bound and this kernel is the one built for
it, so anything from 1.3x up is real. **Stop rule:** if a `--gate-sgladder` run kills the machine, stop
the whole session. After the reboot, report the ladder file's contents and nothing else — the human
decides what happens next.

### P2 — submit batching

```sh
./loop.sh bench e2-flush128 LOGSCOPE_FLUSH_EVERY=128
./loop.sh bench e2-flush256 LOGSCOPE_FLUSH_EVERY=256
```

`FLUSH_EVERY=64` already took Qwen-14B from 1.02 to 2.44 tok/s, so this knob is known to matter. It
stops mattering once one submit's GPU time exceeds the ~2 ms per-submit cost. **Stop rule:** two
consecutive steps flat (within 3%) — report the best and move on.

Related, one at a time if P0 pointed at sync: `LOGSCOPE_SUBMIT_BUDGET_MS`, `LOGSCOPE_SUBMIT_INFLIGHT`.

### P3 — dispatch fusion

Around 210 dispatches per token today. Each of these collapses some of them; run one per tag:

```sh
./loop.sh bench e3-qkv   LOGSCOPE_FUSED_QKV=1
./loop.sh bench e3-glu   LOGSCOPE_FUSE_GLU=1
./loop.sh bench e3-down  LOGSCOPE_FUSE_DOWN=1
./loop.sh bench e3-pass  LOGSCOPE_PASS_FUSE=1
./loop.sh bench e3-norm  LOGSCOPE_FUSE_NORM=1
```

Expect 5–20% each while `sync_ms` dominates. **Stop rule:** any tag with a gate FAIL is discarded
immediately and reported.

### P4 — the CPU side

Only if P0 showed `setup_ms` is a real share:

```sh
./loop.sh bench e4-bgcache LOGSCOPE_BG_CACHE=1
```

Expect `setup_ms` to fall toward zero. If it does not move, say so: that is an absence worth recording.

### P5 — prefill

Prefill is the wall for agent work (51 tok/s measured against ~700–1000 for llama.cpp Metal), and the
loop has no prefill metric yet. **If `kernel-validate --bench-prefill` does not exist, stop here and
report that** — adding it is a code change, which is the human's job. If it does exist:

```sh
./loop.sh bench e5-prefill-tiled   LOGSCOPE_PREFILL_TILED=1
./loop.sh bench e5-prefill-naive   LOGSCOPE_PREFILL_NAIVE=1
```

Note: an error message in the engine mentions "lower `LOGSCOPE_PREFILL_CHUNK`", but **nothing reads that
variable** — the chunk is `WorkerPool::PREFILL_CHUNK`, a constant. Setting it does nothing. If a chunk
sweep is what the measurement calls for, report that the knob has to be added; do not set it and report
a number, because the number will be the default either way.

**Stop rule:** if the tiled path is not faster than the default here, say so and move to P6.

### P6 — the memory ceiling (the other dangerous one)

The engine refuses more than ~31 GiB of concurrent buffers with `Failed to allocate BO VMA`, which caps
big models at streaming speed. Three runs that tell you WHICH limit it is — read
`docs/asahi-headstart.md` first, it explains what each outcome means:

```sh
./target/release/kernel-validate --gate-residency                         # confirm the wall is still ~31 GiB
HK_SYSMEM=$((64 * 1 << 30)) ./target/release/kernel-validate --gate-residency   # heap size?
LOGSCOPE_PROBE_SLAB_MB=64 ./target/release/kernel-validate --gate-residency     # bytes or object count?
```

* ceiling moves with `HK_SYSMEM` → it was the reported heap size.
* ceiling identical with 64 MB slabs as with 2 GB slabs → it is bytes (VA), not object count.
* lower ceiling with small slabs → it is count / page-table.

Report which, with the three numbers. Do not go above 64 GiB on `HK_SYSMEM` without the human.

### P7 — speculative decoding

`LOGSCOPE_SPEC_DRAFT` is read by the **app** (`logscope-egui`), not by `kernel-validate`, so this one is
not a `loop.sh bench` experiment. Measure it from the app on a fixed prompt at temperature 0:

```sh
LOGSCOPE_SPEC_DRAFT=lookup logscope-cli ...    # then the same prompt with =off
```

The simulated ceiling is ~1.5x tokens per verification pass on repo-reading, ~1.32x on Rust source,
~1.04x on prose — an acceptance ceiling, not a wall-clock measurement, so the wall-clock number is the
point of running it. Only valid at temperature 0: `autospec` enables it there by itself, and forcing it
above 0 changes which sample you get, so the comparison would be meaningless.

---

## Recording nothing

A hypothesis that did not pan out is the cheapest result you will produce and the first one a reset
loses. Write it down:

```sh
hive absence \
  --looked-for "setup_ms improvement from LOGSCOPE_BG_CACHE" \
  --did-not-find "any change beyond noise" \
  --command "cd apps/logscope && ./loop.sh bench e4-bgcache LOGSCOPE_BG_CACHE=1 && ./loop.sh compare baseline e4-bgcache" \
  --scope "M2 Ultra, Asahi, <mesa version>"
```

`hive recheck` re-runs those commands later, so a negative result stays re-runnable instead of going
stale.

---

## What to report back

One block, in this shape, nothing else:

```
baseline   : <decode tok/s @ctx512>, sync <ms>, setup <ms>, engine/ref <x>, roofline <%>
<tag>      : <tok/s>  <Δ%>  KEEP|NOISE|DISCARD   <one clause of why>
...
ladder     : <the verdict line, or "not run">
blocked on : <the one thing a human has to decide, or "nothing">
```

Append every KEEP to `perf-loop/LEDGER.md` in its table format, and re-run `./loop.sh bench baseline`
after a KEEP is committed so later comparisons are against the new floor.

---

## Things you must not do

* Do not run two experiments in one bench.
* Do not `FORCE=1` a busy GPU.
* Do not set `LOGSCOPE_MATVEC_SG=1` outside the ladder.
* Do not raise `HK_SYSMEM` above 64 GiB, or `LOGSCOPE_MAX_GPU_GB` at all, without the human.
* Do not edit `.wgsl` or Rust. If a measurement says a kernel needs changing, report that and stop.
* Do not report a number you cannot point at a JSON for.
* Do not keep going after a GPU hang. Report the ladder file and stop.
