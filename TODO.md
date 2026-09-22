# TODO — monorepo unification

Living checklist. Companion to `maintenance/SCRATCHPAD.md` (design log),
`maintenance/C10_TODO.md` (the c10 session's open threads — start here) and
`maintenance/C9_TODO.md` (the larger backlog behind it; c10 edited its status
lines in place, so it is current). Each item in both is marked with how it was
established — verified, read, or assumed.

**C20** (after c19unified-003): `maintenance/C20_TODO.md` -- the rig work, blackstar-rs
findings, and the voiceterm / aperture / tomescope investigations that changed no code.

`maintenance/journal.txt` was referenced here as the session catalog and does
not exist in the tree; tracked as C9-7c rather than silently dropped.

## Shared kits — status

| kit | what | state |
|-----|------|-------|
| registry-kit | small shared registry primitives | ✅ built |
| snapcore | pure-Rust Kademlia DHT (node + store + cluster) | ✅ built, 71 tests |
| crypto-kit | PQ-hybrid + sign + AEAD + group ratchet/seal | ✅ built, 28 tests + 1 ignored |
| net-kit | transport plumbing: auth / wire / json / http (client+server) | ✅ built, 22 tests |
| share-kit | the MAILBOX over snapcore + crypto-kit (DHT-as-host two-layer) | ✅ v0.1, 7 tests |
| audio-kit | DSP primitives: level / goertzel+spectrogram / pitch / window / gen | ✅ v0.1, 25 tests |
| voice-kit | whisper post-proc: hallucination filtering (union) + ggml model resolution | ✅ v0.1, 10 tests |
| models-kit | the canonical `~/models` store: `<kind>/<name>`, `GITMIND_MODELS` override, legacy dirs searched read-only | ✅ 7 tests; every model consumer resolves through it — do not add a sixth directory |
| core-kit | foundational leaf utils: time / hex / rng (SplitMix64) | ✅ v0.1, 12 tests; share-kit repointed |
| vidkit | video playback: caps model / MP4+WebM demux / player loop / egui surface | ✅ v0.1, 472 tests, 181 mutants; see `apps/vidkit-player/TODO_vidkit.md` |

All build & test `--offline` (crypto-kit/share-kit need the cached crates.io deps).

## App repoints (the payoff) — STATUS

- [x] bo-suite audio_features.rs (both copies) → audio-kit shim (+ engine_timbre_score local). (This checkbox was stale as of 2026-08-19: the file was consolidated to ONE copy in bo-core with bo-controller re-exporting it, but the copy itself still had the full local implementation, never actually pointed at audio-kit. Actually repointed now — bo-core/src/audio_features.rs re-exports audio_kit::goertzel's SpectrogramConfig/spectrogram/spectrogram_at_freqs verbatim (byte-identical logic — audio-kit's own doc already said these were "lifted from bo-suite's audio_features"), engine_timbre_score stays local per the original plan, now calling audio_kit::goertzel::goertzel instead of a local copy. cargo test -p bo-core: 41/41. bo-controller itself (the actual Qt consumer) is unbuildable here — got past its OpenSSL blocker by installing libssl-dev, then hit a real qmake-not-found wall, consistent with every other Qt app this session — but its only touch point is a one-line `pub use bo_core::audio_features;` re-export with no signature changes, so risk is low.)
- [x] bo-controller podcast_diar rms + voiceterm separation rms → audio_kit::level::rms.
      (This checkbox was stale as of 2026-08-19 — both were still full local
      implementations, byte-identical to audio-kit's own `rms`, which itself
      already claimed both as unified copies. Actually repointed now, in the
      same "bo-suite audio-kit round" as `audio_features.rs` and `bo-mux`'s
      `ct_eq`. Both apps are Qt-blocked in this sandbox (qmetaobject/qmake) —
      unverified by compilation, verified instead by line-by-line comparison;
      `cargo test -p audio-kit level::` — 5/5 — confirms the primitive
      itself, including the exact zero/full-scale-DC cases both apps'
      original test suites also covered under different names.)
- [x] rusty-board voice/hallucination.rs → voice-kit shim. (This checkbox was
      stale as of 2026-08-19 — the file was still a full standalone
      implementation, not a shim. Actually done now; see the "rusty-board
      (newly ingested)" section's voice-kit consumers item below for the
      real commits.)
- [x] Verified: standalone harness crates cargo-check green vs the kits.
- [ ] User builds + runs all apps via GUI to confirm full builds and actual
      on-screen behavior (a display/window/touch input, not a toolchain, is
      the remaining gap). Toolchain-wise, this sandbox is less GTK/Qt-blocked
      than earlier sessions assumed — see the note below.
- [x] **Correction, 2026-08-19: rusty-board's GTK4 profile DOES compile in a
      sandbox** — apt has `libgtk-4-dev` 4.14.5 and `libadwaita-1-dev` 1.5.0
      (the exact versions rusty-board's own Cargo.toml comment calls for),
      plus `libasound2-dev` for the alsa-sys build dependency. `cargo test
      --lib --no-default-features` (the `basic`/GTK-only profile, no
      whisper-rs/reqwest/gtk4-layer-shell) is 531/531 real, on this actual
      crate, for the first time. `--features voice` (whisper-rs) goes
      further — needs `cmake` too, to build whisper.cpp's C++ sources from
      source via `whisper-rs-sys`'s build script — not attempted, real disk/
      time cost for a native build, past the point this pass's checks
      needed to land. `--features wayland` needs `gtk4-layer-shell-0`,
      which apt doesn't package on Ubuntu 24.04 (only the GTK3 fork) — would
      need building from source too. Bo-controller's Qt build got one step
      further too this session (OpenSSL blocker resolved with `libssl-dev`)
      before hitting a real `qmake`-not-found wall — Qt itself remains
      unattempted, a much larger toolchain than GTK4's handful of `-dev`
      packages.
- [x] voiceterm voice.rs hallucination fns → voice-kit (embedded; fix
      loop-trimmer off-by-one at source — DONE). Repoint done in 3 small
      commits (dependency add, `is_sane_transcription`, `trim_hallucination_
      loop`), each substitution checked line-by-line against voice-kit's
      actual source before being made (both functions were byte-for-byte
      duplicates already — voice-kit's own doc says `looks_like_speech` was
      "Lifted from voiceterm's `is_sane_transcription`" — so this restores a
      single source of truth rather than changing behavior).
      `trim_hallucination_loop` kept as a thin wrapper so its `clog!`
      diagnostic survives the repoint (voice-kit stays logging-free; it's
      shared with rusty-board too). Still **unverified by compilation** —
      `voice.rs` is huge (4400+ lines) and almost entirely
      `#[cfg(feature = "voice")]`/qmetaobject (Qt), neither buildable in this
      sandbox — but attempted rather than left undone, with small enough
      commits that a bisect or partial revert has real seams if a real build
      finds a problem.
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
      copies. `ct_eq` half: ~~voiceterm/bo-suite~~ DONE this session (bo-mux's
      `peer_auth.rs` and voiceterm's `mail_crypto.rs` both now delegate to
      `net_kit::auth::ct_eq`, confirmed byte-identical before repointing).
      bo-mux verified with a real `cargo test` (264/264, incl. the specific
      `ct_eq_works` and `peer_auth::` tests) — it's a headless daemon, no
      Qt/GTK. voiceterm's is unverified by compilation (Qt/qmetaobject) but
      line-by-line confirmed, same standard as this session's other
      Qt-blocked repoints. logscope's TWO copies (`logscope-fan::enlist`,
      `gpu-gemm::netserve` — the very two net-kit's own doc says its `ct_eq`
      was "lifted as the union of") are deliberately left untouched: logscope
      has heavy concurrent commit activity this session (other work landing
      on `main` throughout), and touching shared files there risked a
      collision with in-flight work rather than a clean repoint. Still open:
      hand-rolled HTTP + percent-codec → net-kit; snapclone-crypto/group →
      crypto-kit; snapclone groups → share-kit (epoch-gated cutover; feed
      `namespace="group-feed"` gives byte-compatible keys).

- [ ] **audio-kit consumers** (the payoff): ~~bo-suite (speaker control)
      audio_features~~ DONE this session (see the checkbox above — was
      partially stale). ~~rusty-board mic-level metering~~ DONE this session,
      but this TODO's own "→ level::rms" was wrong about what to repoint:
      the actual code (`voice/recorder.rs`'s `push_samples_mono`) is a PEAK
      meter with exponential decay for UI display, not RMS — repointing it
      to `level::rms` would have been a real behavior change (a peak meter
      reads bursty transients; an RMS meter reads average loudness), not a
      safe repoint. Repointed the raw "max abs sample in this buffer" line
      onto `audio_kit::level::peak` instead (byte-identical primitive; the
      exponential-decay smoothing around it stays local as the UI-metering
      strategy it is). Unverified by compilation — this file is behind the
      `voice` feature, which needs `cmake` to build whisper.cpp from source
      (see the GTK4 correction above; not attempted) — but the substitution
      is a single-line, type-matching primitive swap (`peak(&[f32]) -> f32`
      exactly matches the call site), about as low-risk as an unverified
      change gets. Remaining: voiceterm/snapclone-audio `rms`; logscope
      voice-reaction level/voicing gate; unendlich synthesis on `gen`
      (already uses `core_kit::rng` per an earlier check this session — the
      remaining gap, if any, is audio-kit's own `gen` module specifically,
      not RNG). snapclone-audio's `rms` (the third of the three copies
      audio-kit's own doc names) is DONE — pure Rust, no Qt/GTK, so this
      one got a real `cargo test`: 104/104. All three named `rms` copies
      (bo-suite, voiceterm, snapclone-audio) are now repointed.
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
- [x] **voice-kit consumers, hallucination half**: rusty-board `src/voice/
      hallucination.rs` and voiceterm `src/voice.rs`'s `is_sane_transcription`
      + `trim_hallucination_loop` are now real shims over voice-kit (both
      repointed this session, in small verified-by-comparison commits — a
      previous session's [x] for the rusty-board half was stale, it had
      never actually happened). Verified at the time by direct line-by-line
      comparison against voice-kit's actual source plus voice-kit's own
      43/43 tests, since neither app compiled in this sandbox yet. UPDATE,
      later in this same session: rusty-board's GTK4 profile turned out to
      be buildable after all (see "Correction" above) — but `hallucination.rs`
      itself is gated behind the `voice` feature (whisper-rs), which needs
      `cmake` to build whisper.cpp from source, not attempted. So the
      rusty-board half is now ONE step closer to real compilation (the base
      crate compiles; the specific feature gating this file doesn't, yet)
      but still not directly `cargo test`-verified — voiceterm's Qt half
      remains unverified by compilation on both counts. `model_path.rs`
      (below) DID get a real `cargo test` this session, since it lives
      outside the `voice` feature gate entirely.
- [ ] **voice-kit consumers, model half**: rusty-board's half is DONE, real
      `cargo test`-verified (531/531, see the commit). voiceterm's half is
      still open: whisper model discovery/resolution there still has its
      own local copy; voice-kit's `model.rs` exists and is tested (part of
      the same 43) but voiceterm hasn't been repointed onto it yet. Wire
      voice/recorder level metering onto audio-kit `level`; a VAD gate
      (voice-kit over audio-kit) is the next voice-kit growth.

## Source-of-truth hygiene

- [x] Monorepo's `apps/unendlich` was a STALE snapshot beside `games/unendlich`.
      Resolved: `games/unendlich` is the source of truth (verified ahead of the
      r746 drop — it carries the netcode wiring the drop predates), and the
      `apps/` copy has been purged from history. `apps/logscope` remains the
      live tree and is not a duplicate.

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

> **Entities, buckets and paths**: see
> `apps/aperture/TODO_entities_buckets_paths.md` — the reconciliation of the
> 2026-09-11 spatial-editor design conversation, Kess's inline-TODO patch,
> and what is actually in the tree, with a dependency-ordered queue. Five of
> its items have landed (`aperture_core::{bucket, entity, nav, edgesrc}`,
> the caret policy, the Ctrl+D complement); the verification boundary is in
> `apps/aperture/HANDOFF_entities_session.md`, and it matters — the
> aperture-egui half of two of those patches has never been compiled.

Session scope was: rope core + invertible-Op history, line-lexer syntax,
ghost-text completion loop (ollama FIM / subprocess / demo), smooth-camera
egui shell, loop pane, palette. Deliberately deferred, in rough order of pull:

- **"Go to via route" (Ctrl+G)** — DONE, both tiers, wired end to end.
  Deterministic tier: `aperture_core::route::candidates(root, files, symbols,
  query, limit)` scores the workspace symbol index + file content against a
  natural-language query's keywords ("finding the button on the wizard page
  for account setup" → keyword set {button, wizard, page, account, setup},
  symbol-name hits weighted over text hits), returns a ranked, capped list
  with a one-line description each. Modeled directly on logscope's
  `filesummary.rs` two-tier design (structural digest = instant/
  deterministic; model summary = background/cached). Wired into the palette
  (`Mode::GoToRoute`) with real jump-to-line. 7 tests,
  `cargo test -p aperture-core --features tree-sitter`: 89/89 (was 73 before multicursor was declared).
  AI tier: `aperture_complete::route_ai` is the request-builder + pending/
  fulfill cache (prompt construction, content-hash-keyed dedup,
  `take_pending`/`fulfill` — the same servicing-seam shape as
  `filesummary.rs`'s worker-drains-a-queue pattern), 4 tests. Now WIRED to a
  live `Engine`/`Provider` — `AppState::route_engine`, a THIRD independent
  engine instance alongside `engine` (ghost completion) and `describe`
  (revision notes), same `APERTURE_CMD` provider construction as `describe`,
  its own generation counter so it can't collide with either. Palette calls
  `request_route_ai` every render (idempotent per exact query+candidate set
  via `route_ai`'s own dedup) and reads `route_summary`/`in_flight` to show
  the answer or an "asking the assistant…" placeholder beneath the
  deterministic list. Real integration test (`route_ai_fires_and_lands_
  through_a_real_engine`) drives an actual `CommandProvider` end to end —
  request, poll, land, dedup-on-repeat. `cargo test -p aperture-complete`:
  23/23; `cargo test -p aperture-egui --lib`: 62/62 (was 58, +1 new
  integration test — the rest already existed and stayed green).
  No `APERTURE_CMD` set = zero cost, same as `describe`: tier one still
  works standalone.

- **"AI: Edit Selection…"** — DONE, wired end to end. `aperture-complete::
  edit::make_edit_request` packs an instruction + selection into the
  `CompletionRequest` envelope; the shell now sends it through
  `AppState::edit_engine` — a FOURTH independent `Engine` instance (own
  `APERTURE_CMD` provider, own generation counter, alongside `engine`
  (ghost completion), `describe` (revision notes), and `route_engine`
  (go-to-route's AI tier)) — exactly the fix this note used to describe as
  "not attempted this pass." Palette command opens `Mode::AiEditPrompt` for
  the instruction; on landing, a review overlay shows original vs. proposed
  with explicit Accept (Enter)/Reject (Esc) — never auto-applied, unlike
  ghost completion's Tab-to-accept (an instruct-edit result is a bigger,
  easier-to-get-wrong bet than "what comes next"). Accept applies via
  `Editor::replace_range` at the ORIGINAL selection's char offsets (not
  wherever the caret ended up while the request was in flight) as one
  undo step. Two new integration tests drive a real `CommandProvider` end
  to end (request → poll → land → accept-changes-the-buffer, and
  request → poll → land → reject-leaves-the-buffer-untouched) — not mocks.
  `cargo test -p aperture-egui --lib`: 64/64 (was 62). No `APERTURE_CMD` =
  zero cost; no selection = a status message, not a panic.

- **tree-sitter** behind the existing `LineLexer` seam (`aperture-core/syntax.rs`).
  The seam is `(line, entry_state) -> (spans, exit_state)` so an incremental
  parser slots in without touching the renderer. Deferred because grammar
  crates churn their `Language`/`LANGUAGE` symbol and version-pair with the
  runtime crate; pin both when adding.
- ~~**Streaming ghosts**: ollama `stream:true` + a channel of deltas so
  long completions paint progressively~~ **DONE — provider, engine, and
  shell all built** (`OllamaProvider::complete_streaming` over a real
  `TcpStream` with a cancel path, `Engine`'s worker sending one
  `CompletionOutcome` per delta plus one final, `AppState::note_outcome`
  updating the ghost on every outcome and logging only the final one to
  the loop pane) — but **it was silently broken**, found by this session
  while adding a full-stack test rather than trusting the existing
  coverage.

  `Ghost::on_outcome`'s guard only ever accepted an outcome while the
  ghost was `Pending`. The first successful delta transitions it to
  `Shown` — so every LATER delta for that same request, the final one
  included, silently failed the guard and did nothing. Streaming ghost
  completions never actually streamed: one fragment appeared, then
  froze, no matter how many more deltas the server sent. Undetected
  because every existing test either fed outcomes one at a time by hand
  (never two for the SAME generation in sequence) or checked only
  `is_shown()`, which stays true on a frozen ghost and can't tell the
  difference — a lesson worth stating plainly: a passing test whose
  assertion is weaker than the behavior it claims to cover is a false
  sense of coverage, not real coverage.

  Fixed by giving `Ghost::Shown` its own `generation`, so a later delta
  for the SAME generation still updates it while a genuinely different
  or stale one is still correctly rejected. 3 new `ghost.rs` tests (the
  actual bug; a different generation while `Shown` still rejected; a
  stale generation with the buffer moved on still rejected), plus a
  strengthened `state.rs` test that now checks the actual text at each
  step instead of just `is_shown()`.

  Real proof: `streaming_ghost_e2e.rs` — a real `Engine` thread, a real
  `OllamaProvider`, a real local TCP server sending genuinely separate
  HTTP chunks with small delays between them (sending everything as one
  blob would complete before even one `poll_completions()` call, which
  drains everything queued per call and would hide the bug by only ever
  showing the final state). Watches the actual `ghost` field over time,
  asserts it was observed at more than one length and only grew,
  confirmed to fail with exactly the described symptom against the
  pre-fix code before the fix went in.

  Found in passing, named rather than fixed (separate scope):
  `multipass::make_block` trims each block's text, harmless for a
  complete final answer but dropping meaningful trailing whitespace on
  an intermediate streaming delta (`"fn "` → `"fn"` until the next delta
  arrives) — real, but minor and self-correcting by the time the final
  delta lands.

  `cargo test -p aperture-egui --lib`: 113/113 (was 108). `cargo test -p
  aperture-complete`: 97/97, unaffected — the bug was entirely one layer
  above the already-correct provider/engine streaming. Full workspace
  check clean.

  **A second bug found by checking whether the fix ITSELF introduced a
  problem** — the fix makes an interaction possible for the first time
  (streaming was frozen, so nobody could ever accept mid-stream while
  more deltas were still arriving): accepting part of a streaming
  suggestion (Ctrl+→) WHILE the server has more coming gets silently
  reintroduced by the next delta. `on_outcome` replaces the ghost with
  the server's own running-total text, which has no idea anything was
  typed out from underneath it — accept "let " from "let x", the next
  delta arrives with "let x = 1" (the server's own total), and the ghost
  jumps back to the FULL text rather than the correct remainder.
  Deliberately NOT fixed — a real fix needs `Shown` to track how many
  characters were already consumed and strip that prefix before
  re-running each new delta through `multipass`, and multipass's block-
  splitting on a SUFFIX rather than the whole reply is its own separate
  question. Documented with a test written to make the CURRENT
  (undesirable) behavior explicit and say plainly that a future fix
  breaking it is the correct outcome, not a regression.
- **infer-kit extraction**: `aperture-complete/providers.rs` ollama client is
  adapted from `logscope-fan/src/ollama.rs` (adds FIM suffix + de-chunking).
  Two call sites now exist — next co-change, lift `shared/infer-kit`.
- **Cloud provider** behind a `ureq` feature (TLS). Until then
  `APERTURE_CMD` subprocess is the escape hatch (curl in a script).
- **File-fuzzy palette**: DONE (`file_finder.rs`, Ctrl+O). ~~Find in
  project~~ DONE (`project_search.rs`, Ctrl+Shift+F). ~~Go to symbol in
  workspace~~ DONE (`workspace_symbols.rs`, Ctrl+T, name index over every
  file's existing tree-sitter outline — not semantic go-to-def).
- ~~**Multi-cursor**~~ **the core was already done; now wired into the shell.**
  `aperture-core/src/multicursor.rs` — `Cursors`, `Edit`, `merge_overlaps` —
  implements exactly what this line described, in almost the same words
  ("milestone 4: `Selection` becomes `Vec<Selection>` and edits fan out").
  Found orphaned (no `mod` line, never compiled, its tests never ran) by
  `maintenance/tools/orphan_check.py`, declared 2026-08-22.

  Fixed a real bug on the way: `after_insert` computed the cursor shift as
  `text.len()` (bytes) against a crate where every other offset — `Op`,
  `Buffer`, `Selection` — is char indices. Invisible for ASCII, wrong by the
  extra-byte count for any non-ASCII insert; no existing test caught it
  because none used non-ASCII text.

  Added the piece that actually needs the buffer, not just `Selection`
  values: `find_occurrence`/`all_occurrences`/`word_at` plus the three
  gestures built on them (`add_next_occurrence` — Ctrl+D, `select_all_
  occurrences` — Ctrl+Shift+L, `add_vertical` — column cursors). 15 new
  tests. `cargo test -p aperture-core --features tree-sitter`: 130/130.

  Shell side: `AppState` keeps `editor.sel` as the primary and adds
  `extra_cursors: Vec<Selection>` for the rest — NOT a rewrite of `Editor` to
  own `Vec<Selection>` directly, which would touch every one of the ~30
  existing motion/edit call sites in one commit. `cursors()`/`set_cursors()`
  are the only crossing points; every pre-existing single-cursor path is
  unaware the field exists and is provably unchanged (empty `extra_cursors`
  ⇒ `cursors()` returns exactly the old single-`Selection` case).
  Ctrl+D, Ctrl+Shift+L, typing, Backspace, Delete, Enter, Tab, arrow/word/
  doc motion, and Escape (collapse to primary) are wired; `fan_out_ranges`
  applies per-cursor edits as one undo step using the SAME ascending-shift
  arithmetic as `Editor::replace_many`, generalized to ranges that aren't
  the stored selection (backspace deletes text no selection represents).
  Multi-cursor typing deliberately bypasses ghost completion, auto-pair and
  the symbol popup rather than guessing a fan-out story for any of them.
  `view.rs` draws secondary carets (static, not blinking — blink is a
  "where's MY cursor" cue for the one the keyboard follows) and secondary
  selection fills. `cargo test -p aperture-egui --lib`: 77/77 (was 64; 13
  new, 10 of them exercising this).

  Not done: no smooth-camera trailing for secondary carets (they're drawn at
  their raw position, no interpolation — fine, since only the primary's
  position feeds the follow-camera).

  Found by running the actual binary under Xvfb, not by `cargo test`:
  Ctrl+D twice, type "sum" over two selections — both replaced correctly,
  but Ctrl+Z only undid the last character. `fan_out_*` went through
  `Editor::replace_many`, which never coalesces (correct for its other
  caller, find-and-replace-all, wrong for typing). Added `History::
  record_group_coalescing` / `Editor::replace_many_coalescing` — the
  multi-cursor analogue of `insert_text`'s single-char coalescing — and
  wired the three fan-out functions to the same coalesce condition their
  single-cursor equivalents already use. Also fixed a second bug the same
  pass turned up: multi-cursor Tab inserted a literal `\t` where the rest
  of the editor is space-only and tab-stop-aligned per column; now computes
  each cursor's own stop. Re-verified interactively after the fix: one
  Ctrl+Z now restores a three-keystroke typed run at both cursors.

  A THIRD bug, same method: after verifying the coalescing fix, the very
  same undo left no visible secondary cursor on screen — worth noticing
  rather than assuming was fine. `extra_cursors` was never told when the
  buffer got mutated or replaced outside the fan-out path: undo, redo,
  paste, cut, an AI edit landing, opening a different file, and every
  revision-preview/materialize/branch path (all funnel through
  `set_preview_text`) left stale offsets sitting there for the next
  keystroke to fan out to. Added `discard_extra_cursors` as one choke
  point, wired into all six sites. Verified by immediately typing a
  character right after undo and confirming it landed exactly once.

  Also wired `add_vertical` (column cursors) to Ctrl+Alt+Up/Down — it
  existed in core from the start of this work but had never been bound to
  a key, an orphan at the function level. Verified interactively: three
  presses built a column across three lines, mixed correctly with an
  existing selection from Ctrl+D (typing replaced the selection, inserted
  at the plain carets), one undo restored all three.

  `cargo test -p aperture-core --features tree-sitter`: 134/134.
  `cargo test -p aperture-egui --lib`: 85/85.
- ~~**CJK/double-width**~~ **DONE, and TODO.md didn't know it.** This entry
  said "caret x assumes monospace `col * char_w`" — true of the code this
  entry described, but `aperture-core/src/width.rs` (`char_col_to_visual`,
  `visual_to_char_col`, the `_clamped` variant for past-EOL selections) landed
  in a later merge (`0ff2ab17`) and was never wired back into this note.
  `view.rs` already runs every column→pixel site (primary caret, click→caret,
  both selection-fill loops, secondary carets and selections) through it —
  checked by reading the call sites, not assumed. 7 tests in
  `aperture-core`'s own suite (`common_cjk_ranges_are_double_width`,
  `visual_to_char_col_is_the_inverse_and_rounds_inside_wide_glyphs`, …).
  Verified interactively too: a Rust file with a CJK identifier (`你好`),
  clicking the pixel position after both glyphs landed the caret at the
  correct CHAR column (11, not the byte or glyph-index equivalent), and
  Backspace removed exactly one CJK character, not a partial codepoint or
  both at once.
  What's still real: this is `char_w`-scaled (each wide glyph costs exactly
  2 cells), not font-shaped — egui does no full shaping, so a glyph whose
  true rendered width isn't a clean multiple of `char_w` (some fonts, some
  emoji) can still be a few px off. That's an engine limit, not a bug in
  this table, and not worth chasing further without shaping support egui
  doesn't have.
- ~~**IME preedit**~~ **DONE, and this note didn't know it either.**
  `egui::Event::Ime` → `Ev::Preedit`/`Ev::Text(commit)` is fully wired
  (`view.rs` translates it, `state.rs` shows composing text with an
  underline distinct from the ghost, reports `IMEOutput` so the platform's
  own candidate window appears at the caret) with its own test coverage
  (`preedit_shows_composing_text_and_dismisses_ghost_and_popup` and
  siblings). Same pattern as the CJK and minimap entries below: the work
  landed, the note just never got told.
- ~~**minimap** on the scrollbar rail~~ **DONE** — `view.rs`'s `scrollbar()`
  already draws it (its own doc comment says so: "Minimap marks: where in
  the WHOLE FILE... does something worth noticing sit?"), diff lines and
  find matches over the full-file scrollbar rail, tested
  (`scrollbar_marks_reflect_real_uncommitted_lines_and_find_matches`). Marks
  rather than a literal miniature code render — if a real pixel-preview minimap
  is what's wanted instead, that is a different, bigger feature, not a
  follow-up to this one.
- ~~**egui_kittest** snapshots for the view layer~~ **DONE.** A prior pass
  proved the harness→render→compare pipeline worked at all with a
  deliberately text-free solid-rectangle baseline
  (`tests/kittest_snapshot.rs`), reasoning that a system-font substitution
  could make a text-bearing snapshot flaky across machines. Checked that
  assumption before extending it: `view.rs` uses `FontId::monospace`, which
  resolves to egui's own EMBEDDED font, not a system one — bundled bytes
  shipped inside the `egui` crate, so two machines on the same `egui`
  version render identical glyph bitmaps. Added `tests/view_snapshot.rs`:
  an empty buffer, a small syntax-highlighted Rust file, and a multi-cursor
  selection (the one piece of this session's multi-cursor work that had
  only ever been eyeballed in an Xvfb screenshot, now a standing regression
  test instead of a one-time look). Needed `mesa-vulkan-drivers` (lavapipe)
  installed in-sandbox for egui_kittest's wgpu-based renderer to have a
  device at all — undocumented prerequisite, worth knowing for the next
  session that tries this cold.

  Found a real bug on the way, by looking at what the baseline actually
  rendered rather than only checking the test passed: at 400px width the
  status bar's right-to-left "Ctrl+P commands · F2 loop" hint visually
  overlapped the left-to-right "Rust" language label — mixing writing
  directions inside one `ui.horizontal` doesn't reserve space between them
  in egui, so at narrow widths (the mobile-Linux targets this project
  cares about, not just a small desktop window) the two drew on top of
  each other. Fixed by measuring remaining width before the right-aligned
  section and hiding the hint — the single most droppable thing in the
  bar — when there isn't room for it, rather than letting it collide.
  Verified both directions with baseline snapshots: hidden and legible at
  400px, present and legible at 700px.

  `cargo test -p aperture-egui --lib`: 88/88, unaffected (the fix and the
  new tests are both `view.rs`/integration-test-only).
- ~~**LSP**~~ **client crate DONE and verified against a real server; not
  wired into the shell.** New `crates/aperture-lsp`: `rpc.rs` (Content-Length
  JSON-RPC framing, tested against adversarial byte streams — truncated
  body, missing header, stray extra header, back-to-back messages — not
  just its own writer's round-trip), `client.rs` (spawn a server, the
  `initialize` handshake, `did_open`/`did_change`/`completion`).

  Deliberately BLOCKING per request, not `aperture_complete::engine::
  Engine`'s worker/inbox/outbox shape — building that concurrency story
  before this crate had ever been driven against a real server would mean
  guessing which calls are slow instead of finding out. The seam for an
  `Engine`-style wrapper is this client's own public methods.

  The real proof, not just unit tests of parsing logic:
  `tests/rust_analyzer_smoke.rs` downloads and spawns an ACTUAL
  `rust-analyzer` (no apt package for it in this sandbox; a GitHub release
  binary) against a real minimal cargo package on disk. Three tests:
  completing a locally-defined function (the whole handshake → didOpen →
  completion round trip), a blank-line edge position, and `did_change`
  actually taking effect (writes a v2 file, sends `didChange`, completes a
  function that only exists in v2 — would fail if `didChange` were a
  no-op). Skips rather than fails when `rust-analyzer` isn't on `PATH`.
  `cargo test -p aperture-lsp`: 17/17 (14 unit + 3 real-server), ~2s,
  reproducible across repeat runs, no lingering server process after
  `shutdown()`.

  ~~no wiring into `aperture-egui` at all~~ **DONE, same session,
  follow-up commit.** `lsp_worker.rs` — the `Engine`-shaped wrapper this
  crate's own doc pointed at — plus `AppState` fields and a merge into
  the existing symbol popup. Opening a `.rs` file spawns `rust-analyzer`
  best-effort (silent if it's not on `PATH`); every edit sends
  `didChange` unconditionally from `after_edit`; the popup now opens (or
  gets extra items merged in) from an LSP answer even when the LOCAL
  tree-sitter-outline tier found nothing — a std or cross-file symbol,
  which that tier is structurally unable to see (it parses only the
  currently open buffer, confirmed by reading `gather_symbols`, not
  guessed).

  Two more real bugs found building the end-to-end proof, not guessed:
  the first version of the test used a std type (`String`) to prove the
  LSP tier specifically, and hung for 110s producing nothing —
  `rustc --print sysroot` showed no `rust-src` component installed in
  this sandbox, so rust-analyzer genuinely cannot resolve std types here
  at all. Rewritten around a cross-file symbol instead (same
  "structurally invisible to the local tier" property, no sysroot
  needed) — and THAT still failed until a second bug was found: a
  completion queried immediately after a rapid burst of `didChange`
  notifications with no pause can race ahead of rust-analyzer's own
  reanalysis. Confirmed with an isolated repro (identical edit sequence:
  zero items with no pause, correct items with a 1s pause) before
  touching the test again. Also bumped `LspClient::CALL_TIMEOUT` from
  30s to 90s, found too short on this same path.

  `cargo test -p aperture-egui --lib`: 98/98, unaffected. The new
  `lsp_popup_e2e` test: 1/1, through the REAL `AppState -> LspWorker ->
  rust-analyzer -> popup` path, reproducible, no lingering process.

  ~~Known limitation~~ **FIXED, one commit later.** LSP `position.
  character` is a UTF-16 code-unit offset; `caret_pos().col` was a raw
  char count, wrong for anything needing a UTF-16 surrogate pair
  (astral-plane codepoints). Added `char_col_to_utf16`/
  `utf16_col_to_char` to `aperture_core::width` — same module CJK/
  double-width visual columns live in, same shape of problem (one char
  isn't always one unit of whatever a consumer counts), different unit.
  5 new tests. Caught a self-inflicted bug while writing this one too: an
  earlier `str_replace` had dropped `#[cfg(test)]` off `width.rs`'s test
  module, so the whole thing was compiling into the production build —
  found via an "unused import" warning that made no sense for a module
  whose tests obviously use it, not dismissed as noise.

  ~~diagnostics (`publishDiagnostics` — a server-initiated notification
  the reader thread still drops on the floor)~~ **client-level piece
  DONE, one commit later; NOT wired to the shell.** `Diagnostic`/
  `DiagnosticSeverity`, parsed in `reader_loop` (the notification
  replaces the FULL list per URI each time, no delta to reconcile), read
  via `LspClient::diagnostics(uri)` — there's no request/response for
  diagnostics in the protocol, only a push, so this just reads the reader
  thread's own cache. 6 new parser unit tests. The real proof: a genuine
  `&str`-to-`i32` mismatch against a real `rust-analyzer`, checked for
  the right severity and line.

  Found and fixed a real test bug on the way: `scratch_package()` built
  its temp directory from `std::process::id()` alone — identical for
  every test in one binary, and `cargo test` runs them concurrently on
  separate threads by default, so parallel tests were racing on the same
  directory and the same rust-analyzer workspace root. Fixed to take a
  per-test name.

  What did NOT make it in, named rather than hidden: a second test
  asserting that fixing the error via a follow-up `didChange` clears the
  diagnostic. An isolated repro showed the SECOND `didChange`'s content
  genuinely never reflected in a later diagnostics update, even after 40+
  seconds — the diagnostic stayed pinned to the FIRST version's error.
  Real client bug, a rust-analyzer version/config quirk, or a sandbox
  resource limitation — not enough signal yet to know which, and a flaky
  or wrong test claiming more than is proven would be worse than naming
  the gap. Follow-up work.

  ~~Still not done: ... wiring diagnostics into the shell at all (no
  gutter marks, no status count — the client can fetch them, nothing in
  aperture-egui asks it to yet)~~ **DONE, later commit.** `lsp_worker.rs`
  wakes on a 200ms `recv_timeout` instead of blocking indefinitely on
  `rx.recv()` — diagnostics are PUSHED by the server, not requested, and
  the common case ("type an error, then pause") means the diagnostic
  arrives AFTER the last edit, exactly when the worker would otherwise
  have nothing left to wake it. Forwards only when the list actually
  changed. `AppState.lsp_diagnostics`, a second gutter bar (deliberately
  not sharing the git-diff bar's x position or color — an uncommitted
  line with a live error is a common, real combination), and a status-bar
  error/warning count, measured into the same room-budget check the
  earlier status-bar overlap fix established so this can't reintroduce
  that bug. Real proof: `lsp_diagnostics_e2e.rs` opens a genuine
  `&str`-to-`i32` mismatch against a real `rust-analyzer` and polls
  `AppState` (not the client) until the error surfaces on the right
  line — deliberately does NOT test that fixing the error clears it,
  since the paragraph above already found that unreliable and a second
  test on the same unstable ground would just be more flakiness, not
  information. Screenshot-verified afterward too: a real red gutter bar
  on the mismatched line, "✕ 2 errors" in the status bar, both sourced
  from a live server process.

  ~~go-to-definition~~ **client-level DONE, one commit later; not wired
  into the shell.** `LspClient::goto_definition(uri, line, character) ->
  Vec<Location>` — every location the server names, not collapsed to the
  first (a trait method can legitimately resolve to several
  implementations). Handles all three response shapes the spec allows
  (`null`, a single `Location`, an array, and `LocationLink` — a shape
  this client doesn't advertise support for but nothing on the wire stops
  a server from sending anyway), preferring `LocationLink`'s
  `targetSelectionRange` (the identifier) over `targetRange` (the whole
  enclosing item) so a jump lands on the name, not the top of a
  potentially long function body. 10 new unit tests. Real proof:
  `goto_definition_finds_a_locally_defined_function_through_a_real_
  rust_analyzer` — a call site and its definition in the same file,
  checked against what a real server actually resolves. `cargo test -p
  aperture-lsp`: 29/29 (24 unit + 5 real-server), ~4s.

  ~~wiring go-to-definition into the shell at all (no Ctrl+click, no
  palette command — client capability only)~~ **DONE, F12.** Third
  `lsp_worker.rs` outcome channel (`DefinitionOutcome`, its own generation
  counter — two unrelated answers sharing one `Option<T>` is confusing
  fastest), same coalesce-to-newest rule as completions. `go_to_definition`
  converts the caret's char column to UTF-16 via `char_col_to_utf16` (a
  second real caller for the position fix above); `jump_to_lsp_definition`
  takes the first location LSP names, opens the target file through the
  normal `open_file` path if it isn't the current one, converts the
  response position back with `utf16_col_to_char`. `poll_lsp` now
  collects definition outcomes into a local `Vec` before processing —
  applying one can call `open_file`, which reassigns `self.lsp` itself,
  which can't happen while still holding the `&self.lsp` borrow
  `poll_definition()` needs.

  Real proof: `lsp_goto_definition_e2e.rs` jumps ACROSS FILES through a
  real `rust-analyzer` and checks the editor actually landed there.
  Retries F12 if the first answer comes back empty — the same
  "server needs a moment right after `didOpen`" finding from completions
  and diagnostics, confirmed again here with an isolated raw-client repro
  (identical query: empty on the first attempt, succeeds ~1s later)
  before assuming a retry was the right fix rather than a symptom of
  something actually broken.

  ~~hover~~ **client-level DONE, one commit later; not wired into the
  shell.** `LspClient::hover(uri, line, character) -> Option<Hover>`,
  flattened to plain text regardless of which of the THREE legal response
  shapes the server used (a `MarkupContent` object, a bare `MarkedString`,
  the older `{language, value}` per-block shape, or an array of any of
  those) — no renderer in this client would benefit from keeping Markdown
  structure separate. `None` for a `null` response (nothing hoverable, a
  normal answer) and for an empty-string value (a hover with nothing to
  say is the same as no hover). 8 new unit tests. Found and fixed a
  genuine copy-paste bug while writing this — a duplicated `#[test]`
  attribute that made `cargo test -- --list` report the same test twice
  under an identical path — traced with `--list | grep -c` rather than
  dismissed once every individual run still said "ok". Real proof: a
  real `rust-analyzer`'s hover text for a function call, checked for the
  function's actual return type. `cargo test -p aperture-lsp`: 36/36 (30
  unit + 6 real-server), ~4s.

  ~~wiring hover into the shell at all (no hover-on-hover UI, no palette
  command — client capability only)~~ **DONE, Ctrl+I.** Fourth
  `lsp_worker.rs` outcome channel, same coalesce-to-newest shape as the
  other three. Shown as a one-line status-bar message, not a full hover
  overlay (future work). `hover_headline` picks which line of the
  flattened hover text to show — NOT just the first line: found
  interactively that rust-analyzer's hover for a same-crate function
  commonly puts a bare crate-name annotation line first, and a naive
  first-line version showed the package name instead of the type
  signature. Prefers the first line that looks like Rust code (a keyword
  or `->`), falls back to the first non-blank line otherwise. 4 new unit
  tests including the exact bug scenario. Real proof: `lsp_hover_e2e.rs`
  drives Ctrl+I through the real path and checks the status bar for the
  actual return type; retries on "nothing found" the same way completion
  and go-to-definition's tests already do. `cargo test -p aperture-egui
  --lib`: 106/106 (was 102).

  ~~incremental sync (`did_change` always sends the whole document)~~
  **client-level DONE, one commit later; the shell still uses full sync.**
  `LspClient::did_change_range(uri, start_line, start_character, end_line,
  end_character, new_text, version)` for a RANGE-based delta, gated on
  `supports_incremental_sync()` reading the server's own advertised
  `capabilities.textDocumentSync` — refuses rather than sending a range
  delta to a server that only understands full-document sync, which
  would silently desync its copy of the file. `parse_sync_kind` handles
  both legal shapes (a bare integer or the modern `{change: N}` object).
  Real proof: two range-based inserts against a real `rust-analyzer` (a
  new function, then a call site), confirmed via completion that the
  incrementally-inserted symbol actually landed. `cargo test -p
  aperture-lsp`: 40/40 (33 unit + 7 real-server, was 36).

  Two self-inflicted bugs found and fixed while building this: a
  `str_replace` that landed a struct field inside the wrong function's
  parameter list (caught immediately by the compiler), and — worth
  naming since it's a genuine lapse — a duplicated `#[test]` attribute
  that I INITIALLY misdiagnosed as a harmless test-harness quirk from a
  symbol-table dump, before `cargo check --workspace`'s own `warning:
  duplicated attribute` on the very next build showed the source-grep
  reasoning had been wrong. Left in TODO.md as a lesson, not smoothed
  over: check the compiler's own warnings before building a case for why
  something odd "must be" benign.

  ~~Still not done: the worker/shell side still calls the full-document
  `did_change` unconditionally — this is client capability only. Wiring
  the shell to prefer `did_change_range` needs an actual text diff
  between old and new buffer content, which `aperture-core` has no
  utility for yet~~ **DONE, one commit later.** New `aperture_core::
  textdiff` module: a common-prefix/common-suffix diff, not a general
  Myers diff — deliberately, since the one real caller (incremental
  `didChange`) only ever has ONE contiguous edit to describe at a time
  (a keystroke, or a single `replace_range`/`replace_many` call), which
  this finds in O(n). 11 unit tests, including the "middle char of a
  repeated pattern" case that needs the suffix scan bounded by remaining
  length rather than by zero, and a multi-byte-codepoint safety test
  rather than an assumption.

  `lsp_worker.rs`'s `Msg::Change` now carries an optional edit tuple
  ALREADY UTF-16-converted by the caller (the worker has no line-text
  access to do that conversion itself); the worker decides whether to
  actually use it — checking `supports_incremental_sync()`, which only
  it can call — falling back to full-text sync on any failure.
  `AppState` tracks `lsp_last_synced_text`, updated unconditionally after
  every send so the next diff is always against what the server was
  actually told.

  Real proof: `lsp_incremental_sync_e2e.rs` types a CROSS-FILE symbol
  character by character (many separate incremental edits), backspaces
  and retypes part of it, then confirms a real `rust-analyzer` still
  resolves it via completion. Cross-file specifically — a same-file
  symbol would let the test pass even if incremental sync silently did
  nothing, since the local tree-sitter tier alone already knows same-file
  symbols; caught my own first draft using the weaker same-file version
  and rewrote it before trusting a pass. `cargo test -p aperture-core
  --features tree-sitter`: 171/171 (was 160). All four existing LSP e2e
  tests re-run and still pass, since they all now exercise this path on
  every keystroke.

  ~~Also still not done: a real hover overlay (currently a one-line
  status message, not a panel showing the full flattened text)~~ **DONE,
  one commit later — and building its test uncovered a serious
  pre-existing bug.** `AppState.hover_overlay` holds the full flattened
  hover text; a new `view.rs` overlay (modeled on `edit_review_overlay`'s
  shape) renders it in a scrollable panel, cleared on Escape, on the next
  edit, or when superseded by a newer answer.

  **The bug found building it**: `type_text`'s SINGLE-CURSOR plain-typing
  path inlined three of `after_edit`'s six responsibilities and silently
  never ran the other three — including `notify_lsp_changed`. Concretely:
  `didChange` was NEVER sent to the LSP server for an ordinary keystroke,
  only for backspace/delete/enter/tab and the multi-cursor fan-out path.
  Every LSP e2e test in this whole session happened to still pass despite
  this, because they all ask for completions at a position whose
  surrounding SCOPE is already correct from `didOpen` alone — an
  unfiltered "what's in scope here" answer doesn't need the exact typed
  prefix to have been communicated to look right. Found only because a
  test for the overlay's "clear on next edit" behavior failed and the
  failure didn't match the mental model of what should have been true.

  Verified BOTH directions before trusting the fix: `lsp_typing_sync_
  regression.rs` defines a brand-new local variable by typing its full
  declaration character-by-character, then asks for completions against
  that exact name from a different line — answerable ONLY if the
  keystrokes were actually synced. Confirmed it fails against the
  pre-fix code (reverted `state.rs` by hand, ran it, watched it fail)
  and passes against the fix, in that order, before committing either.

  Also found and fixed a SECOND duplicated `#[test]` attribute while
  writing tests for this — same bug shape as twice before this session,
  this time caught immediately via `cargo check --all-targets`'s own
  warning rather than grep-based reasoning, which is the lesson the
  previous occurrence was supposed to have already taught.

  `cargo test -p aperture-egui --lib`: 108/108 (was 106). Full workspace
  check clean, no lingering `rust-analyzer` process.
  `workspace_symbols`'s local tree-sitter outline and `route`'s
  keyword-scored candidates stay the fast, dependency-free default tiers;
  LSP is an ADDITIONAL tier alongside them, never a replacement.

  Interactive Xvfb verification DID happen, one commit later, and found
  a THIRD real bug the automated test's own code path couldn't have
  caught: `AppState::new` (what `aperture some_file.rs` — the normal way
  anyone runs the binary — actually calls) never went through
  `open_file`, so LSP spawning silently never fired for a file opened
  from the command line, only via the in-app Ctrl+O dialog. Found by
  launching the real binary the normal way and watching nothing happen;
  fixed by calling `start_lsp_for_current_file` from `new()` too.
  Screenshot-verified afterward: typed a cross-file prefix in a real
  window, the popup showed `cross_file_only_symbol  fn() -> i32` merged
  in with the local tier's own candidates.
- **Voice commands — the text-side half, DONE; the audio half deliberately
  not attempted.** aperture has no microphone, no ASR, no wake-word
  detector, and building one from scratch would duplicate `shared/
  voice-kit` and `apps/voiceterm/egui-frontend/src/voice_routing.rs`'s
  hallucination-trimming, wake-prefix, and consumer-arbitration work —
  those own "how do we hear" and "who gets to act"; this session added the
  piece neither of them does: turning an already-decoded transcript into
  an EDITOR command.

  `aperture-core/src/voice.rs`: a small, deliberately unforgiving
  `VoiceCommand` grammar (save, undo/redo, select all, copy/cut/paste,
  find/search-for, go-to-line, add-cursor/select-all-occurrences, a
  `type <words>` escape hatch for literal dictated text) — unforgiving
  because voice_routing.rs's own philosophy is that a near-miss should not
  execute, and a command vocabulary loose enough to catch every phrasing
  is also loose enough to fire on ordinary dictated prose. 9 tests.

  `AppState::run_voice_command`: every arm calls an EXISTING method
  (`save`, `Editor::undo`, `add_next_cursor`, ...) rather than
  duplicating its logic, so a voice command and its keyboard equivalent
  can never disagree about what it does. Returns clipboard text for
  Copy/Cut the same shape `Effects` does from `handle()`, since this
  headless method has no path to the platform clipboard itself. 10 tests.

  Palette entry "Voice: run command…" — a typed stand-in for what a
  transcript would say, useful on its own for testing command shapes
  without a working microphone.

  Two real bugs found by running the actual binary, not by `cargo test`:
  a first attempt bound this to Ctrl+Shift+V, which silently never fired —
  traced to egui delivering Ctrl+V-family combos as its own clipboard
  `Event::Paste`/`Event::Copy` before they become a raw key event, in
  every environment where that interception applies. Removed the
  keybinding rather than ship one that quietly does nothing; the palette
  route is the only entry point now, and it works (verified: "find total"
  opens the bar seeded and matched, "go to line 3" moves the caret,
  "save" writes the file and shows "saved" in the status bar). Also found
  two now-divergent copies of the same "open voice mode, discard stale
  cursors" logic (the removed keybinding's handler and the palette
  dispatch) and merged them into one method before they could drift.

  `cargo test -p aperture-core --features tree-sitter`: 143/143 (was
  134). `cargo test -p aperture-egui --lib`: 98/98 (was 88).

  Not attempted: an actual dictation sink (voice_routing.rs's
  `Listening::Commands` state is the model to follow — this parser would
  be the intent matcher it describes as "suspended during dictation," not
  something that runs on every transcript unconditionally); this needs a
  real audio pipeline wired to aperture's process model, which is a
  materially different and larger piece of work than the grammar above.

  **Follow-on, same session: vocabulary buildup + multi-speaker
  sessions.** `parse_with_vocab` resolves "go to <name>" / "jump to
  <name>" / "navigate to <name>" against a LIVE vocabulary (a project's
  own symbol names) rather than a fixed word list — substring/prefix
  matching, not a general fuzzy scorer, since `uikit_egui::fuzzy` lives
  in a UI crate `aperture-core` can't depend on without giving up its
  headless property, and the one transformation actually needed (joining
  spoken words with underscores, since a recognizer emits pauses as
  spaces) doesn't need general fuzzy matching to get right. Vocabulary is
  refreshed from the current outline on every command, not cached — a
  voice command is rare enough that re-deriving costs nothing, and a
  stale vocabulary missing a function someone just wrote would be a
  confusing silent failure.

  `VoiceSession`: per-speaker "again"/"repeat that" history, keyed by a
  `SiteId`-shaped string matching `aperture_collab`'s existing
  multi-participant model rather than inventing a second one (though
  collab itself still isn't wired to the shell either). The one piece of
  multi-user behavior that's silently wrong by default if missed: Bob
  saying "again" must replay BOB's own last command, not whoever spoke
  most recently session-wide. `run_voice_command` now delegates to
  `run_voice_command_as("local", ...)`; a real multi-participant caller
  uses `run_voice_command_as` directly.

  `cargo test -p aperture-core --features tree-sitter`: 160/160 (was
  148). `cargo test -p aperture-egui --lib`: 102/102 (was 98).

  Not done: no audio pipeline (still 100% text-side — an utterance has
  to arrive as a string from somewhere upstream), no real speaker
  diarization/voiceprint (speaker identity is an opaque string this
  layer trusts, not something it derives), no wiring of
  `aperture-collab`'s actual multi-participant session into
  `aperture-egui` (`run_voice_command_as` exists and is tested, but
  nothing in the shell calls it with more than one speaker yet).

## Reported from device use (2026-09-05) — six, unverified here unless noted

Raised by the operator running the apps; each is marked with how far I got.
None of these are merge fallout: the two bundles merged this session touched
only `apps/bscale` and `games/blackstar-rs` (checked with
`git diff --name-only` on both merge commits), so voiceterm, aperture and
beatspace are untouched by them. Answering that directly because it was
asked as a question.

- [ ] **voiceterm-egui: Ctrl+C does not kill a running terminal process.**
  REPORTED; traced through the code here but not reproduced live. The
  encoder is CORRECT and tested: `pty_input::encode(Key::Char('c'), ctrl)`
  returns `[0x03]`, with a test for the Ctrl+Shift+C case too. Ctrl+C is not
  in `uikit_term::keys::is_widget_chord` (that reserves only Ctrl+F/Ctrl+P
  and Shift+PageUp/Down) and not in `pty_input::chord`, so nothing on the
  reserved paths eats it.

  **CONFIRMED 2026-09-05** by `ctrl_c_reaches_the_pty_with_no_focus_and_is_swallowed_by_a_focused_field`
  in `pty_input.rs`: with nothing focused Ctrl+C produces `[0x03]`; with a
  focused `TextEdit` on screen the drain returns EMPTY and the interrupt
  never leaves the app. No longer a suspicion.

  THE CAUSE: `pty_input::drain_egui` line ~176 —
  `if ui.ctx().egui_wants_keyboard_input() { return Vec::new(); }`. That
  discards the WHOLE event batch, Ctrl+C included, whenever any egui widget
  wants the keyboard. voiceterm has many text fields (the say box, filters,
  the graph source, the palette), and if focus is sitting in one — or if the
  terminal pane itself does not hold egui focus — the interrupt never
  reaches the PTY. The guard exists for a real reason (typing a search query
  also typed it into the shell), so the fix is not to delete it: it is to let
  a small set of control chords through even when a widget has focus, or to
  ensure the terminal pane takes focus when clicked.

  THE FIX IS A JUDGEMENT CALL, deliberately not made here. The guard exists
  for a real reason — its comment records that typing a search query was
  also typing it into the shell — and Ctrl+C is ALSO the copy chord in a
  text field, so "let control chords through" would break copy where a
  person expects it. Three options, in rough order of preference:

  1. Make the terminal pane take egui focus when clicked, so a focused field
     genuinely means the field is where you are typing. Fixes the class, not
     just Ctrl+C.
  2. Allow a small chord set through only when the focused widget is NOT a
     text field (egui can distinguish).
  3. Allowlist Ctrl+C/D/Z unconditionally and accept losing copy in fields.

  The characterising test asserts today's behaviour, so whichever is chosen
  it will fail loudly and can be updated deliberately.
- [ ] **voiceterm-egui: further visual bugs.** REPORTED, unspecified. Known
  and open from this session: the locator strip was ~39 px for an 11 px
  label; PARTLY fixed 2026-09-05 (now ~24 px — it owns its trailing gap
  instead of inheriting the touch-scaled one). The remaining ~13 px is the
  pane's own top inset, not measured yet. Also a one-frame panel overlap after a programmatic
  resize. The demo scaffolding (fake "kesslerd" workspaces, canned
  transcript) is now behind `VOICETERM_DEMO=1`, so anything still appearing
  by default is a genuine bug rather than seed data.
- [ ] **aperture: command palette and sizing.** REPORTED, not investigated.
  `uikit_egui::command_bar` (new this session) is the intended answer —
  aperture is a second consumer for it, alongside snapclone.
- [x] **bscale launches the CLI from gitmind, not the GUI.** FIXED
  2026-09-05. The subcommand was REQUIRED, so a launcher running the bare
  binary got clap's usage text and bscale looked CLI-only.
  The binary is CLI-first with the GUI behind the `app` subcommand
  (`Cmd::App` in `crates/bscale/src/main.rs`) — so a launcher invoking
  the bare binary gets the CLI usage text. Whatever gitmind's app catalog
  records for bscale needs `app` appended, or bscale needs to default
  to the GUI when run with no subcommand and a display present.
- [x] **bscale: drag-and-drop onto the input, with a confirmation.**
  DONE 2026-09-05 (`d2x_core::dropped` + a confirm dialog). Dialog itself
  not screenshotted — synthesising a real X11 drop needs a drag source.
  WAS: eframe surfaces dropped files via `ctx.input(|i| &i.raw.dropped_files)`;
  the confirmation matters because a drop replaces the input path and the
  previous one is not recoverable from the UI.
- [x] **Leaf LOD A/B done 2026-09-05 — it works.** First look now
  that the graph can be cleared (`LOGSCOPE_CONVO=/tmp/tiny.logscope`):
  `LOGSCOPE_THEME=Canopy LOGSCOPE_CAM="0.7,0.28,55"` shows crowns as
  scattered specks with gaps you can see through
  (outputs/canopy-leaf-lod-at-distance.png). That is the exact symptom
  `leaf_lod` was written to prevent.

  ANSWERED. Same camera, same crop, one binary with a temporary env switch
  between the two behaviours: outputs/leaf-lod-A-before.png (blades at
  original size, crowns are fine scattered specks with sky through them)
  versus outputs/leaf-lod-B-after.png (blades visibly larger, crowns read as
  continuous foliage). The coverage-preserving growth does what it was
  written to do. It is not fully dense — 69% coverage is 69% — but the
  see-through look is gone.

  The arithmetic says coverage should hold at ~69% of full at this distance:
  `detail` bottoms out at 0.14, so 84 blades become 12 and growth wants
  `sqrt(84/12) = 2.65` but clamps to `LEAF_GROWTH_CAP = 2.2`, giving
  `12 * 2.2^2 = 58` of 84. If the crowns still read see-through at 69%
  coverage then the cap is the thing to revisit — or the blade `len` clamp
  `.clamp(0.05, 0.30 * blade_growth)` is biting before the growth does.

  IF MORE DENSITY IS WANTED, the cap is now the known lever and the method
  is established: a temporary `if std::env::var_os("D2X_LEAF_OLD")` switch
  in `leaf_lod`, one build, two runs at the same `LOGSCOPE_CAM`. Do not
  raise `LEAF_GROWTH_CAP` past ~2.2 without looking — that is where blades
  stop reading as leaves.

- [~] **One-character entry bug in text fields.** CAUSE FOUND and reproduced
  2026-09-05: a `TextEdit` with no explicit id gets one derived from its
  position in the frame, so anything that changes the widget order above it
  — a conditional row, an error label appearing, a list that grew — gives it
  a NEW id. egui treats that as a brand new widget, focus and caret are
  gone, and the person sees one character land and the field go dead.

  NOT what I first assumed. A repeated `request_focus()` was the obvious
  suspect (and logscope has a comment blaming it), but a two-arm test shows
  the unguarded version still accepts both characters on this egui version.
  `focus::focus_once` is still correct practice and is in the kit; it is not
  the cause.

  FIXED: switcheroo's six fields, all now salted. `uikit_egui::focus::
  text_field(ui, buf, salt)` makes the salt a required argument so it cannot
  be forgotten.

  REMAINING UNSALTED (grep `TextEdit::singleline` and filter out
  `id(`/`id_salt(`): voiceterm 4, snapclone 2, studio 2, logscope 1. Each is
  only a bug if the widget order above it can change, so they want reading
  rather than a blind sweep — but salting is harmless, so salting them all
  is a defensible sweep too.

- [ ] **Act on the GPU pacing numbers (`gpu_kit::pace`).** The MEASUREMENT is
  wired: `WgpuUpscaler` times both `device.poll` sites (there are two —
  instrumenting one would have reported half the GPU time as zero) and
  exposes `gpu_metrics()`. What is NOT done is acting on them: the pacer's
  `chunk()` is ignored because a whole-frame upscale is submitted as one
  command buffer. Splitting it means banding the frame — `PackDst` already
  carries `crop_x/crop_y/crop_w/crop_h`, so the shape is there. Do the
  measurement pass on real hardware FIRST; if `overrun_rate()` is near zero
  on the target machine the splitting is not worth its complexity.
- [~] **A metrics pane in each app.** bscale DONE 2026-09-05
  (`d2x_core::jobmetrics` + `metrics_pane`, rendered in `carved::inset`).
  Other apps still to do. `gpu_kit::pace::Metrics::summary()`
  renders the line and `overrun_rate()` is the single number worth showing.
  bscale can read it today via `gpu_metrics()`; logscope needs the same
  instrumentation around its `glow_scene` draw first.
- [ ] **logscope scene props need more depth.** Reported alongside the
  stutter; not started. The props read as flat next to the reference images.

- [ ] **logscope scenes: ground and leaf textures gone flat (reported with a
  screenshot, 2026-09-05).** NOT REPRODUCED here — at HEAD under llvmpipe the
  Canopy terrain still shows its texture, so this is environment-dependent
  rather than a regression in the recent scene-kit commits (leaf LOD, the
  label floor, textsurface names — none of which touch the material path).

  TRACED TO A MECHANISM, though, and it is a plausible fit. `85ab9670`
  ("auto-detect weak GPUs, default power down") made `theme::power_mode()`
  fall back to `perf::suggested_power_for_renderer(GL_RENDERER)` when nothing
  chose one. Low power then does TWO things that erase fine texture:

  * `glow_scene.rs:126` renders the whole scene at that fraction of
    resolution (cost goes as scale²) and upscales — quarter power means 25%,
    which smears out exactly this kind of high-frequency ground/leaf detail.
  * `theme::effective_variant()` CAPS the baked texture tier: `p <= 0.3`
    clamps the variant to 3, `p <= 0.6` to 6, "regardless of the chosen
    quality dial". So the texture arrays themselves are baked smaller.

  Both are silent — nothing in the UI says the quality dial has been
  overruled, which is why this reads as "the textures disappeared" rather
  than "power saving is on".

  FIRST THING TO TRY, one line: `LOGSCOPE_POWER=full` in the environment.
  That branch (`theme.rs:330`) takes priority over the auto-detect entirely.
  If the textures come back, the auto-detect is firing on that device and the
  fix is either a better `suggested_power_for_renderer` arm for its
  `GL_RENDERER` or making the override visible in the UI.

  ALSO WORTH CAPTURING: the app logs its renderer at startup —
  `[logscope] GL_VENDOR=... GL_RENDERER=...`. That string is what
  `suggested_power_for_renderer` matches on, and the function'"'"'s own comment
  says it grows "the next time a specific device is slow and its GL_RENDERER
  string is in hand, not from a guess about silicon not yet measured". This
  is that moment.
- [x] **beatspace: workspace on desktop vs tabs on mobile.** DONE
  2026-09-05, verified at both widths. Previously started and
  REVERTED — a width-fraction panel was tried, looked at, and
  traded one clipping for another. The recipe is in commit `0ac14413`:
  `controls_body` = lines 431-517, `content_body` = the Train branch plus
  552-632, both in a NEW `impl App` block (not the `eframe::App` one), the
  `Panel::bottom("scope")` declared first, then `two_pane_ui`.

## voiceterm — loop-trimmer off-by-one (fixed at source, 2026-08-19)

`apps/voiceterm/src/voice.rs`'s `trim_hallucination_loop` predates the
`voice-kit` extraction and still carried the bug the kit's copy already
fixed: `for start in 0..(words.len() - window * 3)` is an EXCLUSIVE range,
so a hallucination loop whose shingle repeats exactly 3× and fills the
*entire* word list (no trailing words after it — e.g. "go left go left go
left") has its only valid `start` position sit exactly at the excluded
upper bound and goes undetected. `shared/voice-kit/src/hallucination.rs`
already uses the inclusive `0..=(...)` and has a test naming this exact
case ("the off-by-one case"). Ported the same one-character fix
(`0..` → `0..=`) to voiceterm's copy, with a comment pointing at the kit's
test. Verified the ported logic in isolation (a standalone `rustc`
compile of just the function against voice-kit's own test cases, since
`voice.rs` itself is behind the `voice` feature + qmetaobject/Qt, neither
buildable in this sandbox) — not a substitute for a real `cargo test` in
a Qt-capable environment, but confirms the fix is a correct, minimal,
byte-for-byte port of already-tested code rather than a guess.

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

## Reachability audit — real orphans found (2026-08-22)

`maintenance/tools/reachability.py` asks a different question than
`parity_audit.py`: not "does a surface exist" but "does anything in the crate
call it". Ran it fleet-wide. Most apps' orphan counts are dominated by
cross-crate re-exports the single-crate scan can't see (logscope at 216,
gitmind at 38, snapclone at 42 — not independently verified, likely mostly
false positives from the `-egui` frontend being a separate crate from the
core the tool scans). The SMALL apps are cheap to hand-verify and worth
recording precisely because a small orphan list is actually trustworthy:

**tomescope — 3 orphans found, all now wired (2026-08-22):**

- `lookup.rs` — `Lookup::lookup()`, checks the book's own back-of-book index
  first then a builtin glossary. Nothing calls it; there is no "define this
  word" affordance anywhere in `reader.rs`/`panels.rs`.
- `opds.rs` — parses OPDS 1.2 Atom catalogs (Calibre-Web, COPS, etc), network
  fetch stubbed behind a trait so the parser is unit-tested standalone.
  Nothing calls it; there is no "add a catalog" UI.
- `tts.rs` — sentence-queue builder + optional espeak-ng subprocess backend
  behind an `espeak` feature. Nothing calls it; no "read aloud" button.

All three are declared `pub mod` in `lib.rs` and fully implemented with their
own test suites (3 tests each) — this is the expensive kind of orphan the
tool's own docstring calls out: it passed review, it passes CI, and it does
nothing for a user.

**Not wired in this session, and why:** wiring any of these into
`reader.rs`/`panels.rs` means touching the actual selection/rendering flow
(`Selection`, `draw_page`, `draw_footnote_popover` in `reader.rs`) without a
compiler in this sandbox to catch a wrong borrow or a wrong signature. Guessing
at UI glue in a 500-line rendering file with no build feedback is how a
"working" commit turns into a broken one nobody notices until a real host
builds it. Recorded here as three concrete, scoped next steps instead:

- [x] tomescope: `lookup.rs` wired for real (2026-08-22). Added
      `lookup::selection_text(content, range)` (single-block only — a
      multi-block selection returns `None` rather than guessing how to join
      text across a paragraph break) and `lookup::draw_lookup_popover`,
      mirroring `draw_footnote_popover`'s shape. `ReaderState` gained
      `lookup_open: bool`; `TomeApp` gained a shared `lookup: Lookup`; the
      reader toolbar gained a "🔤 Define" button enabled only when
      `selection.is_active()`.
      **A test caught a real bug before it shipped**: the first version
      built `selection_text` on top of `BookContent::snippet()` (the
      existing search/dictionary-context helper), which turned out to
      unconditionally wrap the result in `…ellipsis…` whenever the range
      doesn't start at 0 or end at the block's length — correct for
      `snippet`'s actual job (showing context around a search hit) but wrong
      here, since it would make every lookup search for `"…ephemeral…"`
      instead of `"ephemeral"` and silently fail. Caught by
      `selection_text_extracts_the_highlighted_word` actually asserting the
      exact string rather than just "is Some" — fixed by slicing the
      block's `plain_text()` directly instead of going through `snippet`.
      Kept a regression test
      (`a_lookup_looks_up_the_real_selected_word_not_the_whole_block`)
      specifically for this failure mode.
      Also fixed, while here: `egui::Context::run` is deprecated in 0.34 in
      favor of `run_ui` — both this file's own new test and the two added to
      voiceterm's `bars.rs` two commits ago had used the deprecated form;
      fixed all three once the correct replacement was known, re-verified
      both crates still clean.
      Verified: `cargo check --bin tomescope` clean in both the default and
      `--features espeak` configurations (9 warnings, same as before both
      times), `shelfctl` still builds, full `cargo test --lib`: 50/50
      passing (45 previous + 5 new `lookup::` tests).
- [x] tomescope: `opds.rs` wired for real (2026-08-22, compiler now
      available). `Screen::OpdsBrowser` already existed in the nav but
      `draw_opds_screen` was a static label with no textbox — the exact
      "placeholder feed" class `wiring_check.py`'s docstring warns about.
      Added a paste-XML `TextEdit::multiline`, a "Parse catalog" button
      calling the already-tested `opds::parse_opds_xml` for real, and a
      results list (title/author, navigation-vs-download distinction via
      `is_navigation()`/`acquisition_link()`, both already-tested methods).
      Verified: `cargo check --bin tomescope` clean (same 9 pre-existing
      warnings, none new), full `cargo test --lib` 45/45 passing including
      the 3 untouched `opds::tests`. Network fetch stays intentionally out
      of scope — `NoNetworkFetcher` is the deliberate stub the module was
      built around, so downloading an acquisition link is separate, opt-in
      work, not part of this fix.
- [x] tomescope: `tts.rs` wired for real (2026-08-22). `ReaderState` gained a
      `tts: TtsQueuePlayer` field; the reader toolbar gained Read/Stop/Next
      buttons; `TomeApp` gained a `tts_backend: Box<dyn TtsBackend>` chosen by
      `default_tts_backend()` (real `EspeakBackend` if the `espeak` feature is
      on AND `espeak-ng` is actually on PATH, `SilentBackend` otherwise, so
      the button never appears broken on a machine without the binary).
      Read builds the queue from the CURRENT page's start locus
      (`page.start`, already computed for pagination — reused rather than
      duplicated), not from the top of the book. **Deliberately NOT
      auto-advancing**: `TtsBackend::speak` fires-and-returns (a spawned
      subprocess for espeak, a no-op for silent) with no completion signal in
      the trait, so this session shipped Play/Stop/Next rather than invent a
      fake "done speaking" detector — real auto-advance needs a
      completion-detection addition to the trait itself, which is a
      different, slightly bigger task than wiring the existing pieces.
      Verified both feature configurations: `cargo check --bin tomescope`
      clean (9 warnings, same as before) and `--features espeak` also clean
      (9 warnings) after adding the `#[cfg(feature = "espeak")]`-gated
      `TtsBackend` trait import the `is_available()` call needs. Full
      `cargo test --lib`: 45/45 passing, unchanged.

**Confirmed false-positive theory for the big multi-crate apps.** Checked
gitmind's three highest-profile "orphans" (`flow`, `incoming`, `appcatalog`,
1868/922/566 lines, 14/20/15 tests) by grep across the whole app tree, not just
within `crates/cockpit`. All three are extensively used — `cockpit::flow` alone
has 50+ call sites across `crates/gui`, `crates/gui-egui/src/pages/{apps,
network,artifacts,fleet,dupes,push,import}.rs`, and `crates/cli`. The tool
scans one crate directory at a time and can't see a sibling crate importing it
as a path dependency, so gitmind's 38, snapclone's 42, logscope's 216 are very
likely almost entirely this same false-positive class rather than real gaps.
**Not claiming this generalizes without checking** — but the one app checked
confirms the mechanism, so treat the large multi-crate counts as noise until
someone teaches the tool to scan a whole workspace rather than one directory,
not as a queue of real orphans to work through.

**A second false-positive class, checked on snapclone.** Its top "orphans" by
line count are `ffi.rs` files (`snapclone-face`, `snapclone-group`) — `extern
"C"`, `#[no_mangle]` functions called from the C++ side (`ffi.rs`'s own header
says so directly: "Exposes the face library... to C/C++ callers"). A Rust-only
reachability scanner cannot see a C++ call site by construction, so every FFI
boundary module in a hybrid Rust/C++ app like snapclone will show as an orphan
regardless of real usage. Between the cross-crate class (gitmind) and the FFI
class (snapclone), that likely accounts for most of both apps' counts — still
not verified item-by-item, but two different confirmed mechanisms now explain
away the two biggest numbers in the table without needing to check all 80.

**logscope's 216 checked the same way, same result.** Its top orphan by tests,
`machinery.rs` (97 tests, `crates/logscope/src/machinery.rs`), IS referenced —
from `crates/logscope-egui/src/{scene3d,enginestrip}.rs` and
`crates/logscope/src/{persona,dupscan}.rs` — same core/`-egui`-frontend crate
split as gitmind's `cockpit`/`gui-egui`. Three confirmed false-positive
instances across the three biggest counts in the table now (gitmind, snapclone,
logscope), all from the same root cause (single-directory scan on a
multi-crate app). Fixing the tool to walk a whole workspace's `Cargo.toml`
members rather than one path would be the real fix; not attempted here since
it risks silently changing what the tool reports for every app, and there's no
compiler in this sandbox to validate the fix against the actual crate graph.

**av1tuner/av1tuner-m1's `eta.rs`/`config.rs` triaged — a fourth confirmed
false positive, not a real gap.** `av1tuner_core::eta::EtaTracker` and
`av1tuner_core::config::UserConfig` are both extensively used — imported and
called throughout `av1tuner_app/src/app.rs` (`EtaTracker::new()`,
`record_chunk`, `frames_per_second`, `eta_string`; `UserConfig::load()`, a
`current_config()` builder). The reachability tool flagged them because
`av1tuner_core` and `av1tuner_app` are separate crates in a two-crate
workspace — same cross-crate blind spot as gitmind and logscope above, just on
a smaller, two-crate example that made it easy to confirm by hand in under a
minute. Fourth confirmed instance of the same root cause; not fixing anything
here, just closing the loop on the one item this doc left untriaged.

Separately, `apps/av1tuner/DUPLICATE.md` (written by an earlier session)
already says `av1tuner` is the stale fork and `av1tuner-m1` is live, "should be
retired once someone confirms nothing external references it." Checked:
`manifest/projects.toml`'s `av1tuner` entry notes `av1tuner-m1` is "the live
fork and is ahead in all five" differing files, matching `DUPLICATE.md`
exactly, and nothing in `build/`, other apps, or the manifest points at
`apps/av1tuner` as a build target distinct from `apps/av1tuner-m1`. That
confirmation was already implicitly done by whoever wrote `DUPLICATE.md`;
not re-litigating it further since deleting an app tree is a one-way,
non-code-verifiable action better left for the user to greenlight explicitly
(matches the repo's own "Parked backlog (no execute without greenlight)"
policy for anything destructive).
