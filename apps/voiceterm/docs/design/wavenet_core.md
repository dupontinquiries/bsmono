# wavenet_core — shared neural audio-prediction core (v0.1.648)

## What & why
A WaveNet-style next-sample audio predictor in candle (pure Rust). One
core, three realtime-audio uses:
- web2qml: generate a FEW realistic utterance variations from a real voice
  sample to augment the fingerprint DB (better than many crude synthetic
  seeds — the user's "fuzz with less values but better ones").
- bo-mux (bo-suite): a neural upgrade to the classical phase-locked
  template predictor in bo-mux/src/predictor.rs (predict → play inverse for
  cancellation).
- the third realtime-audio project.

## Architecture (compiles + forward-pass verified)
Causal exponentially-dilated conv1d stack with gated (tanh·sigmoid)
activations, residual + skip connections, 1x1 output head → per-timestep
next-sample logits over mu-law buckets. Defaults are tiny (16 ch, 6 layers,
kernel 2 → receptive field 64) for "cheap/realtime". Behind
`--features wavenet` (candle-core + candle-nn, pure Rust, no C++/ONNX).
Verified: forward([1,1,N]) → [1,256,N], receptive_field=64, mu-law
round-trips. (Standalone candle harness + in-tree compile under the
feature.)

## HONEST scope (what is and isn't done)
- DONE: the architecture — it compiles, runs a forward pass, correct
  shapes, mu-law companding. A reusable core to train + call.
- NOT done: training. Fresh weights produce NOISE, not audio. Useful 1-2s
  prediction needs a training loop + real audio, and audio QUALITY can't be
  judged in a headless sandbox (no listening). Training + perceptual
  validation happen on the user's machine.
- Realtime caveat: autoregressive generation at 16kHz is O(samples)
  sequential. True realtime needs the fast-wavenet trick (cache per-layer
  dilation queues so each new sample is O(layers)). The offline forward
  path is what's verified; the cached generate_step is a documented TODO.

## Next steps (when prioritized)
1. A training loop (teacher-forced cross-entropy over mu-law targets) — can
   be written + compile-checked here; user runs it on real audio.
2. fast-wavenet cached generation for realtime.
3. The DB-augmentation wrapper: seed from a real fingerprint's source
   audio, sample N variations, add as fingerprints.
4. Factor into a shared crate bo-suite + web2qml both depend on (avoid
   copy-paste across the three projects).

## Training source + cross-project roles (user clarification, v0.1.648 session)
The "who trains it" question is answered by the EXISTING pipelines — the
core does not need a separate hand-labeled training corpus:

1. **Self-supervised from the whisper reference pipeline (web2qml).** Every
   utterance already flows audio → whisper transcript. That's a free,
   continuous stream of (audio, label) pairs. The predictor trains
   self-supervised on next-sample prediction over that same audio (no
   transcript needed for the acoustic model; whisper just confirms/segments).
   So training data = normal use. No extra collection.

2. **Music / realtime channel expansion (project 2).** The SAME small core,
   trained/conditioned on instrument+speaker-separated stems, predicts
   per-source continuation — used to drive channel expansion (upmix) by
   predicting what each separated source does next. (bo-mux already has
   upmix.rs + the separation surface.)

3. **Phone-calls app: predictive transmission (project 3).** Send BATCHES of
   PREDICTED audio ahead over the network, with a small CONFIRMATION packet
   arriving later to correct/accept — speculative execution for audio to cut
   perceived latency. The predictor generates the speculative batch; the
   confirmation reconciles. This reframes "realtime" from "generate sample-
   by-sample on time" to "generate a short predicted batch + reconcile" —
   which is MORE forgiving of the O(samples) autoregressive cost, and makes
   the batch the natural unit (not the single sample).

### What this changes in the design
- Training: add a self-supervised next-sample training loop fed by the
  whisper-pipeline audio stream (web2qml) — writable + compile-checkable
  here; runs on the user's machine. No separate dataset.
- The core stays SMALL + shared; conditioning differs per project (raw
  voice / separated stems / call audio).
- Add a BATCH-predict + CONFIRM/reconcile API (predict N ms ahead, then a
  correction step) — that's the shape projects 2 & 3 want, and it sidesteps
  the strict realtime sample-clock. Keep the offline forward path as the
  trainer; add generate_batch(seed, n) + reconcile(predicted, actual).
