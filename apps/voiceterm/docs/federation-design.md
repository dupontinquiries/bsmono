# Federation design — small-web publishing + curator handshake

**Status**: design only. v0.1.367 ships the first concrete piece
(Gemini archive viewer). The rest is multi-ship, multi-quarter
work. This document captures the architectural vision developed
in conversation across v0.1.366 → v0.1.367 so it isn't lost as
chat history compacts.

## Vision

web2qml grows from a single-user reader into a **federated reading
network** of trusted peers — "curators" who hand-publish small
amounts of high-quality content to each other. The pitch:

- Each user marks items in their local corpus as PUBLIC
- The public subset gets published via small-web protocols
  (Gemini primarily; Gopher / raw markdown / HTTPS as adjuncts)
- Published items carry **link-graph metadata** — how strongly
  this connects to my other notes, public or private (private
  links surface only as anonymous counts, never as named links)
- Other web2qml users can subscribe to curators they explicitly
  follow
- Trust is established **in person** via a voice-bound,
  Bluetooth-and-acoustic handshake ceremony
- The result: voice query "what have my curators published about
  Asahi Linux this week?" returns a handful of hand-written
  notes from people you've explicitly chosen

This is **not** a social network. There are no algorithmic feeds,
no popularity rankings beyond what curators flag for their own
followers, no engagement metrics. The model is closer to:
"academic citation graph + zine swap + village notice board."

## Why this fits web2qml

The project's existing primitives carry most of the design:

- **Voice fingerprinting (PC1)** binds identity to a biological
  property of the user rather than a credential. Curator
  handshakes use it as the trust anchor.
- **Voice intent system** drives the handshake UX. The whole
  ceremony is voice-initiated: "Add curator." "I am Bob."
  "Confirm with…"
- **Local corpus + interest extraction** (already partially
  designed) provides the published-and-private data to share.
- **Small-web rendering pipeline** (v0.1.367 Gemini viewer is
  the foundation) makes consuming federated content cheap.
- **Anti-modern-web identity** — the federation surface inherits
  the project's no-JS, no-tracking, no-engagement-optimization
  stance.

## Threat model

Realistic adversaries we defend against:

- **Curious neighbor / wardriver**: defeated by any cryptographic
  primitive
- **Bluetooth MITM with replay**: defeated by acoustic OOB
  verification + emission timing jitter
- **Moderate-resource acoustic injection** (parabolic dish from
  outside room): defeated by room impulse-response fingerprinting
  + spatial separation requirement
- **Voice cloning of one party**: defeated by PC1's confidence
  scoring + live voice ceremony with phrase that wasn't predictable
  before the handshake started

Explicitly NOT defended against:

- Software compromise of one party's device (no protocol defeats
  this)
- Nation-state-level attack with directed acoustic injection +
  real-time voice synthesis + perfect room replica (would
  require physical security beyond software)
- Trusted insider acting maliciously (needs social trust, not
  technical defenses)

The realistic protection target: **harder to attack than Signal
Safety Numbers, while preserving voice-first hands-free UX.**

## Trust ceremony — what the user experiences

Alice and Bob are sitting at the same table. They want to become
mutual curators.

1. **Alice**: "Web2qml, add curator."
2. **Web2qml-A**: "Who would you like to add?"
3. **Alice**: "Bob, here at the table."
4. **Web2qml-A**: "Bob, please speak your name to confirm."
5. **Bob**: "Hi, I'm Bob."
6. (PC1 enrolls Bob's voiceprint on Alice's device; same in
   reverse on Bob's device for Alice's voice.)
7. **Web2qml-A**: "Place your devices about a meter apart. I'll
   make some sounds — this is normal."
8. (Devices spread out. Both emit calibration sweeps for ~5
   seconds — sounds like dial-up's training tones. Room IR
   captured at each device. Geometry estimated via click ToF.)
9. (Bluetooth ECDH handshake completes in parallel — invisibly
   to the user.)
10. (Acoustic challenge-response runs — ~4 seconds of warbling
    tones with cryptographic timing jitter. If anything is wrong
    a discordant chord plays and the ceremony aborts. If
    everything matches, a clean two-tone chime plays.)
11. **Web2qml-A**: "Alice and Bob: please say the phrase 'green
    coffee mug Sunday' aloud."
12. (Both speakers say the phrase. The phrase was derived from
    the shared data — perhaps the title of the first public note
    they're each about to share, or a small randomly-chosen
    phrase from the link-graph metadata. The phrase is PUBLIC
    information that both parties will be sharing anyway, so
    there's no secret being read aloud — the ceremony confirms
    LIVENESS and IDENTITY in the room, not data confidentiality.)
13. (Both devices verify: voice fingerprint matches the enrolled
    speaker AND the spoken phrase matches the expected phrase.)
14. **Web2qml-A**: "Alice and Bob are now mutual curators. You'll
    see each other's published notes."

Total elapsed time: ~30 seconds for the full ceremony. Most of it
is the acoustic phases. The voice prompts feel social. The
ceremony rewards being IN THE SAME ROOM TALKING — which is
exactly what the trust primitive needs.

### The phrase design

The phrase used in step 11-12 deserves a specific note:

- It's derived deterministically from the shared handshake data
  (hash → word selection from a fixed dictionary). Both devices
  compute the same phrase from the same inputs.
- The dictionary is small (~2000 common words) so the phrase
  is human-pronounceable.
- The phrase length is 3-4 words → ~33-44 bits of entropy from
  the dictionary alone, plus voice-identity binding makes the
  effective attack difficulty much higher.
- It's **public information** — derived from data both parties
  are about to publicly share anyway. Reading it aloud leaks
  nothing.
- **It's fun.** "Green coffee mug Sunday" is the kind of thing
  you might mention casually if asked "what's your day looking
  like." The ceremony doesn't feel paranoid.
- It encourages **spreading community knowledge** — the phrase
  itself is a small artifact of the federation that anyone in
  earshot might overhear and think "oh, those people are doing
  the curator thing." The handshake becomes a recognizable
  signal of the federation network's existence.

The shared cultural surface of these phrases — over time, users
hearing each other do the ceremony in cafés — becomes part of
how the network propagates socially. The protocol enables
discovery by being *noticeable in the right way*.

## Hardening stack — what defeats what

Layered defenses, listed in dependency order:

| Layer | Defeats | Cost |
|---|---|---|
| PC1 voice fingerprint enrollment | Identity spoofing by non-present party | Already shipped |
| Bluetooth ECDH | Passive Bluetooth eavesdropping | Trivial |
| Bluetooth RSSI gate | Out-of-room Bluetooth attack | S |
| Acoustic OOB hash broadcast | Bluetooth MITM | M |
| Acoustic challenge-response | Replay attacks | S |
| Cryptographic emission jitter | Phased-array acoustic injection | S |
| Room impulse-response fingerprint | Directional acoustic injection from outside | M-L |
| Time-of-flight + spatial separation | Co-located one-sided attacker | M |
| Voice phrase callback (PC1 + ASR) | Voice cloning of either party | S |
| Repeated handshake chaining | Sustained MITM across meetings | S |
| Friend-of-friend witness | Anything visible to a 3rd-party curator | Free once federation exists |

The stack is **deep on purpose**. Each layer attacks a different
attacker capability. The combined attack requires:

- Predicting cryptographic state (to time emissions)
- Controlling acoustics in a room you're not in (multiple mics
  spatially separated)
- Matching multiple voiceprints in real time (with
  unpredictable spoken phrases)
- Maintaining the deception across multiple natural meetings

That combined attack is genuinely beyond moderate-resource
adversaries. Against nation-state adversaries with directed
acoustic + real-time voice synthesis, no software protocol
fully suffices — but neither does Signal, neither does anything
else. We're well within the design's stated threat model.

## Ship sequence

### v0.1.367 — Gemini archive viewer (LANDED)

Tracer bullet. Pure local rendering. User pre-populates archive
with `gemget` or similar.

- `src/gemini.rs` module with Gemtext parser + archive layout +
  launcher renderer
- `web2qml://gemini` → launcher page
- `gemini://*` URLs → archive lookup → manpage render

24 unit tests in `gemini::tests`.

### v0.1.368 — Gemini live fetch + TOFU (NEXT, M)

The first real network code on the federation path.

- Add `rustls` with custom `ServerCertVerifier`
- TCP+TLS to port 1965, send request, parse response
- Save cert fingerprint on first connection; verify on
  subsequent
- Warn on fingerprint change with options: trust new / reject
  / view history
- Auto-save fetched capsules to archive (replaces manual
  `gemget` workflow from v0.1.367)
- New `web2qml://gemini/certs` page shows cert history per
  host

### v0.1.369 — Public flag + Gemtext export (M)

Foundation for publishing.

- Add `public: bool` field to corpus items
- Voice command "make this public" / "make this private"
- Background exporter: public items → Gemtext files under
  `~/.config/web2qml/published/`
- No serving yet — files on disk only
- `web2qml://published` shows what's exported

### v0.1.37x — LAN-only Gemini server + mDNS (M)

Make published files reachable on local network.

- Small Gemini server (~500 lines) serving from
  `~/.config/web2qml/published/`
- Self-signed cert generated at first run, persisted
- mDNS announce `_gemini._tcp.local` for LAN discovery
- Settings: enable LAN serving / disable / port choice
- Cross-NAT reachability deferred (Tailscale integration or
  hosted relay — separate ship)

### v0.1.37x — Manual curator add + data model (M)

Federation primitive without the trust ceremony.

- `~/.config/web2qml/curators.toml` schema:
  ```toml
  [[curator]]
  name = "Alice"
  pubkey = "ed25519:..."
  url = "gemini://alice.tail.net/feed.gmi"
  cert_fingerprint = "sha256:..."
  voice_fingerprint = "pc1:..."  # optional
  added_at = 1736000000
  trust = "manual"               # "manual" | "in-person"
  ```
- `web2qml://curators` UI: list, add (paste fields), remove
- Periodic background fetch of each curator's feed
- Voice queries surface curator publications

Useful immediately as plain federation. The handshake later
populates the same data model automatically.

### v0.1.37x — Link graph foundation (M-L)

Without this, publication is just file copies. With it,
publications carry semantic structure.

- Linking convention: `[[wikilink]]`-style (Obsidian-compatible)
- Index corpus for forward-links and back-links
- Centrality computation: which notes are most-linked?
- Voice queries: "what links to this?" / "what's connected to
  Asahi?"

### v0.1.37x — Link metadata in exports (S)

Published Gemtext gains a metadata footer:

```
## metadata
=> ./other-note.gmi (strength: 4) Related notes
=> ./other-note-2.gmi (strength: 3) Less-strong relation
private-link-count: 7
```

The `private-link-count` field surfaces how many PRIVATE notes
the publisher links to this one — useful signal of how
much they actually care about the topic, without leaking the
private notes themselves.

### v0.1.37x — Bluetooth pairing infrastructure (M-L)

Foundation for the handshake.

- BLE via `btleplug` crate (cross-platform, MIT-licensed)
- Service UUID for `web2qml-curator-handshake`
- One side advertises; other scans + connects
- ECDH (`x25519-dalek`) over BLE GATT characteristics
- RSSI threshold gate (refuse if too weak — proxy for
  same-room)

At this stage we can do BLE-only curator pairing with manual
"yes I want to add" confirmation. MITM still possible without
the acoustic stage — but the UX win over typing is large.

### v0.1.37x — Acoustic OOB integrity (M)

Adds the integrity verifier on top of BLE.

- After ECDH completes, compute
  `H = hash(my_pk || their_pk || my_voicefp || their_voicefp)`
- Encode H as ~4 seconds of audio (FSK or musical-note encoding)
- Each device emits its H; other decodes and verifies match
- Mismatch → audible failure tone + abort

Defeats Bluetooth MITM: attacker MITMing BLE produces
different shared secrets with each side, so H values differ.

### v0.1.37x — Acoustic challenge-response + emission jitter (M)

Hardens acoustic stage against injection.

- Replace one-way broadcast with back-and-forth
- Emission timings derived from `hash(shared_secret, nonce,
  sequence_no)` — unpredictable to outsiders
- Receiver verifies bit decode AND arrival timing within
  expected window
- Defeats phased acoustic arrays trying to beamform injected
  sound (array can't recompute beamforming faster than the
  jitter)

### v0.1.37x — Room impulse-response fingerprint (M-L)

The acoustic foundation upgrade.

Pre-handshake calibration (~5 seconds):

- Both devices emit known training signals (frequency sweeps
  or maximum-length sequences)
- Each records what it hears, computes its local IR
- Extract room-invariant features: modal frequencies, T60,
  broad reverb shape
- Exchange features over BLE; both sides verify agreement on
  shared room properties

Geometry estimation (~3 seconds):

- Each device emits a click at a random known time
- Other device records time-of-arrival
- Compute inter-device distance from speed of sound
- Verify: distance > 0.5m (OOB needs separation), < 10m
  (otherwise probably not same room), and consistent with
  reflections in measured IR

Handshake hash now includes shared room features. Acoustic
verification confirms not just "audio arrived" but "audio
arrived through a path consistent with the physical room."

### v0.1.37x — Voice phrase callback (S)

The social-feeling capstone.

- Derive a short phrase (~3-4 words) deterministically from
  the handshake state via a fixed-dictionary hash
- Web2qml prompts both parties to read the phrase aloud
- ASR + PC1 verify: correct words AND correct speakers
- Phrase is public information (derived from data being
  shared) — no secret read aloud
- Fail-loud failure mode if either check fails

Makes the ceremony feel like a small community ritual rather
than a security overhead. Encourages noticeability of the
federation network in shared spaces.

### v0.1.37x — Integrated handshake UX (S)

Glue. Voice intent grammar for "add curator." Confirmation
prompts. Failure handling. Setup wizard for first-time use.

### Long-term ships (each L+)

- **Cross-NAT federation** via Tailscale integration or
  hosted relay
- **Fingerprint sharing** (opt-in publish/subscribe of cert
  observations across curators — community CT for Gemini)
- **Federated semantic search** ("voice query across all my
  curators")
- **Web-of-trust visualization** — show your curator graph,
  the strength of links, which curators share other curators

## Implementation notes

### Why not just use Signal / Briar / Matrix

These exist. They solve adjacent problems. Why a custom protocol?

- **Different identity model**: Signal binds to phone numbers,
  Briar to one-time invite codes. Web2qml binds to voice
  fingerprints — the project's existing identity primitive.
- **Different transport assumption**: Signal expects always-on
  internet. Web2qml's federation is happy with intermittent
  connectivity (LAN-only, periodic syncs, offline reads).
- **Different content model**: Signal is messaging. Web2qml
  federation is *publishing* — small, slow, considered.
- **Different aesthetic**: the federation is part of an
  anti-modern-web reading tool, not a real-time chat platform.

We benefit from these projects' cryptographic primitives
(ECDH, Ed25519, ChaCha20-Poly1305 — all available as Rust
crates) without adopting their full architecture.

### Why not just use Tailscale + Gemini

Tailscale solves NAT traversal beautifully. We absolutely
should integrate with it as a transport option. But:

- Tailscale requires a Tailscale account (centralized control)
- Tailscale handles the network layer but not the trust layer
  between users (still requires manual trust establishment)
- The acoustic handshake's value isn't replaced by Tailscale —
  it provides BIOMETRIC + PHYSICAL co-presence trust on top
  of network connectivity

Architecture: federation is happy on Tailscale, on plain LAN,
on hosted relay, on hand-carrying USB sticks if that's all
you have. The handshake establishes trust regardless of
transport.

### Privacy properties

Throughout the federation arc, these invariants hold:

- Private notes never leave the device
- Link-graph metadata exposes counts of private links but
  never their identities
- Voice fingerprints used in the handshake are HASHES, not the
  raw acoustic data
- Cert fingerprints are pinned per-host; no central CA
- Subscription is opt-in per-curator; unsubscribing immediately
  stops future fetches
- All curator data is stored locally in human-readable TOML so
  users can audit + edit

## Open design questions

- How do we handle a curator who **removes** an item from their
  feed? Do we drop our cache? Keep it but mark "removed by
  publisher"?
- How do we handle conflict between OUR copy of a curator's
  cert fingerprint and what a friend reports having seen?
- Should we let curators distinguish between "trusted curators
  see this" and "anyone can see this"? (Adds complexity but
  matches actual social use cases.)
- What's the right format for link-graph publication when the
  reader doesn't have the publisher's full corpus?
- How do we handle voice-fingerprint drift over time (illness,
  aging)? The handshake captures current voiceprint; PC1's
  ongoing recognition needs to allow gradual drift.

These don't block shipping but will need answers as ships land.

## Reference: cryptographic primitives

- **Identity**: Ed25519 (signatures) + X25519 (key exchange) —
  `ed25519-dalek` + `x25519-dalek` crates
- **Symmetric encryption**: ChaCha20-Poly1305 — `chacha20poly1305`
  crate
- **Hashing**: BLAKE3 — `blake3` crate
- **Acoustic encoding**: custom FSK + FEC, no external crate
  needed for the bandwidth we want (32 bytes / 4 seconds)
- **TLS for Gemini**: `rustls` with custom `ServerCertVerifier`
  for TOFU

All MIT/Apache2 licensed; all well-vetted; all pure Rust.

## References

- Gemini Project: https://geminiprotocol.net/
- Gemtext specification: https://geminiprotocol.net/docs/gemtext.gmi
- Signal Safety Numbers (similar OOB verification UX):
  https://signal.org/blog/safety-number-updates/
- GGWave (acoustic data transmission): https://github.com/ggerganov/ggwave
- Howard Hinnant's date algorithms (used in token_log v0.1.364):
  https://howardhinnant.github.io/date_algorithms.html
- Perspectives / Convergence (Moxie Marlinspike, 2011) — the
  multi-witness model that informs cert federation
