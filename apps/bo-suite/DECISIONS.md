# DECISIONS.md

Non-obvious design choices in this repo. Each entry carries an
epistemic-status tag: **anchored** (tied to an external fact),
**considered** (alternatives evaluated), or **default** (first
thing tried, works, nobody investigated alternatives). `default`
entries invite revisiting; `considered` entries discourage casual
change.

This file complements but does not replace README.md or other
docs. Where a topic is explained more fully elsewhere, an entry
here is a short pointer.

---

**Threading & IPC**

## Kill subprocess BEFORE joining the reader thread in Drop

**Status**: considered
**Location**: `bo-controller/src/sanitizer_driver.rs`, `bo-controller/src/mic_capture.rs`

The previous ordering was: set stop_flag → `thread.join()` → kill child. If
paplay/parec was paused or stuck (SIGSTOP, full pipe buffer, slow consumer),
the reader thread was blocked in `stdin.write_all` / `stdout.read` and would
never re-check `stop_flag`, so `join()` hung forever. The fix kills the
subprocess first — that closes the OS-side pipe, the in-progress I/O call
returns immediately with EPIPE/EOF, the thread exits, and join completes
promptly.

Rejected: a `join_with_timeout` wrapper (std doesn't ship one, would need a
sentinel channel) and SIGKILLing the thread itself (not portable, unsafe).

---

## `poll_results` is a 100ms QML Timer that drains worker channels

**Status**: default
**Location**: `bo-controller/qml/volume_knobs.qml:40`, `bo-controller/src/main.rs` (`poll_results` method, `drain_*` helpers)

Worker threads send result messages over `mpsc::channel`s; the QML side runs
a `Timer { interval: 100 }` that calls `qtobj.poll_results()`, which
`try_recv()`s every channel until empty and updates qt_properties.
Alternative: Qt cross-thread signals from each worker, which would avoid the
polling but requires more `qmetaobject` plumbing per channel. Revisit if
the poll loop ever shows up as battery cost or feels laggy at the 100ms
granularity — for now it costs ~10 try_recv calls per tick on empty
channels, which is nothing.

---

## `mux_bridge::daemon_call` uses 1-second read/write timeouts

**Status**: considered
**Location**: `bo-controller/src/mux_bridge.rs`

The unix-socket I/O used to be untimed. A hung `bo-muxd` (deadlock, slow
fsync, paused under debugger) would block the QML thread indefinitely.
1 second is generous enough that legitimate slow commands (scene-load that
stats files) complete, tight enough that a truly stuck daemon manifests as
"empty refresh" instead of a frozen UI. Connect itself is left untimed
because UDS connect is kernel-local and essentially instant; the lurking
hang is on read after sending the request.

Rejected: 100ms (too tight for scene-load), 5s (long enough to feel like a
freeze), and async-via-thread-per-call (architecturally bigger; the daemon's
own command path should evolve toward async-by-design rather than the
client side guessing).

---

## `bo-muxd` runs as a separate daemon over a Unix socket

**Status**: considered
**Location**: `bo-mux/` crate, `bo-controller/src/mux_bridge.rs`

The GUI is a thin client that talks NDJSON to the daemon over
`$XDG_RUNTIME_DIR/bo-mux/control.sock`. Alternative considered: run the
soundscape engine in-process inside `bo-controller`. Rejected because
soundscapes need to survive GUI restarts, be systemd-managed, and let
multiple frontends drive one playback engine. Full rationale in
`bo-mux/DAEMON.md` §"Why a daemon + socket (not in-process)".

---

## Stem-split / diarize workers use `std::thread::spawn`, not tokio

**Status**: default
**Location**: `bo-controller/src/main.rs` (`process_stem_split`, `process_diarize`)

The pipeline is CPU-bound (STFT/HPSS/MLP/ISTFT) and writes WAV files
synchronously via `hound`. There's no I/O concurrency to gain from tokio,
and the main reqwest/tokio runtime is reserved for network calls. Plain OS
threads keep the dependency boundary obvious. Revisit if these workers
ever need to interleave with network requests (e.g. fetch a model file
mid-run) or need cancellation.

---

## Sanitizer driver retunes via `AtomicU32(freq.to_bits())`

**Status**: default
**Location**: `bo-controller/src/sanitizer_driver.rs`

The audio generator thread polls `AtomicU32` for the current fundamental
frequency once per chunk and reconstructs the `f32` via `from_bits`. This
is wait-free and avoids a mutex around what's effectively a single-float
control parameter. Alternative: a `Mutex<f32>` (would block the audio
thread under contention), an `Arc<RwLock<f32>>` (overkill), or a SPSC
channel (more allocations). Revisit if more control parameters appear —
at that point a small struct in an `Arc<Mutex<_>>` becomes simpler.

---

**Performance Constants**

## STFT window 1024 / hop 256

**Status**: default
**Location**: `bo-controller/src/stem_split.rs:53-65`

Window of 1024 samples ≈ 23 ms at 44.1 kHz, a reasonable balance of
frequency resolution (43 Hz per bin) and time precision. Hop 256 = 75%
overlap, which is the common figure for source separation. The code
comment justifies the choice but no alternatives (512/128, 2048/512) were
benchmarked against real material. Revisit if separation quality is
poor on transient material (try smaller window) or smearing dominates
on harmonic content (try larger).

---

## HPSS uses 17-frame time and 17-bin frequency median filters

**Status**: default
**Location**: `bo-controller/src/stem_split.rs:HPSS_HARMONIC_FRAMES`, `HPSS_PERCUSSIVE_BINS`

17 was picked because at hop=256, sr=44100 it's ≈ 100 ms of time context
(reasonable for harmonic continuity) and ≈ 730 Hz of frequency context
(reasonable for percussive sparsity). Odd number keeps the median
unambiguous. The literature uses 17 sometimes (Fitzgerald 2010 uses 17 and
17) and 31 sometimes (FitzGerald & Gainza 2010); the choice for this code
was not benchmarked. Revisit if drums leak into the harmonic stem on
slow tracks (try larger time median) or harmonic content bleeds into
drums on dense mixes (try larger frequency median).

---

## `HPSS_POWER = 2.0` for soft Wiener masks

**Status**: default
**Location**: `bo-controller/src/stem_split.rs:HPSS_POWER`

Mask formula is `m_h = h^p / (h^p + p^p)`. p=1.0 is plain Wiener; p=2.0
is "soft-binary" — more aggressive separation, slightly more audible
musical-noise artifacts. p=∞ is hard binary masking with the harshest
artifacts. 2.0 is a common middle ground but wasn't tuned for this code.
Revisit if outputs sound watery (try p=1.0) or if separation is too
gentle (try 4.0).

---

## `SmallMlp` hidden layer size 128

**Status**: default
**Location**: `bo-controller/src/stem_split.rs:MLP_HIDDEN`

The total parameter count `513×128 + 128 + 128×1539 + 1539 = 263,427`
keeps the model under 1 MB as f32 — small enough to bundle as binary data
without bloating the release archive. 64 hidden units would be 132k params
and risk underfitting; 256 would be 525k and start to feel heavy. No
training was run against alternatives. Revisit when actual training data
exists and capacity vs. overfitting can be measured.

---

## Diarization runs at 16 kHz with 25 ms / 10 ms frames

**Status**: anchored
**Location**: `bo-controller/src/podcast_diar.rs:TARGET_SR`, `FRAME_SAMPLES`, `HOP_SAMPLES`

These three numbers (`16_000`, `400 = 16_000 × 0.025`, `160 = 16_000 ×
0.010`) are the de-facto standard for telephony-bandwidth speech analysis
across MFCC pipelines, x-vector papers, and pyannote.audio. Picking
anything else would diverge from training-data conventions if a real
neural model ever drops in. Anchored to general speech-recognition
practice; no single canonical citation but ubiquitous in the literature.

---

## `SHELF_Q = 0.707`, `PEAK_Q = 1.0` in the realtime EQ

**Status**: anchored
**Location**: `bo-controller/src/realtime_mixer.rs:SHELF_Q`, `PEAK_Q`

0.707 (= 1/√2) is the Butterworth-flat shelf Q from the RBJ Audio EQ
Cookbook — gives no overshoot at the corner. 1.0 is the conventional
peaking-EQ Q for a moderately focused bump (~1 octave wide). Anchored to
RBJ Audio EQ Cookbook conventions; values are not exposed to the user
and shouldn't change without re-measuring the speaker/instrumental band
crossover behavior.

---

## Biquad Q is clamped to `[0.01, 100.0]` internally

**Status**: considered
**Location**: `bo-controller/src/realtime_mixer.rs` (`peaking`, `low_shelf`, `high_shelf`)

`alpha = sin(ω)/(2q)` produces ∞ at q=0 and NaN coefficients downstream.
The clamp turns pathological callers (or future bugs) into "extremely
sharp filter" rather than silent NaN-poisoning of every subsequent sample.
0.01 is below any musical Q anyone would use intentionally; 100 is
already a needle-thin filter. Rejected: returning identity for `q ≤ 0`
(would hide the bug instead of degrading gracefully); panicking (audio
thread must never panic).

---

## `process_buffer` sanitizes NaN at buffer boundaries, not per sample

**Status**: considered
**Location**: `bo-controller/src/realtime_mixer.rs:process_buffer`

A single non-finite input would poison the biquad delay line (z1, z2)
and turn every subsequent output into NaN until `reset()`. Two-layer
fix: (1) replace non-finite inputs with 0 before processing, (2) after
the buffer, if any filter's state went non-finite anyway, reset all
filters. The per-sample input check is one `is_finite()` per channel
which is a single bit comparison; the per-buffer state check is six
`is_finite()` calls per buffer, amortized over hundreds of samples.

Rejected: a per-sample `is_finite()` after the biquad output (would add
a branch to the hot path with no clear win since the buffer-boundary
check already catches it on the next buffer).

---

## VAD noise floor is the 10th-percentile RMS across all frames

**Status**: considered
**Location**: `bo-controller/src/podcast_diar.rs:voice_activity_mask`

Original implementation used the median RMS of the first 1 second of
audio as the noise floor. That failed any recording that didn't start
with ≥1 s of silence — the median included loud frames and the threshold
became too high to detect speech. The 10th-percentile approach assumes
at least 10% of any recording is quiet (room tone between utterances,
inter-sentence breaths, intro/outro silence), which holds for almost
every podcast.

Rejected: shorter first-window (300 ms — still fails if a recording starts
mid-sentence), explicit silence-detector pre-pass (more complex for no
gain), and adaptive per-segment noise (would require a second pass).

---

## `CLUSTER_JOIN_THRESHOLD = 0.4`, `MAX_SPEAKERS = 6` for diarization

**Status**: default
**Location**: `bo-controller/src/podcast_diar.rs`

Cosine distance below 0.4 → join nearest cluster; above → start a new
one (until `MAX_SPEAKERS`). 0.4 is an eyeball; 6 is "more than any
podcast format that isn't a roundtable" and prevents runaway cluster
growth on noisy material. Neither was tuned against labeled data.
Revisit if real podcasts produce over-segmentation (lower threshold) or
mash distinct speakers together (raise threshold); revisit MAX_SPEAKERS
if anyone diarizes panel discussions or seminars.

---

## Circuit breaker uses 30 s failure window, 10 s half-open probe timeout

**Status**: default
**Location**: `bo-controller/src/circuit_breaker.rs:FAILURE_WINDOW`, `HALF_OPEN_PROBE_TIMEOUT`

3 consecutive failures within 30 s → OPEN. After 5 s backoff (doubling
to 60 s) the breaker enters HALF_OPEN; the first attempt is the probe
and the breaker reserves a 10 s window for its outcome before allowing
another. The 10 s came from "long enough for a normal request including
DNS + handshake + body, short enough that a wedged probe doesn't lock
out recovery." Not benchmarked. Same caveat applies to `conn_warmer`'s
20 s warm interval and `aggression`'s 0.55/0.45/0.35 confidence floors —
all eyeballed and unevaluated against real session data. Revisit when a
test corpus of real session logs can measure prediction accuracy.

---

**Cross-System Contracts**

## `SmallMlp` save format: 4-byte magic "STEM" + u32 version

**Status**: default
**Location**: `bo-controller/src/stem_split.rs:save_to_bytes`, `load_from_bytes`

Compact little-endian binary: magic, version, n_bins, hidden, output_rows,
then row-major f32 weights. `load_from_bytes` validates magic, version,
and shape before returning Some, and returns None (never panics) on any
mismatch. Magic discriminates from other f32 dumps; version reserves the
right to evolve the layout. No serde format because the file is
performance-sensitive and binary is faster to read/write than JSON of
263k floats. Revisit if a non-Rust trainer needs to produce these files
(consider adding ONNX export).

---

## WAV bit-depth scaling uses `1u64 << (bits - 1)`, not `1i32 << (bits - 1)`

**Status**: anchored
**Location**: `bo-controller/src/main.rs:wav_to_mono_f32`

For 32-bit integer WAV, `1i32 << 31` overflows to `i32::MIN`
(-2147483648), making the scale negative and flipping every sample's sign
on conversion to f32. The u64 shift produces the correct 2^31. Anchored
to two-level integer arithmetic — `1u64 << 31` is well within u64 range
and the subsequent `as f32` produces +2147483648.0 exactly.

---

## Children inherit `PR_SET_PDEATHSIG = SIGTERM` on Linux

**Status**: anchored
**Location**: `bo-controller/src/main.rs` (search `PR_SET_PDEATHSIG`)

Tokio's `Command` pre_exec hook calls `prctl(PR_SET_PDEATHSIG, SIGTERM)`
in the child between fork and exec. When this controller process dies by
any means (clean exit, SIGKILL, window-manager close, crash), the kernel
sends SIGTERM to all such children — paplay, ffmpeg, parec — instead of
reparenting them to init and leaving them playing audio after we're gone.

Anchored to Linux kernel API `prctl(2)`. Linux-only (the suite is
Linux-only — see `bo-controller/Cargo.toml` `[target.'cfg(unix)']`).

---

## NDJSON over `$XDG_RUNTIME_DIR/bo-mux/control.sock` for daemon control

**Status**: considered
**Location**: `bo-mux/src/api.rs`, `bo-controller/src/mux_bridge.rs`

One JSON object per line, request/response correlated by `id`. Alternative:
gRPC (heavier dep), DBus (more conventional on KDE but couples to the
session bus lifecycle). NDJSON is the minimum-viable line protocol — every
language can speak it, `jq` debugs it, no schema-evolution headache for a
small command set. Full rationale: `bo-mux/DAEMON.md` §"The API (api.rs)".

---

**UI Patterns**

## Equal-power pan uses `cos(θ)` / `sin(θ)` mapping

**Status**: anchored
**Location**: `bo-controller/src/stem_stage.rs:equal_power_pan`

Maps pan ∈ [-1, 1] to θ ∈ [0, π/2], L gain = cos θ, R gain = sin θ. The
identity `cos²θ + sin²θ = 1` keeps perceived loudness constant across the
pan field — fuzz-proven across 200 random pan/sample combinations.
Alternative linear panning has a 3 dB dip at center which sounds like the
sound recedes when moved off-axis.

Anchored to standard audio engineering practice (e.g. Pohlmann,
*Principles of Digital Audio*; also covered in any DAW manual under
"pan law").

---

## STFT uses reflective padding at signal edges

**Status**: anchored
**Location**: `bo-controller/src/stem_split.rs:stft`

The signal is reflectively padded by `STFT_WINDOW/2` samples on each
side so the first hop's window is centered on sample 0. This matches
the librosa default — important so any neural weights trained externally
against librosa-prepared spectrograms align with what this code produces
at inference time.

Anchored to librosa's `stft` documentation
(`librosa.stft(..., center=True, pad_mode='reflect')` defaults).

---

## Stem stage default placements: voice center, piano L 30°, synth wide Haas

**Status**: default
**Location**: `bo-controller/src/stem_stage.rs` (`StemPlacement::voice_default` etc.)

Voice dead center (vocal-forward mixes feel natural), drums center with
2 ms pre-delay on R (slight body without losing the kick), piano left
30° with 12 ms early reflection (mimics concert-stage placement), synth
right 30° + 15 ms Haas widening (pads/leads feel big without phase
cancellation). The Haas range (5–25 ms) is anchored to the precedence
effect literature, but the specific values per stem were eyeballed.
Revisit when real users describe what feels uncentered or smeared.

---

## Bo design tokens live in a single QML singleton

**Status**: default
**Location**: `bo-controller/qml/Bo.qml`

Motion curves (`settle`, `spring`, `smooth`), color palette (mauve, teal,
sage, rust, terracotta, steel; success/warning/danger/info), type scale,
and durations all draw from one place. Alternative: per-page constants
that drift over time. The design-system approach was a choice but its
rationale is documented in `bo-controller/README.md` §"Motion +
materials, centralized".

---

**Sim Framework & Testing**

## Pure-model state-machine simulator separate from `main.rs`

**Status**: considered
**Location**: `bo-controller/src/sim/`

The sim is a parallel implementation of the controller's routing logic
in pure functions (no I/O, no Qt, no async) so 1M+ random transitions
per `cargo test` run cost ~8 s wall clock. The model mirrors `QtState`'s
fields verbatim so a future refactor can pull the real routing decisions
into a shared module. Alternative considered: shadow the actual main.rs
via an `EventBus` indirection (more invasive, slower tests, real I/O
risk in fuzz). Alternative: integration tests against real devices
(can't get 1M iterations on a Beosound Stage).

---

## Sim invariants check after every transition

**Status**: default
**Location**: `bo-controller/src/sim/fuzz.rs`, `bo-controller/src/sim/invariants.rs`

All 17 invariants run after each `apply_action` in fuzz tests. Per-step
checking gives a deterministic `(seed, walk_index, step_index)` triple
that reproduces any violation. Cost is O(invariants × state size) per
step but each invariant is cheap (hash lookups, set membership). No
attempt was made to interleave or sample-check; revisit only if fuzz
runtime grows past a useful budget.

---

## `AntiToneGen` phase wraparound uses `rem_euclid(2π)`

**Status**: considered
**Location**: `bo-controller/src/cancel_tones.rs:next_sample`

Original code did `if phase > 2π { phase -= 2π; }` — a single
subtraction. That works for per-sample phase increments < 2π, but at
high fundamental × high harmonic × low sample-rate combinations the
increment can exceed 2π and phase grows unbounded until f32 precision
degrades. `rem_euclid(2π)` wraps cleanly for any positive increment
and always returns a non-negative result.

Rejected: a while-loop subtraction (works but verbose), and modulo via
`%` (returns negative values for negative inputs, requiring a branch).

---
