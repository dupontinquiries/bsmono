# Custom wake word never fires — root cause + fix (v0.1.676)

## Symptom (longstanding, user)
"I always use a different [wake word] than the default and it never fires."
Logs: `enrolled="hey apple"` ... `long burst rejected — acoustic=0.000
(noise, not a wake-or-AI phrase)` — the burst was vetoed BEFORE Whisper ran.

## Root cause
Two independent registrations back a wake word:
1. a TEXT phrase (wake_phrase.txt → registered_wake_phrase()) — checked by
   classify_wake_phrase, the GROUND TRUTH for a custom phrase.
2. an ACOUSTIC fingerprint (__wake_word__ in phonetic.rs) — a cheap prefilter
   score (wake_word_acoustic_score).

The gate (wake_gate) treated the acoustic score as a VETO: a long burst with
acoustic < 0.005 was skipped as `NoiseFloor`, so Whisper never ran and the
TEXT phrase never got checked. But a near-zero acoustic on a REAL utterance is
common when the enrolled fingerprint doesn't capture how the user actually
says the word in the current room/mic/conditions. So every genuine "hey apple"
whose fingerprint match happened to score ~0 was thrown away before
transcription → the custom word NEVER fired. (The default could appear to work
if its fingerprint enrolled cleanly; a custom word enrolled under worse
conditions would be permanently vetoed.)

The reasoning bug: acoustic ~0 was assumed to mean "definitely noise." It
actually means "the fingerprint didn't match" — which, for a custom phrase
the TEXT classifier knows, is not a reason to skip transcription.

## Fix (src/wake_gate.rs wake_gate_v2, wired in voice.rs)
A long burst now runs Whisper UNLESS SNR confirms it's ambient — i.e. its RMS
does NOT rise an ~1.8× margin above the rolling noise floor. So:
- real utterance (RMS rises over floor) → RunWhisper → text classifier checks
  the enrolled phrase → custom word FIRES. (the fix)
- café babble / broadband noise (no SNR rise) → still skipped (no Whisper
  waste — also helps the café-voice report).
- no noise-floor estimate → run long bursts (safe side: never miss a wake).
Acoustic >= 0.40 still fast-paths to RunWhisper; short low-acoustic bursts
still skip.

## Verification
wake_gate.rs 13 tests, incl. one that asserts the OLD gate skips the exact
burst the NEW gate runs (documents the bug). compile gate green; harness
1148 + 28. NEEDS the user's build to confirm end-to-end that "hey apple" now
fires — but the logic that was discarding it is removed, and the discard path
is precisely what the logs showed.

## Note
This also subsumes the v675 cafe_voice_triage primitive (noisy_env_skip) —
it's now the actual gate criterion for long bursts, not just a spare part.
