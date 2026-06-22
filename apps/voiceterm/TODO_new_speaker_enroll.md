# TODO: new-speaker enroll offer (brand-new unknown voice → owner confirms)

## Goal (from demo requirement)
A brand-new UNKNOWN voice (someone who wants to jump in) should surface an
"add new speaker?" prompt — but the OWNER confirms first, and the owner's
own voice must never falsely trigger it. The existing _mutedEnrollPopup
only fires for an already-enrolled-but-muted speaker, so a true newcomer
gets nothing today.

## Done (pure, tested — v0.1.623)
`src/new_speaker.rs`: NewSpeakerDetector.
- `observe(Utterance{identified, sim_to_prev_unknown}) -> Decision`
  ({Idle, OfferEnroll}). Offers only after a clustered RUN of unknown
  utterances (default min_run 3, cluster_thr 0.80), latches after one
  offer, resets on a known speaker or explicit reset().
- 7 unit tests + 3 probes. Probe-verified: an owner-only session
  (scattered ambiguous reads) produces ZERO false offers; a recurring
  guest DOES offer.

## Wiring (build-gated — listen path FFI + QML, unverifiable in sandbox)
1. **State**: hold a `NewSpeakerDetector` + a cached `Option<TonalProfile>`
   of the previous unknown utterance, on the Backend (or a Mutex static
   like LISTEN_PENDING).
2. **Feed it** in `build_listen_dual_result` (main.rs ~15147), in the
   `multi_user_active` block:
   - compute `profile_gate` (already done).
   - `let id = speakers::identify(&profile_gate);`
   - if `id.is_none()`: compute `sim_to_prev_unknown` =
     `prev_unknown.map(|p| profile_gate.similarity(&p)).unwrap_or(0.0)`,
     then `det.observe(&Utterance{identified:false, sim_to_prev_unknown})`;
     update `prev_unknown = Some(profile_gate)`.
   - if `id.is_some()`: `det.observe(&Utterance{identified:true, ..0.0})`
     (resets the run); leave the existing allowed/muted logic as-is.
   - if `observe` returns `OfferEnroll`, return a new sentinel:
     `format!("\x01NEWSPEAKER\x01")` (distinct from \x01MUTED:). Do NOT
     drop the audio — keep it in last_audio_buffer so the enroll flow can
     use it as the first sample.
3. **QML**: handle `\x01NEWSPEAKER\x01` in the listen result handler
   (main.qml ~16899, beside the MUTED: branch). Show a NEW owner-facing
   popup (clone _mutedEnrollPopup): "Someone new is speaking — add them
   as a speaker?" with [Add new speaker] / [Not now]. On Add → run the
   existing add_speaker flow (speakers::add_speaker uses last_audio_buffer
   profile — see qml ~9418); on either button → call a backend method
   that does `det.reset()` so a later newcomer can offer again.
4. **Owner-confirms-first** is satisfied structurally: the prompt is shown
   to whoever holds the device (the owner), asking permission — the guest
   isn't enrolled until the owner taps Add.

## Tuning knob for the demo
Eagerness = min_run (default 3). If the demo shows the guest has to talk
too long before the offer, lower to 2. If false offers appear, raise it
or raise cluster_thr. These are constructor args to NewSpeakerDetector::new.

## Known conservative tradeoff (documented, verify on-device)
A guest who only speaks ONE word between owner turns won't accumulate a
run (each known() resets it), so won't offer. If the demo wants that case
covered, switch the strict consecutive run to a "K unknowns within the
last N utterances" sliding window — the detector's observe() is the only
thing that changes; the sentinel/QML stay the same.

## Test plan (logs to send)
Watch for the "speakers" clog lines:
- owner-only: should see occasional "identify ambiguous … → unknown" but
  NO offer sentinel.
- guest jumps in + speaks 3+ times: should see the run build and a
  \x01NEWSPEAKER\x01 emitted → owner popup appears.

## v0.1.624 — WIRED (needs on-device build)
Done:
- NEW_SPEAKER static (detector + prev_unknown profile cache) next to
  LISTEN_PENDING in main.rs.
- build_listen_dual_result feeds it: identified speaker → observe(identified)
  + clear prev_unknown (resets run, protects owner); unknown → compute
  similarity(prev_unknown), observe, cache profile; on OfferEnroll return
  "\x01NEWSPEAKER\x01" (audio KEPT, not dropped).
- new_speaker_reset qt_method (Backend) → clears detector + prev_unknown.
- QML listen handler: on "\x01NEWSPEAKER\x01" triggers the PROVEN
  _registerBox flow (_userRequestedAdd=true; _refreshNeed()) + toast
  "Someone new is speaking — add them?", then backend.new_speaker_reset().
Reused the existing register UI rather than a new popup (lower risk).

VERIFIED in-sandbox: pure detector tests (922) green; brace/qt-struct/qml
lint clean; all signatures pub + resolve; reused hooks (_userRequestedAdd,
_refreshNeed) confirmed present. NOT verifiable in-sandbox: full main.rs
compile (clang/Qt) + runtime (static Mutex, sentinel round-trip, register
surfacing). NEEDS the build.

Test (send logs): owner-only → no register prompt + no NEWSPEAKER clog;
newcomer speaks 3+ times → "new-speaker run reached threshold" clog +
register UI surfaces. Tune min_run (default 3) from what you see.
