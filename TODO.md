# TODO — monorepo unification

Living checklist. Companion to `maintenance/SCRATCHPAD.md` (design log) and
`maintenance/journal.txt` (session catalog).

## Shared kits — status

| kit | what | state |
|-----|------|-------|
| registry-kit | small shared registry primitives | ✅ built |
| snapcore | pure-Rust Kademlia DHT (node + store + cluster) | ✅ built, 25 tests |
| crypto-kit | PQ-hybrid + sign + AEAD + group ratchet/seal | ✅ built, 36 tests |
| net-kit | transport plumbing: auth / wire / json / http (client+server) | ✅ built, 22 tests |
| share-kit | the MAILBOX over snapcore + crypto-kit (DHT-as-host two-layer) | ✅ v0.1, 7 tests |
| audio-kit | DSP primitives: level / goertzel+spectrogram / pitch / window / gen | ✅ v0.1, 25 tests |

All build & test `--offline` (crypto-kit/share-kit need the cached crates.io deps).

## In-flight / next on the kits

- [ ] **net-kit growth**: generic UDP transport (abstract enlist's `UdpTransport`
      over `wire` encode/decode); then the Transport-trait question as an explicit
      design item (4 app traits — snapcore RPC, unendlich sim, logscope bus,
      enlist — deliberately NOT unified yet).
- [ ] **share-kit growth**: the networked `Backend` (iterative `lookup_value` +
      store-to-closest over a `Transport`) replacing the local-`Store` test
      backing; cluster replication for `Durable` (`snapcore::cluster::
      replicate_to_cluster` = the network seam); `SenderChain` epoch management +
      `seal_sender_key` fan-out onto the snapcore cluster roster (announce
      co-members = the seal-to set); a higher-level `Channel`/`Group` over the
      mailbox.
- [ ] **app repointing** (the payoff): apps still carry their pre-extraction
      copies. Repoint them onto the kits — voiceterm/bo-suite/logscope `ct_eq` +
      hand-rolled HTTP + percent-codec → net-kit; snapclone-crypto/group →
      crypto-kit; snapclone groups → share-kit (epoch-gated cutover; feed
      `namespace="group-feed"` gives byte-compatible keys).

- [ ] **audio-kit consumers** (the payoff): repoint bo-suite `audio_features`
      → audio-kit (delete both copies); voiceterm/snapclone-audio `rms`; wire
      logscope voice-reaction level/voicing gate; unendlich synthesis on `gen`.
- [ ] **bo-dsp split** (app-local, NOT a kit): bo-suite's drone/passby cancellers
      (~27K tok beyond audio_features, bo-controller↔bo-mux) are STRATEGIES — pull
      into a bo-internal shared crate/module that consumes audio-kit primitives.

## patches/ (sister-session rendezvous)

- [ ] Mine the logscope distributed-fleet patch's remaining modules
      (dispatch/voicefan/speculate) — if the fan-out/scheduler pattern recurs in
      other apps it becomes a **fleet-kit**.
- [ ] The reaction CLASSIFIER (transcript → Steer/Interrupt/Affirm) is pending
      logscope-side per the handoff; lands when voicefan is wired.
- [ ] PROMISED: add a `username:filename` reference type to
      `voiceterm/src/zbrackets.rs` — queued for when share-kit reaches that app.

## Tooling (NEW — this session)

- [ ] **kitfinder** (this turn): structural clone miner — normalize functions to
      a rename-insensitive token stream, cluster, score cross-crate duplication →
      rank the next kit candidate. Built to AUTOMATE the hand-survey that produced
      net-kit/share-kit. v0.1 = exact (Type-2) clustering + MinHash near-dup +
      cross-crate kit aggregation. Future: full AST/PDG, call-graph + community
      detection (doc's Algorithm 2), the LOC-ROI work queue, the AI-reviewer layer
      that names + drafts each extraction.

## Source-of-truth hygiene

- [ ] Monorepo's `apps/unendlich` + `apps/logscope` are STALE snapshots (real
      fixes live in the standalone repos). Make the monorepo source of truth; pull
      standalone fixes ONCE via `git subtree`.

## Parked backlog (no execute without greenlight)

- [ ] B5 — nix flake for the workspace
- [ ] C1 — snapclone C++ → Rust oxidation
- [ ] E1 — net-sim CI (`snapcore::sim` is the seed)
- [ ] F3 — gitmind delta compression
- [ ] G2 — uikit (shared egui across apps)
- [ ] G3 — unendlich ⨯ logscope overlap
- [ ] H  — deployment / installer

## Hardware-gated (need real devices)

- [ ] Real UDP transport swapping `SimTransport` (STUN/UPnP/relay done in
      snapclone; the hole-punch FSM is the one port — user has field-validated the
      C++ `NatTraversal.cpp`). Can reuse net-kit `auth` + `wire`.
