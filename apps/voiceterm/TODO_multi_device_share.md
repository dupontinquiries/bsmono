# TODO: "multi-device share dial-up" — OPEN DESIGN QUESTION (needs scoping)

## Why this file exists
In the v0.1.62x session you said: "I will also have to test the multi
device share dial up soon… there was previous talk here in our chat on
these topics." You sounded keen to test it — but it has NO code and NO
TODO in web2qml, so it was dangling. Capturing it so it isn't lost, but
HONESTLY: I could not find a web2qml design for it, and I don't want to
build the wrong thing.

## What I checked (so we don't re-investigate)
- No device-to-device / pairing / DB-sync / "dial" code exists in the
  web2qml tree (grepped: device_id, pairing, sync, dial, federation,
  handshake, QR, NFC, broadcast — only a federation-DESIGN doc about
  Gemini curator-publishing, which is a different thing).
- The "previous talk" appears to be from OTHER projects in chat history,
  NOT web2qml:
  - a GTK4 thermal-sensor app: device_id calibration sharing + transfer
    learning between same-hardware phones (SQLite device table).
  - a Syncthing photo-sync project: per-device IDs + block-hash dedup DB
    for laptop+phone photo sync at home.
  Both use "device id / share / sync between phones" language that likely
  bled into the web2qml mental model.

## What web2qml DOES have (the real, in-project multi-user roadmap)
These are session-LOCAL (one running app, multiple speakers), not
cross-device:
- new-speaker enroll (BUILT/WIRED v0.1.624) — a newcomer's voice offers
  the owner an enroll prompt.
- hand-touch handoff gesture (filed) — tap your speaker circle to take
  the floor.
- observer mode (filed) — allowed-but-doesn't-drive.
- same-voice merge (filed) — one person, multiple voice sub-profiles.

## The open question for you (pick one, then I'll scope+build)
"Multi-device share dial-up" could mean any of:
A. Two PHONES/devices running web2qml sharing a SPEAKER DB (so a guest's
   enrolled voice carries between your devices). → needs a transport
   (QR / LAN / file export-import) + the speakers.toml is already a clean
   serializable payload to build on.
B. A phone "dialing into" a desktop web2qml session (remote mic / second
   participant over the network). → needs a session/networking layer that
   does not exist; biggest build.
C. Sharing LEARNED DATA (decision_cache / fingerprints / corpus) between
   your own devices so voice recognition improves everywhere. → like (A)
   but the payload is the learning DBs, not speakers.
D. It actually meant the session-LOCAL handoff above, just worded as
   "share" — in which case it's the handoff-gesture TODO, no new work.

## Recommendation
Don't build until you confirm which of A–D. If A or C, the first safe,
pure, testable piece is a versioned export/import payload + an integrity
check for the chosen DB (speakers.toml / fingerprints.bin) — that's
transport-agnostic and I can build+test it in-sandbox without guessing the
transport. If B, that's a dedicated design session. If D, see
TODO_new_speaker_enroll + the handoff item — nothing new needed.
