# C12_TODO — open threads from the c12 session

Same convention as `C9_TODO.md` / `C10_TODO.md` / `C11_TODO.md`: every item
says how far it actually got. None of those files are superseded; C11-2a was
edited in place this session because it turned out not to be a real finding.

The c12 session installed the offline 1.96.0 toolchain, merged two bundles
(c9-006g studio, c5blackstar-008 blackstar-rs), and corrected two pieces of
inherited sandbox lore. What follows is what it found and did not close.

---

## 1. blackstar-rs: CLOSED — 1215 passed, 0 failed

All thirteen arrival-state failures from the c5blackstar-008 import are
fixed; the suite is green for the first time since that merge.

They were **eleven distinct bugs plus two impossible assertions**, not
the four clusters recorded here originally. Grouping by symptom was
attempted three times and was wrong every time: "one root cause wearing
four names" was two bugs and a bad test; "compact is the root and the
others follow" was backwards — compact was correct and starved of
input. The method that worked, after reading code failed repeatedly,
was to add a temporary test that PRINTS the geometry, read the numbers,
delete it. Three bugs fell in one run each that way, and twice the
printed struct held a value (`legs: 2`, `4 turret prims`) that no
reasoning about the intended logic would have surfaced.

The eleven: a superellipse fitted axis-wise, so rooms escaped at the
diagonal; a walkability gate testing the bounding box instead of the
hull skin; a core test centred on the bbox, which one tail fin drags
off the fuselage; a flatness rule classifying wings as debris; a
containment cap evaluating to zero for exactly the oversized limbs it
existed for; two loose turret grow guards, both broken by the barrel
setting the scale; a debris filter counting prims where it meant
points, with an unweighted spread letting an outlier set its own
threshold; a leg guard comparing height to a caller-supplied parameter
rather than the footprint its comment named; a skirt read as thrusters,
because nozzle detection counts points in the aft fifth; a wingspan
that cancelled itself out of the stall formula by appearing on both
sides of the fraction; a density constant contradicting the comment
beside it (0.25 t/m³ described as "a 1.5 t hatchback", actually 2.57);
and a blast RADIUS used as a flight DISTANCE, killing every missile at
12 m.

The two impossible assertions: a wall test sampling a point 9.88 units
outside the ship, and a containment test demanding a 5.2-wide wing fit
a 1.25-wide envelope by translation alone. Both replaced with checks
asserting more than the originals did, not less.

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
  it being real. `apps/bot-arena` is DONE (`d6fa56bb`): all twelve
  widgets — five setup fields, Start/Stop, and the studio-host config
  including the shared `path_edit` helper — register with stable
  string-derived ids. Two notes for the next app: a shared widget
  HELPER must take the id from its caller, since an id derived inside
  it collides across every call site and `A11yScene` keeps focus by id
  (silent focus-jumping, easily misblamed on egui); and a control whose
  LABEL changes with state (Start/Stop) still wants one stable id —
  that split is what `Semantics` is for. `every_registered_widget_id_is
  _distinct` guards the collision case, which the compiler cannot.
  bot-arena is the first app fully legible to the registry. Every other
  uikit-egui app is still untouched.
- **C12-4b** — no app currently calls `A11yScene::glove_targets()` from its
  glove-mode wiring even where both a11y AND glove are already adopted
  (checked: neither `gitmind` nor `bo-suite` uses `glove.rs` at all, so
  there is no existing double-registration to consolidate yet — the
  bridge is ready for the first app that adopts both, not fixing a
  present duplication).
- **C12-4c — CLOSED (`7a62206a`).** `A11yScene::palette_matches` is the
  reader. It ranks the actionable widgets drawn THIS FRAME by
  `fuzzy::score`, matching on either the `command_id` or the spoken
  label and taking the better — the id is what someone who knows the
  app types, the label is what someone reading the screen types.
  Unnamed widgets are still offered by label (dropping them would make
  the palette quietly incomplete); headings and labels never are (an
  entry that does nothing when chosen is a bug). The property a static
  command table cannot have, and the one the tests pin hardest: a
  button stops being offerable the moment the app stops drawing it.
  4 tests. NOTE: no app has wired a palette to this yet — what changed
  is that adopting `command_id` now yields something instead of filling
  in a field nothing reads.

## 5. blackstar-rs: CLOSED (see §1)

This section tracked the ship-interior cluster while it was the active
thread. All of it, and the rest of the thirteen, is done — the details
and the corrected diagnoses live in §1. Kept as a pointer rather than
deleted so the C12-5a/5b identifiers referenced from commit messages
still resolve.

- **C12-5b** — the `games/blackstar` vs `games/blackstar-rs` question is
  answered there too: unrelated projects sharing a Transformers-themed
  name, not a port pair. Neither README mentions the other. A
  disambiguating line in each is still worth adding and is NOT done.

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

- **C12-6a — READ, and the gap is much narrower than this section
  assumed.** Nothing needs building from scratch; the video path is
  three-quarters wired already:
  - `library::Media` has a `Video` variant, `media_of` classifies by
    `VIDEO_EXTS`, and `Focus::rank` deliberately ranks video FIRST on
    the Timeline focus and merely deprioritises (never hides) it in the
    Rack — with a comment explaining why refusing a video's audio to a
    sampler would be the tool overruling the musician.
  - `LibraryAction::ToTimeline` already exists and the sidebar already
    emits it for a video entry.
  - `studio_core::Source` already carries `has_video: bool`.
  What is missing is the JOIN: the `ToTimeline` handler decodes with
  `studio_media::decode_bytes` and constructs its `Source` with
  `has_video: false` hard-coded at every call site in `studio-app`
  (lib.rs:341, 1162, 1211, 2446). `has_video: true` appears only in
  tests and in `studio-engine`'s device fixture. So a video dropped on
  the timeline today becomes an audio-only source and the video half is
  silently dropped on the floor.
  The honest fix is small — set `has_video` from the entry's `Media`
  when importing, and give the timeline a video track to put it on —
  but it should NOT be done until there is something that can decode
  and display a frame, or it just moves the silent drop one layer
  later. That is §13's av1/fao-codec item, which is the real blocker
  and is still not started.

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
| Settings | **WRONG AS WRITTEN — a settings screen exists and is wired.** `shell.rs::settings_panel` (a real panel: animation, palette, text size, surface opacity, gesture sensitivity/commit/flick, mic, offload host, speaker id), reached as `Surface::Settings` from `voiceterm_egui.rs:1463`, applied live via `App::apply_settings` and persisted through `load_prefs`/`save_prefs` onto `shared/settings-kit`'s kv schema. voiceterm-egui has depended on settings-kit the whole time. What IS missing is four of the five areas the Qt popup carries: model setup, wake-word config, alignment recalibration, and notification toggles — none of which appear in the panel. Also unexposed: `Prefs` fields the kit defines and no voiceterm screen offers — `touch.min_target`, `touch.scale`, `touch.button_scale`, `input.handedness`, `input.double_tilt`, `input.movement`, `a11y.voice`, `a11y.tts`, `a11y.voice_filter`, `display.white_point`, `display.color_filter`, `model.pool_dir`. Those sit at their defaults forever with no way for a user to reach them, and handedness plus touch sizing are precisely the MOBILE affordances (a left-handed PinePhone user, a bigger tap target) this fleet is meant to serve. |
| Manpages | `manpage_view.rs` exists but documents its own real limitation in its header comment: it renders documents handed to it rather than owning the registry, because that logic lives inside `manpages.rs` (4,117 lines) tangled with Qt-only code, and splitting it needs a Qt-buildable host to verify against — which this sandbox has never had. An honest, named blocker, not neglect. |

- **C12-9a — the recommendation was built on the wrong row, and the order
  changes.** This said "IRC and Settings are the two genuine zero-to-something
  gaps" and put Settings first. Settings is not a zero-to-something gap: the
  screen exists, is wired, persists, and applies live (see the corrected row
  above). Only IRC is. Verified against code this session rather than against
  the table, which is CLAUDE.md 5's exact failure — a session following this
  entry would have built a settings screen next to an existing one.
  Revised order, with what each actually needs:
  1. **IRC** — still genuinely absent (zero irc-anything in egui-frontend), and
     `shared/irc-kit` exists, so check it before writing a client.
  2. **The unexposed `Prefs` fields — DONE (`55480f35`).** All twelve now have
     rows: `input.{handedness,double_tilt,movement}`, `touch.{min_target,scale,
     button_scale}`, `a11y.{voice,tts,voice_filter}`,
     `display.{color_filter,white_point}`, `model.pool_dir`. Rows iterate the
     kit's `all()` rather than a local variant list, and `6d9541d5` makes the
     compiler enforce that `all()` stays complete, so the panel cannot drift
     silently behind the kit.
     **NOT rendered.** No display here, so layout, wrapping and touch ergonomics
     are unverified, and the sections roughly double the panel's length —
     whether it now needs scrolling, grouping or a second surface is a question
     only a screen answers. That is the first thing to look at on hardware.
  3. **Niftycharts** — unchanged, still a wiring job over `nifty-core`.
  4. **The four Qt settings areas** the panel lacks (model setup, wake-word,
     alignment recalibration, notification toggles) — these are the real
     Settings gap, and unlike (2) they need backing state that may not exist
     on the egui side yet. Check before assuming it is only UI.
  5. **Depth-check graph/mail/browser** before touching them — unchanged; their
     line counts say "real module", not "feature-complete".

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
- **C12-10b — CLOSED (`007218a0`).** The guesser did need generalising, and the
  defect was narrower than this item guessed: it was not a family-specific key
  convention. `arch()` anchored on `<prefix>.embedding_length` ALONE, so a file
  whose hparams sit under one prefix but which omits or renames that single key
  fell through to the declared arch — and then every `<arch>.*` lookup missed at
  once, which presents as a pile of "missing X" rather than one resolution
  failure. It now trusts a declared arch backed by ANY known hyperparameter and
  scores candidate prefixes by how many they carry, `min` on ties as before.
  `HPARAM_SUFFIXES` is an allowlist rather than "count every key", because the
  latter resolves the architecture to `tokenizer` — hundreds of entries in a
  real GGUF against an arch block's dozen — and that failure would be silent
  and total. `NON_ARCH_PREFIXES` gained `audio` and `adapter`, since a wider
  net needs wider exclusions. 1242/1242; --gate-arch/-qwen3/-loadpath green.
  Does NOT make `block_count_from_tensors` redundant: that fallback handles a
  file with no metadata hparam block at all, and is untouched.

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
- **C12-16b — AUDITED, both clean.** The concern was that either app
  might gate op-log replay, presence or outbound edits on tab
  visibility, which would stall a collaborator the moment the local
  user looked away. Checked both update loops directly:
  - **logscope** calls `drain_scan` and `drain_infer` as the first two
    statements of `ui()`, before any tab dispatch or glass-mode
    branching, and `autoload_if_changed` right after. Nothing about
    which tab is showing reaches them.
  - **studio-live**'s wire block (`carrier.poll()` -> `integrate_packet`
    -> `drain_outbox`) is gated only on a carrier EXISTING, not on any
    panel state, and it deliberately calls `request_repaint_after(50ms)`
    so a peer's edit lands without waiting for a local one — the
    opposite of the failure mode feared.
  So the rule §16 documents was already being followed here; the doc
  section is a guard against a regression rather than a description of
  one. Recorded as a real result rather than left open, since "not
  audited" and "audited and clean" are different states and the next
  reader should not have to redo it.

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

## 19. gitmind fleet reporting: the header lied

**CLOSED (`646638d5`).** The exported fleet report hard-coded
`ok: 0, failed: none` at the Export call site, so every report claimed
nothing failed — including the beatspace run pasted into this session,
whose own log carried `error[E0433]` and `could not compile` ten lines
below the header saying all was well. `build_report::tally` now derives
both from the log, using the same `── name ──` section markers
`times_by_crate` reads so "a crate" means the same thing in both halves
of the report.

Worth keeping as a pattern rather than a one-off: a reporting tool whose
SUMMARY is not computed from the same data as its BODY will eventually
disagree with itself, and the summary is the half people trust. Any
future report field should be derived or absent, never a literal.

- **C12-19a — not done.** `fleet_build.sh` has its own summary
  (`OK`/`FAILED` counters) which IS computed, so it does not share this
  bug — but nothing cross-checks the two tools against each other on the
  same run. Worth a look if they ever disagree.

## 20. snapclone camera: the plan exists, the capture does not

**Half closed (`50a5f03d`).** `device_kit::source_plan` was hoisted out
of camera-bridge for this app and then nothing here called it.
`camera_source::pipelines_for` now answers HOW to open a chosen camera —
chosen device leads, probe findings stay as fallbacks, libcamera
outranks v4l2 on a phone without dropping it, front cameras flip to
match the mirrored preview. 4 tests, snapclone-egui 1303/1303.

- **C12-20a — `frame_pump` still runs no pipeline.** This is the
  remaining half and the bigger one. The decision layer is complete and
  tested; nothing executes it, so no frame has been captured. A capture
  thread now has something to be handed, which it did not before —
  that is the whole of what changed. Needs GStreamer at runtime and a
  real camera to verify, neither of which this sandbox has, so it
  should be written against the same `Backend`-style seam
  `aperture-engine` uses (construct-and-inject) rather than reaching
  for gstreamer inline, so the wiring stays testable with a fake.

## 21. blackstar-rs transformations: limbs hinge now

**`0bbd5501`.** Nothing on the fold path rotated anything — `compact`
displaces vertices, `contain` translates whole limbs, `foldsim` drives
both. So "vehicle mode" was a body with its limbs slid as far inboard as
translation allows and still protruding, which is why the containment
test had to be relaxed to "centred" rather than "contained".

`skeleton::fold_rotation`/`fold_point` add the hinge: a quarter-turn
about the limb's root, axis DERIVED from the geometry (perpendicular to
the limb and to where it stows) because these skeletons come from
arbitrary meshes with nobody to author an axis per part. 5 tests,
1220/1220.

- **C12-21a — nothing calls it.** `contain` still translates and its
  test still asserts the centred property, correctly, since that is what
  translation can promise. The open question is a design one, not an
  edit: how a limb's rotation should COMPOSE with the containment
  translation. Rotate first then translate the rotated bounds? Rotate
  only what still overhangs after translating? They give different
  silhouettes and the answer probably differs for a wing versus a leg.
  Worth deciding deliberately rather than by whichever is easier to
  write.

## 22. aperture modal + the asks not yet built

`helix.rs` (10 tests, aperture-core) is the keymap as a pure table:
hjkl, w/b, 0/$/g/G, x/d/y/p, i/a/o/O, v, Escape, plus `f` routed to the
Ctrl+G palette and `c` cycling the completion debounce. Live ONLY while
typewriter mode is on, so an ordinary editor cannot trap you in Normal.
Wired into `state.rs`; NOT compile-checked this pass (asked to hand over
the bundle instead).

- **C12-22a — the stutter fix is a guess that fits.** `trigger_completion
  _now` never checked `ghost.is_pending()`, so a typist faster than the
  model produced overlapping requests, each replacing the ghost with an
  answer computed from an older buffer. That IS a stutter and the fix is
  right, but it was found by reading rather than by reproducing — no
  measurement here, no model. If flicker survives it, the next suspect
  is the caret-move dismissal path, not the debounce.
- **C12-22b — Ctrl+G tiles, not started.** Asked for: multiple results
  rather than one, laid out as tiles with metadata rather than lines,
  and reaching PAST the current crate into the wider cargo workspace.
  `workspace_symbols` already indexes files; what is missing is the
  workspace walk (read the root `Cargo.toml`'s `members`, follow path
  deps) and a tile layout. The tile grid is the smaller half.
- **C12-22c — AI `gg`, not started, and the interesting one.** "I wrote
  a function, hit ESC gg, and the AI finds where it is useful" — that is
  a retrieval problem before it is a generation one. The pieces exist:
  `scope::QuickContext` extracts identifiers, `workspace_symbols` has
  the index, `prefixcache` holds prior answers, and `route_ai` already
  has the ask-and-rank shape with a batch seam (`take_pending_batch`).
  What is missing is the RANKING PROMPT and the tile UI to show several
  candidates, since there is rarely one right answer — which is also
  why it should present alternatives rather than jumping.

## 23. logscope hive mind: a strong backend with no front door

Surveyed `logscope-fan` (46 modules) against `logscope-egui`. The
distributed-compute backend is substantial and largely finished —
discovery, rendezvous, NAT pathing, peer auth, dispatch, consensus
clustering, power budget, build farm, queen/worker roles. The gap is
almost entirely at the UI seam.

**C12-23a — the device-add flow (`3f0…`, this session).** `pairing`
(typable checksummed codes, errors that name the misread character),
`discovery`, and `trustkit` (ordered Trust levels) all existed and NOTHING
JOINED THEM UP. `adddevice::Flow` is that join, as a pure state machine:
scan -> found -> code -> trust -> done. 7 tests.

The rule worth keeping wherever this is rendered: **discovery proves
something answered on the network, a typed code proves someone can read
the other machine's screen, and only the second justifies handing over
conversation.** `Flow::may_grant` caps an unverified peer at `Build`
(source only) and REFUSES a higher grant with an explanation rather than
silently downgrading — a person who asked for Handoff and quietly got
Build would believe the wrong thing about their own network. That rule
lives in the flow, not a screen, so no UI can offer what the flow would
not.

- **C12-23e — CLOSED (`b3d4fd3f`), and it was a live defect.** Taking
  the flow's rule and checking whether the existing UI obeyed it found
  that it did not: the Models tab granted `Trust::Compute` — "sees
  prompts and replies" — from a typed machine id, which carries no key.
  Its comment argued that Compute was safe because it is not the
  strongest level, but the meaningful line in `Trust` is between Build
  and Compute (source vs conversation), not between Compute and
  Handoff. Now caps at Build, with the status line saying what that
  means. Pinned as an inequality (`may_grant(false) < Trust::Compute`)
  so it survives a level being inserted later. Worth repeating as a
  method: writing the rule down as testable code and then AUDITING the
  paths that predate it is what surfaced this — the flow itself was
  never wrong.
- **C12-23f — `Trust::permits` had ZERO CALLERS (`5f7658e1`).** The
  bigger version of 23e, found by following the same audit. The function
  encoding "everything that generates sees the prompt" was written,
  documented and tested, and nothing consulted it: `enlist` mentions
  trust nowhere and `dispatch` delegates membership to `enlist`, so a
  Build-only machine could be invited into a fleet — ranking FIRST if it
  had the most slots, which a build box usually does — and handed a
  Thinker slice carrying the prompt. `select_invitees_for` gates at
  ADMISSION (not assignment: once a member, dispatch hands it whatever
  the queue holds). 3 tests; 404/404.
  - **CLOSED (`b41ad86c`): the caller now passes a registry.** `Coordinator`
    takes `registry` + `duty` via `with_trust()`; `new()` stays ungated so no
    existing caller changed. BOTH admission paths are gated — `invite()` and
    `best_untried()`, the latter because gating only the first would let a
    fleet acquire the refused peer by the side door of someone else declining.
    `refused()` reports who and why. 409/409; each gate broken in turn to
    confirm its test is the one that catches it.
    The original wording of this bullet — "the fleet code still calls the
    ungated form ... threading the registry through is the remaining work" —
    is no longer true and is replaced rather than left standing, since a
    stale open item reads exactly like a real one.
    What IS still open: no CALLER of `Coordinator` opts in yet. `with_trust`
    exists and nothing in the app constructs a coordinator with it, so the
    fleet is gated only where someone asks for it. That is a smaller gap than
    before (the mechanism now reaches the decision point) but it is the same
    shape of gap, and worth naming as such rather than calling 23f finished.
  - Method note, now twice-proven: writing a rule as testable code and
    then grepping for its callers is what found both 23e and this. A
    predicate with no callers is the shape to look for — it reads as
    finished and enforces nothing.
- **C12-23b — no UI renders `adddevice` yet.** The state machine and its
  prompts exist; nothing draws them. This is the actual "device add
  workflow UI/UX" ask and it is now a rendering job rather than a design
  one — `Prompt` already carries `needs_answer` so a screen knows when a
  spinner is wrong.
- **C12-23c — no UI renders `pairing::code_for` either.** The local
  machine must SHOW its own code for another device to type. Without
  that half, the flow above can only ever be driven from one side.
- **C12-23d — dispatch/consensus have no operator view.** `dispatch`
  tracks per-peer tasks and `consensus::cluster` groups divergent
  outputs, which is exactly what a "what is my hive doing" panel needs,
  and no panel reads either.

## 24. Everything known-unfinished, in one place

Recorded because it was asked for explicitly, and because several of
these are only in commit messages where nobody will find them.

**Verified-by-reasoning, not by running:**
- `aperture-engine::LocalEngine::load` — compile-checked only. No GGUF
  and no real adapter in this sandbox (lavapipe is a CPU rasteriser).
- All aperture interaction fixes (Tab, focus, palette, stutter) — no
  display here. The logic is tested; the FEEL is not, and feel is what
  was reported wrong twice.
- The KV prefix-stability work (§16): the property is pinned, the
  millisecond saving is not measured.
- This session's final aperture modal + beatspace commit was not
  compile-checked at all, by request.

**Built but with no consumer:**
- `kvplan::order_for_kv` — the batch issuer does not exist.
- `route_ai::take_pending_batch` — nothing calls the batch form.
- `a11y::glove_targets` (C12-4b) — no app wires it.
- `skeleton::fold_rotation` (C12-21a) — nothing folds with it, and how
  rotation composes with `contain`'s translation is undecided.
- `adddevice::Flow` — see §23 above.
- snapclone `pipelines_for` (C12-20a) — `frame_pump` runs no pipeline.

**Known-wrong and deliberately left:**
- studio `has_video` is hard-coded false at four import sites (C12-6a).
  Fixing it without a decoder just relocates the silent drop.
- beatspace: the archive named epoch337 contains epoch 200, and
  `gen_latest` is byte-identical to `gen_000200`. Either it stopped or
  ~137 epochs were never persisted.
- blackstar-rs `games/blackstar` vs `games/blackstar-rs`: READMEs now
  disambiguate, but the `c5blackstar-NNN` bundle series still carries
  the misleading name.

## 27. Worker creation mode: two of C11-20's four pieces exist now

- **C11-20c — CLOSED (`ab20b46e`).** `workermem::WorkerMemory`: append-only,
  provenance-tagged (Observation | Decision | Inference), capped, fenced on the
  way out. Built to `docs/12-queen-and-workers.md`, not from guesswork. Three
  decisions where the obvious implementation inverts the intent, each pinned by
  a test: the cap REFUSES rather than evicting (a ring buffer keeps the worker's
  newest guesses and discards the operator's oldest instructions);
  `summarize_with` collapses PER PROVENANCE (one blob would destroy the
  distinction at exactly the moment a revived worker reads it first); and
  `established()` excludes inferences (recording provenance and then handing
  consumers one flat list would store the distinction without acting on it).
  The fencer is INJECTED because `read_harness::fence` lives in logscope-egui,
  which depends on this crate — copying it is the voiceprint-kit mistake.
- **C11-20b — HALF DONE (`56d28956`).** `roster::Roster` + `Staffing` +
  `queen_request`. `QUEEN_PROMPT` always ended with "reuse before creating" and
  nothing ever gave the Queen a roster to reuse from; it does now. Staffing is
  by LANE (`owns`) rather than purpose, most-specific-wins, with equal claims
  returning `Ambiguous` instead of a tiebreak.
  **Still open from 20b:** the seed workers the doc suggests (reader,
  test-runner, patcher) are not shipped, so a cold system is still
  every-task-is-a-Queen-call — which the doc names as an open question
  ("who writes the first roster?") rather than something already decided.
- **C12-27a — the two new modules do not know about each other.**
  `WorkerSpec.name` is documented as the memory directory and nothing joins a
  roster entry to a `WorkerMemory`. Neither module is constructed at runtime by
  anything, so both are built-with-no-consumer in the sense of section 24.
  The join is small; what it needs first is a decision about persistence, since
  `workermem` is deliberately pure (no I/O) and a memory directory implies
  someone owns the reading and writing of it.
- **C12-27b — `continuation::Command` is still not the Delegator's.** The doc
  says the post-turn question ("keep going linearly / fan out / verify / stop")
  belongs here, answered by cheap rules first. `roster` answers the STAFFING
  question only; nothing answers the continuation one.
- **C11-20a remains the experiment that decides the thesis** and is untouched:
  same 1B, same task, with and without a lane + two-tool whitelist. Everything
  in doc 12 — and therefore both modules above — is speculation until that
  number exists, and it needs a device.

## 26. logscope: flush_every was inert, and what that leaves open

Closed this session (`adf1ecfd`, `1bcec8eb`): `Frame.passes` was never
incremented, so `over_flush` could not fire and the whole `flush_every`
mechanism did nothing — including the watchdog-prone `clamp(1, 64)` that
exists so a mobile driver is not handed one enormous submit. Details in the
commit; `--gate-submit` is green for the first time (lavapipe).

Also corrected: **C11-21a names the wrong function.** The field path is
`stream_session_window` (`logscope-egui/src/backend.rs:1543`, `:2103`), not
`generate_kv_stream`, which has two non-test callers and is not the chat path.

- **C12-26a — the mobile submit bound is now LIVE and unmeasured on real
  hardware.** Turning the mechanism on changes submit counts wherever a frame
  reaches its bound. Measured on lavapipe at k=4, 16 layers: desktop path 5
  submits per 8 tokens, watchdog path 29. That is the intended trade (bounded
  submit duration vs throughput) but nobody has run it on an actual Asahi or
  Adreno device since it started working. The field's own numbers were taken
  while it was inert, so every previous submits/token figure describes the
  unbounded path and should not be compared against a new run without saying so.
- **C12-26b — PARTLY CLOSED (`8d0e53f4`).** `LOGSCOPE_FORCE_WATCHDOG=1` forces
  `watchdog_prone()` on (never off, deliberately) so the mobile decode branch
  runs on any device; it is what showed the bound was inert. The DECISION is now
  a pure seam — `window_flush_plan(k, n_layers, bounded, saved)` — and is tested
  without a GPU, including that the mobile branch is bounded above regardless of
  k and strictly tighter than the collapsing branch.
  **Still open:** nothing tests the env var or the end-to-end submit COUNT. An
  env-based test would mutate process-global state while 1236 tests run in
  parallel, so it wants a serial-test mechanism (or a `Gpu` constructor taking
  the flag) rather than `set_var` in a `#[test]`. The arithmetic is pinned; the
  wiring from `watchdog_prone()` to the plan is not.
- **C12-26c — `per_tok` is now `n_layers * 16`, derived from a lavapipe
  measurement** (12.5–14.7 dispatches per layer-token across 3/8/16-layer
  fixtures). That ratio is measured on ONE driver with tiny fixtures; a real
  model on real hardware may sit elsewhere, and the constant 16 is a generous
  round-up rather than a law. `count-dispatch <model> <n> <k>` reports it.
- **C12-26f — `--gate-qkv`'s end-to-end failure is ONE ULP, and the gate now
  says so (`f125064f`).** It read `len 48 vs 48, max|Δ|=2.384e-6, rel=5.712e-7`
  after the diagnostic was added — floating-point reassociation, not a fusion
  that concatenates wrongly. The evidence is in the same gate: every isolated
  fusion comparison is EXACTLY zero (int8 m=1/m=4, q4 m=1/m=4, fused bias), and
  a wrong q/k/v block order produces garbage rather than a last-place bit.
  NOT diagnosed and deliberately NOT "fixed" by relaxing to a tolerance —
  lavapipe is documented here as departing from the WGSL spec in four ways
  including FMA contraction, which would make bit-exactness correct on
  conformant hardware and unreachable in this sandbox. Telling "the engine
  reassociates" (fails everywhere) from "this driver contracts differently"
  (fails only here) needs a second driver. Whoever runs this on real hardware
  next sees 0.00e0 or 2.4e-6 immediately and the question answers itself.
- **C12-26g — `forward_kv_q8_fused`'s doc cites `--gate-qkv-e2e`, which does not
  exist** in `kernel-validate`'s gate list. Its claim ("bit-exact to it") is
  unverified by anything in the tree. Same class as C11-19b's dangling
  HiveMind_Strategy citation.
- **C12-26h — the physical UI's `u_touch` is still dead, and wiring it is a
  design task, not a fix.** `gpuscene.rs` declares the uniform and
  `glow_scene.rs` uploads it every frame from `Uniforms::touch`, which nothing
  outside that file ever sets, so it is `None` forever and the shader helper's
  first line returns immediately on every fragment. Confirmed further this
  session: there is NO picking/raycast anywhere in the crate (no `unproject`,
  no `hit_test`, no ray code), so producing a world-space touch point needs
  screen-to-world picking that does not exist. `docs/physical-ui-findings.md`
  is right that this is small ONCE the UV attribute lands; it is not small
  today, and it cannot be visually verified in this sandbox at all.
- **C12-26d — two gates fail on lavapipe and predate this work.** Verified
  pre-existing by re-running with the flush pinned high (the old inert
  behaviour) and getting identical failures. `--gate-matvec`: the subgroup
  matvec is wrong on lavapipe (rel 2.0 on attn-kv-gqa and vocab>32768) — a
  FOURTH driver for the known subgroup problem WORKPLAN-next records.
  `--gate-qkv`: fused decode is not bit-exact against separate-projection
  decode. Neither is diagnosed.
- **C12-26e — the tiny fixtures only go to 3 layers.** `gen3.py` hardcodes
  `L` per architecture. Depth is what makes the dispatch-per-token figure
  interesting, so the generator taking a layer count as an argument would be
  worth doing rather than patching a copy into /tmp as this session did.

**Sandbox lore, both cost time:** cargo cannot run its rustc probe in this
harness ("couldn't read from stdin, as it did not contain valid UTF-8")
regardless of `< /dev/null`, `exec 0</dev/null` or RUSTFLAGS, while the same
probe by hand succeeds — so any workspace without a populated
`target/.rustc_info.json` is unbuildable. gpu-gemm is a SEPARATE workspace from
apps/logscope and had a truncated 524-byte cache; copying the good 986-byte one
from `apps/logscope/target/` fixed it (fingerprints match). This is CLAUDE.md
5d's "cached poisoned rustc probe" and the transplant is the workaround. Second:
backgrounded builds (`nohup ... &`) do NOT survive between tool calls; builds
must run in the foreground.

## 25. Fleet run on 30 apps: 7 real test failures, and the disk was NOT the cause

An uploaded `fleet-logs.zip` from a run that hit ENOSPC. Checked whether
the failures were disk noise before treating them as bugs, and they are
not: `grep -ilE "no space left|os error 28" *.test.log` matches NOTHING.
Every ENOSPC is in a BUILD log; the 7 non-zero test return codes are
genuine. Worth separating explicitly, because "the disk was full" is a
very easy way to dismiss a real result.

Failing: ambient-learner (4), bo-suite (5), bot-arena (3), fao-codec
(8), gitmind (1), logscope (1), logscope+features (2).

**C12-25a — fao-codec, PARTLY FIXED.** A quality gate (>= 50 vertices,
non-degenerate PCA) was added to `add_web` default-ON and the tests were
never updated, so every test that stores a web failed on a two-vertex
fixture. Fixed by making the fixtures REALISTIC (`gate_passing_mesh`,
64 vertices spread over three axes so the PCA has real eigenvalues)
rather than by setting `FAO_LIB_QUALITY=0` — a test that switches the
gate off stops covering the path real callers take. Also replaced the
hardcoded `2` vertex assertions with `GATE_MESH_VERTS`, since a literal
that the fixture already invalidated once will do it again. 65 -> 66
passing.

**C12-25b — fao-codec `manifest_persists_across_open` is a REAL bug,
not a fixture problem. Diagnosed, not fixed.** After `add_web`, the
log-db has the entry (`log_db opened (1 entries)`) and the reopened
JSON manifest has ZERO. `add_web` does call `write_manifest` (line
~431), and `open()` reads that file plainly, so the write is not
reaching the file the reopen reads. The strong suspect is `Drop`: it
rewrites the manifest only `if self.touch_dirty`, and `touch_dirty` is
set in exactly ONE place (`touch`, line 470) — never by `add_web`. So
the design comment "JSON gets rewritten on close via Drop" is true only
for touches, and a library whose only operation was an add may never
flush. Next step: assert the file's contents immediately after
`add_web` returns, which separates "never written" from "written then
lost".

**C12-25d — ambient-learner's four, and why they cannot be verified
here.** All four (`forecast::test_forecast_model`,
`dbmaint::test_deduplicate`,
`screen_temp_hints::choose_resolution_scales_with_sensors`,
`security::test_pairing`) live in modules of the GTK `main.rs`, not in
a lib target. Building them needs `gdk-pixbuf-sys`, which this sandbox
does not have — `cargo test --lib` reports 0 tests and the default
feature set fails at the build script. So the fleet machine can run
these and I cannot, which makes them exactly the failures worth
reporting from a fleet run and the hardest to fix blind.

Fixed one on arithmetic alone, and it needs a real run to confirm:
`test_forecast_model` builds 100 points ramping 20.0 -> 29.9, then
asserted the 1-hour forecast lands in 19..25. `forecast()` baselines on
`self.history.back()` — 29.9 — so that bound cannot hold for this
fixture; it was written against a flat one. Re-anchored to the LAST
READING (drift < 5 deg) rather than to absolute degrees, so it keeps
meaning if the fixture is retuned and still catches the real bug (a
model returning 20 for a series ending at 29.9). NOT COMPILED.

Two more read and fixed, one still unread:

  - `security::test_pairing` — A REAL LIBRARY BUG, not a test bug.
    `SecurityManager::new(data_dir)` writes the identity, the private
    key and the trusted-device list into `data_dir` and NEVER CREATES
    IT, so it worked only for a caller who had made the directory by
    some other means. The test passes `<tmp>/device2`, nothing creates
    that, and it failed with a bare "No such file or directory (os
    error 2)" naming neither the path nor the operation. `new` now
    creates its own directory and its error names the path — the
    function decides the layout inside that directory, so a caller
    cannot know to pre-create it.
  - `dbmaint::test_deduplicate` — the test asserted something the
    schema forbids. `calibrations.timestamp` is INTEGER PRIMARY KEY in
    the test helper AND in production (main.rs, db_writer.rs), so the
    second insert at timestamp 1000 is a UNIQUE violation, not a
    duplicate to clean. Rewritten to pin what is true: the constraint
    holds, and dedup is a no-op against a table that cannot hold
    duplicates. NOTE the larger finding — `deduplicate_calibrations`
    appears to be DEAD CODE for this schema. Either the column was
    meant to be non-unique, or the function is vestigial; worth
    deciding rather than leaving a function nothing can exercise.
  - `screen_temp_hints::choose_resolution_scales_with_sensors` — the
    test probed a range where its parameter cannot matter. At the
    default sigma 0.35 the kernel cap is 2/0.35 = 5.71 per dimension,
    giving (3, 6) before clamping, and MIN_COLS/MIN_ROWS are (4, 6) —
    so EVERY sensor count from 1 to 100 returns exactly (4, 6). It
    asserted strict growth from 1 to 10 sensors there, which is an
    assertion against the clamp rather than against the behaviour.
    Now probes sigma 0.05 (kernel cap 40) where the Nyquist term
    governs and "more sensors, more resolution" is the actual rule, AND
    pins the saturation as its own property: with a broad kernel, extra
    sensors buy nothing because the kernel blurs away the detail they
    would add. That second half is a real invariant that had no test.

  **All four ambient-learner failures are now addressed** — one real
  library bug (SecurityManager), three tests asserting things their own
  fixtures/schema/clamps forbid. None compiled here; all need a run on
  a machine with gdk-pixbuf-sys.

**C12-25e — bo-suite's five: all one bug, a test-parallelism race.
Fixed, NOT compiled (needs openssl-sys, absent here).** All five live in
`audio_capture_dump`. The module has an `isolated_xdg` helper that gives
each test its own tempdir, which READS like isolation and is not:
`XDG_DATA_HOME` is a process-global env var and `state()` is a
process-global Mutex, while cargo runs the six tests on parallel
threads. One test's `set_var` and state reset land mid-run in another.

The observed numbers say exactly this and are worth keeping as the
signature: "disabled should produce no files" saw 1 (another test's
file), and "second trigger should not start a new file" saw 11 (several
tests' windows in a shared ring). A per-test tempdir cannot fix it,
because the shared thing is not the directory — it is the variable
naming which directory to use. Added a module-level serialising lock
held for the whole body, ignoring poison so one panicking test does not
mask the rest.

**C12-25c — the other five apps, unexamined.** bot-arena's
`clean_repo_passes_and_broken_fails` and logscope's
`verdict::compile_grades_real_code` both SHELL OUT TO CARGO, so those
two plausibly ARE environment-caused even though the log has no ENOSPC
line — a cargo that fails for lack of space reports its own error, not
the OS one. gitmind's `repo_root_finds_monorepo_and_resolves_app_paths`
builds a fixture tree under /tmp and is the same shape. Worth checking
those three against a clean disk before spending time on them;
ambient-learner (4) and bo-suite (5) have no such excuse.

## 26. Mobile: reaching the palette without a keyboard

A command palette bound to Ctrl+P is unreachable on a phone, which makes
every command in it desktop-only by accident rather than by decision.
voiceterm solved this with a hold gesture on a bottom bar; the timing and
the model are now in the kit (`uikit_egui::cmdstrip`) rather than in one
app, so the second app to need it does not re-derive them.

`Hold` reports the threshold crossing ONCE and mid-press, so the menu
opens while the finger is still down — opening on release makes a hold
feel like a slow tap. `HOLD_SECONDS` is matched to voiceterm's value
deliberately: two apps in one fleet disagreeing about what a long press
feels like is worse than either number being slightly off.

`Strip` is recents-then-pinned, de-duplicated so a pinned command that
was just used appears ONCE in its recents position — where the thumb
last found it, rather than in a fixed slot and twice. Off by default: a
bar that appears uninvited takes height from what the person came to
look at. 9 tests.

Aperture has the palette toggle ("View: Bottom command strip") and the
state; `Strip::used` should be called from the palette's own dispatch so
recents populate themselves.

- **C12-26a — DONE, the strip is drawn.** A bottom panel above the
  status bar (so the status line stays last), shown only when the
  palette toggle turns it on. The palette button opens on TAP-AND-HOLD
  via `cmdstrip::Hold`, which is the way into the palette on a touch
  screen where Ctrl+P is unreachable.

  Two decisions in the drawing: `Hold::tick` runs BEFORE the widgets, so
  the menu opens on the frame the threshold is crossed rather than one
  late — a hold that visibly lags reads as a missed press. And a TAP
  opens the palette rather than running the first recent, because the
  strip's own buttons are already the fast path for recents; making the
  palette button do something else on tap would be two behaviours on one
  control.

  `AppState::run_strip_command` matches on the label, since the palette
  has no separate command id and inventing one would be a second naming
  scheme to keep in step. An unknown id says so rather than guessing at
  a near match — a stale entry is a real possibility once commands are
  removed.

  The palette now ALSO records what it ran, so recents populate from
  normal keyboard use. Without that the strip would only learn from
  itself, and the command someone reaches for most — picked from the
  palette — would never appear on it. aperture-egui 67/67.

  **OLD ENTRY:** The model, the gesture and
  the toggle exist; there is no bottom bar in aperture's frame and no
  hold target wired to `cmd_hold`. That is the remaining half and it is
  a rendering job.
- **C12-26b — TIMING converged, state deliberately not.** voiceterm's
  `HOLD_SECONDS` is now a re-export of `cmdstrip::HOLD_SECONDS`, so the
  two apps cannot drift on what a long press feels like, with a test
  asserting the equality — which is not a tautology, it is the guard
  that keeps it a re-export if someone re-declares a local constant to
  "just tweak voiceterm".

  The GESTURE STATE was NOT merged, and that is the finding rather than
  a shortcut: `cmdstrip::Hold` is one button with a threshold;
  voiceterm's `BarState` is a bar of buttons with a fan-out and a
  hovered option inside it. Collapsing the richer into the simpler
  would lose the hover — the part that makes sliding onto an option and
  back off again select nothing. Sharing the timing and not the state
  machine is the honest split, and "two things look similar" is not on
  its own a reason to merge them. voiceterm 1106/1106.

  Still open from the original report: the Qt->egui port's FAITHFULNESS
  more broadly, which is a per-screen comparison and was not attempted.
- **C12-26c — the strip should read the a11y registry.** `Cmd.id`
  matches `Semantics::command_id` on purpose, so
  `A11yScene::palette_matches` (§4c) could supply the strip's candidate
  list instead of a hand-kept table. Not wired.

## 27. Qt -> egui porting: the middle artifact

Screen-to-screen QML reading was tried and failed. The measurement says
why: voiceterm's `main.qml` is 32,111 lines holding 1,364 visual
elements. That is not a reading problem to push harder at — it is the
wrong pathway.

`maintenance/tools/qml_outline.py` makes it three steps:

    QML  ->  outline (plaintext)  ->  egui code

The middle artifact is the point. `apps/voiceterm/docs/QML_OUTLINE.txt`
is 34,708 lines of QML across 8 files reduced to 1,551 lines of
structure — holdable, reviewable by someone who knows what the screen
should look like WITHOUT their reading QML, diffable when the QML
changes, and auditable against a finished port.

Sizes are CLASSIFIED, not computed, which is what makes it work offline:
`fill` / `frac` / `fixed` / `expr` / `implicit`. Those five map onto
egui directly (`available_width`, a proportion, a literal, needs-a-human,
size-to-content) and none of them need a running Qt.

- **C12-27a — DONE for ChatPanel, and the outline earned itself.**
  Auditing the existing egui `chat_panel` against the spec found it
  rendered the MIDDLE of the QML panel and none of its chrome: no header
  row, no "New", no close button, no empty state. A person could not
  start a new conversation or close the panel. All four are in the QML
  and all four were absent, and reading 244 lines of QML had not
  surfaced it — reading a 15-line spec did, immediately.

  Added via `ui_with` returning `(sent, action)`; `ui` stays as the
  one-value shim so existing callers are untouched. `ChatAction::{New,
  Close}` is RETURNED rather than acted on, because both touch the
  backend and the window and a widget doing them itself would need both.
  uikit-egui 289/289.

  The transferable finding: the outline is more valuable as an AUDIT of
  a port that already exists than as a source to port from. A finished
  port looks finished; diffing it against a structural spec is what
  shows the parts that were quietly skipped.
- **C12-27b — no `expr` review pass.** Elements whose size is an
  arbitrary binding are marked `expr` and shown verbatim; those are
  exactly the ones a port will get wrong silently. Counting and reading
  them per screen is a small job that would de-risk the port.
- **C12-27c — CLOSED, and it paid immediately.** Added a NOT RENDERED
  report: every capitalised name that was neither drawn nor recognised
  as machinery, with counts. On voiceterm's own QML it found real
  missing widgets — `TextInput` x14, `ToolBtn` x26, `Drawer` x5,
  `RadioButton` x4 — that had been silently absent from the outline the
  previous commit shipped. An outline that omits the text fields is not
  a description of the screen, and nothing would have said so.

  Three fixes followed from what it found: the genuinely-missing Qt
  widgets joined `VISUAL`; project-local components resolve by scanning
  for a matching `<Name>.qml` beside the file, so `ChatPanel`,
  `SlidePopup` and `GestureHost` no longer read as unknown; and
  NAMESPACED widgets match on their last segment, so hatch's
  `PlasmaExtras.Menu` and `Kirigami.Icon` resolve without listing every
  KDE widget. Verified on a second app: hatch went from outlining as a
  few bare Items to 39 elements on its main page, with the genuinely
  unknown KDE types still reported rather than guessed at.

  voiceterm's residue is four inline-defined components (`ToolBtn`,
  `Pill`, `RadialBubble`, `ThoughtBubble`) declared inside main.qml
  rather than as files. Correct to report: they are real widgets the
  harness cannot resolve, and saying so beats pretending.

## 28. Port audit: the other three voiceterm screens

Ran the same outline-vs-port diff that found ChatPanel's missing header
(§27a) against the other ported screens. One is complete, one is a
third of its spec, one unmeasured.

**ConfirmDialog — COMPLETE.** 15 elements in the spec and the port has
all of it, including the two things easiest to skip: the shrinking
progress border (`remaining_fraction` is pure and tested) and the
auto-dismiss countdown, plus all three actions (confirm / "don't save
yet" / discard). Nothing to do. Worth recording as a pass — an audit
that only ever reports problems teaches people the passes were not
checked.

**C12-28a — ModelPicker is roughly a third of its spec.** 159 lines of
Rust against 41 elements of QML, and the shortfall is structural rather
than cosmetic. The QML has THREE sections:

  - "Downloaded:" — local models, `✓` on the active one
  - a curated list — per-model `label`, `blurb`, `★ Recommended`,
    `~N GB`, download state, and a heading that varies on
    `picker.detectedRam` (it recommends against models that will not
    fit)
  - HuggingFace search — a text field ("Search HuggingFace GGUF models
    (Enter)…"), a result list, and a drill-down into a model's FILES
    with a `‹ back to results` affordance

The port had ONE flat list of rows with star / size / Use / Download.

  PARTLY CLOSED. The RAM-aware heading is done (C12-28c) and the
  DOWNLOADED/PICKS SPLIT is done: `split_sections` is pure and tested,
  each heading drawn only when its section has rows. The split matters
  because those are different decisions — a downloaded model costs one
  click, a curated one costs a multi-gigabyte download — and a single
  list makes them look alike.

  BLURB: done. `ModelRow::blurb` renders indented under its row and is
  skipped when empty — an empty line under every model is worse than
  none under some. It is what makes a curated list worth having over a
  list of file names.

  SEARCH: the STATE MACHINE is done and tested (`SearchState`), the UI
  and the queries are not. One result list swapped by a `SearchView`
  enum, matching the QML's single list with a `showingFiles` flag —
  two lists would let both be populated and disagree about which is
  current. Transitions clear the results, so repo names never appear
  under a files heading while a fetch runs. `empty_hint` distinguishes
  the three ways a list can be blank (nothing typed / searching / no
  matches), because conflating them makes a pending search look like a
  failed one.

  STILL MISSING: the search PANE itself (text field, result rows,
  `‹ back to results` affordance) and the HuggingFace queries behind it,
  which need backend wiring. The pure half is done and testable; the
  half that needs a network is not.

**C12-28b — SetupWizard MEASURED, and it is better than the ratio
suggested.** The four-phase skeleton (setup -> choose -> downloading ->
done) is all there, with a real ProgressBar. Four specific features are
not, and they are not chrome:

  - **Voice models.** The QML has a whole section — "🎤 Voice input",
    "Speech models aren't installed yet…", a "Download voice models"
    button and a `downloadingWhisper` progress line. The port has no
    mention of speech at all, so a person setting up voiceterm cannot
    set up the VOICE half of it from the wizard.
  - **The Ollama fallback.** `ollamaFallback` text plus an "Enable
    Ollama" button shown when it is off. Absent, so the escape hatch
    for a machine that cannot run a local GGUF is unreachable.
  - **Dismissal.** The QML's last button reads "Later" when setup is
    required and "Close" when it is not, and the downloading phase has
    its own "Hide" plus "You can keep using the app — this continues in
    the background". The port has "Done" only, on the final phase. So
    during a multi-gigabyte download there is no way out of the wizard,
    which is the moment someone most wants one.
  - **RAM-aware recommendation.** "★ Recommended for <ram>" — the same
    guidance missing from ModelPicker (C12-28a), from the same source.

  **Dismissal is now FIXED.** `WizardEvent::Close`, with "Later"/"Close"
  on Intro (the word changes on whether a model is active, as the QML
  does — postponing and finishing are different promises) and "Hide" on
  Downloading, under the QML's own reassurance line. `Close` has NO
  phase transition on purpose: dismissing is the host hiding the view,
  not the wizard forgetting what it was doing, so reopening a hidden
  download lands back on Downloading rather than restarting at Intro.
  That is what "continues in the background" promises, and it has a
  test. uikit-egui 290/290, voiceterm checks clean.

  **Voice models and the Ollama fallback are now DONE.** Both are side
  offers on the Intro screen with their own events
  (`DownloadVoice`, `EnableOllama`), and a test pins that NEITHER moves
  the phase — advancing for them would skip someone past choosing a
  model because they enabled a fallback. The voice section is gated on
  `!voice_installed` (an offer to install what is installed is noise)
  and shows a disabled "Downloading voice models…" while fetching, since
  an ~88 MB download behind a live-looking button reads as a click that
  did nothing. Both flags default FALSE: the wizard must not claim voice
  is present or Ollama is on until the host says so, because guessing
  optimistically hides the offers this screen exists to make.

  **SetupWizard is now complete against its outline.** The RAM
  recommendation arrived via the shared `ModelPicker` (C12-28c).

**C12-28c — the RAM heading, DONE and shared.** It appeared in BOTH
audits, which is the signal it should be written once. `picks_heading`
is now in `model_picker` and renders above the list, so the star is
explained rather than being unexplained decoration — a person cannot
otherwise tell whether it means "popular", "newest" or "will actually
run on this machine", and it means the third. Fixing it in the shared
widget fixes SetupWizard too, since the wizard embeds the same picker.

What it deliberately does NOT do: decide WHICH model is recommended.
That logic already exists and is good — `hf_hub::recommend_curated` in
voiceterm is reserve-aware (it subtracts resident whisper), speed-first
(smallest that fits, not biggest), and falls back to the smallest rather
than to nothing. It needs the curated list and a whisper-reserve
estimate that a shared widget has no business knowing, so the host keeps
setting `ModelRow::recommended` and the widget only explains what the
star means. Duplicating the fit arithmetic to avoid passing a bool would
be two answers to one question, and they would drift.

The wording distinction is tested: with no RAM figure the heading
promises ORDERING ("fastest first") rather than suitability, because
claiming a recommendation "for your machine" when nothing was measured
is a guess wearing a measurement's clothes. uikit-egui 291/291.

**Method note.** Three screens took minutes to audit because the spec is
15-40 lines each. Doing this by reading QML was attempted before and
did not happen at all. The outline's value is concentrated here rather
than in generating ports.

## 29. Port audit, second pass: terminals / web / graph / settings

Redirected here by the operator: ModelPicker's remaining search pane is
deferred (that screen is likely to become a logscope backend, like
aperture's), and the terminal, web, graph and settings surfaces matter
more.

Survey of the egui side: `term_grid` (435), `browser` (362),
`layered_graph` (384), `graph_view` (323), `connections_graph` (317) all
exist with real substance. **Settings did not exist at all** — no module,
no re-export, nothing.

**C12-29a — CORRECTION: settings was NOT missing.** I recorded that it
"did not exist at all — no module, no re-export, nothing". That was
wrong, and wrong because of how I looked: I searched `uikit-egui` and
the front-end's module names, not `shell.rs`. voiceterm HAS a settings
panel (`shell::settings_panel`, L194) and it IS rendered
(voiceterm_egui.rs L1463). It covers animation, palette, display and
gestures — real preferences, backed by `settings_kit::Prefs`.

What I ported is a DIFFERENT surface: the QML `settingsPopup` is four
navigation doors (model setup, chat, recipes, sessions) plus voice
activation and theme. The two are complementary — one is preferences,
the other is a launcher — and the kit's widget is now named
`quick_launcher` rather than `settings_panel`, since two things called
settings_panel in one app is how the next reader repeats my mistake and
concludes one is a duplicate.

**The original C12-29a entry, kept for what it built:** Ported from
`main.qml`'s `settingsPopup` (L27729). Six entries: four DOORS (model
setup, chat, recipes, sessions) and two actual settings (voice
activation, theme). Returns a `SettingsEvent` rather than mutating a
config, because a widget that opened the chat panel itself would need a
handle to it, the setup wizard and the session store — most of the app.
The QML's trailing `→` on the doors is kept and is in the type too: a
test asserts no event is both a door and a setting. 4 tests.

Consequence, restated correctly: the four DOORS had no route from the
egui port — model setup, chat, recipes and session history are not
reachable from `shell::settings_panel`, which is a preferences screen.
So the gap was real but narrower than "settings is missing": it is the
LAUNCHER that was missing, not the settings.

- **C12-29b — CLOSED, the launcher renders.** Wired into voiceterm's
Settings surface, above the preferences panel — doors first, because
that is what someone opening this menu usually wants; the sliders are
for a visit made on purpose.

All four doors currently report "not ported yet" through the existing
toast system, and that is deliberate rather than a stub: `Surface` has
Blocks, MissingPkg, Workspaces, Settings, Favorites, CmdSheet and
Palette — no Setup, Chat, Recipes or Sessions. A button that silently
does nothing reads as a bug in the app; one that says what it is reads
as the truth. Two of the four widgets (setup_wizard, chat_panel) EXIST
in the kit and are unhosted, so adding those Surface variants is the
next step and is a separate change from putting the launcher on screen.
voiceterm 1106/1106.

**OLD ENTRY:** Same shape as the
  cmdstrip (§26a): the widget and its events exist, no app draws it.
- **C12-29c — region slicing DONE, and the first findings are that
  there is less QML here than expected.** `qml_outline.py --region <id>`
  now outlines the subtree under a named element, so a screen inside
  main.qml can be audited the way a standalone .qml file can.

  Fixing it required a real parser fix: QML allows
  `id: termScreen; Layout.fillWidth: true; height: 200` on one line and
  main.qml uses that form constantly, but the property reader only took
  the FIRST pair per line — so most named regions had no id recorded and
  slicing found nothing at all. Split on `;` only when the line has no
  braces, since a `;` inside a binding or JS block is not a separator.

  What the slices actually say, checked against the source rather than
  trusted:
  - `termScreen` is ONE element. The terminal is drawn, not composed —
    its only children are FontMetrics/TextMetrics helpers. So there is
    no element tree to port and the egui `term_grid` (435 lines) is not
    missing structure; comparing them element-wise would be a category
    error.
  - `swipeView` is one element because the web page was REMOVED
    (v0.1.839, per its own comment: "the terminal is the only page").
    The egui `browser.rs` may therefore be ahead of the QML rather than
    behind it — worth confirming before treating web as a port gap.
  - `previewPanel` gives 19 elements and nests correctly, which is the
    control showing the slicing works.

- **C12-29d — the graph audit is done, and it inverts the assumption.**
  `vSysGraph` and `sysGraphTab` turned out to be tab BUTTONS (2 and 4
  elements). The real surface is `graphRoot` (L25469): an Item sized
  from `cardsCanvas`'s bounds with a Repeater of node cards — so edges
  are canvas-drawn and nodes are elements, and the whole QML graph is
  4 visual elements.

  The egui side is `graph_view` (323) + `layered_graph` (384) +
  `connections_graph` (317), and `layered_graph` does layered assignment,
  back-edge detection, barycentre/median ordering and CROSSING
  MINIMISATION. main.qml contains no equivalent — grepping it for
  layer/crossing/sweep/barycentre near "graph" returns zero. The egui
  graph is substantially AHEAD of the QML, not behind it.

  **Conclusion for §29 as a whole.** Of the four areas asked about:
  settings was genuinely missing and is now ported; the terminal is
  canvas-drawn and has no element tree to port; the web page was
  DELETED from the QML in v0.1.839; and the graph is more capable in
  egui than in Qt. Only settings was a port gap. The other three are
  cases where "faithful port" is the wrong frame — two because there is
  nothing to be faithful TO, one because faithfulness would be a
  downgrade.

  These five surfaces carry 65 tests between them (term_grid 16,
  connections_graph 15, browser 13, layered_graph 13, graph_view 8), so
  they are built rather than stubbed. That is worth stating because the
  earlier audits established that substance is not coverage — here it
  turns out to be, and saying so is the point of having checked.

## 30. One GPU, several apps: daemon or per-app backend?

Asked directly, and the honest answer is that the question splits into
two that have different answers. Grounded in this fleet's own numbers
rather than in general advice, because the general advice ("modern
kernels schedule it") is true and not the constraint that bites here.

**SCHEDULING is mostly fine; MEMORY is not.** The M2 Ultra run in
`fleet-logs` reports the shape clearly:

    resident memory: 7.2 GiB total = 2.5 anon + 4.7 file-backed
    GPU budget 48.0 GiB clamped to 31.0 GiB
    loaded in 86.0s
    prewarmed 16 pipelines; 42 decode + 37 GEMM tiles from autotune cache

Two apps each running their own backend on one 8B model is ~5 GiB of
resident weights EACH, an 86-second load EACH, and two pipeline
compilations. `gpu-gemm`'s own docs already record where this ends: a
session touching three models held "~110 GiB of address space for one
model's worth of work", and the fix was releasing page-cache residency
because "on a machine where the GPU budget is already the binding
constraint it is competing for exactly the memory the live model needs".
That lesson was learned WITHIN one process. Across processes there is no
equivalent lever at all — no app can release another's residency.

So: per-app wgpu is fine when each app's resident set is small and its
GPU work is bursty (a shader-based viewfinder, a graph layout, a
spectrogram). It stops being fine the moment two apps each want a
multi-gigabyte model resident, and that is exactly the aperture +
logscope case that prompted the question.

**Scheduling is the weaker argument for a daemon, but not zero.** Modern
drivers do preempt, but compute dispatches preempt at coarse
granularity, and this engine already fights that: the log shows
`prefill chunk = 128 tokens/dispatch (auto-lowered from 256 ... a fixed
token count means a bigger model gets a longer dispatch, which is what
trips the GPU watchdog)`. Two uncoordinated consumers issuing long
compute dispatches interleave badly — not incorrectly, just with each
other's latency doubled and both closer to the watchdog. A daemon giving
each consumer a longer uninterrupted run is a real improvement, but it
is a second-order one next to not loading the model twice.

**The recommendation: the daemon is a MODEL-RESIDENCY server, not a GPU
scheduler.** That framing decides the design questions:
  - It owns the weights and the pipeline/autotune caches — the expensive,
    shareable things.
  - It does NOT arbitrate every GPU call. Apps keep their own wgpu
    device for their own rendering, which is bursty, small, and already
    scheduled fine by the driver.
  - Apps that want inference talk to it; apps that want to draw do not.

**Most of it already exists.** `logscope-serve` is that daemon minus a
socket: `netserve::serve(listener, keyring, gen)` where `gen` is
`Fn(&str, usize) -> String`. `aperture-engine::Backend` was deliberately
given that same shape so a `DaemonBackend` drops in beside `GpuBackend`
without the provider, the prompt assembly or the loop pane learning
anything (see its module docs). The missing pieces are a unix socket,
a lifecycle (start on demand, idle-unload, one instance per model path),
and the client.

- **C12-30a — the CLIENT is built; the daemon side is not.**
  `daemon::DaemonBackend` speaks to a unix socket using the SAME request
  shape as `logscope-serve`'s HTTP endpoint (POST /generate, prompt as
  the raw body, `?max=N`) — same shape on purpose, so the daemon is that
  server with a second listener rather than a second protocol, and one
  handler cannot drift from the other. No Bearer key: on a unix socket
  the filesystem already answers who may connect, and a key here is
  ceremony with a storage problem attached. `probe` fails fast and names
  the path, since it is what `resolve` calls and "connection refused"
  with no path is the least actionable error available.

  `socket_path` prefers `$XDG_RUNTIME_DIR`, falling back to `$HOME`
  rather than `/tmp` — a socket in a sticky shared directory is
  world-visible and this one carries prompts.

  4 tests against an in-process fake daemon on a real UnixListener.

  The DAEMON SIDE now has its listener: `netserve::serve_unix`. The
  change that unlocked it was one signature — `read_request` was typed
  to `&mut TcpStream` and never used anything TCP-specific, so making it
  generic over `Read` let the same parser and the same `handle` serve
  both doors. That is why this is a second listener rather than a second
  protocol: nothing about the endpoint is duplicated.

  No keyring on the unix door, and it is safe BY CONSTRUCTION rather
  than by a flag: `Keyring::accepts` returns true on an empty ring
  (verified, not assumed), so passing `Keyring::default()` authorises
  local callers through the filesystem's own permissions. The TCP path
  keeps its keyring for the LAN case. A stale socket file is removed
  before binding — a daemon that will not start because its predecessor
  was killed is a daemon people stop running.

  2 end-to-end tests over a real UnixListener rather than calling
  `handle` directly, because the thing in the way WAS the transport and
  a test that skipped the socket would not have caught it.
  gpu-gemm 1237/1237.

  The FLAG is in: `logscope-serve --unix [path]` opens the local door
  ALONGSIDE the TCP one, on a thread, because both listeners block and a
  daemon reachable only one way is what the flag exists to prevent. With
  no value it takes the conventional path.

  One rule, one place: `netserve::default_socket_path` is the single
  definition, `logscope-serve` delegates to it, and
  `aperture-engine::daemon::socket_path` delegates too under the `gpu`
  feature (reimplementing it only in the dependency-free default build).
  Two copies of that rule is exactly how a client and a server end up
  looking in different places and each concluding the other is not
  running.

  The LIFECYCLE POLICY is now `daemonlife`, pure and tested (7 tests).
  Both decisions take the clock rather than read it, so the awkward
  cases are testable without a process, a socket or a model:
  - `Life::decide` never unloads with a request in flight, however long
    the idle window says — a prefill can take a minute and a daemon that
    unloads mid-generation turns a slow answer into a lost one.
  - Overlapping requests both have to finish; a stray `end` saturates
    rather than wrapping, since `in_flight` at four billion would be a
    memory leak with no symptom until the machine swaps.
  - A backwards clock reads as "no time passed" rather than unloading
    immediately.
  - `should_start` names the third case rather than folding it into the
    other two: a daemon running a DIFFERENT model must not be joined and
    must not be duplicated, because two daemons hold two models' weights
    — the exact situation this whole design exists to prevent.

  Default idle is 10 minutes: long enough to cover reading a reply,
  switching windows and a compile, short enough that a machine left
  alone gets its memory back. The window exists because the expensive
  thing is the LOAD (86s + pipeline warm-up), not the process — it is
  not a politeness delay.

  The WIRING is in. `serve_unix_tracked` takes an optional `LifeHandle`
  and brackets each HANDLER (not each connection — the window that must
  not be interrupted is the generation, and a client that opens a socket
  and sends nothing should not hold 7.2 GiB). It uses a `Busy` DROP
  GUARD rather than paired calls, so a panicking handler cannot leave
  `in_flight` raised forever; both the guard and `Life::end` ignore lock
  poison, because a count that never comes down makes the daemon
  immortal, which is worse than reading state a panicking thread wrote.

  `daemonlife::janitor` polls rather than waiting on a condvar: the
  thing being waited for is a CLOCK, not an event — nothing signals "ten
  minutes of nothing happened" — and a poll every few seconds costs
  nothing next to the weights it guards. It returns after firing, since
  the daemon's response is to exit and a janitor watching a `Life`
  nobody updates is just a thread. A poisoned lock reads as `Stay`:
  unloading on the strength of state written by a panicking thread could
  drop weights mid-generation.

  **WIRED END TO END.** `logscope-serve --unix [path] [--idle SECS]`
  constructs the `Life`, spawns the janitor, and on unload removes the
  socket before exiting so the next client's probe fails fast rather
  than connecting to a path nothing serves. `--idle 0` disables it.

  A BUG CAUGHT BEFORE SHIPPING: the `Life` initially saw only unix
  requests, but ONE PROCESS SERVES BOTH DOORS — so a busy LAN client on
  the TCP port read as silence and the daemon would have exited
  mid-conversation. Idle means the PROCESS is unused, not that one
  listener is. `serve_tracked` now reports TCP requests to the same
  `Life`, with a test that holds a handler open and asserts
  `in_flight == 1` from outside. gpu-gemm 1247/1247.

  The daemon is now complete: client, transport, both listeners, one
  rendezvous path, lifecycle policy, and the binary that runs it.

- **C12-30c — CORRECTION to §30's own recommendation.** I wrote that the
  daemon "owns the weights and the pipeline/autotune caches — the
  expensive shareable things". The autotune cache is already shared
  WITHOUT a daemon: `store::cache_root()` is
  `$XDG_DATA_HOME/logscope/int8cache` (override `LOGSCOPE_CACHE_DIR`),
  per-user and process-independent, and the startup log names the file
  it writes there. Two apps on one machine already hit the same cache;
  the second pays the 0.6s probe once and never again.

  So the daemon's case rests on the WEIGHTS — 7.2 GiB resident, 86s to
  load — plus the in-process pipeline warm-up, which IS duplicated per
  process. Still overwhelming, but ONE reason rather than two. Worth
  being exact about: "it shares the caches too" is the kind of claim
  someone leans on when the memory argument does not apply — a small
  model on a large machine — and it would be wrong there.

  The general form, which is the reusable part: **state that is already
  a file in a user-level directory is shared by construction; only
  state held in a process needs a process to share it.** Weights are
  resident memory. Autotune results are a file. That distinction, not
  "expensive things", is what decides whether a daemon earns its
  keep.
- **C12-30b — DONE, and built before the daemon on purpose.**
  `Reach::{Daemon, InProcess}` plus `resolve(order, daemon, in_process)`:
  try each in order, take the first that constructs, report which one
  was used so a UI can say where answers come from. Takes CONSTRUCTORS
  rather than doing the work, so the decision is testable with no
  daemon, no model and no GPU — and the decision is the part worth
  pinning.

  Four tests: a missing daemon falls through to the local load; the
  daemon wins when present (or the memory saving silently never
  happens); total failure reports the LAST error, since "no such model
  file" is more useful than "connection refused" from a service the
  person may not know they were meant to have; and an empty order errors
  rather than panicking.

  Built first because it is what makes the daemon SAFE TO ADD. A daemon
  whose absence breaks the editor is worse than no daemon: the failure
  is invisible, intermittent, and looks like the editor's fault.

## 31. logscope-egui as a daemon CLIENT

aperture now asks the daemon before loading its own weights (§30). The
same is worth doing for logscope-egui, which is the heaviest consumer —
it is the app the 7.2 GiB and 86-second figures came from.

**The seam already exists and is a good one.** `backend::BackendKind` is
`{LlamaCpp, Wgpu}` with a `make(kind) -> Box<dyn Backend>` factory and a
`preferred()` that picks by what is compiled in. A `Daemon` variant
drops into exactly that shape, and `preferred()` is where "use the
daemon if one is listening" belongs — it already answers "which backend
should this be" and is consulted by both the load and the chat path, so
they cannot disagree.

**C12-31a — CLOSED. The daemon is selected, used, and streams.**
`BackendKind::Daemon` exists, `preferred_reachable()` picks it when a
socket answers and falls back to the compiled-in backend otherwise, and
the worker uses that at startup. Both directions are tested: no daemon
falls back (the property that makes this safe to add at all), and a
live one wins (or the memory saving silently never happens, which is the
failure that looks like success).

Probed ONCE at worker startup rather than per request: a daemon
appearing mid-session should not move the model out from under a chat
that already loaded one, and a daemon vanishing is noticed where it
matters — the generate call fails and says so. `preferred_reachable` is
kept SEPARATE from the pure `preferred()` because it touches the
filesystem, and a pure-looking function that connects to a socket is a
trap the moment someone calls it in a draw path.

logscope-egui 1053/1053.

**The original entry, for the reasoning it records:** `Backend::
generate` takes `on_token: &mut dyn FnMut(&str) -> bool` and streams.
The daemon's HTTP endpoint returns the whole reply. So a naive Daemon
backend would call `on_token` once with everything, and the difference
is visible: today tokens appear as they are produced, and through the
daemon the UI would sit still and then paint a paragraph. On a 4 tok/s
decode that is a ~40-second stare at nothing for a 170-token reply.

Three options, in increasing order of work:
  1. Accept it, and say so in the status line ("via daemon — replies
     arrive whole"). Honest, cheap, and worse to use.
  2. Chunked transfer on the daemon side, streaming through the socket.
     The endpoint already speaks HTTP/1.1; this is the conventional
     answer and it keeps one protocol for both doors.
  3. A second framed protocol on the unix socket only.

(2) is almost certainly right — it preserves the property that the unix
door is the SAME endpoint rather than a second one, which is what has
kept the two from drifting so far. Worth doing before wiring
logscope-egui, because shipping (1) first would teach people that the
daemon makes the app feel worse.

**The chunked CODEC is now built** (`netserve::chunked`, 6 tests). Four
of those pin failures that would each be invisible in a happy-path test:
  - an EMPTY token must encode to nothing, since a zero-length chunk is
    the terminator — a generator emitting one would end its own response
    early and silently truncate the reply;
  - a truncated body decodes to `None`, not to a short string: a daemon
    killed mid-generation must not look like it finished, because a
    half-written completion accepted as complete is worse than a visible
    error;
  - lengths are BYTES, so the first non-ASCII token does not
    desynchronise the stream and garble everything after it;
  - the head does not send Content-Length alongside Transfer-Encoding,
    which is the classic way to produce a response nobody can parse.
  Chunk extensions (`1a;foo=bar`) are tolerated — legal HTTP that
  hand-rolled parsers usually choke on.

  **The streaming SERVER is now built.** `StreamGen` is
  `Fn(&str, usize, &mut dyn FnMut(&str) -> bool)`, `serve_unix_streaming`
  serves it, and `stream_whole` adapts an existing whole-reply generator
  so no current caller has to be rewritten first.

  Only `/generate` streams; everything else goes out whole through the
  existing `handle`, so there is ONE routing table rather than two that
  could disagree about what `/version` returns. `on_token` returning
  false stops generation when the reader has gone — otherwise a closed
  pane would keep burning GPU time producing tokens nobody reads.

  The test asserts the FIRST token is readable while the generator is
  still working (it sleeps 300ms before its second, and the client reads
  with a 150ms timeout). A test that only checked the final text would
  have passed against the non-streaming server too, which is the trap
  here.

  CLIENT DONE (`6ceb59c4`). Not cosmetic: the server grew a streaming
  endpoint while `DaemonBackend` still read whole bodies, so enabling
  streaming would have delivered HEX LENGTH PREFIXES inside the
  completion text — not a crash, a completion containing "1a" and stray
  CRLFs, which gets blamed on the model rather than the transport.
  Decided by the `Transfer-Encoding` header rather than by sniffing,
  since a completion can legitimately start with a hex-looking line. A
  truncated body is an ERROR, not a short answer.

  The decoder is duplicated rather than imported, on purpose: the
  default build does not link gpu-gemm at all (the point of the `gpu`
  feature), and pulling a GPU engine into an editor to parse eight lines
  of framing is the wrong trade. A test under `--features gpu` pins the
  two equal across six inputs, so it cannot drift — duplication WITH a
  proof of equivalence, which is a different thing from duplication.
  19/19 without the feature, 20/20 with. gpu-gemm 1255/1255.

  STILL TO DO: `Backend::generate` returns `String`, so tokens are
  reassembled before the caller sees them — invisible for aperture's
  short completions, not for logscope's chat. A streaming variant of the
  trait is what C12-31a needs next, then logscope-egui's `Daemon`
  backend variant.

**Note on scope:** this is where the daemon's value is largest AND where
its UX cost is most visible, which is why the streaming question has to
be answered here rather than deferred. aperture's completions are short
bursts where whole-reply delivery barely shows; logscope's chat is not.

## 32. The uuid "failure" was a stale index cache, not our lockfiles

A fleet run reported 25 of 38 targets broken with

    failed to select a version for the requirement `uuid = "^1"` (locked to 1.24.1)

which reads as a lockfile problem and is not one.

**The evidence, in the order that settled it.**
1. Checked crates.io directly: NOTHING in uuid 1.2x is yanked, up to
   1.26.0. So the locked versions exist.
2. Read the full error rather than the summary line. The second line is
   the one that matters: `candidate versions found which didn't match:
   0.8.1, 0.8.0, 0.7.4`. Those are the versions cargo could SEE. uuid
   1.x has existed since 2022; an index offering 0.8.1 as its best has
   not learned uuid 1 exists.
3. Confirmed the same lockfile resolves here: `cargo metadata` in
   `apps/aperture` returns uuid 1.24.1, no error, from the identical
   `Cargo.lock` that failed on the reporting machine.

So the lock is right and the local index cache is stale for that one
crate. The giveaway that it is ONE crate rather than a generally old
index: the same build resolves egui 0.34 and accesskit 0.24 fine — a
genuinely old index would have failed on those first.

**Why it takes down "half the monorepo" and none of it is ours:** `uuid`
is a transitive dep of `accesskit`, which every egui app pulls in. One
bad cache entry fails every GUI app simultaneously, which looks exactly
like a repo-wide breakage.

**The fix is one directory:**

    rm -rf ~/.cargo/registry/index/*/.cache/uu/id/uuid

`maintenance/tools/registry_doctor.sh [root] [crate]` automates the
diagnosis: locked versions across all lockfiles, what crates.io actually
publishes, and what the local cache's newest known version is. When that
last number is older than the locked one, this is the fault.

- **C12-32a — do NOT "fix" this by regenerating lockfiles.** Every
  search result for this error says delete Cargo.lock or run `cargo
  update`. On this repo that is 36 lockfiles of collateral damage for a
  fault in none of them: it would replace a one-directory fix with a
  fleet-wide version bump nobody asked for, and produce a diff large
  enough to hide whatever the next real problem turns out to be.
- **C12-32b — `rusty-board` is a DIFFERENT failure** and was hidden in
  the same list: `failed to run custom build command for
  gtk4-layer-shell-sys`. A missing system library, unrelated to uuid.
  Worth separating, because a summary that groups 25 failures under one
  heading buries the one that is not that heading.

## 33. blackstar-rs merge: what is wired and what is a subcommand

Operator report: "the code from the patch either didn't do much or
wasn't wired yet". Checked each module for callers OUTSIDE itself, and
the answer splits cleanly in two.

**GENUINELY WIRED into the playable game:**
- `align` + the fold work. `skeleton::FoldStep { rot, pivot, shift }` is
  the type of `Model::pack`, and `build_morph` (main.rs:2824) is called
  from the live transform path at 2788, gated on `Mode::Play` and
  offline. So pressing transform in-game does go through the new
  alignment and the hinge. This is the part that closed C12-21a and it
  is real.
- `fleet` — 12 call sites outside itself.

**NOT wired — headless subcommands only:**
- `helmai` — ZERO references outside itself. Reachable solely via
  `--helm` (main.rs:1212), a CLI simulation.
- `holomap` — its only "reference" outside itself is a COMMENT inside
  helmai.rs. Reachable solely via `--holo`.
- Neither `sim.rs`, `world.rs` nor `physics.rs` mentions any of
  helmai/holomap/fleet. The live game loop does not touch them.

So the merge landed one real feature (folding) plus two simulators that
run as `blackstar --helm` / `--holo` and never appear in the game. That
is not nothing — a headless sim is how this codebase has tested
everything from raids to driveline physics, and both have tests — but it
is also not what "the ship flies itself" and "the system on the plot
table" sound like in a commit subject.

- **C12-33a — decide what those two are FOR.** If they are design
  spikes, the commit subjects should say so; if they are meant to be in
  the game, they need a call site in the update loop and something to
  draw with. Worth asking the session that wrote them rather than
  guessing, since a simulator that was always meant to stay headless is
  finished work and wiring it would be wrong.
- **C12-33b — this is the FOURTH built-but-unhosted finding this
  session** (snapclone's ~35 entry points, logscope's trust gate,
  voiceterm's launcher, now these). Consistent enough to be a standing
  check rather than a coincidence: after any merge, grep each new module
  for references outside itself. `orphan_check` catches files nothing
  compiles; `maintenance/tools/unhosted_check.py` now catches modules
  that compile fine and nothing calls.

  It independently reproduced the manual finding — helmai and holomap as
  `cli-only [tested]`, fleet and align correctly absent — and found 12
  more uncalled-but-tested modules in blackstar-rs I had not looked at,
  plus 36 in snapclone.

  Three states, not pass/fail, because "no callers" alone is too blunt
  to act on: tested+uncalled is usually work that stopped one step
  early; untested+uncalled is usually dead code; cli-only may be
  entirely intentional, since headless sims are how this repo tests
  raids, physics and folding. It never fails a build — a gate would grow
  an allowlist nobody maintains, and the value is a list a person reads
  after a merge.

## 34. Fleet run after the index fix: 6738 passing, 9 crates failing

Clearing the uuid index cache recovered almost everything: 35 of 38
builds ok (was 13), 26 crates green, 6738 tests passing. The remainder
is not one problem — it is at least four, and grouping them hid that
last time.

**Fixed here:**
- **studio `analysis_runs_far_faster_than_realtime`** — the test measured
  the MACHINE, not the code. A flat `took < 0.25` bound: 0.13 s on the
  x86_64 sandbox, 0.961 s on the Asahi aarch64 laptop, same debug build,
  algorithm untouched. Now calibrates against a fixed float workload on
  the same machine and asserts a RATIO (measured 1.9 healthy, limit 12).
  That still catches an algorithmic regression — decimated coarse search
  back to a full one is an order of magnitude — while surviving hardware
  that is uniformly 7x slower. A perf test that fails on slow hardware
  trains people to ignore it, which costs more than the regression it
  guards.
- **gitmind `binpin`** — THIRD instance this week of the same shape.
  `Scratch` gives each test its own directory and then points at it by
  setting `HOME`, a process-global env var, while cargo runs the five
  tests in parallel. One test's `set_var` (and its `remove_dir_all` on
  drop) lands mid-run in another, so the assertion loaded a manifest
  from a different test's HOME. Production behaviour was correct; the
  test could not see it. Serialised, with HOME restored on drop.

**Still open:**
- **beatspace** (5 build errors) — already fixed after the run's commit
  (`a66bf03a`); the fleet ran at `aa2816ce`.
- **rusty-board** — `gtk4-layer-shell-sys` build script. A missing
  system library, not our code.
- **C12-34a — bot-arena, fao-codec, logscope, voiceterm, ambient-learner,
  rusty-board-crates-board-layouts** still failing, unexamined here.
  `voiceterm::snapshot_store::snapshot_restore_roundtrip` says "3 files
  restored, left: 0, right: 3", which has the same smell as the gitmind
  one — a filesystem test whose fixture another test removed. Worth
  checking for a shared directory or env var before assuming the restore
  logic is wrong.

**The pattern worth acting on:** three of the failures I have diagnosed
across two fleet runs were test-isolation races over a process-global
(HOME, XDG_DATA_HOME, XDG_RUNTIME_DIR), and one was a machine-speed
assertion. None were product bugs. A `parallel_test_hazard` check —
grep for `set_var` inside `#[cfg(test)]` without a serialising lock —
would have found all three mechanically.

## 35. snapclone's `wired 51%` is a measurement artifact, not a gap

Last commit ended by naming snapclone's 25-of-49 `wired` score as "the
next thing worth understanding". Understood, and the answer is that
there is nothing to fix.

All 24 surfaces the audit cannot see in code DO exist:

    SegmentedTabStrip -> src/nav_chrome.rs (+ uikit segmented_tabs)
    UnifiedNavBar     -> src/unified_nav.rs
    FocusReticle      -> src/reticle.rs
    GlassIconButton   -> src/glass.rs
    GlassScrim        -> src/glass.rs
    SnapTile          -> src/snaps.rs
    ContactCard       -> src/lib.rs
    SwipeNavigator    -> src/swipe_physics.rs
    ... and 16 more

Checked mechanically: of the 24, **24 are mentioned somewhere in the
source and 0 are absent entirely**. `covered_in_code` requires the Qt
name OUTSIDE a comment, and this app follows the same house style the
function's own docstring warns about — the Qt class is named in the
MODULE DOC and the code below uses idiomatic Rust names (`glass.rs`,
`reticle.rs`, `unified_nav.rs`). That is the documented false negative,
in a new app.

So `wired` is behaving exactly as designed: it is the LOWER bound, and
`named`/`live` at 100% are the upper. The gap between them is where to
LOOK, not a count of missing work — and here looking found nothing
missing.

- **C12-35a — do not "fix" this by renaming.** Making the code say
  `SegmentedTabStrip` to satisfy an audit would trade idiomatic Rust for
  a metric, and the metric already reports both bounds precisely so it
  does not have to be gamed.
- **C12-35b — DONE.** The audit now splits the two: `doc_only` (Qt name
  in a module doc, idiomatic Rust below — house style, not work) and
  `absent` (nowhere at all — real). Printed under the table so a low
  `code` column can be read at a glance instead of investigated from
  scratch, which is what cost me an hour here.

  Fleet-wide: bo-suite 13 doc-only, snapclone 24, voiceterm 2,
  bot-arena 1 — and **two apps DO have absent surfaces**:
  ambient-learner 1 (`EguiLib`) and logscope 6 (`BlocTab`, `CargoTab`,
  `CodeTab`, `ModifyTab`, `RotationTab`, `SourcesTab`).

  CORRECTION, recorded because I claimed the opposite in the commit that
  added this: I wrote "zero absent in any app" from a grep that returned
  before the ABSENT lines printed. It was wrong within a minute of being
  written, which is a fair argument for the split existing — the
  distinction only helps if the numbers are then read.

  The six logscope ones are NOT missing work either, and my diagnosis of
  them was half wrong: they are not Qt ports at all. The "originals" are
  RUST — `crates/logscope/src/ui/{bloc,cargo,code,modify,rotation,
  sources}_tab.rs` — so this is a Rust-to-Rust port between two crates
  in one app. Four of the six already cited their origin FILE ("Ports
  `ui/bloc_tab.rs`") and none named the TYPE, which is what the audit
  matches on: the citation was half-written everywhere.

  RESOLVED (`89db8456`) by naming the type in each: "Ports
  `ui/bloc_tab.rs`'s `BlocTab`". That renames no code — C12-35a's rule
  holds — it states which type the module ports, which four of the files
  were already trying to say. Each original was verified to exist first,
  including `rotate.rs` -> `rotation_tab.rs`, where the names differ and
  a guessed citation would have been wrong.

  **The fleet now reports ZERO ABSENT surfaces.** Every remaining
  sub-100% `code` score is the doc-only house style. The column is
  finally meaningful: the next entry to appear in it will be real. `ambient-learner`'s `EguiLib` is now RESOLVED: it was
  `src/egui_lib.rs`, a feature-gate shim whose own header says "the
  module is empty unless the feature is on" — not a screen. It
  CamelCased into a phantom surface. The file now carries
  `parity-audit: not-a-surface`, and the tool honours that marker in the
  Rust branch, which it previously only did for `.qml`. ambient-learner's
  code score went 67% -> 73% and its ABSENT line is gone.

  Wrong turn worth recording: I first guessed the phantom came from
  CamelCasing `lib.rs`/`main.rs` and added a guard for those. It did
  not — the file is `egui_lib.rs` — so the guard protected nothing and I
  reverted it. An unused guard is a thing the next reader has to rule
  out, which is a real cost for a line that never fires.

## 36. Multipass autocomplete: one reply is several candidates

Operator report from build 80: completions "rely on manual cleanup
afterward". The pasted example shows why, and it is not model quality.
One reply contained four different kinds of thing at once:

  - real code (`if state.show_loop_pane { ... }`)
  - a HALLUCINATED construct (`egui::Panel { title = "Frame"; }` — not
    egui's API)
  - an ELISION (`// ... rest of the code remains the same`)
  - a STUB (`fn main() { ... }`)
  - PROSE ("The provided code snippet appears to be...")

Single-pass insertion pastes all of it. The elision is the dangerous
one: it LOOKS like a comment, so a naive filter keeps it, and accepting
it into a real file replaces working code with a note saying the code
used to be there.

`aperture_complete::multipass` is the fix, as four pure passes:
split_fences, blocks, classify, assign_keys. Nothing talks to a model or
an editor, so the awkward cases are testable — and the awkward cases are
the point, since a well-formed reply was never the problem.

Wired: `Ghost::on_outcome` runs `plan` and shows the FIRST insertable
block, so Tab still means "take the obvious one" and the common case is
unchanged. The rest go to `AppState::menu` on home-row keys (f j d k s l
a ;), drawn above the status bar. ESC clears the menu and returns to
typing.

Decisions worth keeping:
  - Only INSERTABLE blocks get keys, so there is no keystroke that can
    accept an elision. Enforced at assignment rather than re-checked at
    the accept site — two places deciding one safety rule is how they
    drift.
  - Unkeyed blocks are shown greyed WITH their reason ("would delete
    code", "stub") rather than hidden, so "why is that not offered" has
    an answer on screen.
  - The menu claims home-row keys only WHILE IT IS UP, and
    `accept_menu_key` returns false for an unbound key so the letter
    still reaches the buffer. The menu never swallows a keystroke it has
    no use for.
  - Prose is kept and rendered as a comment. It is often the most useful
    part of a reply and must never be compiled.
  - No mode flag: an empty menu IS the no-menu state. A flag and a menu
    that can disagree is how a UI gets stuck in a mode with nothing in
    it.

- **C12-36a — NOT COMPILED.** Written under an explicit "don't compile,
  I need to iterate" instruction. The multipass module is pure and its
  tests are written; the wiring (ghost signature change, two call sites,
  state fields, key routing, the panel) has not been built once.
- **C12-36b — the ghost still shows one block.** A person cannot see the
  alternatives until they look at the menu. Showing the chosen block
  inline and the others below is right for now; whether the ghost should
  cycle with the keys is a real question and not obviously yes.

## 37. voiceterm-egui had no shell at all

Operator: "mostly still don't see keys appearing in terminal and can't
run commands". Traced it, and this was not a bug in the input path —
every piece of the input path is correct and complete.

`pty_input::drain_egui` encodes keystrokes properly (arrows, Ctrl-C,
Backspace, Alt word motions). `Session::pty_out` collects them. A
previous fix even corrected this loop for THROWING THE BYTES AWAY, and
its comment says so at length. `term_grid` parses VT/ANSI and draws it.

What did not exist anywhere in the crate was a PROCESS. No `openpty`, no
`CommandBuilder`, no spawn, and no `portable-pty` dependency — while the
Qt side has all of it in `pty_term.rs`. The bytes were encoded perfectly
and written into a `Vec<u8>` nothing read; nothing ever wrote bytes
back. The comment at the echo site said it plainly — "the demo has no
PTY to write to" — and that had stopped reading as a gap.

So the symptom was not one bug. It was the absence of the component that
would make either half work, behind a UI complete enough to look like it
should.

`pty_host` is that component: spawn `$SHELL -l` with TERM=xterm-256color,
one reader thread appending to a shared buffer, a writer that FLUSHES
every time (a keystroke sitting in a buffer is indistinguishable from
one that was ignored), resize that skips no-ops (a SIGWINCH every frame
makes full-screen programs redraw forever), and EOF-based exit detection
rather than polling `try_wait`.

Wired: keys go to the shell instead of the local echo when one exists,
its output feeds the SAME parser the echo used, and the grid size is
pushed to the pty each frame.

- **C12-37a — NOT COMPILED.** Written under "don't do long tests or
  compiles". The two end-to-end tests are `#[ignore]`d because they
  spawn a real shell.
- **C12-37b — local echo is now a FALLBACK, not the normal path.** With
  a shell attached the echo is skipped, because the shell echoes what it
  receives and doing both doubles every character. If the operator sees
  doubled characters, that branch is the place to look.

## 38. rusty-board typed into itself

Operator: "turned into a demo rather than a keyboard that happens to
have testability on desktop, and needs to work in KDE Plasma Mobile on
the Fairphone."

Traced it. `KeyboardService::commit_text` queues onto `pending`;
`take_pending` drains it; and NOTHING outside the service's own tests
called `take_pending` for its content. `demo_app`'s frame loop did
`for _ in self.service.take_pending() { self.committed += 1; }` — the
text discarded, the count kept. So every tap, swipe word and dictation
result was routed correctly into a Vec nobody read, and the only visible
effect was a preview string drawn in the app's own window.

A faithful simulation of typing, into itself.

WHAT WAS ALREADY THERE: nearly all of it.
`rusty_board::compositor::virtual_keyboard` has a `VirtualKeyboard`
trait (`commit_text`, `press_keysym`, `backend_name`), backend
DETECTION, and implementations over `zwp_input_method_v2` /
`zwp_virtual_keyboard_v1`. The egui front-end simply did not depend on
that crate and never reached for it.

`sink::Sink` is the join: `Preview` for desktop and tests, `SystemSink`
over the existing backend behind a new `wayland` feature. The front-end
holds a `dyn Sink` and does not know which it has, which keeps it
buildable standalone (its stated promise) AND testable without a
compositor.

Two decisions:
  - `Preview` APPLIES editing keys rather than only recording them. A
    backspace that visibly did nothing in the demo would hide a real bug
    in key routing.
  - The status line now says WHERE TEXT IS GOING, always. "Typing into
    this window" and "typing into the focused app" looked identical from
    outside, and that ambiguity is most of why this went unnoticed.

- **C12-38a — NOT COMPILED**, per instruction. The `wayland` feature
  path especially: it references `Detected::detect().build()` from the
  parent crate and has never been built once.
- **C12-38b — FIXED in the same pass.** Writing up 38a surfaced a
  second instance one layer in: Backspace and Return wrote to
  `self.typed` directly rather than going through `press_keysym`, so on
  device letters would type and editing keys would do nothing. Both key
  paths now go through `emit_text`/`emit_key` — ONE place each, so a key
  path added later cannot quietly skip the sink the way these did. The
  preview string is now a MIRROR of what was sent rather than the
  destination, which is what stops it and the focused window
  disagreeing.

## 40. unhosted_check was wrong about five modules out of five

Ran it on voiceterm-egui and it reported five tested-but-uncalled
modules: favorites, layered_graph, mail, missing_pkg, model_picker.
Checked each by hand before acting, and ALL FIVE ARE CALLED.

The idiom it missed:

    use voiceterm_egui::missing_pkg as mp;
    mp::missing_pkg_ui(ui, ...)

`missing_pkg::` never appears anywhere, so a `name::` search finds
nothing. voiceterm aliases every one of its surface modules this way, so
the checker was wrong about the entire app.

Fixed by also matching `use ... <name> as <alias>`. voiceterm now
reports 0 uncalled, which matches reality.

RE-RAN the apps I had already reported on with the broken tool:
  - blackstar-rs 14 -> 13 uncalled, and the finding that mattered
    survives: helmai and holomap are still `cli-only [tested]`, so §33's
    conclusion stands.
  - snapclone 0, unchanged.
  - logscope 12 uncalled, spot-checked `debatepanel` and `chat_layout`
    by hand — genuinely declared, documented, and never called. Real.

The lesson is about the tool's own risk, and it is worth stating
plainly: a checker that is WRONG ABOUT THE APP IT IS RUN ON is worse
than no checker. Five false positives out of five would have taught
anyone reading it to ignore the report — including on blackstar, where
it was right. Every finding it produced before this fix deserved the
hand-check I gave the voiceterm five, and §33's did get one.

- **C12-40a — DONE: all twelve hand-checked, all twelve real.**
  Grepped each for `name::`, `as name` and `name as` outside its own
  file. Eleven have zero references; `picking`'s single hit is the word
  "picking" in three prose comments, so it is uncalled too.

  2,119 lines of tested, documented, unreached code in one crate.

  The interesting one is `picking` vs `scenepick`. Both do screen-space
  hit-testing for the physical UI:
      picking.rs   — "screen-space hit-testing for the PHYSICAL UI"
      scenepick.rs — "making the physical UI an actual UI"
  and only `scenepick` is called. That is not one stalled module, it is
  a SECOND ATTEMPT at the same job that replaced the first without the
  first being removed — which is a different problem from "stopped one
  step early" and wants deleting rather than wiring.

  The other pairs to check for the same shape: `tab_layout` (whose own
  docs call it "the sibling of chat_layout") and `chat_layout` are both
  uncalled, so the adaptive-layout decision path may be a third
  duplicate. `workbench` (5 tests) claims to own "the non-chat tabs",
  which the `tabs/` modules already do.

- **C12-40b — NOT started: decide replace-vs-wire per module.** These
  split into two piles and the piles want opposite treatment. A module
  superseded by a working sibling should go; a module nothing else does
  should be wired. Guessing wrong in either direction is expensive —
  deleting live design work, or wiring a second hit-tester alongside the
  one already in use. Each needs a look at its sibling before anything
  is touched, which is more than a session's tail can do responsibly.

## 39. snapclone-egui: the host boundary is honest, and one arm is not

Operator says the QML/Qt-translation approach is working here — agreed,
and the audit backs it: 49 surfaces, all present, 0 absent, and
`unhosted_check` reports 0 uncalled modules. It is the healthiest of the
three front-ends.

Checked the failure shape that caught rusty-board and voiceterm this
turn — a queue with no reader — and snapclone does NOT have it.
`Host::drain_commands` feeds `on_event`, the binary consumes the result,
and the sink is explicitly labelled:

    Deliberately a debug sink and labelled as one. Capture, export and
    retry all need a camera pipeline, an archiver and a socket that this
    binary does not have yet; printing is not a stand-in for them, it is
    a way to SEE that the request reached the boundary.

That is the right way to leave an unfinished boundary: the gap is named,
and an unhandled command is LOUD on stderr rather than silently dropped.
Worth recording as a pass, since the same author wrote the two that
failed.

- **C12-39a — one arm contradicts its own comment.**
  `MarkViewed`/`MarkExpired` are matched with `=> {}` under the note
  "these two the app can answer for itself: bookkeeping about what the
  user already saw". But `HostCommand::MarkViewed`'s own doc says "mark
  as seen IN STORAGE", and `MarkExpired`'s says "storage does different
  things with each". So they are not app-internal bookkeeping; they are
  the two commands that need a store, and they are the two that are
  silently discarded rather than reported like the rest.

  In-memory viewed state exists (`snaps.rs`, tested:
  `opening_a_snap_marks_it_viewed_immediately`), so the effect today is
  that a snap is viewed until restart and then unviewed again. Not
  built here because it needs a persistence layer decision — a JSON
  sidecar, the settings store, or the same place `snap_expiry` will
  want — and guessing at it is how two stores end up disagreeing about
  what was seen.

  The narrow fix meanwhile: make those two arms report like the others
  instead of claiming to have handled them. A comment that says a thing
  is handled is worse than no arm at all.
