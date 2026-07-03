# snapclone-gstpipeline

The pure decision logic of `GstLaunchBackend`, extracted so it can be tested
without GStreamer, pipewire, or Qt — and so the FP5 "not-negotiated on resume"
bug stays fixed.

## What it is (and isn't)

It does **not** simulate a real GStreamer pipeline or libcamera negotiation —
that's a hardware/driver interaction, and a fake of it would only prove what we
told it to. What it models is the backend's *control flow*:

- `classify_error(stderr)` → `GstError` (Negotiation / GlLink / MissingNode /
  Other), with the same precedence the C++ uses (GL-link before negotiation).
- `RecoverySm` — the negotiation-failure recovery ladder and the per-session
  guard lifecycle: settle-retry the same node → relaxed-caps the same node →
  swap node, plus the GL-link "force CPU + retry" path. Its fields map 1:1 onto
  the C++ member flags (`m_negotiationSettleRetried`, `m_relaxCaps`,
  `m_gpuConvertFailed`, `m_isRecoveryRestart`, `m_active`, `m_currentNode`).

## The regression it locks down

`stop()` doesn't clear the current node, so resuming the *same* sensor used to
take the "node unchanged → don't reset guards" path and inherit a stale
settle-retry guard — turning a transient `not-negotiated` into a terminal
failure (observed on the FP5: camera_29 failed on resume with no settle-retry,
while streaming fine before and after). The fix resets the per-session guards on
any fresh session (`on_start` while inactive, not during our own recovery
relaunch). The test `resume_same_sensor_can_still_settle_retry` reproduces the
scenario; reverting the fix makes that test fail (verified), so it genuinely
guards the bug.

## Status

16 tests (13 model + 3 FFI); in-workspace `cargo test -p snapclone-gstpipeline`
green; header verified C and C++; staticlib linked + run from C++ replaying the
FP5 resume scenario. Zero dependencies. The C++ backend currently keeps its
inline copy of this logic (now matching this model); driving it from this crate
via the FFI is the optional next step.
