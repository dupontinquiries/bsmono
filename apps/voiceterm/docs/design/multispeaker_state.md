# Multi-speaker — current state, demo test plan, and what's left

Consolidated so testing + log-gathering is targeted. Covers the two
threads you raised: the "add new speaker" popup (for the demo-watcher) and
the multi-device/handoff thread.

## What's BUILT and WIRED (works on your build today)
- **Speaker registry + whitelist** (speakers.toml). identify() classifies
  whole utterances; refuses to guess on ambiguous matches (margin guard).
- **Muted-speaker enroll popup** (_mutedEnrollPopup, qml ~10524): fires
  when an ALREADY-ENROLLED but muted speaker talks (the \x01MUTED: path).
- **Per-speaker color** auto-derived from voice (speakers::color_from_profile,
  the MFCC→RGB idea — DONE + tested). New speakers get a distinctive color.
- **Live multicolor transcript** + speaker circles.
- **Background channel** for muted speakers (italic, 0.45 opacity).

## What's BUILT but NOT yet wired (needs your build to finish)
- **New-speaker enroll detector** (new_speaker.rs, v0.1.623). Pure core +
  tested. Decides when a BRAND-NEW unknown voice should trigger an
  owner-facing "add new speaker?" prompt. NOT yet fed by the listen path
  or surfaced in QML — wiring documented in TODO_new_speaker_enroll.md.
  This is the piece your demo needs: today a brand-new voice gets NO
  popup (only muted-known speakers do).

## DEMO TEST PLAN (what to do + what logs to send)
Goal: confirm a newcomer gets offered enrollment, and the owner is never
falsely interrupted.

1. **Baseline (current build, before wiring the detector):**
   - Have someone NEW speak a few times while you (owner, enrolled) are
     active. Expected TODAY: no enroll popup (the gap). Watch the log for
     `speakers` lines — you'll see "identify ambiguous … → unknown" or
     just silent drops. CONFIRMS the gap.
2. **After wiring new_speaker (next build):**
   - Owner-only session: speak normally for a while. Expected: NO
     "add new speaker?" popup (the detector requires a clustered run of
     unknowns; owner utterances reset it). If a popup appears here, that's
     the false-positive to report — send the log.
   - Newcomer jumps in, speaks 3+ times: expected the detector emits
     \x01NEWSPEAKER\x01 → owner sees the prompt. Tap "Add new speaker" →
     enroll flow runs (uses last_audio_buffer profile + auto color).
   - Send the `speakers` clog lines around both cases.
3. **Tuning from logs:** the eagerness knob is min_run (default 3). If the
   newcomer has to talk too long before the prompt, drop to 2. If false
   prompts appear during owner-only speech, raise min_run or cluster_thr.

## Multi-device / handoff thread (the "share dial-up")
Status check: there is NO device-to-device DB/profile sync code in the
tree today (the NFC/pairing design from older chat was a DIFFERENT
project). The web2qml multi-user roadmap items are SESSION-LOCAL, not
cross-device:
- **Hand-touch handoff gesture** (M, filed): tap your own circle = "I'm
  driving now". Build-gated QML.
- **Observer mode** (M, filed): allowed-but-doesn't-drive; voice still in
  transcript. Build-gated.
- **Same-voice merge/diverge** (L, filed): one person, multiple voice
  sub-profiles (sick/whisper/phone). Needs the MFCC fingerprint pipeline.

If "share dial-up" means something specific (e.g. two PHONES sharing a
speaker DB, or a phone dialing into a desktop session), that's net-new
design — say which and we'll scope it before building (don't want to build
the wrong transport/payload).

## Recommendation
Build the app, run the DEMO TEST PLAN baseline + (after wiring
new_speaker) the newcomer/owner cases, send the `speakers` logs. That
tells us whether to tune min_run/cluster_thr and confirms the popup fires
for your demo-watcher. The handoff/observer/share items are separate
sessions of build-gated UI work — pick one when ready and we'll design it
deliberately rather than guess.
