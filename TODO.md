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
- [x] rusty-board voice/hallucination.rs → voice-kit shim. (This checkbox was
      stale as of 2026-08-19 — the file was still a full standalone
      implementation, not a shim. Actually done now; see the "rusty-board
      (newly ingested)" section's voice-kit consumers item below for the
      real commits.)
- [x] Verified: standalone harness crates cargo-check green vs the kits.
- [ ] User builds + runs all apps via GUI to confirm full builds (Qt/GTK, not sandbox-buildable).
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
- [x] **voice-kit consumers, hallucination half**: rusty-board `src/voice/
      hallucination.rs` and voiceterm `src/voice.rs`'s `is_sane_transcription`
      + `trim_hallucination_loop` are now real shims over voice-kit (both
      repointed this session, in small verified-by-comparison commits — a
      previous session's [x] for the rusty-board half was stale, it had
      never actually happened). Neither app compiles in this sandbox (GTK4/
      libadwaita and Qt/qmetaobject respectively aren't installed/available
      here), so both repoints are unverified by compilation — verified
      instead by direct line-by-line comparison against voice-kit's actual
      source before each substitution, and by voice-kit's own 43/43 tests
      covering the functions being delegated to.
- [ ] **voice-kit consumers, model half**: whisper model discovery/resolution
      in both apps still has its own local copy; voice-kit's `model.rs`
      exists and is tested (part of the same 43) but neither app has been
      repointed onto it yet. Wire voice/recorder level metering onto
      audio-kit `level`; a VAD gate (voice-kit over audio-kit) is the next
      voice-kit growth.

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
  `cargo test -p aperture-core --features tree-sitter`: 73/73.
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

- **Instruct-edit shell wiring**: `aperture-complete::edit::make_edit_request`
  (headless, tested) packs an instruction + selection into the existing
  `CompletionRequest` envelope for `CommandProvider` — the request-building
  half of "ask AI to rewrite this selection" exists. NOT wired into the
  shell: needs a way to route an outcome back as "this is an edit result,
  render a diff/accept/reject view" rather than feeding it into ghost text
  the way every other `Engine::poll()` outcome is — the generation-gate
  design assumes one semantic per outcome today. Adding a tag (or a second
  `Engine`) is the shape of the fix; not attempted this pass.

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
- **File-fuzzy palette**: DONE (`file_finder.rs`, Ctrl+O). ~~Find in
  project~~ DONE (`project_search.rs`, Ctrl+Shift+F). ~~Go to symbol in
  workspace~~ DONE (`workspace_symbols.rs`, Ctrl+T, name index over every
  file's existing tree-sitter outline — not semantic go-to-def).
- **Multi-cursor**: `Selection` is already value-based; becomes
  `Vec<Selection>` + edit fan-out with offset fixup.
- **CJK/double-width**: caret x assumes monospace `col * char_w`; wide glyphs
  need galley-measured x (egui does no full shaping — known engine limit).
- **IME preedit** (`egui::Event::Ime`), **LSP** (own crate behind the same
  worker pattern as the completion Engine), **egui_kittest** snapshots for the
  view layer (state layer is already headless-tested), **minimap** on the
  scrollbar rail.

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

**tomescope — 3 real orphans, all tested, all user-facing, none wired:**

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

- [ ] tomescope: wire `Lookup::lookup()` to a long-press/selection → define
      popover (natural home: alongside `draw_footnote_popover` in `reader.rs`).
      Not done: extracting text under a `Selection`/`LocusRange` needs real
      understanding of `BookContent`'s block/run text model, which is a
      bigger job than the OPDS item below turned out to be — left open.
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
