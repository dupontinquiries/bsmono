# TODO: wake-gate efficiency tuning (reduce wasted whisper runs)

## Done (v0.1.632)
Extracted the inline wake-listen whisper-gate into pure, tested
src/wake_gate.rs: wake_gate(registered, acoustic, burst_samples,
sample_rate) -> WakeGate { Skip(ShortLowAcoustic|NoiseFloor), RunWhisper }.
Behavior-identical to the old inline logic (thresholds 0.40 match / 0.005
noise-floor / 1s short-long split); voice.rs now delegates. 7 tests.

## The efficiency opportunity (needs dogfdood logs to tune safely)
The logs show the MIDDLE BAND firing on ambient chatter:
  "long burst with low acoustic (0.227) — running whisper anyway"
i.e. acoustic in [0.005, 0.40) + burst >= 1s → full ~1s whisper, on the
theory it might be an "ask ai <question>". When it's actually ambient
speech, that whisper is wasted CPU (and a hallucination risk).

Candidate refinements (all now a one-line change in wake_gate + a test):
1. Scale the burst-length requirement by how low the acoustic is: e.g.
   acoustic 0.005–0.15 needs burst >= 2s (real questions are long),
   0.15–0.40 keeps the 1s bar. Cuts whisper on short-ish ambient.
2. Add a NoiseFloor upper nudge: if acoustic < ~0.05 AND the burst has
   low pitch-variance (monotone = HVAC/road noise, not speech), skip.
   Needs a cheap variance signal passed in (pure-friendly).
3. Track a rolling false-trigger rate; if the middle band is mostly
   producing empty whisper results ("" transcriptions), raise the floor
   adaptively.

DO NOT guess the thresholds — capture a WEB2QML session in a noisy room,
grep "running whisper anyway", and see how many produced a real command
vs "". Tune option 1's cutoffs from that ratio. Each change is pure +
testable in wake_gate.rs before wiring.

## v0.1.645 — THE NOISY-ROOM LOG ARRIVED (the data this TODO asked for)
A v0.1.642 session log captured the middle band firing repeatedly. Every
single "running whisper anyway" this session (acoustic 0.199–0.275, all
LONG bursts 0.65–4.0s) produced GARBAGE — zero real commands:
  0.231→"go back to the end of the video" (TV/video audio, not the user)
  0.248→"run, run cargo test ×6" (hallucination loop)
  0.203→"okay"  0.238→""  0.275→"go to the end of the video"
  0.266→"okay"  0.249→"go to the end"  0.273→"run cargo build, run cargo test"
  0.248→""  0.246→""  0.234→""  0.260→""  0.262→""  0.199→"run"
  0.246→""  0.237→"...click the link in the description"  0.252→"..."
Summary: ~18 middle-band whisper runs, 0 real commands, mix of empty /
hallucination-loop / video-bleed. Acoustic clustered tightly 0.20–0.28.

### What the data supports (and what it does NOT)
- The middle band [0.005, 0.40) is pure cost in this (TV-on) environment.
- BUT I have NO positive "ask ai <question>" examples in this log, so I
  STILL can't safely raise the lower floor — a real ask-ai phrase scores
  low vs "hey apple" too, and I'd break it blind.
- The distinguishing signal that IS in the data: the garbage runs are
  EMPTY or hallucination-LOOPS. The loop trimmer already catches repetition;
  empties are already discarded. So the wasted COST is the whisper run
  itself, not a wrong action — important: no false commands fired.

### Safest data-informed lever (still needs a positive ask-ai sample to confirm)
Track a rolling middle-band hit rate: if the last N middle-band whisper
runs produced ≥~90% empty/looped/no-parse results, raise the effective
floor adaptively for a cooldown window (back off when a real command lands).
This adapts to a noisy environment without a static threshold that breaks
ask-ai in a quiet one. Pure to implement in wake_gate (pass in a recent
hit-rate); needs the caller to feed results back. Deferred until there's a
positive ask-ai example to set the backoff so it can't starve real use.

## Related finding (v0.1.645): pitch-hint source is wrong for COMMAND queries
Same log: "[phonetic] rejecting stale pitch hint: hint=709Hz vs query=..."
fired on ~every utterance. ROOT: pitch_hint() returns the WAKE-WORD profile
(709Hz avg of "hey apple") even for command dictation whose pitch is
350–680Hz. The 1.3× staleness check then (correctly) rejects it nearly
every time — the rejection is RIGHT (it's the v0.1.258 cross-speaker fix),
but the hint SOURCE is poorly chosen for non-wake queries. Fixed the log
SPAM (throttled the rejection log to every 10th, behavior-neutral). The
deeper fix — hint command queries with the SPEAKER's recent speech pitch
(cached_speaker) rather than the wake-word profile — is NOT done: it
touches the speaker-ID path the 1.3× check protects, so it needs care +
a test, not a blind reorder of pitch_hint()'s source preference.
