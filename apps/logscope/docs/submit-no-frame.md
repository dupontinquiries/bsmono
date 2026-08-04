# 39% of submits happen with no frame open — on a DENSE, RESIDENT model

## The measurement

Qwen3-4B-Instruct-2507-Q4_K_S. Dense. Resident. No MoE, no streaming, no expert cache.

```
decode is 25.2x SLOWER PER TOKEN than this model's own prefill, on the same weights
submit causes  no-frame 6821 (39%), passes 7137 (41%), download 3543 (20%)
```

**The readback is 20%.** Everything in the Scout work targets that 20%. The other 80% is submission
structure, and it is present on a model with none of Scout's problems.

`no-frame` means a dispatch ran with no frame open, so it created its own encoder and submitted
immediately — one submit per dispatch instead of one per frame.

## Why this is surprising

`decode_step_q8` calls `begin_frame()` on its first line. So the frame IS opened, and 6821 dispatches
per run still escape it.

That means **the frame is being dropped mid-step and not reopened.** The known place this happens is
`download`, which drops the frame and reopens it only `if had_frame` — with the comment "a caller that
downloads without a frame does not want one opened behind its back". That guard is correct, so either
the reopen is not firing, or something else closes the frame.

`passes` at 41% is a related symptom: pass boundaries forcing a submit is what happens when work
cannot accumulate into one encoder.

## Why it matters more than the Scout chain

The Scout work — prescale, layer_can_route, IQ1_S residency — attacks the per-layer readback. That is
real and it is 20% of submits on this model. **The submission structure is 80%, applies to every
model, and is a property of the engine rather than of any checkpoint.**

25x slower per token than prefill *on the same weights* is the cleanest statement of it: prefill
batches its work into frames and decode does not.

## What to measure next

1. **Instrument where the frame closes.** The counter says 6821 dispatches ran frameless; a
   backtrace or a per-callsite tally on the `else` branch at lib.rs:3401 names them in one run.
2. **Check whether `download`'s reopen fires.** `had_frame` is captured before the drop; if decode's
   frame is already closed by something earlier, every subsequent dispatch is frameless and the
   reopen correctly declines.
3. **Then** decide whether decode can hold one frame across the whole token, which is what prefill
   effectively does.

## Unrelated, from the same log

```
fn main() -> Result<(), Box<dyn std::error ::Error>> {
```

A space inside `std::error ::Error`, in one of three otherwise identical lines. That is a
detokenization artifact, not model output — a BPE piece boundary emitted with a leading space where
the previous piece did not end one. Worth a separate look; it corrupts generated code silently.
