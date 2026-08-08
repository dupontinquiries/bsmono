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
| voice-kit | whisper post-proc: hallucination filtering (union) + ggml model resolution | ✅ v0.1, 10 tests |
| core-kit | foundational leaf utils: time / hex / rng (SplitMix64) | ✅ v0.1, 12 tests; share-kit repointed |

All build & test `--offline` (crypto-kit/share-kit need the cached crates.io deps).

## App repoints (the payoff) — STATUS

- [x] bo-suite audio_features.rs (both copies) → audio-kit shim (+ engine_timbre_score local).
- [x] bo-controller podcast_diar rms + voiceterm separation rms → audio_kit::level::rms.
- [x] rusty-board voice/hallucination.rs → voice-kit shim.
- [x] Verified: standalone harness crates cargo-check green vs the kits.
- [ ] User builds + runs all apps via GUI to confirm full builds (Qt/GTK, not sandbox-buildable).
- [ ] voiceterm voice.rs hallucination fns → voice-kit (embedded; + fix loop-trimmer off-by-one at source).
- [ ] unendlich worldgen RNG → core_kit::rng (opt-in — changes sequence).
- [ ] scattered now_ms / hex app copies → core-kit (follow-up sweep).
- [ ] reconcile only-in-monorepo dead files in synced apps (see maintenance/SISTER_STATUS.md).

## In-flight / next on the kits

- [x] core-kit dogfooded: share-kit hex → core_kit::hex (first kit-to-kit dep, verified).
- [ ] More kit-to-kit dogfooding (opt-in, low-priority): net-kit::auth hex,
      crypto-kit hex → core-kit; audio-kit::gen RNG → core_kit::rng (changes noise
      sequence — fine, but opt-in). kitfinder stays standalone (portable export).
- [ ] ESCAPING family — needs a home decision: percent-escape is transport
      (net-kit::wire, already there); html/xml/toml/path are general text (core-kit
      `escape` module? a text-kit?). Heterogeneous — design before building.

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

- [ ] **audio-kit consumers** (the payoff): repoint **bo-suite** (speaker control)
      `audio_features` → audio-kit (delete both copies); voiceterm/snapclone-audio
      `rms`; logscope voice-reaction level/voicing gate; unendlich synthesis on
      `gen`; **rusty-board** (voice keyboard) mic-level metering → `level::rms`.
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

## rusty-board (newly ingested)

- [ ] Swipe gestures BROKEN — breadcrumb recorded in
      `patches/rusty-board-swipe-regression.NOTES.md` (good core ≈ commits
      876a0fa..7d3efea; rusty-board-git.zip has the history). FUTURE: bisect/diff
      to recover the core → a reusable **swipe/touch-kit** (path→keys→words). Not active.
- [ ] **voice-kit consumers**: repoint rusty-board `src/voice/` + voiceterm
      `src/voice.rs` onto voice-kit (hallucination + model). Wire voice/recorder
      level metering onto audio-kit `level`; a VAD gate (voice-kit over audio-kit)
      is the next voice-kit growth.

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

## paperbound-core → unendlich GPU path (recorded 2026-07-18)
`games/paperbound/rust/paperbound-core/src/gpu.rs` exports battle scenes as
GPU-ready interleaved buffers (pos/nrm/col + u32 indices, per-face normals)
and probes for a loadable libvulkan the vk-harness way. The actual
`unendlich-engine --no-default-features --features render-core` dependency is
BLOCKED in the headless sandbox by unendlich's version-4 Cargo.lock (needs a
newer cargo than 1.75); wire it on a GPU host with a current toolchain — the
buffer layout already matches the engine's mesh upload, so it's a consume,
not a rewrite.

## aperture (apps/aperture) — follow-ups

Session scope was: rope core + invertible-Op history, line-lexer syntax,
ghost-text completion loop (ollama FIM / subprocess / demo), smooth-camera
egui shell, loop pane, palette. Deliberately deferred, in rough order of pull:

- **tree-sitter** behind the existing `LineLexer` seam (`aperture-core/syntax.rs`).
  The seam is `(line, entry_state) -> (spans, exit_state)` so an incremental
  parser slots in without touching the renderer. Deferred because grammar
  crates churn their `Language`/`LANGUAGE` symbol and version-pair with the
  runtime crate; pin both when adding.
- **Streaming ghosts**: ollama `stream:true` + a channel of deltas so long
  completions paint progressively. Engine already coalesces newest-wins;
  streaming needs a cancel path (drop the TcpStream) added to `Provider`.
- **infer-kit extraction**: `aperture-complete/providers.rs` ollama client is
  adapted from `logscope-fan/src/ollama.rs` (adds FIM suffix + de-chunking).
  Two call sites now exist — next co-change, lift `shared/infer-kit`.
- **Cloud provider** behind a `ureq` feature (TLS). Until then
  `APERTURE_CMD` subprocess is the escape hatch (curl in a script).
- **File-fuzzy palette**: Ctrl+O currently takes a typed path; walk the
  workspace root and reuse `uikit_egui::fuzzy` for ranking.
- **Multi-cursor**: `Selection` is already value-based; becomes
  `Vec<Selection>` + edit fan-out with offset fixup.
- **CJK/double-width**: caret x assumes monospace `col * char_w`; wide glyphs
  need galley-measured x (egui does no full shaping — known engine limit).
- **IME preedit** (`egui::Event::Ime`), **LSP** (own crate behind the same
  worker pattern as the completion Engine), **egui_kittest** snapshots for the
  view layer (state layer is already headless-tested), **minimap** on the
  scrollbar rail.

## scene-kit (shared/scene-kit) — logscope migration

`shared/scene-kit` is logscope-egui's physical-UI core (spatial, mesh3d,
gpuscene, glow_scene, texgen, textsurface) lifted at its second call site
(aperture-view), **plus** the two fixes `apps/logscope/docs/physical-ui-findings.md`
called the enabling changes: per-vertex UVs (12→14 floats, `MAT_INSCRIPTION`
sampling `u_page` in page space) and a *wired* `u_touch` (emissive proximity
response, age by frame delta). logscope still runs its own copy; migrate at
the next scene co-change:

- Replace the six module files with `pub use scene_kit::<mod>::*;` shims
  (paths stay valid for all 123 modules).
- Sweep `Mesh3 {` literals in scene3d.rs to add `uvs: None,` (mechanical;
  the compiler's E0063 list is the site inventory — 22 sites in mesh3d took
  minutes this way).
- `theme::{power_mode, detail}` → `scene_kit::perf` (env names change to
  `SCENE_KIT_*`; logscope's shell can keep its LOGSCOPE_* dials by setting
  the kit env or calling its own values through).
- `local_hour()` in the kit is UTC + `SCENE_KIT_HOUR`/`LOGSCOPE_HOUR`
  override (chrono severed); logscope's shell should set the override from
  its chrono local time to keep true local solar time.
- Then delete the copies and let scene3d's inscriptions use real UVs — the
  street signs and runestones textsurface.rs already designed for.
