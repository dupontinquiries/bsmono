# Upscaling architecture — what's built, what's the hole, what's research

## The shape

Per-group toggle (`UpscaleConfig` on `MergeGroup`) drives a block-
processing layer that sits between the video pipeline and a swappable
model:

```
frames/chunks
    │  extract blocks (with provenance: clip,frame,x,y)
    ▼
DedupBatch ── fingerprint ── process only UNIQUE blocks ──► BlockProcessor
    │                                                          (the hole)
    │  fan results back to every occurrence by BlockId
    ▼
reassemble frames (provenance-validated write-back)
```

## What's built now (real, pure Rust)

- `blockproc.rs`: `Block` + `BlockId` (provenance token), `DedupBatch`
  (dedup + fan-out), `BlockProcessor` trait (the swappable hole),
  `validate_writeback` (the "can't be reapplied wrong" guard).
- `basic_upscaler.rs`: `BasicUpscaler` fills the hole. Reference-transfer
  when a clean reference block matches; bilinear blind upscale otherwise.
  Honest: copies real pixels or smooths — does NOT hallucinate.
- `UpscaleConfig`: per-group enable / factor / use_references /
  reference_clip_indices. Persists. Off by default.

## The contract (why provenance matters)

A processed block carries the `BlockId` of the exact slot it came from.
`result_for(id)` returns processed pixels stamped with the QUERIED id, so
repeated content shares pixels but each occurrence can only be written
back to its own location. `validate_writeback` rejects any mismatch.
This is the "agreement that the video portion is unable to be reapplied
[wrong]" — placement errors are hard errors, never silent.

## The hole (swap a better model in here)

Implement `BlockProcessor` with anything: an ESRGAN/RefSR network, an
out-of-process Python+PyTorch worker, an Asahi/Mesa Vulkan compute path.
The contract is fixed (preserve BlockId, one output per input); the
dedup + provenance + reassembly machinery is unchanged.

## The gap (research, NOT built)

1. **Verification gating.** The spec wants: synthesize detail, then VERIFY
   against real footage, apply only where it checks out. The current
   `BasicUpscaler` does reference-MATCHING (a cheap content-distance
   gate) but not output-VERIFICATION. A real version would compare the
   synthesized block against reference content and keep it only above a
   confidence threshold. This needs the model + a verification metric.

2. **Block extraction/reassembly from real video.** The layer operates on
   `Block`s; wiring it to pull blocks out of decoded frames and write
   them back into frames for re-encode is not yet connected to the
   pipeline. That's the next integration step.

3. **The actual model.** Neural SR needs an inference runtime. Target per
   the user: Falcon + Mesa on Asahi as the standard; Vulkan/honeykrisp
   compute. Until then, `BasicUpscaler` keeps the pipeline real.

## Why this ordering

The model is the uncertain, research-y, runtime-heavy part. The data
prep + provenance contract + dedup is the certain, reusable, pure-Rust
part — and it's the foundation everything else needs. Building it first
means when a real model arrives, it drops into a tested hole instead of
needing the whole scaffold built around it under time pressure.
