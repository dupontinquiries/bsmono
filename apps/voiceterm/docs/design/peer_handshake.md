# Peer handshake & workspace sharing (foundations, v0.1.711)

Goal: two developers connect and share notes workspaces. Distinctive trust
model — an **acoustic dial-up handshake** carrying a small identity blob, with a
**voice print** spoken alongside for verification. Received workspaces land in
`peers/<author>/` as a git repo (author metadata preserved) so git does the
diffing/syncing.

This turn lays the PURE, tested foundations. Wiring (mic/speaker, git exec, QML
UI) comes next.

## Modules (all pure, 25 tests)

### src/audio_dialup.rs (7 tests) — the modem
Data-over-sound carrier. 4-FSK (2 bits/symbol) over tones 1200/1500/1800/2100
Hz, a 900 Hz preamble, frame = [preamble][len][payload][crc16]. Full pipeline:
- `encode_payload(bytes) -> symbols`, `decode_payload(symbols) -> bytes?`
- `symbols_to_samples(symbols) -> Vec<f32>` (sine synthesis @ 8 kHz)
- `samples_to_symbols(samples)` via a Goertzel filter bank
- CRC-16/CCITT (verified against the 0x29B1 vector)
Round-trips IN MEMORY end to end (payload→symbols→SOUND→Goertzel→payload),
including a ±0.15 noise test — so it's genuinely verified without an audio
device. The real playback/capture are thin cpal wrappers (next turn).

### src/peer_handshake.rs (9 tests) — payload + FSM + voiceprint binding
- `HandshakePayload { peer_id, key_fp, workspace_hash, voiceprint_ref }` with a
  compact `k=v;…` wire form and straight-to/from acoustic-symbols helpers.
- `HsState`/`HsEvent`/`step()` state machine: Idle → Dialing → Listening →
  (decode) → VerifyingVoice → Trusted | Rejected. No-voice-binding payloads
  trust on the blob alone; bad frames and voice mismatches reject.
- `voice_matches(live, claimed, threshold)` — cosine distance against the
  speaker layer's embeddings (reuses speaker_embed::cosine_distance).

### src/peer_workspace.rs (9 tests) — author metadata + manifest + git plan
- `stamp_author`/`strip_author`/`author_of` — idempotent `web2qml-author`
  frontmatter block, preserves the body.
- `ShareManifest` — (path, FNV-1a hash) list + author; `digest()` (the value the
  handshake's workspace_hash carries), `diff()` (added/changed/removed).
- `peer_subdir(author)` → `peers/<slug>`; `git_receive_plan` / `git_diff_plan`
  return argv vectors (init/add/commit-with-author / diff --stat) — NOT executed
  here; the caller runs them with the notes root as cwd.

## Roadmap (next turns)
1. **Wire the modem to audio**: cpal playback of `symbols_to_samples`, mic
   capture → `samples_to_symbols`. Backend qt_methods: `handshake_dial(payload)`,
   `handshake_listen() -> payload-wire`, plus the FSM driver.
2. **Voiceprint step**: capture the spoken phrase during the handshake, embed it
   (existing MFCC→embed path), call `voice_matches` against the claimed print.
   Bind `voiceprint_ref` to a real speaker profile id.
3. **Workspace share/receive**: build a ShareManifest from the notes root, stamp
   author on shared notes, write the peer copy under `peer_subdir`, run
   `git_receive_plan`; expose a diff view via `git_diff_plan`.
4. **QML**: a handshake panel (Dial / Listen, live FSM state, the dial-up
   "connecting…" affordance, trust/reject result) + a peers browser.
5. **Key material**: derive `key_fp` from a stable local key; the
   "buried in docs + conversation history" shared-secret idea can seed/verify it.

## Status
Foundations: compile-gate green; harness 1351 + 28; 25 new pure tests. NOT yet
wired to audio, git, or QML — those are the next turns. The modem is verified by
in-memory acoustic round-trip; the FSM + manifest + git-plan are pure logic.

## Wiring progress (v0.1.712)
qt_methods added + compiled:
- `handshake_payload_wire(id, fp, wh, vp)` → wire string.
- `handshake_dial(wire)` → encode → tones → `audio_feedback::play_modem`
  (feature-gated; no-op without audio). Returns tone count. RUNTIME-UNTESTED
  audio.
- `handshake_decode_symbols(csv)` → payload wire. CSV→symbols→wire round-trip
  VERIFIED in a staged crate.
- `workspace_manifest(root, author)` → "digest\t…\tcount\t…" (real dir walk).
- `workspace_receive(notes_root, author, src_dir, label)` → copies the peer tree
  into `peers/<author>/`, runs git init/add/commit-with-author. The
  receive→commit→diff flow was VERIFIED end-to-end with real git 2.43 in a temp
  workspace: the snapshot commits with author "<name> <peer@web2qml>" preserved,
  and a subsequent edit shows in `git diff --stat`.
- `workspace_diff(notes_root, peer_rel)` → `git diff --stat` output.
fs helpers `collect_workspace_files` / `copy_tree` (skip .git/.tmp, depth-bounded).

STILL TO DO: mic capture → `samples_to_symbols` → `handshake_decode_symbols`
(the listen side needs a modem-rate capture path); the voiceprint capture+embed
step during the handshake; and the QML handshake panel + peers browser.

## Wiring progress (v0.1.713)
- LISTEN side wired: `voice::capture_modem_samples(rate)` snapshots the mic ring
  buffer resampled to 8 kHz; `handshake_listen()` decodes it via Goertzel →
  payload wire. RUNTIME-UNTESTED audio (no device); resample math is simple.
- SHARE side wired + VERIFIED: `workspace_share(root, author, dest, ts)` copies
  notes into a bundle, author-stamps each .md, writes MANIFEST.web2qml with the
  digest. The share→receive round-trip was verified with real fs: .md notes get
  stamped, raw files copied verbatim, the receiver strips the stamp back to the
  exact original body, and the digest is content-derived (stable across
  re-shares at different times). With the v712 git-receive verification, the
  full share → receive → git-commit → diff loop is proven (minus audio).
- QML: `peerPanel` Popup — handle/notes-folder/digest fields, Dial (plays
  tones) / Listen (polls handshake_listen), Share / Receive / Diff buttons, a
  live FSM-state dot, and a monospace status/diff pane. Voice triggers "connect
  to a peer" / "share my workspace" open it. compile + qml_lint clean.

REMAINING: the voiceprint capture+embed+verify step during the handshake
(voice_matches is built + tested; needs the live embed call wired in), and a
richer peers browser (list received peers, per-peer diff history).

## Voiceprint step wired (v0.1.714)
The identity half of the trust model is now in place:
- peer_handshake gained `embed_mfcc`, `embedding_to_wire`/`embedding_from_wire`
  (11 tests now). voice::embed_samples wires samples → phonetic::compute_mfcc →
  embed_mfcc → embedding.
- qt_methods: `voiceprint_capture()` (capture mic → embedding CSV wire, to
  enroll your print) and `handshake_verify_voice(claimed_wire, threshold)`
  (capture live → embed → cosine compare → "match\t<dist>" / "no\t<dist>").
- QML: Enroll-voice + Verify-peer-voice buttons. When a received peer payload
  carries a vp ref, the panel enters a "verifying" state (amber dot) and only
  flips to trusted after the voice matches — or rejected on mismatch.
The embed/compare math is unit-tested; the mic capture is RUNTIME-UNTESTED.

REMAINING: exchanging the full claimed embedding (the wire carries a short vp
ref today; the actual print needs a larger transfer or out-of-band enrollment),
and a richer peers browser (list received peers, per-peer diff history).

## Voiceprint persistence + peers browser (v0.1.715)
The flagged "exchange the full voiceprint" gap is closed via local persistence:
- src/peer_registry.rs (4 tests): parse_record (MANIFEST.web2qml → PeerRecord),
  list_wire (browser rendering), valid_voiceprint (dim check).
- qt_methods: peer_save_voiceprint (writes peers/<author>/.voiceprint, dim-
  validated), peer_load_voiceprint (reads it), peer_list (scans peers/ →
  registry wire). The fs scan + voiceprint save/load round-trip is VERIFIED with
  real files: two received peers scanned, the one with a .voiceprint shows
  has_voiceprint=1, list wire renders, stored print loads back exactly.
- QML: verifyPeerVoice now falls back to the peer's STORED voiceprint (no need
  for it in-band); a "Paired peers" ListView shows each peer (🔒 = voiceprint on
  file) with a per-peer diff button. refreshPeers on show.

The handshake now verifies against a real on-file embedding instead of needing
the 26-float print squeezed through the modem. Enrollment is: pair once (out of
band or via a future larger transfer), save the print, then every later
handshake verifies automatically.

REMAINING: the larger/QR transfer to capture a peer's print on first pairing
(today peer_save_voiceprint is the storage primitive; the capture-from-peer
moment still needs a channel), and key material for key_fp.
