# uclamp — bias background voice work toward efficiency cores

Status: **DESIGN + module (v0.1.515).** A power-efficiency tuning:
hint the Linux scheduler to run NON-latency-sensitive background voice
threads at low utilization, which on heterogeneous (big.LITTLE / P+E)
phones and laptops biases them onto the efficiency cores at lower
clocks. The pure pieces (struct, util_max math, gating) are built +
tested; the actual syscall is Linux-only and can't be sandbox-verified.

## What uclamp actually is (verified against man pages + LKML)

Per-task "utilization clamping" (kernel ≥ 5.3), set via
sched_setattr(2) with SCHED_FLAG_UTIL_CLAMP. It hints the scheduler
about the [util_min, util_max] range (0..1024 scale) a task should run
at. The util affects BOTH frequency selection and task PLACEMENT.

For our goal, the lever is **util_max**:
  - "By setting a task's uclamp_max below 1024 ... it'll never be able
    to go above this utilization value" — caps the clock the task can
    drive.
  - "if util_max is smaller than or equal the capacity of the little
    cores, then the scheduler can still choose to place it there" — a
    low util_max BIASES placement onto E-cores.

## Three honest caveats (all verified, all handled by the design)

1. **It's a BIAS, not a pin.** "the scheduler will do its best." For a
   hard guarantee you'd use sched_setaffinity (CPU mask). We choose
   uclamp ON PURPOSE: it cooperates with EAS (energy-aware scheduling)
   instead of fighting it, and if the scheduler decides a P-core is
   needed (system busy), it still can — graceful, not brittle.
2. **Governor-dependent.** "Only schedutil cpufreq governor
   understands util clamp hints." On performance/ondemand the
   frequency effect is ignored (placement bias may still apply via
   EAS). So: a best-effort win, not a guarantee. Never assume it took.
3. **Kernel ≥ 5.3 + arch.** Probe at runtime; no-op cleanly otherwise.

## Critical scoping — WHAT to clamp (and what NOT to)

Clamping the wrong thread HURTS the product. The latency-sensitive
path (wake-word detection, live transcription feedback) must run FAST
— never clamp it. Only clamp threads where a small latency cost is
invisible but the power saving is real:
  - corpus WRITE thread (corpus_recorder: WAV write + opus encode) —
    already backgrounded, disk-bound, user never waits on it;
  - opus conversion (ffmpeg shell-out) — pure throughput;
  - embedding / speaker-vector computation (speaker_embed) — batch;
  - dead-air analysis (dead_air) — post-hoc, not real-time;
  - corpus reconciliation / index rebuilds.
NEVER: the audio capture callback, wake-word, the live ASR decode that
the user is waiting on.

## util_max target

E-core capacity varies by SoC (often ~30–50% of a P-core on the
1024 scale). We pick a CONSERVATIVE cap so "≤ little-core capacity"
holds on most devices: ECORE_UTIL_MAX = 256 (= 25% of 1024). Low
enough to sit at/under typical E-core capacity (biasing placement),
high enough that a clamped batch task still makes progress. Tunable.
util_min stays 0 (we never need to BOOST these; we only cap).

## API shape

`cpu_efficiency::clamp_current_thread_to_ecores()` — call from INSIDE
a background thread after spawn; clamps that thread (tid 0 = self).
`cpu_efficiency::is_supported()` — runtime probe.
Both no-op (return false) on non-Linux or kernel/permission failure,
never panic, never error the pipeline.

Implementation: libc 0.2 exposes SYS_sched_setattr (274 on aarch64)
but NOT the sched_attr struct or SCHED_FLAG_UTIL_CLAMP — so we define
the struct (stable kernel UAPI) + the flag (0x08 | 0x20|0x40 for the
util_min/max valid bits) and call via libc::syscall.

## What's tested (pure) vs deferred (live)

Tested: the sched_attr struct layout/size, util_max clamp value in
range, the build_attr() that fills the struct, support-gating returns
a bool not a panic. Deferred (needs real kernel + heterogeneous
silicon + a power meter): that placement actually moves to E-cores and
that battery improves — measure on the device with the governor set to
schedutil.

## Wiring (one line per background thread, when ready)

Inside each background spawn listed above, first statement:
`crate::cpu_efficiency::clamp_current_thread_to_ecores();`
Best-effort; safe to call unconditionally.

## v0.1.516 — conditional (screen-off) clamp via EcoGuard

User refinement: clamp the FINGERPRINT-DB background work ONLY when
the screen is off, so operations whose speed doesn't improve UX take
the efficiency path while the system is idle — but run unthrottled
when the user is present.

The app already tracks this: the window's onActiveChanged →
backend.set_app_backgrounded() → voice::is_app_backgrounded() (a
global atomic, debounced 1500ms, already used to slow the wake-poll
4x when backgrounded). So the screen-off / backgrounded signal exists;
EcoGuard just consumes it.

EcoGuard::new(defer): an RAII guard that clamps the current thread to
E-cores iff `defer` is true (pass is_app_backgrounded()), and lifts
the clamp on drop (so a reused thread isn't left throttled for later
foreground work). When defer=false (user present) it's a pure no-op —
a foreground op is NEVER slowed.

Wired: acoustic_index::rebuild_from_corpus_background (the O(N_corpus)
~10s MFCC index rebuild — fingerprint-adjacent, never user-waited).
NOT wired to phonetic::save_to_disk directly because that's also
called on the wake-word hot path (lines 10032/10087/10252) — clamping
there could hurt recognition latency. The startup stale_sweep is left
alone too: at startup the app is foregrounding, so EcoGuard would
correctly NOT clamp it (defer=false).

HONEST caveat: the guard samples is_app_backgrounded() ONCE at thread
start. If the user foregrounds mid-rebuild, the clamp persists until
that ~10s op finishes; the next rebuild re-evaluates. For a ~10s
best-effort op this is acceptable; a mid-flight re-check would add
complexity for little gain. Documented, not over-engineered.
