# GPU hangs & watchdog timeouts — causes, fixes, and how they map to LogScope

Compiled from how games, CUDA apps, and GPU compute tools (Substance Painter, Metal apps) handle GPU
crashes, mapped onto our wgpu compute engine. Written after the Llama-3.2-3B Asahi/Vulkan hang
(`asahi gpu: GPU timeout / Halted: 1 / Attempting recovery` in dmesg).

## The one mechanism behind almost all of it: the watchdog timer

Every GPU platform has a watchdog that kills any single GPU job that runs too long, on the assumption a
job that long must be a hang:

| Platform | Watchdog | Default limit | User-tunable? |
|---|---|---|---|
| Windows | TDR (Timeout Detection & Recovery) | ~2 s | Yes — registry `TdrDelay`/`TdrDdiDelay` |
| Linux + NVIDIA/X11 | X11 display timeout / `cudaErrorLaunchTimeout` | ~2–5 s | Partly (headless, persistence mode) |
| Linux + Asahi (Apple GPU) | **firmware timeout** ("GPU timeout nya~") | ~1–2 s | **No user knob** |
| macOS + Metal | command-buffer execution limit | a few s | No |

**The universal symptom:** a *single dispatch / command buffer* runs longer than the limit → the driver
assumes the GPU hung → it halts and resets the GPU → the app either crashes or the work silently dies.
It appears after a delay of several seconds, and it shows up *only on large inputs* (small ones finish
under the limit). That "only on big data" signature is the tell that it's a watchdog, not a logic bug.

**The universal fix (every source agrees):** don't raise the timeout — *split the work* so no single
dispatch exceeds it. Raising the timeout (Windows registry) is a per-machine workaround the app can't do
for the user and that Windows updates reset; on Asahi there's no knob at all. Splitting is the real fix
and it's portable.

## What this means for us specifically

Our engine is pure GPU compute (matmuls, attention) via wgpu. The dispatches whose runtime scales with
input — and therefore can cross a watchdog on a big enough prompt/model — are:

1. **Prefill.** Processing the prompt. Each block does projections at `M = chunk` and attention over the
   growing cache. A big chunk over a long prompt = a long dispatch. → **Fix shipped:** `PREFILL_CHUNK`
   is env-tunable (`LOGSCOPE_PREFILL_CHUNK`); lower it (64, 32) to shorten each prefill dispatch under the
   watchdog. This is the direct analog of the CUDA "process in smaller chunks" and Metal "split into
   multiple grids" advice.
2. **Large output projection** (`n = vocab ≈ 128k`, `k = d`). One big mat-vec per token. On Asahi the
   *subgroup* variant of this kernel is the extra risk (see below).
3. **Long-context decode attention.** Bounded already by the flash split-K kernel (chunks the KV so
   parallelism grows with context instead of one workgroup crawling all of it).

## Asahi/Honeykrisp (Apple GPU on Linux) — the extra-fragile bits

This is the tightest driver we target and needs the most caution:

- **Incomplete subgroup support.** It advertises the Vulkan `SUBGROUP` feature, but subgroup ops can
  misbehave (hang or miscompute) on large workloads. A tiny load-time self-check passes (it only checks
  correctness on a small shape) yet the kernel can still hang at scale. → **Fix shipped:** default the
  subgroup mat-vec kernel OFF on Asahi (`conservative_vma` detection), use the barrier-reduction kernel;
  `LOGSCOPE_MATVEC_SG=1` re-enables for experiments.
- **Allocation-count / VMA pressure.** Tracked by the governor (Vulkan `AllocationCount` budget, tighter
  for Apple-Vulkan). Separate from watchdog hangs, but same "Asahi is stricter" theme.
- **No TDR knob.** Can't raise the timeout; must split work.

## Diagnostic playbook (what to do when the GPU hangs)

1. **Confirm it's a watchdog**, not a CPU stall: `journalctl -b -f` (or dmesg) during the hang. A
   `GPU timeout / Halted / Attempting recovery` line = watchdog. A frozen CPU with no GPU log = a
   host-side wait (e.g. a submit governor) — different fix.
2. **Locate the phase** from the app log: which line printed last? Before `decoding…` = prefill; after =
   decode. (Ours: the hang bracketed prefill.)
3. **Bisect the dispatch size:**
   - Prefill suspected → `LOGSCOPE_PREFILL_CHUNK=64`, then 32. If a smaller chunk clears it, it was a
     prefill dispatch; the value that first works tells you the watchdog threshold on this GPU.
   - Decode/subgroup suspected → `LOGSCOPE_MATVEC_NOSG=1` (barrier kernel) or `LOGSCOPE_FLASH_ATTN=0`.
4. **Isolate the kernel** with the env flags (each disables one suspect): `LOGSCOPE_FUSED_QKV=0`,
   `LOGSCOPE_PREFILL_TILED=0`, `LOGSCOPE_NO_AUTOTUNE=1` (rules out a bad cached tile), `LOGSCOPE_DECODE_TILE=2`
   (force a small tile).

## Design principles going forward (so we stop hitting this)

- **Cap every input-scaled dispatch.** Any dispatch whose runtime grows with prompt length, context, or
  vocab should have a bounded work-per-dispatch, tiling internally past a threshold — like the flash
  attention kernel already does. New kernels (e.g. the staged-KV two-span attention) inherit this: they're
  built on the flash split-K structure, so they're already chunked.
- **Prefer many small dispatches over one huge one on fragile drivers.** A modest throughput cost buys
  not-wedging the GPU. The governor's philosophy (measure the device, steer conservatively) applies here
  too — a per-backend "max dispatch work" budget is the natural next step if hangs recur.
- **Detect-and-degrade, per backend.** We already do this for subgroups and VMA on Asahi; a watchdog-aware
  prefill-chunk default (smaller on Asahi) is the same pattern and a candidate once the threshold is known.
- **A load-time probe can't catch scale-dependent hangs.** Correctness self-checks run on tiny shapes;
  they won't reveal a kernel that only hangs at n=128k. Trust conservative defaults on fragile drivers over
  "it passed the check."

## Open follow-ups

- Make the Asahi prefill-chunk default smaller automatically (once the safe threshold is measured on
  hardware), instead of relying on the env override.
- Consider a per-backend "max work per dispatch" in the governor, so any input-scaled dispatch auto-splits
  to stay under an estimated watchdog budget — turning this from a manual knob into another self-tuning
  guard, consistent with how the window governor works.
