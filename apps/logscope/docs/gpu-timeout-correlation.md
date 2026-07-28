# Asahi GPU timeouts during Scout — what the correlation shows

## The window

Diagnostic report stamps against `journalctl -k | rg 'GPU timeout'`, same session:

```
21:11:53   Scout mmap+retain completes          (diag @@STAGES)
21:13:20   first GPU timeout                    (journal)   ← 87 s later
21:16:51   last GPU timeout                     (journal)
21:17:20   profile line printed, generation ends (diag @@SUBMITS)
```

Timeouts begin **87 seconds after the model is mapped** and continue until roughly the end of
generation. They do not start at load and they do not start immediately.

## What precedes them

Reported from the run: **thousands of expert fills and 100+ GiB moved** before the first timeout. At
~68 MiB per fill that is on the order of 1500 fills.

This rules out two explanations and leaves one:

* **Not a single oversized dispatch.** Those fail on the first occurrence, not after 1500 successful
  ones. The prefill chunk and conversion chunking were already sized against the watchdog and behave
  the same at fill 1 as at fill 1500.
* **Not a cold-start driver problem.** The first 87 seconds of heavy GPU work complete cleanly.
* **Consistent with a COMPLETION RACE in the staging ring** — the leading explanation, and better than
  accumulation. `end_frame()` orders SUBMISSION; it does not wait for COMPLETION. A fill is cheap on the
  CPU (one `write_buffer` plus a few recorded dispatches), so the CPU races ahead of the GPU — and on
  Honeykrisp nothing throttles queue depth, because the submit governor disables itself when completion
  callbacks are slow or absent. The CPU can be on fill 50 while the GPU still executes fill 10; slot
  `50 % 8` then overwrites the bytes fill 10 is mid-read.

  This fits the shape better than a leak does: **thousands of fills succeed, then faults begin and
  continue.** A leak degrades gradually from the start. A hazard that needs deep queues to manifest is
  clean until the CPU pulls far enough ahead, then fails constantly. The 87-second delay is the queue
  filling.

  Supporting detail: the **dims ring already fences with `flush_and_wait`**. The staging ring using the
  weaker primitive was an inconsistency with no recorded justification.

* **Also consistent with ACCUMULATION** — something that grows with fill count until the device degrades.
  Candidates, in the order worth testing:
  1. Retained bind groups and dims uniforms. `pending_keep` is only released on a poll, and with 99% of
     submits on the no-frame path the poll cadence is whatever `DIMS_RETAIN_CAP` forces.
  2. Buffer-object count. The governor's Metal/Vulkan budget models a soft cap; expert slot buffers plus
     per-dispatch dims could approach it over thousands of fills.
  3. Firmware queue depth on Honeykrisp specifically, where the submit governor disables itself because
     completion callbacks are slow or absent — so nothing throttles depth at all.

## What would distinguish them cheaply

Run the same generation with `LOGSCOPE_MOE_STREAM=0` on a model that fits resident. If the timeouts
disappear, the fill path is implicated rather than decode in general. If they persist, it is the
per-dispatch submit volume and not the fills.

The counter delta in `@@ERRORS` is the measure to compare across runs: the driver's `Timeout count:` is
cumulative, so only the delta says whether a change helped.


## Result: the completion fence removed 96% of them

Same model, same prompt, with the staging ring fencing on completion instead of submission:

```
before (_277, submit-only fence):  56 timeouts in a 3.5-minute burst
after  (_279, completion fence):    2 timeouts
```

That confirms the race. The CPU was lapping the 8-slot ring while earlier fills were still in flight, and
waiting for completion at each wrap stops it.

## CORRECTION: I cannot tell whether the survivors are a boundary

The analysis below claimed the two remaining timeouts are "mid-run, therefore not boundaries". That
claim is not supported by the data, and the reason is worth recording.

**A watchdog fires a second or two AFTER the GPU hangs.** The journal timestamp says when the fault was
NOTICED, not where the offending work sat in the sequence. "43% through the run" is entirely compatible
with "the first fill of a phase that begins at 43%" — and with only two events, a boundary is at least
as likely as a coincidence. Wall-clock position cannot separate those.

What the timings below DO establish is narrower: they are not RUN-level boundaries (not the first or
last thing that happened). They say nothing about per-layer, per-chunk or per-wrap position, because
nothing in the log records those.

`LOGSCOPE_PROGRESS_EVERY=N` now emits a positional breadcrumb every N fills, including one at each ring
wrap:

```
wgpu-engine: PROGRESS ring-wrap layer=0 fills=800 bytes=53.1GiB fence
```

With those in the diag report, a journal timestamp lands between two known positions and the question
becomes answerable instead of arguable. If faults cluster on wrap ticks, the ring is implicated; if they
fall between them, it is not.

## What the wall-clock timings show (run-level only)

```
Scout mmap done   +  0s
timeout #1        +141s   43% through the run
timeout #2        +218s   67% through
run ends          +327s
```

Neither is first nor last, and 109 seconds of clean running follow the second. An off-by-one on the
first wrap, or a teardown race, would land at an edge — these do not.

They fall in the **prefill** window (load completes around +60 s). This model prefills at **chunk 256**,
kept large by the MoE sparsity rule in `tuneprofile::prefill_chunk_moe` because refill traffic dominated
when each fill was cheap. A 256-token chunk through 48 layers makes each attention dispatch far longer
than decode's, which is the classic watchdog trigger — and it is a different mechanism from the ring
race, which is why fixing the ring did not remove these.

Note the rule's justification has weakened: with a completion fence, fills now cost a poll each, so the
"bigger chunk means fewer refills" argument is partly offset by the fence cost. The chunk size deserves
re-measuring rather than inheriting a verdict from before the fence existed.

**Cheap discriminator:** `LOGSCOPE_PREFILL_CHUNK=64`. If the last two disappear, it is dispatch
duration; if they persist, it is neither the ring nor the chunk and the search moves elsewhere.
