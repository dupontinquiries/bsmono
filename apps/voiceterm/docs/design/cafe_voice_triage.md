# Voice in a café — triage (v0.1.675)

## Symptom (user)
App didn't pick up voice in a café over ~1hr across runs. Log pattern seen
earlier: `burst start, rms=0.0455 thr=0.0053` then `whisper said: ""` — the
gate let a loud ambient burst through and Whisper transcribed nothing.

## Likely causes (NOT yet confirmed — needs café audio capture)
1. Sustained loud ambient noise keeps RMS well above the absolute VAD floor,
   so long bursts repeatedly clear the wake gate and waste Whisper on
   non-speech. Real speech is also buried under café babble → empty/garbled
   transcription.
2. The noise-floor estimate (audio_profile, speech_threshold = floor × 1.8)
   may not be adapting fast/high enough to a loud room.
3. Speaker isolation / corpus: still maturing — separating one voice from
   café babble is hard; the 39-dim fingerprint may not be enough margin in
   this SNR.

## What shipped (pure, safe, NOT yet rewired into the live path)
wake_gate::noisy_env_skip(burst_rms, noise_floor, snr_margin): skip a burst as
ambient unless its RMS rises a MARGIN (≈1.8×) ABOVE the rolling noise floor —
a RELATIVE rise, since in a loud room the absolute RMS isn't informative.
Tested. Deliberately not wired into the live wake path yet: tuning the margin
safely needs a café audio capture (changing gating blind risks making it
DEAF, which is worse). 

## To confirm + tune (needs the user)
- Capture café audio with WEB2QML_PROFILE (or the voice debug log) showing
  noise_floor + burst rms over a session, plus a few real spoken phrases.
- Then: pick snr_margin from the separation between ambient and speech rises,
  wire noisy_env_skip into the long-burst branch of the live wake path, and
  measure FP (ambient transcribes) vs FN (speech missed) on the capture.
- If speech itself doesn't separate (cause 3), the lever is the speaker
  isolation / multi-mic path, not the gate.
