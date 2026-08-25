# C12_TODO — open threads from the c12 session

Same convention as `C9_TODO.md` / `C10_TODO.md` / `C11_TODO.md`: every item
says how far it actually got. None of those files are superseded; C11-2a was
edited in place this session because it turned out not to be a real finding.

The c12 session installed the offline 1.96.0 toolchain, merged two bundles
(c9-006g studio, c5blackstar-008 blackstar-rs), and corrected two pieces of
inherited sandbox lore. What follows is what it found and did not close.

---

## 1. blackstar-rs: 13 tests fail, and they failed on arrival

The c5blackstar-008 merge is committed and the crate now builds every target
for the first time on this line (`cargo ailite test --no-default-features`:
**1200 passed, 13 failed**). All 13 are in the imported work.

**The attribution matters, so here is the evidence rather than the claim.**
The test binary did not compile on the incoming branch (three separate
breakages, fixed in `35ad4fd77`), so no test in the crate had run there. Main
had not touched any of the failing modules since the fork point at
`695fc3b73` — the only files both sides changed are README.md, orientsim.rs
and physics.rs. And `skeleton`, `turret` and `driveline` name no `crate::`
sibling at all: they are glam-only, so nothing this line did can reach them.

- **C12-1a — skeleton, three failures.** `a_vehicle_decomposes_the_same_way_a_body_does`
  ("the wings did not read as limbs: core 1 prims, 1 limbs (reach 1.20), 2
  orphans joined"), `compaction_halves_the_overhang_without_detaching_it`
  ("the wing barely moved: 1.2005669 -> 1.2005669" — the compaction is a
  no-op), `vehicle_mode_packs_the_limbs_inside_the_hull` ("prim 1 still pokes
  out by -0.55/5.35 after containment"). The middle one looks like the root:
  if `compact` moves nothing, the other two follow. `read`, not diagnosed.
- **C12-1b — turret: `the_whole_turret_follows_the_barrel`** — "the hull was
  swept into the turret: fittings: 4 turret prims (ring 0.00,0.10,0.23), 2
  tread prims". The ring test is admitting hull prims. `read`.
- **C12-1c — driveline, two failures.** `the_reference_car_is_left_alone`
  ("a hatchback weighs 2.574 t") and `wing_loading_is_actual_aerodynamics`
  ("the big wing stalls faster" — both configs report an identical
  `stall_ms` of 57.400238, so wing area is not reaching the stall term).
  The identical number across two very different masses is the lead.
- **C12-1d — the ship-interior trio.** `hullform::the_hull_contains_every_room_it_was_fitted_to`
  ("len 40: room bridge is outside the hull it was fitted to"),
  `interior::the_hull_between_rooms_is_still_solid` ("the ship is mostly
  corridor: 1484 open, 244 solid"), `aboard::the_camera_probe_finds_open_volume`
  + `aboard::the_hull_is_solid_where_the_walls_are` ("the hull corner is
  walkable — the ship is a shell with no walls"). These four are one story:
  the exterior fitted in `6a29d2708` does not enclose what it was fitted to.
  Highest-value of the group — it is the feature the commit series is named
  for.
- **C12-1e — `shots::a_locked_blast_catches_a_crossing_mark`** ("the missile
  spent itself without arriving") and
  **`model::a_flung_panel_no_longer_sets_the_ride_height`** ("debris still
  owns the ride height: 18"). The second is the same bug family as
  `2cdd88a14` (bounds measured over debris, not over the body) — that commit
  fixed it for bounds and orientation; the ride height still reads debris.

Run them with `CARGO_TARGET_DIR=/tmp/bd-bs maintenance/tools/cargo-ailite test
--no-default-features` from `games/blackstar-rs`. Default features
(audio/analog) now BUILD here too — `libasound2-dev` is apt-installable
(§3, corrected this session) — but were not run.

## 2. voiceprint-kit duplicates voiceterm, with nothing keeping them in step

The c9-006g merge brought `shared/voiceprint-kit`, whose `fingerprint` and
`compute_mfcc` are byte-identical to `apps/voiceterm/src/acoustic_index.rs`
and `phonetic.rs` apart from `pub(crate)` → `pub`. Verified by diffing the
function bodies, not by reading the commit subject.

That identity is the whole point — studio compares against the same
fingerprint voiceterm computes — and nothing enforces it. If either side
drifts, cross-app comparison silently stops meaning anything, and the failure
looks like "the match rate got worse" rather than an error.

- **C12-2a** — the real fix is voiceterm depending on the kit, which touches
  the AI01→AI02 file format and `N_MFCC`'s visibility; not a merge-integration
  change. Still open.
- **C12-2b — closed.** `maintenance/tools/voiceprint_parity_check.py`
  compares normalised `fingerprint`/`compute_mfcc` bodies across the two
  files and fails on drift OR on either function going missing (a rename
  is not a pass). Break-tested three ways on a scratch copy: a real logic
  change (`N_MFCC` → `N_MFCC + 1` in the DCT call) is caught as DRIFTED; a
  renamed function is caught as MISSING rather than silently skipped; the
  known structural difference (voiceterm's `fingerprint` calls
  `crate::phonetic::`-qualified paths that voiceprint-kit's unqualified
  version does not need) is correctly normalised away rather than reported
  as drift. Currently PASSES on this tree.

## 3. Inherited lore that was wrong, both corrected in place

- **C11-2a withdrawn** — there was never a stale lock; `lock_check.py` with a
  relative root could not recognise workspace members. Fixed in `dd56a20a0`.
  **Consequence: C10-1a's feature-sweep FAIL on `unendlich-engine` has lost
  its explanation and is undiagnosed again.** Do not start from the lock.
- **CLAUDE.md §3's missing-deps list** — `libasound2-dev`, `libopus-dev`,
  `cmake` and `mesa-vulkan-drivers` all install from apt here (`f31e894a5`).
  A fresh sandbox has NO Vulkan ICD at all, which C11_TODO §21 assumes it
  has; install `mesa-vulkan-drivers` before reaching for lavapipe.

## 4. a11y/Semantics adoption is the widget metadata, not yet the apps

`943cbec7` gave `uikit-egui::a11y` a stable `command_id` field, a bridge to
`glove::Target` (`Item::as_glove_target`, `A11yScene::glove_targets`), and
`semantic_text_edit` — the shared mechanism a person asked for "hinting...
across all apps" for voice, AI integrations, smart tabbing, palette
filtering, eye-tracking, cold-weather mode. That commit is the foundation
only. Repo-wide, exactly two apps register anything with it at all
(`gitmind`, `bo-suite`, both via the builder functions, neither touched by
this change). Everything else — every other uikit-egui app's buttons and
input fields — still renders with plain `ui.button`/`ui.text_edit_singleline`
and is invisible to voice, glove mode, or any future consumer of this
registry.

- **C12-4a** — sweep apps for their button/text-field call sites one at a
  time and switch them to `semantic_button`/`semantic_toggle`/
  `semantic_text_edit`, or hand-call `A11yScene::add` where a widget's
  shape does not fit the automatic wrappers (a custom-painted button,
  for instance). Mechanical but real: this is dozens to hundreds of call
  sites per app, and each needs a stable `id` (the wrappers take one, they
  do not invent one), which is the part that cannot be done blind — a
  hash of position or draw order is not stable across a resize or a
  reorder, and `A11yScene::focus persists across frames BY ID` depends on
  it being real. `apps/bot-arena` is IN PROGRESS: the setup form's five
  text fields are converted (stable ids via `egui::Id::new("bot_arena.
  setup.*")`, not position-derived), uncommitted, and not yet built —
  last known state before the tool budget ran out. Start/Stop (two
  locations, both need `add_enabled`/`on_hover_text` so they need a
  hand-called `A11yScene::add` rather than the automatic wrapper) and
  four loop-based `text_edit_singleline` sites are untouched.
- **C12-4b** — no app currently calls `A11yScene::glove_targets()` from its
  glove-mode wiring even where both a11y AND glove are already adopted
  (checked: neither `gitmind` nor `bo-suite` uses `glove.rs` at all, so
  there is no existing double-registration to consolidate yet — the
  bridge is ready for the first app that adopts both, not fixing a
  present duplication).
- **C12-4c** — `command_id` has no reader yet. No command palette, no AI
  integration, nothing walks `A11yScene`'s items looking for one. Adding
  it was the cheap half; something has to actually want it before it is
  more than a documented seam.

## 5. blackstar-rs: 13 failures, ship-interior first

Restated from §1, since it's the active thread: **1200 passed / 13 failed**,
all attributed there with the evidence for calling them arrival state rather
than merge damage. The ship-interior trio (`hullform`, `interior`, `aboard`
x2 — four failures total) is the pick for next work: it's the feature the
c5blackstar-008 series is named for (the exterior fitted to the rooms inside
it), and the failure messages ("room bridge is outside the hull it was
fitted to", "the ship is mostly corridor: 1484 open, 244 solid", "the hull
corner is walkable — the ship is a shell with no walls") all point at one
root — the fitted exterior does not enclose what it was fitted to — rather
than four separate bugs.

- **C12-5a** — `hullform::the_hull_contains_every_room_it_was_fitted_to`,
  `interior::the_hull_between_rooms_is_still_solid`,
  `aboard::the_camera_probe_finds_open_volume`,
  `aboard::the_hull_is_solid_where_the_walls_are`. Not yet read past the
  failure messages captured in §1.
- **C12-5b — why is there both `games/blackstar` and `games/blackstar-rs`?**
  Checked: they are NOT the same project in two languages. `games/blackstar`
  is "Cybertron: Arena of Gods," a transforming-robot arena brawler on a
  vendored C engine (`exengine`, CMake, PhysFS/MojoAL) — its own README,
  own build, own tests, no mention of blackstar-rs anywhere in it.
  `games/blackstar-rs` grew out of `skydribble` (a Rust wgpu/winit
  Rocket-League-style sandbox) into an unrelated Transformers open-world
  game — its own README says so directly and doesn't mention `blackstar`
  or `exengine` either. The shared "blackstar" name is a coincidence (both
  are Transformers-themed), not a port relationship; `-rs` here does not
  mean "the Rust version of the other directory." Worth a one-line note in
  each README saying so explicitly, since the naming collision is exactly
  the kind of thing a future session (or the c5blackstar-NNN bundle
  naming scheme itself) could misread as a merge target.

## 6. Video library: the audio sidebar becomes the video one too

Studio's clip library sidebar (`library.rs`) — the one that lets you add an
audio file to the drum rack — is the intended UI for adding a video clip to
the timeline and inserting it under the video track too, not a separate
video-library screen. Recorded here rather than under §"video library
management, not started" from the earlier snapclone/studio session, because
the shape changed: this is "extend one sidebar to a second media kind and
a second destination (drum rack vs. timeline+video track)," not "build a
new library UI." `studio-video`/`studio-collab` already exist as the
decode/asset side; nothing wires them into `library.rs`'s add-flow yet.

- **C12-6a** — not started. Needs reading `library.rs`'s current add-to-
  rack flow before touching it, so the video path reuses the same asset
  registration (`register_asset`, the pattern `morph_pad`/`voice.rs`
  already use) instead of inventing a second one.

## 7. aperture / aperture-view: a semantic-navigation and marker system

Design ask, not started. Two related asks:

- **`apps/aperture` (aperture, the tool):**
  - Create a new struct with boilerplate fill-in — a codegen/template
    command, presumably keyed on struct name + field list, that writes a
    real `.rs` file or inserts at a cursor.
  - Go to next/previous CALLED struct via a marker, distinct from a plain
    line-based "go back" — this wants a call-graph or reference index, not
    a text-search jump.
  - Markers settable on STRUCTS and FUNCTION NAMES, not source lines — a
    marker needs to survive the function moving in the file, so it has to
    be keyed on the symbol (name + kind), not a line number.
- **`apps/aperture-view` (the spatial UI):** an extension of the same
  system that abstracts files away entirely and shows data relationships
  in physical (2D/3D?) space — symbols and their call/reference edges laid
  out as a graph a person navigates visually instead of by file tree.

Not investigated yet: what `apps/aperture` currently is (name suggests it
already exists — needs a read before designing on top of it rather than
next to it), whether an AST/symbol-index crate already exists in `shared/`
that this should build on (`shared/` has had duplication problems this
session already — check before writing a new one), and whether
`nifty-core`'s graph/layout code (`graph_layout.rs`, force-directed
positioning) is the right foundation for aperture-view's spatial layout
rather than a fourth graph-rendering system.

## 8. Fleet-wide bug and performance hunt

Broad ask, not started, six named angles:

- FPS / frame-time regressions across the app and game fleet.
- Backend logic bugs that cost performance (not just visible bugs).
- Documentation reorganization where subsystems have moved or merged and
  the docs haven't caught up (this session alone found several: C11-2a's
  lock finding was withdrawn, the §3 apt list was stale, blackstar-rs's
  own README admits its title and package name are copied from skydribble
  and never updated).
- Searching for new shared kits — candidates for extraction the way
  `device-kit`'s `source_plan` module was pulled out of `camera-bridge`
  this session, or duplication the way `voiceprint-kit` shadows
  voiceterm's own fingerprint code.
- Bugs in existing kits, found via their own test suites — an audit pass,
  not a rewrite.
- Bugs in tooling/maintenance scripts themselves (this session found and
  fixed one: `lock_check.py`'s relative-root bug produced a false
  positive that stood unquestioned as C11-2a for a whole session).
- Poor Linux-distro integration (packaging, desktop files, XDG paths —
  `system/snapclone`'s `com.snapclone.SnapClone.desktop` naming fix from
  an earlier session is the shape of bug this names).

This is fleet-wide in scope and needs breaking into per-app or per-kit
passes before it's actionable — recorded here as the standing direction
rather than a single task, so a session picking a narrower slice
(one app's FPS, one kit's test gaps) can cite this entry rather than
re-deriving the mandate.



C11's §21 perf hunt (the submit-cause counting around `generate_kv_stream`
that this session was oriented to do), the whole hive/queen thread (§19–20),
and C11-2b's seven orphaned .rs files.

## 9. voiceterm-egui: feature parity by direct comparison, not the audit tool

The parity-audit tool's "87% code coverage" for voiceterm is close to
meaningless: it counts one surface per `.qml` file, and voiceterm-qt puts
almost everything — IRC, mail, browser, settings, manpages, the graph view
— as inline `Item`/`Component` blocks inside one ~30,000-line `main.qml`.
Eight `.qml` files total. The tool cannot see inside the monolith, so its
number is not evidence either way. Compared directly instead, file by file:

| Area | egui-frontend status |
|---|---|
| Graph view | `graph_view.rs` + `connections_graph.rs` + `layered_graph.rs` exist (323/317/384 lines) — real modules, not stubs. Feature-for-feature depth vs Qt's graph not yet checked. |
| Email | `mail.rs` + `mail_view.rs` + `mail_selector.rs` exist (337/453/273 lines) — same, real modules, depth unchecked. |
| IRC | **Nothing.** Qt has irc-search logic reachable from main.qml; egui-frontend has zero irc-anything. Not partial — absent. |
| Mastodon | **Not a port gap.** Doesn't exist in voiceterm-qt either — `C9_TODO.md` names Tokodon (a separate KDE app) as reading material for an idea never built on either side. Don't spend effort "porting" something with no source. |
| Niftycharts | Qt has niftyplot-equivalent rendering wired to `nifty-core` (extended this session with the hardware-diagram parser, `fd59fd3c`). egui-frontend has no nifty view file at all, despite already depending on `nifty-core` elsewhere (voiceterm's own `link_graph.rs`). Closest thing to a pure "the pieces exist, nobody wired the last one" gap. |
| Web browsing | `browser.rs` exists (362 lines) — real module, depth unchecked. |
| Settings | **Nothing.** No settings module, no settings file, anywhere in egui-frontend. Qt's Settings popup is large (model setup, wake-word config, alignment recalibration, notification toggles, per-section state) and none of it has a home on the egui side. |
| Manpages | `manpage_view.rs` exists but documents its own real limitation in its header comment: it renders documents handed to it rather than owning the registry, because that logic lives inside `manpages.rs` (4,117 lines) tangled with Qt-only code, and splitting it needs a Qt-buildable host to verify against — which this sandbox has never had. An honest, named blocker, not neglect. |

- **C12-9a** — IRC and Settings are the two genuine zero-to-something gaps.
  Niftycharts is close behind (the hard part, `nifty-core`, already exists —
  this is a wiring job, not a build-from-scratch). Recommended order:
  Settings, then niftycharts, then IRC, then a real depth-check on
  graph/mail/browser before touching them (their line counts say "real
  module," not "feature-complete").

## 10. logscope gpu-gemm: an architecture the wgpu engine correctly refuses

Not a bug — the engine's refusal is working as designed, recorded here so a
future session doesn't "fix" the refusal instead of the actual gap:

    loader: .block_count is absent, but it IS present as ["muse-glimmer"] —
      the hparam prefix is not the architecture name.
    worker: load failed: architecture 'muse-glimmer' isn't supported by the
      wgpu engine yet — use the llama.cpp backend. Also UNREAD tensors
      (attn_gate.weight, 27262976 weights) — nothing in this engine consumes
      it. A tensor the forward pass ignores does not error; it produces
      plausible wrong output.

The tensor dump (731 tensors, `attn_gate`/`attn_q_norm`/`attn_k_norm`/
`post_attention_norm`/`post_ffw_norm` alongside the usual attn/ffn set) is
a QK-norm architecture with a gating tensor the current engine's forward
pass has no path for — a Gemma-2/3-shaped or Cohere-shaped variant, not one
of the architectures already wired. `LOGSCOPE_TRY_UNSUPPORTED_ARCH=1` exists
specifically so someone CAN force it and get wrong-but-plausible output on
purpose; that is the documented expected failure, not a surprise to debug.

- **C12-10a** — add `muse-glimmer` (or whatever GGUF actually names this
  architecture family once the real name is confirmed — `hparam` prefixes
  are not architecture names, per the loader's own log line) to the wgpu
  engine: needs `attn_gate`, `attn_q_norm`/`attn_k_norm` (QK-norm), and
  `post_attention_norm`/`post_ffw_norm` (post-norm placement, not the
  usual pre-norm) wired into the forward pass. Per the refusal message
  itself: an architecture becomes supported only once it has completed a
  forward pass on a real checkpoint and produced coherent output — that is
  the acceptance test, not "it loads without erroring."
- **C12-10b** — separately, `.block_count` being absent under the expected
  hparam key but present under `["muse-glimmer"]` suggests the GGUF
  metadata key convention for this model family differs from what the
  loader's hparam-prefix guesser expects. Worth checking whether that
  guesser needs a fallback more general than "read the tensor names"
  (which is what it fell back to here and which worked) for OTHER
  metadata fields beyond block count.

## 11. bo-suite-egui: split into the apps it already claims to be

`README.md` already describes `bo-mux` (headless soundscape engine +
multi-speaker mux, meant to survive GUI restarts, systemd-managed — see
`DECISIONS.md`'s rejection of running it in-process inside the GUI) as
architecturally separate from `bo-controller` (the GUI). The BACKEND is
already split. The FRONTEND is not: `apps/bo-suite/egui-frontend` has
exactly one binary target (`src/bin/bo_suite_egui.rs`) and one crate
holding soundscape UI (`soundscape.rs`, 307 lines), music-player UI
(`library.rs`/`queue.rs`/`transport.rs`/`all_tracks.rs`), device/ANC
control (`anc.rs`, 506 lines — the whole-home noise-cancelling piece),
and settings (`settings_page.rs`) all in one crate, all launched by one
binary. Splitting the GUI to mirror the backend's own architecture is the
ask, not building new backend separation.

- **C12-11a** — not started. Candidate split: a soundscapes app (wraps
  `soundscape.rs` + `spaces.rs`, talks to `bo-mux`), a music player (
  `library.rs`/`queue.rs`/`transport.rs`/`all_tracks.rs`/`browse.rs`/
  `catalog_source.rs`/`tags.rs`), and a control center (`anc.rs`/
  `device.rs`/`outputs.rs`/`settings_page.rs` — the whole-home ANC piece
  named in the ask). `chrome.rs`/`shell_bits.rs`/`dial.rs`/
  `dial_watermark.rs`/`scrollfeel.rs`/`product_silhouette.rs`/
  `safety.rs`/`dialogs.rs`/`channel.rs`/`anticipation.rs`/`fileproc.rs`
  not yet sorted into a bucket — need reading before assuming which app
  each belongs to, or whether some (chrome, shell_bits) are genuinely
  shared shell code all three should keep depending on rather than
  duplicate.
- **C12-11b — media theater integration.** Not started, and no existing
  file found to build on (`grep -rl "theater"` across bo-suite and
  studio came back empty). Needs a design pass: is this bo-suite driving
  a media-theater DEVICE/room profile (an ANC/spaces preset, closer to
  `spaces.rs`), or bo-suite integrating WITH `apps/studio`'s video
  pipeline for a home-theater playback surface? The ask doesn't say
  which, and the two are different enough in scope that guessing wrong
  wastes the work.

## 12. bsgames: launch already works one way; the reverse and the rest don't exist yet

`games/bsgames` already does more than a fresh read would guess. Its
README states the design goal directly ("launch the fleet's games, see
friends' presence over the netcode DHT, and chat via snapclone"), and
`wiring.rs` backs it with real tests: `host_then_join_over_the_fleet_dht`
publishes a session over snapcore's real Kademlia DHT (with a fake-DHT
test double for when OpenDHT itself is down) and a deterministic session
token so re-hosting reuses the same DHT key. `games/shared/netcode::
handoff` — adopted verbatim from `soldier`'s original, now the one
canonical copy every fleet game (`clawbound`, `soldier`, `blackstar-rs`,
`skydribble`, `unendlich`, `echelon`) reads at startup — is the
BSGAMES_HANDOFF_* env-var protocol: bsgames launches a game with
`BSGAMES_HANDOFF_ROLE=host|join`, `_ME`, `_PEER` already set, so the game
presents the SAME identity bsgames published rather than minting a second
one, and joins the peer directly, skipping that game's own menus.

What's genuinely missing, checked directly rather than assumed:

- **C12-12a — no reverse direction.** bsgames -> game (launch with a
  handoff) exists and is tested. Game -> bsgames (summon the centre from
  inside a running game, to invite a friend or check who's online
  without quitting) has no mechanism anywhere — grepped for any
  in-game invocation of bsgames across every game directory, found
  nothing. This is the literal "open bsgames from any game" half of the
  ask, and it's the harder half: it needs either an overlay bsgames can
  render inside another process's window, or a way for the running game
  to spawn/focus a separate bsgames instance and hand control back
  cleanly, which is a real design decision, not a wiring job.
- **C12-12b — friends list.** `identity.rs` (118 lines) has the
  player's own identity; not yet checked whether it or anything else
  persists a FRIENDS list (as opposed to ad-hoc peers discovered per
  session) — needs reading before claiming this is missing or present.
- **C12-12c — voicechat through snapclone pipelines, with in-game
  reverb.** No trace of this anywhere: grepped bsgames, netcode, and
  snapclone for "voicechat"/"reverb" and found nothing. snapclone's own
  voice-call pipeline (`voice_bridge.rs`, `group_voice.rs` per the
  earlier snapclone session) is the thing to reuse rather than a fourth
  voice stack, per this session's own recurring lesson about duplicated
  kits (voiceprint-kit, glove vs a11y) — but reuse here means bsgames
  depending on snapclone's voice crates directly, or a smaller shared
  voice-pipeline crate extracted from snapclone the way `source_plan`
  was pulled from camera-bridge into device-kit. Not designed yet.
  "In-game reverb" specifically — sending a peer's voice through a
  spatial/room effect tied to the game's own environment rather than
  playing it dry — has no existing DSP hook identified in either
  snapclone or any game's audio path.

## 13. studio: the running tally against the original ask

Restated so it isn't scattered across several earlier sessions' turns.
Done: clip delete via `d` (already existed, `Action::Delete` was already
wired), 32k/64k opus fidelity rungs (`914efea3`), the tune-key selector
defaulting to no scale (`a7da20e4`), the mode HUD (`f054a1ef`). Still
open, unchanged since last recorded: keybind-customization settings
(needs a Settings-popup section — voiceterm-qt turned out NOT to have a
customizable-keybind UI to copy, only a toggle-row settings popup shape,
so this is new UI, not a port), the `Settings…`/`Archive…`/`Export…`
buttons plus the scale switcher unified into one hamburger menu, video
library management (reframed in §6 above — the audio-add sidebar gains a
video path, not a new screen), and native av1/fao-codec decode wiring
with proxy pushback for other formats (the biggest of the four, spans
`studio-video`, `apps/fao-codec`, `apps/av1tuner`, not started).

