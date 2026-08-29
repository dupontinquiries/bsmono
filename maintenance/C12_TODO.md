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

- **C12-1a — two of three closed (`d0ef5a07`).** The recorded guess was
  that `compact` being a no-op was the root and the others followed. It
  was the reverse: `compact` was correct and had nothing to work on,
  because limb DETECTION was wrong upstream. Two bugs, both required:
  (1) the core test measured distance from the bounding-box centre,
  which a 90-point tail fin drags off the 500-point fuselage, so no prim
  qualified as core; `mid` is now the centroid of all points. (2)
  `orphanish` adopted away anything thinner than FLAT — but a wing, fin,
  blade and rotor are all thin ON PURPOSE. Share cannot separate a wing
  (0.214) from an underside plate (0.194), and neither can flatness or
  elongation; DIRECTION can — the offset's component along the group's
  own longest axis is 1.00 for a wing reaching outward and 0.00 for a
  plate lying against the body. Note for future work: fixing (1) alone
  changed nothing at all and looked like a wrong guess; it is
  load-bearing only in combination, confirmed by reverting it at the
  end. `vehicle_mode_packs_the_limbs_inside_the_hull` — CLOSED
  (`a5d73585`), and it did NOT share a cause, which is why not assuming
  one mattered. `contain`'s cap on limb travel was
  `(env * 2 - span).max(0)`, which is zero for any limb wider than the
  envelope — so the widest limbs, exactly the ones containment exists
  for, were the only ones it could never move. Cap is now the shift that
  CENTRES the limb. The test additionally demanded < 0.35 overhang,
  which no translation can achieve for a 5.2-wide wing in a 1.25-wide
  envelope (minimum 1.975); it now asserts what translation CAN
  promise — centred, and no worse than the limb's own excess width —
  with the translation-only limit written into `contain`'s doc comment.
  Rotation is what would actually fit a wing inside a silhouette, and
  that is a different operation on a different data model.
  **C12-1a is fully closed.**
- **C12-1b — CLOSED (`4b1d7abf`).** The seed pass was right; the GROW
  pass admitted the hull through two loose guards, and the barrel broke
  both. `high_enough` compared the part's TOP against the turret base,
  which a 0.05 sliver of hull roof passes — now compares its centre.
  `big` compared longest-axis to longest-axis, and a barrel is long by
  design: it stretched the seed to 3.40, making the limit 6.80, under
  which a 4.80 hull is "not big" — now compares against the turret's x/z
  FOOTPRINT, since length is the barrel's business and letting it set
  the scale is what made a tank hull look like a cupola.
- **C12-1c — CLOSED (`e7611ad8`), two separate bugs.** The recorded lead
  ("the identical number") was the right thread and the diagnosis it
  suggested was wrong: wing area WAS reaching the stall term. Mass came
  from the full bounding box and area from the planform, so mass/area
  reduced to `h * density` and the span cancelled itself out on both
  sides of the fraction — no wingspan could change the answer. Fixed by
  splitting the planform for aircraft (solid fuselage + wings at 12% box
  fill); ground vehicles keep the solid box. Separately, `D_CAR` was
  0.25 t/m³ while the comment beside it claimed that made a 1.5 t
  hatchback; it makes 2.57 t on the reference hull's own 10.30 m³.
  Corrected to 0.145 with the arithmetic written into the comment. Safe
  because ground handling derives from a LENGTH ratio, not absolute
  mass — checked, not assumed.
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

- **C12-5a — CLOSED, four of four, and the "one root cause" framing was
  wrong.** These were grouped as one bug wearing four names. They were
  two real bugs covering three tests, plus one bad test:
  - `hullform::the_hull_contains_every_room_it_was_fitted_to` — CLOSED
    (`0b161972`). `fit` sized the hull's axes independently, which is
    right for a rectangle and wrong for the superellipse the hull
    actually is: at N=3 a room filling both half-extents sits ~3.5%
    outside a skin that exactly meets each axis. Fixed by scaling both
    axes by the room's own corner radius, per-room rather than the
    worst-case constant.
  - `interior::the_hull_between_rooms_is_still_solid` and
    `aboard::the_camera_probe_finds_open_volume` — CLOSED (`e44188c7`),
    one cause. `walkable` gated on the BOUNDING BOX, so the whole
    volume between the tapered hull and its box counted as open: on the
    480m ship the test builds, 1112 of 1191 "walkable" samples were
    outside the skin against 79 genuinely in a room. Never "mostly
    corridor" — mostly outside.
  - `aboard::the_hull_is_solid_where_the_walls_are` — CLOSED
    (`217dbe4e`), and it was a bad TEST rather than bad code, which is
    why not assuming a shared cause mattered: it sampled `hull_half *
    0.95`, a corner of the bounding BOX, measured 9.88 units OUTSIDE
    the skin on the ship it builds. `sdf_local` is positive there by
    construction, so the test asked whether a point not in the ship was
    solid and read the honest answer as a failure. Now samples the
    origin (inside the skin at -6.75, between rooms, `sdf_local`
    -1.04), with a guard that the sample really is inside the skin so
    it cannot degrade into checking nothing.
  Method note, because it is what actually worked twice after reasoning
  failed twice: add a temporary test that PRINTS the geometry (which
  corner, how many samples where), read the numbers, then delete it.
  Both fixes fell out in one run each that way.
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

## 14. studio: multiple songs (an album + its music videos) in one project

`39c23c40` did the first slice — the timeline's zoom/scroll is per-song now
(`Studio::song_views: BTreeMap<GroupId, View>` keyed by `active_song`), so
that half of "the project scale has to become per-group aware" is real and
tested. `GroupId`/`Group` (studio-core) already exist as "a named set of
clips the timeline folds to one block" — the right existing primitive to
build a song on, not a new concept.

What's still needed, roughly in dependency order:

- **C12-14a — closed (`38b83972`).** `Op::SetGroupKey { id: GroupId, key:
  Option<Key> }`, separate from `SetGroup`, exactly as designed below.
  `SetGroup`'s own apply now preserves an existing key across a rename
  (it didn't before this commit — would have silently nulled it), and
  `RemoveGroup`'s undo bundles the key back in via `Op::Group` since by
  the time that inverse runs the group is already gone and has nothing
  to read a key from. Wire protocol (tag 30), command classification,
  and the file-based project reconstruction path all updated too — each
  would have silently dropped a song's key on its own path otherwise.
  Tested at every layer (apply/invert, wire round-trip covering every
  `Scale` variant, full-suite regression). Not yet wired to any UI —
  needs 14d's song switcher first, so there's something to attach a key
  picker to.
- **C12-14b — closed for tempo (`52e7b093`), still open for markers.**
  `Op::SetGroupTempo { id: GroupId, tempo: Option<TempoMap> }`, same
  shape as 14a's key, including the same `SetGroup`-preserves-it-across-
  rename and `RemoveGroup`-undo-bundles-it-back-in fixes, generalized
  to handle key and tempo together. `TempoMap` itself: piecewise-
  constant BPM, `tick_to_beat`/`beat_to_tick` as a read-only lens over
  ticks a clip never gets stretched to match — verified with a
  round-trip sweep across a tempo change, not just at cherry-picked
  points. Whether MARKERS should be per-song or shared across the whole
  project remains an open design question, unchanged from when this
  item was first written.
- **C12-14d — closed (`07f9b8fc`).** Landed in the command palette
  ("switch song" / "set song key"), per direct instruction to put it
  there until it has an obvious permanent home rather than invent one.
  Tested: resolves to the right group and to "whole project" with and
  without existing groups; the key path is checked to land on
  `SetGroupKey`, never the project-wide `SetKey`.
- **C12-14c — the music-video link.** No existing field anywhere
  associates a `Group`/song with a video clip or a `studio-video` asset.
  Needs the video pipeline this session already tracked as unstarted
  (§13, av1/fao-codec wiring) to exist in some form first, or at minimum
  a placeholder association (a `GroupId` -> `ClipId` map for "this
  song's video") that doesn't depend on decode actually working yet.
- **C12-14f — the four pieces still genuinely untouched, named
  explicitly so "the tempo work" doesn't read as more done than it is:**
  - **Grid rendering.** Non-uniform tick markers on screen, following
    `TempoMap`'s detents instead of a fixed spacing — "something unique
    in the code," per the original framing, and a separate
    rendering-layer task now that the model underneath it is trustworthy
    (tested), not before.
  - **Clip-stretching/alignment DSP.** Explicitly still hard, explicitly
    NOT "stretch every clip to the workspace tempo" — that's the
    antipattern `TempoMap` was designed around, not toward.
  - **Export-popup wizard grouping.** By-section vs. chapters vs. a
    single export, as a distinct grouping/wizard on the export popup —
    ties into C12-14e below (what "export the album" means) but is its
    own UI work regardless of how that question resolves.
  - **Palette wiring for tempo.** "Set song tempo" doesn't exist yet in
    `palette.rs` — should mirror "set song key"'s three-step shape
    (`Ask::SongForKey` → root → scale becomes something equivalent for
    a BPM value or a tap-tempo entry) now that `SetGroupTempo` is
    verified, the same order C12-14a/14d already followed.
- **C12-14e — what "the whole project" means once songs exist.** Export
  (`bounce`), the mixer, and anything that currently assumes one
  timeline for the whole project need a decision: does exporting "the
  album" concatenate every song's timeline, or is each song exported
  independently and the album is an organizational container only?
  Affects `studio-app`'s export flow, not yet looked at.

## 15. responsive_nav: two apps done, and a bug found along the way

`3619091f`/`e77ba72a`: `uikit_egui::responsive_nav` (`NavLayout::{Tabs,
Pages}`, decided from a real width fit rather than a device-class guess)
is built and adopted in `ambient-learner-egui` (tab bar -> compact picker)
and `bot-arena` (the hand-rolled `is_compact()` check it was written to
replace). Both tested, both green.

- **C12-15a — checked, and neither is actually a fit.** `segmented_tabs`'
  own module doc names snapclone and bo-suite as the other two hand-
  rollers it was hoisted from, which read as "apply responsive_nav
  there next." Read both before doing that, and neither has the problem
  `responsive_nav` solves:
  - **snapclone** shows a bottom `tab_bar` on compact and the top
    `strip` on desktop — BOTH always show every page; the choice is
    ergonomic placement (thumb reach vs. pointer reach), not a fit
    collapse. Forcing `responsive_nav`'s Tabs/Pages binary on top of
    this would remove a page from view on the "Pages" side for no
    reason, since there was never a fit problem to solve.
  - **bo-suite**'s `PageBar`/`page_bar_ui` already has its own answer
    to "too many tabs to fit": `scrolls()` detects overflow and
    `scroll_to_active()` keeps the current tab visible in a
    horizontally-scrolling bar, rather than collapsing to one page at a
    time. A working, different, equally legitimate strategy — not a gap.
  - **Correction to a claim in the commit that introduced this
    section:** I said snapclone's `nav_chrome::TabStrip` was a
    SEPARATE, undedup'd copy of `uikit_egui::segmented_tabs::TabStrip`.
    That was wrong — checked more carefully and `nav_chrome.rs` line 52
    is `pub use uikit_egui::segmented_tabs::{strip_ui, Tab, TabStrip};`,
    a plain re-export, with a comment right above it saying exactly
    that ("The strip moved to the kit... snapclone keeps the peel
    hint"). No duplication, no dedup needed. Recording the correction
    here rather than quietly editing it away, since the wrong claim is
    still sitting in `3b1a7644`'s commit message.
  `responsive_nav` stays a good fit for an app with NO existing overflow
  strategy — which is what ambient-learner and bot-arena both were
  before this session's changes — not a blanket replacement for every
  app's tab row.
- **C12-15b — closed (`b507439b`).** Not a stale example after all: the
  method is real. The doc comment quoted the compiler error `LiveState::
  ui`'s past absence used to produce, as a bare 4-space-indented block —
  rustdoc's implicit "runnable Rust example" syntax — so it tried to
  COMPILE that error message as code. Fenced as ` ```text ` instead.
  `cargo test --doc` now runs 0 doctests (correctly, the false one is
  gone) with 0 failures.

## 16. Energy-aware adaptive dashboarding — the design, and where it stops

Direct instruction: smart adaptive layout (`responsive_nav` and anything
like it) must respect performance and energy constraints — cull what isn't
shown before reaching for a manual tuning knob — and must not create bugs
or stall networking for apps with a collaboration element (`logscope`,
`studio-live`, anything syncing a Doc/op-log across peers).

**Done (`7d8a8ea4`):** `responsive_nav` gained `NavLayoutTracker`
(change-only notification, so a resize drag doesn't retrigger transition
work every frame it happens to straddle the fit boundary) and an explicit
module-doc section stating the two rules directly rather than leaving them
implicit: this module decides chrome shape only, never gates an inactive
page's real work (that is the caller's `match self.view_idx` to skip or
not), and visibility must never gate op-log replay, presence, or outbound
edits — `choose`/`choose_adaptive` take only a width and a count by design,
with no parameter a future caller could wire to network state by accident.

**The cull-vs-tune order, as asked ("your call"):** for a page that IS
hidden and NOT collaborative, the automatic-first ladder is (1) skip the
frame's expensive recompute entirely if the page has no visible surface
and no other reason to run (a physics step, a filter graph, a decode with
nobody watching), (2) if the app needs SOME liveness even when hidden
(a level meter that should read correctly the instant you switch to it,
not a beat late), lower the tick rate rather than stopping it outright,
(3) only reach for a manual per-app tuning knob (an app-specific "how much
to do when hidden" setting) when neither of the first two is a clean fit —
which should be rare, because most dashboards are exactly the "nobody's
looking, do nothing" case. This ladder is not built as reusable kit code
yet — recorded as the decision, not the deliverable, since the actual
skip/throttle logic lives in each app's own update loop and there was no
single obvious primitive to hoist without a second real consumer to check
it against (the same reason a kit function should follow, not precede, a
concrete need — see `device-kit::source_plan`'s own history this session).

- **C12-16a** — apply the ladder above to a real dashboard app once one
  is identified with an actual hidden-page cost worth culling (candidates:
  `bo-suite`'s ANC/soundscape panels, `ambient-learner`'s sensor pages —
  neither audited yet for what their inactive pages actually compute per
  frame).
- **C12-16b — logscope and studio-live specifically.** Neither has
  been audited for whether ANY existing code already makes the mistake
  this section warns against (gating sync/replay on tab visibility).
  Given `responsive_nav` has no foothold into either app yet (§15's
  C12-15a found neither app's tab problem is a `responsive_nav` fit), this
  audit is independent of that module — it's about their OWN update
  loops, not about adopting new nav chrome. Not started.

## 17. The to-merge-c12unified archive: what landed, and the one thing that can't

Five incoming items from a single archive (needs `zstd`, which the
sandbox lacks by default — `apt-get install zstd` first).

**Merged, all verified, failure sets diffed rather than counted:**
- `da8f07b0` logscope physical UI (1043/0/4 — matches baseline exactly).
  Every fix in it is a RENDERING fix and this sandbox has no display, so
  the tests prove compilation and logic, not that the dock is on screen.
- `77c03997` blackstar weapon+animation (1201 passed, same 13
  arrival-state failures; pass count 1200 -> 1201).
- `34907763` blackstar canon reconciliation. Also verified their claim
  that `archive_gen.h` was regenerated: all four new finding ids appear
  in BOTH archive.rs and the generated C header — worth checking because
  nothing in the Rust suite compiles that header.
- `d849a5d2` unendlich perf snapshot (a 2311-file TREE, not a bundle).
- `c75fd165` / `281a7579` / `cd1568a7` — aperture patches 3, 5, 4.

**Two docs in the archive were wrong; the code was right:**
- unendlich's APPLY.md puts the headline INDEX_CAPACITY fix in
  `mesher.rs` (it is in `render/gpu_meshing_octree.rs`) and calls itself
  v0.24.NEXT+9 (the tree is NEXT+11, capacity already raised again to
  262_144 — which we ALREADY had, so the headline change is not even in
  the diff). What the 39 changed files actually carry is other perf
  work: corner-cache generation invalidation replacing a memset-to-NaN
  that evicted L1 on ARM every mesh call, a p90 pacer window widened
  16 -> 32 samples, and an eviction budget defaulting to 16 sync saves
  per frame instead of unlimited (a teleport used to freeze for
  seconds). Read the code, not the doc.

- **C12-17a — unendlich's two remaining test failures are OURS, not the
  snapshot's.** `shader_artifacts::build_list_tracks_every_source_shader`
  and `core_shaders_ship_precompiled_spirv` both trip on `clouds.frag`:
  it is tracked at HEAD, absent from `build.rs` SHADERS[], and its
  `.spv` has no `.spv.hash` sibling. None of those files is in the
  snapshot's changed set, so the test's inputs are byte-identical at
  HEAD and after — it fails identically either way. (Proven by inputs
  rather than a clean-tree rerun, which exceeded the per-command budget
  twice.) Fixing it means registering the shader in the build, which
  belongs to whoever owns it. Not started.

- **C12-17b — the aperture bundle cannot be merged, and it is not a
  rename.** It forks from `dc33bb1e` ("aperture: TODO addendum —
  corridor narrator recorded; count 381"), which this line does not
  have. Checked properly rather than guessed: no commit with that
  subject or any of its distinctive phrases exists here; our aperture
  history is 15 commits and has no `NEXT_SESSION_TODO.md` at all; and
  indexing the raw packfile with `--fix-thin` against our ENTIRE object
  store still leaves 36 unresolved deltas. If it were a renamed commit,
  the blobs and trees would resolve. It is a genuinely divergent base.
  Patches 3/4/5 were applied by hand against our own anchors; patches
  1, 2 and 6 are NOT applicable — 1 needs `gitspace::last_visited`
  (zero occurrences here) and 2/6 edit the absent TODO file. Recovering
  those needs the prerequisite bundle from whatever line produced it,
  or a plain tarball.

- **C12-17c — the logscope provider's F2 "prompt preview" panel is
  omitted.** It borrows `state` for the live caret context while `cfg =
  &mut state.logscope_cfg` is already mutably borrowed — a real borrow
  conflict in our tree's shape, not a transcription slip. Needs the
  borrow split or the caret context snapshotted before taking cfg.
  `assemble_prompt` is public and pure, so nothing is blocked on it.

- **C12-17d — rustdoc's implicit code block bit two unrelated sessions
  in one day.** The incoming blackstar work fixed a `voyage.rs` doc
  comment whose indented math was being compiled as Rust; this line
  independently fixed the identical bug class in ambient-learner
  (`b507439b`). Two sessions tripping over the same rustdoc rule
  suggests a checker (scan doc comments for 4-space-indented blocks
  that are not fenced) rather than a third one-off fix.

## 18. aperture: two review questions from the field

Both arrived as `// KESS:` comments in a patch. One was a correct
criticism of code I wrote and is fixed; one is a design idea worth more
than a quick implementation.

**C12-18a — `LineMap::rebuild`, "why is this the architecture?" —
answered, and the answer was "it shouldn't be" (`0b161972`+).** It was
called from `after_edit`, i.e. every keystroke, and did a full O(lines)
rebuild that allocated twice over: a `Vec<String>` of every line at the
call site, plus a `HashMap<u64, String>` entry per distinct line so
`diff` could return vanished TEXT. Nothing downstream needed the text —
the cache invalidates by line identity and identity is the hash.
Now: `LineMap` is hashes only; `diff` returns `Vec<u64>`; entries
record their own line hashes once at insert so invalidation is a
binary-search intersection rather than `prefix.contains(text)` per
entry per vanished line (which was O(entries x gone x prefix_len) on
every edit); and the map is rebuilt in place through a swapped scratch
buffer so a steady edit loop stops allocating for the hash side
entirely. Same behaviour, 64/64 still green.

**C12-18b — the word-graph typing buffer. NOT started, and deliberately
not rushed.** The idea: build a graph from model output tokens and let
a person assemble words from it via home-row chords (`aoeu`/`htns`),
splitting by space and by syllable, seeded from surrounding lines and
compiler errors so the buffer is useful immediately and gets better as
AI tokens arrive. Ordering free-form, so you type the words you want in
the order you want them.

Why it is worth recording rather than half-building: this is not an
autocomplete variant, it is a different INPUT METHOD, and the hard part
is not the graph — it is the ordering/coherence tradeoff named in the
proposal (maximise words-per-keystroke without the result drifting into
incoherence). That needs a real corpus to tune against and a way to
measure keystrokes-saved, neither of which exists here yet. The pieces
that would feed it DO exist now: `prefixcache` already holds prior
completions, `scope::QuickContext` already extracts surrounding
identifiers, and `kvplan::order_for_kv` is the batching seam for
sampling several continuations cheaply.

Related and smaller, from the same comment: **"can we make this
threaded for smaller models?" — half done, and the premise needed
correcting.** I first wrote that `route_ai` was "single-flight per
key". That was imprecise: requests are idempotent per key, but
`pending` has always been a QUEUE holding many, so the queue was never
the obstacle. What is single-file is the CALLER, which drains one per
frame via `take_pending`. Added `take_pending_batch(n)` as the seam a
threaded or batching worker actually needs (newest-first, since a query
just typed outranks one already navigated past). Deliberately does NOT
order for KV reuse itself — these prompts share a preamble but differ
in their candidate listings, and the caller that knows which engine it
feeds should run `kvplan::order_for_kv` over the batch; baking it in
here would assume an engine this module cannot see. STILL TO DO: the
caller in `state.rs` that would use it.

**C12-18c — closed. `route_prompt` had no example in it.** Correct as
reported: it described the output format in prose and showed nothing,
and small models follow a shown format far more reliably. Added a
one-shot example, with the two properties that keep a demonstration
from becoming a trap: it uses a visibly different query and files that
cannot appear in the real candidate list (so a model copying it
verbatim is obvious rather than plausible), and the real question comes
last so the model finishes on it rather than on the demonstration.
Both pinned by a test.
