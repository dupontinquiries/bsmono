# C13_TODO — open threads from the c13 session

Same convention as `C9`–`C12`: every item says how far it actually got.
None of those files are superseded. C12_TODO §1 (blackstar-rs CLOSED,
1215 green) is still true; this session took the suite to 1342.

The c13 session merged bundle 171, then worked one theme end to end:
**the game looks and sounds like a demo, and each symptom had a
mechanical cause rather than a taste problem.** Four of those were found
and fixed. The rest of this file is what was not.

Session: `134bc4f7..61341bc0`, 6 commits + 1 merge. 1342 tests green
under BOTH `--no-default-features` and `--features audio`.

---

## 0a. GETTING A PLAYTESTER INTO A RAID

```sh
BLACKSTAR_PLAYTEST=all ./skydribble
```

Clears every mission at load. Applied to the loaded log and NEVER written
back, so a tester's real progress survives. It prints
`PLAYTEST UNLOCK ACTIVE` on stderr on purpose — a silent cheat produces bug
reports from a state no player can reach, and neither the tester nor you
knows it when you read one. A named subset works too
(`BLACKSTAR_PLAYTEST=the_pluto_line,the_beach_opens`), and a typo is
reported rather than silently doing nothing.

Or, in-game and without a restart: **SETTINGS → DEBUG → MASTER ACCESS**.

Then **THE HOLD → STOCK THE BAG**, which only appears under master
access. Access alone is not enough: weapons and customisation are not
gated by the campaign at all — they are gated by what you OWN, and a
fresh save owns nothing, so a tester with every gate open walks into
empty rooms. The kit is a bag-sized spread (six weapons across six
elements, plus mods) offered as a PRESS rather than applied
automatically, so a tester with real progress does not lose their bag by
turning a debug flag on.

While master access is on the HUD carries a dim **MASTER ACCESS — GATES
OFF** watermark under the scoreboard band. It is there so a tester's
screenshot or recording says which state it came from: the stderr line
at launch does not survive a video, and a bug from a save with every
gate disabled is not the same bug. A save flag rather than a launch argument, so a tester
toggles it mid-session and the save records that it was on. Every gate
defers to `Shipboard::unrestricted`, one method, so turning it on does
not leave the tester hitting the next gate they had not thought about.

The two are for different jobs: the env var is for a scripted or
one-shot launch, the setting is for a tester already in the game who has
just hit a wall.

Why either is needed: this session added three progression gates, and
two of them sit in front of the thing most worth testing.

| gate | opens after | film |
|---|---|---|
| the Wardeck (raids) | `the_beach_opens` | 1 |
| the space bridge (leaving Sol) | `what_jhiaxus_grew` | 2 |
| the far worlds | `the_far_gate` | 4 |

The forge and echo rows sit OUTSIDE all of them by design — solo practice
content is the on-ramp to the thing being gated, so it must not be gated
too. A tester with no unlock can still forge and fly echoes.

## 0. READ THIS FIRST: nothing here was seen or heard

There is no GPU and no audio device in this sandbox. Every visual and
audio claim below is verified to the level of *the math is right and the
shader compiles*, and no further. Two of the four fixes change what
every frame looks like.

**The single highest-value next action is a new screen recording**, on
the same chassis and a couple of maps, so the Fresnel and value-structure
changes can be judged. Until then they are hypotheses with tests attached.

## 0b. WHAT THE PLAYTESTER WILL ACTUALLY MEET, and what to ask them

Shipped this session and worth watching in a session, in rough order of
how likely it is to be visibly wrong:

* **The frame looks different.** Ground reflections were turning terrain
  into sky at grazing angles; the default map moved from MEADOW (0.58
  stops of value separation, the flattest of 36) to OBSERVATORY (4.16).
  Both are unverified — no GPU here. First question to a tester: does the
  ground read as ground.
* **Audio is stereo and occluded** for ball bounces, car-ball touches,
  boost pads and flip resets. Never heard. Ask whether anything sounds
  like it is coming from inside a wall.
* **The HUD was rasterising ~3x taller than its layout claimed** and is
  rebuilt. The in-game HUD is fixed; the hand-placed menus (pause,
  garage, tweaks, academy, maps) are NOT and share the bug class.
* **Bots stood up wrong** because `settle` asked a car-shaped heuristic
  about a robot. Fixed. The other two reported symptoms — falling
  through the floor, fractured parts protruding — are measured
  (`stance.rs`) but NOT fixed.
* **The sortie list now names its film and its cast**, so a mission says
  "you play this one as Prime" before you launch it.

Do NOT ask a tester about: Films 7-9 (held on purpose, see 2j), the
mocap/swing work (no phone has touched it), settlements or planet
palettes (derivations with no geometry or renderer behind them).

## 1. The fast loop cannot see the audio module — fix this early

`games/blackstar-rs/tools/fastcheck` runs `--no-default-features`, but
`audio` is a **default** feature and the entire cpal implementation is
`#[cfg(feature = "audio")] mod imp` — about 400 lines. So the crate's own
edit/test loop never type-checks the synth, and a green fastcheck says
nothing about the configuration players actually build.

This bit hard: several commits this session reported green while
`sound.rs` had never been compiled once. It is the same shape as the
c12 `cargo-ailite` gap — *the verification tested the artifact against an
environment the artifact does not ship*.

- **C13-1a** — teach fastcheck a second pass, or a `FEATURES=` env, and
  make the default loop cover both. Needs `libasound2-dev` present;
  `apt-get install -y libasound2-dev` works here, but **`apt-get update`
  fails first** until `/etc/apt/sources.list.d/nodesource.sources` is
  deleted — it is malformed, not merely 403ing as CLAUDE.md §3 says.
- **C13-1b** — 345 warnings under `--no-default-features` vs 118 under
  `--features audio --tests`. Nobody has looked at why the counts differ
  that much; some of the 345 may be things that should be called.
  **Partly answered:** 325 of them are `never used`, now counted per file
  by `tools/deadweight` against a committed baseline. The ratchet fails
  only when a file gets WORSE, which is the part that was missing — the
  330 were real and unreadable, so one more changed nothing anybody
  noticed.

- **C13-1d — `tools/deadweight` PARTLY DUPLICATES A TOOL THAT WAS
  ALREADY HERE, and I did not look.** `maintenance/tools/unhosted_check.py`
  predates this session and does the same job better at the granularity
  that matters. Its own docstring names the exact defect and the exact
  offenders:

  > Four independent sessions in one week shipped the same defect: logic
  > that was written, tested, and never reached from the running program.
  > … blackstar-rs: helmai and holomap, reachable only as CLI subcommands

  Run it: `python3 maintenance/tools/unhosted_check.py games/blackstar-rs`.
  It reports **20 uncalled modules, 23 subcommand-only**, and names four
  of mine directly — `combo`, `footing`, `settlement`, `stance`, each
  `[tested]`. That is C13-1c confirmed by something I did not write.

  **Use `unhosted_check` first.** It answers "which whole modules does
  nothing call", which is the expensive case — a crate with green tests
  and no call site reads as finished in every summary anyone writes.
  `tools/deadweight` answers a narrower question, "did any file get more
  unreached SYMBOLS than last time", and its value is that it is a
  ratchet rather than a report: it fired on its own author three times
  in one session, including on a test-only constant I had shipped in the
  binary. Keep both, reach for the Python one first.

- **C13-1c — I MADE THIS WORSE, MEASURABLY.** The four largest
  dead-code files in the tree are all modules added in this session:
  `saga.rs` 25, `settlement.rs` 16, `swing.rs` 11, `combo.rs` 11 — 63
  unreached symbols, and the top four positions. Each was written as a
  headless core with its shell deferred, which is the right shape and
  also exactly what "built carefully, tested, and never called" looks
  like from the outside. The ratchet exists because the pattern was mine
  as much as anyone's. Wiring any of them needs the thing that blocks
  it: a screen for `settlement`, a phone for `swing`/`combo`, authored
  claims for `saga`.

## 2. Atmosphere: what was fixed, and the much longer list that was not

### Fixed (mechanical causes, all with tests)
- **HUD text metric** (`6d9475d2`). `font::text` draws glyphs 7 font-px
  tall / 6 wide; `ui.rs` claimed 2.4 and 1.15, and the hand-placed HUD
  was authored to the same fiction. Everything in that population
  rasterized ~3× taller and ~5× wider than its claimed box. Now derived
  from `font::CH`/`CW`, with tests that rasterize emitted `Cmd`s through
  `font::text` rather than trusting the layer's self-report.
- **Ground reflections** (`189b91c2`). Terrain `f0` *was* the map's
  wetness, then plain Schlick drove it to the 0.85 cap at grazing — and
  a chase camera looks along the ground almost exclusively. The
  curvature denoise then distrusted the trace and substituted
  `sky_color`, so the ground became the sky. Split f0 from smoothness;
  dry ground (`wet < 0.25`) no longer traces at all. This is the terrain
  half of the 240 fps problem as well as the wash.
- **Value structure** (`61341bc0`). MEADOW measured 0.58 stops — the
  flattest of 36 styles, and the default map. Grass luma was 0.456; real
  grass reflects 6–15%. Fixed, ZEN GARDEN likewise, default moved to
  OBSERVATORY (4.16 stops).
- **Spatial audio** (`26fb7521`, `f05dbddd`). Ported unendlich's tested
  `sound_mix` + `audio_occlusion` into `mixer.rs`, then made `sound.rs`
  stereo so it could actually be heard.

### NOT started, roughly in value order
- **C13-2a — CONTACT SHADOWS. The biggest single remaining lever.**
  Nothing in the recording touches the ground: the bot floats. The
  renderer ray-marches soft shadows for SDF surfaces, but the read has
  not been done on whether rasterised chassis meshes get any shadow at
  all, and no cheap blob/contact shadow exists. `World::ground_h` is
  already there for a projected decal. Do the cheap version first.
- **C13-2b — dust, dirt and grime.** The operator's stated target is
  Bay-Transformers: realistic animated bots with real dust and dirt amid
  combat. There is no material axis for it — everything is flat albedo.
  Wants surface grime, dust kicked at contact points, and scorch
  accumulating through a fight. Check whether `damage.rs` already tracks
  state a material layer could read.
- **C13-2c — motion weight.** Speed-based FOV kick on boost, a landing
  camera thump, a frame or two of hit-stop on heavy impacts. `camera.rs`
  already has the fold-cinematic machinery to hang these on.
- **C13-2d — the remaining flat maps.** HEARTH HALL (0.84 stops),
  MONKEY RUN (1.02), COLISEUM (1.18), APERTURE (1.28). Dim and bunched
  rather than broken; the arithmetic fix crushes their grounds to
  near-black and 8-bit quantisation eats the hue, so this is art
  direction on maps nobody in this session has seen. `MIN_STOPS = 0.8`
  is a regression floor, not a target.
- **C13-2e — darks vs curiosity. THE LORE HAS NOW BEEN READ**, and it
  answers this more directly than expected. Source: the
  `transformers_blackstar` archive (a small git repo; two Feb-2026
  commits, most files staged-uncommitted, and an UNTRACKED `addendum/`
  holding v2 and v3 of "The Black Archives... Watch Bearer", 2026-08-21.
  v3 supersedes v2 and is the newest material anywhere in it).

  The palette is specified as a three-way contrast, not a mood:

  | | Earth | Pluto | Proto-Earth |
  |---|---|---|---|
  | | Warm | Cold | Molten |
  | | Gold | Blue | Red |
  | | Life | Void | Birth |
  | | Noise | Silence | Pressure |

  Pluto is "blue-white lighting, no warmth", explicitly set against "LA
  warmth, gold sunsets". So the darks-vs-curiosity balance is a per-map
  register the way `MapStyle` already works, and the value-structure
  audit (`valuestruct.rs`) is the tool for holding each map to its
  column. Note OBSERVATORY — now the default — is the Pluto column
  almost exactly.

  Tone for the dread half: "Unicron. Not villain. Event. Cosmic
  inevitability." And for the curiosity half, the Blackstar relic
  "doesn't scream weapon, it whispers: *you knew*."

- **C13-2g — the raid projector IS the Blackstar Relic.** §6's three.js
  reference and the relic are the same object. The lore specifies it as
  a "star alignment projection / orbital prediction engine /
  gravitational anomaly visualizer... possibly Cybertronian dark-energy
  lensing tech". That is a functional spec for the console, not just a
  look: it predicts, it does not merely display. Build it as an
  instrument that shows where things WILL be.

- **C13-2h — the archaeology layer is already ahead of the archive.**
  Checked rather than assumed: v3's "historical strata" (Layers 0-5,
  discovered out of order) is implemented faithfully in
  `archive.rs::Stratum`, down to the out-of-order intent. Every named
  Layer 3/4 entity exists in the tree — Voltage, Dragon Pulse, the
  kinetic operative, Asteri, Witwicky. The one exception is
  **`Grimoire`: 0 hits anywhere.** v3 proposes it as the player's ship's
  name ("You could make the player's ship the Grimoire"), which is a
  suggestion rather than a correction, so it is left for the operator to
  accept or decline rather than named unilaterally.

  v3 also adds a thread with no counterpart in code: **source
  archaeology as a second layer** — the repo being open means the
  dependency graph, disabled content and commit history can themselves
  carry history a player digs through. Nothing in the tree does this.

- **C13-2i — WHICH corrections?** The operator described this archive as
  "a bit stale because there are some corrections in the docs". The
  staleness that IS verifiable is internal: v3 over v2, and the August
  addendum over the February trunk. If there are corrections in a
  *newer* doc set that never reached this archive, they have not been
  seen here — ask before treating anything above as current.
- **C13-2f — exengine's unported shaders.** Named by the operator as
  still missing from the C original. Never enumerated. Someone should
  diff the two shader sets and list what is actually absent.

## 2j. Replacing the inherited maps with planets — state of play

The 36 `MapStyle`s came from skydribble and the operator has said they
are no longer useful here (KUIPER keeps its physics, WYRMFIRE its
lighting; both stay available in skydribble for testing). Replacing them
with procedurally generated planets — invented systems and Sol — is the
direction. What is built, and what is blocked:

**Built.**
- `planetlook.rs` — a planet's palette DERIVED from its physics: star
  blackbody, Rayleigh against atmospheric pressure, published surface
  albedos, insolation. `SOL` carries Earth's system at real numbers
  beside `from_seed` for invented ones, the same real-beside-procedural
  split `exoworlds.rs` already uses. Audited: a thousand random seeds
  must all clear `valuestruct::MIN_STOPS`, which is the property a
  generator has to hold that a hand table only had to pass once.
- Angular LOD, wired (`d62e255a`). Was `d2 > 90²` for every shape
  whatever its size.
- Rescale-safe gravity, wired (`7da50dd4`). Was a literal reference that
  silently collapses every well to the clamp at planet scale.

**Blocked on a runtime, NOT on effort.**
- **C13-2j-1 — the floor under KUIPER.** `space.rs`'s third coupled
  change. Terrain is an always-present heightmap (`d = p.y -
  terrain_h`), so `amp: 0.15` gives an asteroid field a near-flat
  infinite plane. Filling the grid with a sentinel low value would make
  both the Rust SDF and the shader skip it with no shader change — but
  `ground_h` has **72 call sites** across spawning, camera clamps, bot
  pathing and physics, and every one would silently do something absurd.
  The shader's `skip_ter` is a hardcoded ray optimisation (`p.y > 17.0`),
  not a per-style flag, and hiding a floor the physics still collides
  with is worse than a visible one. This wants a decision about what
  "ungrounded" means for those 72 consumers, and play-testing.
- **C13-2j-2 — the far plane.** `far_plane_for` stays uncalled on
  purpose: it returns 525 m for today's field against `Camera::FAR_M`'s
  2600, and clipping the other 35 maps to 525 to fix an invisible
  problem is a regression. A guard now asserts the RELATIONSHIP instead,
  and fails when the field is scaled up without the far plane following.

**Blocked on the operator.** Two questions, asked twice, still open:
which KUIPER behaviour is the keeper (anchored worlds with real gravity
wells, or the five-minute black-hole collapse), and whether Sol should
use real orbital elements — `exoworlds.rs` establishes that a named body
uses catalogued numbers rather than a roll, which argues yes.

**Unexplained.** WYRMFIRE's frame rate. The blend-clustering hypothesis
was measured and disproved (cluster 7, mid-table; PERPLEXUS is 19).
Standing candidates, all unverified: `glow: 0.75`, the highest in the
table, with five emissive-0.95 lava decks. Its `wet` is 0.22, now under
`DRY_GROUND`, so the Fresnel fix already stopped it tracing the ground —
re-measure before hunting further.

## 3. Performance: 60 fps on an M2 Ultra against a 240 target

The terrain half is addressed (§2, dry maps stop tracing). The rest is
untouched and unprofiled.

- **C13-3a** — non-terrain reflections. Glass still takes the full march
  and `PERPLEXUS` is half glass by the shader's own comment. The
  `cheap_refl` threshold was widened to admit dry terrain but the
  glass path is unchanged.
- **C13-3b** — nobody has profiled anything. The frame-drop counter is
  honest (`presented`, not loop iterations) and the recording wandered
  36–59 fps; the spikes near the ship suggest object count, and the
  per-object dynamic-offset uniform path at 8192 objects is a suspect,
  but that is a guess. **Do not act on it without a capture.**

## 4. Audio: the shell is wired, the score is not

`mixer.rs` + `sound.rs` now place one-shots in space and muffle them
through hulls. What is left:

- **C13-4a** — none of it has been heard. First listen may show the
  occlusion constants are wrong; `DEFAULT_PER_UNIT_DB = 6.0` was
  reasoned, not tuned.
- **C13-4b** — unendlich has more that ports cleanly and was left on the
  table: `sim::music_director` (state→cue with crossfades),
  `sim::spatial_audio` stem layout, `sim::sound_masking`. The ship's
  rooms are the natural case for music bleeding through a bulkhead as
  you walk a corridor — the occlusion march already exists.
- **C13-4c** — only 4 of 16 `fire` sites are positional. Weapons fire,
  raid combat and the ship interior are all still centred.

## 5. Phone-as-mocap — designed, not started

Operator's idea: a Fairphone on postmarketOS as a capture device for
emotes, stylized walk overlays and weapon swing combos, bootstrapped
over the DHT. The design review found more in-tree than expected.

**The key reframe: two of the three uses are AUTHORING, not streaming.**
A walk overlay and a swing combo are record-once, bake-to-asset. Only
live emoting needs a wire, so the first build needs no network at all.

**Second: do swings with the IMU, not the camera.** You *hold* the phone
and swing it — accelerometer + gyro at ~200 Hz, no model, no lighting
dependency, no thermal cost. `sensor-kit` already reads IIO. Camera
mocap is the wrong tool for the one case where the phone is the prop.

**Third: the DHT is rendezvous, not transport.** `netcode::store`'s
`put_raw`/`get_raw` for a pairing record, then a direct socket via
`nat-kit` / `peer-handshake` / `peermux-core`. Pose frames through a DHT
would be badly wrong.

**Fourth: stream a pose VECTOR, not joint transforms.** 1454 chassis
include hovertanks and jets; "which bot bone is the human's elbow" is
undefined for most. `anim::Pose` is already the right target and is
half-finished — its `leg_l/leg_r/arm_l/arm_r` fields carry the comment
*"Nothing consumes these yet — but they are what a rigged bot would be
driven by."*

Already in-tree and relevant: `device-kit/source_plan` (libcamera vs
v4l2, phone vs desktop, with a field report), `hover-kit`
(proximity-gated vision policy, confidence gating, loss grace),
`hand-kit` (a labelled Fairphone capture set + detector scoring),
`gesture-kit`, `sensor-kit`.

- **C13-5a** — IMU swing capture → baked weapon-arc asset. No network,
  testable headless. Start here.
- **C13-5b** — `MocapFrame` → `anim::Pose` retarget as pure tested code
  against recorded fixtures. Best fit for a sandbox with no hardware.
- **C13-5c** — phone camera capture, reusing `source_plan` + `hover-kit`.
  **Honest risk: there is no body-pose model in the tree**, `hand-kit`'s
  ground truth is 41 frames of *hands*, and full-body estimation on an
  FP5 CPU under pmOS at a usable rate is unverified. Thermal throttling
  is real — `hover-kit`'s own docs warn a 30 fps front camera is watts.
- **C13-5d** — live emote over DHT rendezvous + direct UDP. Last.

## 6. The raid projector — reference in hand, nothing built

Operator supplied a three.js cosmic-web viewer as the visual reference
for the raid-selector projector: a ship room where six pilots gather at
a console and ping/vote the next raid target, feeding the JSON
AI-authorable raid system. The reference's transferable parts are
additive point splats with a heat ramp, distance fade, label chips that
fade by distance, and its escalating perf safety-valve. Nothing started.

## 7. HUD: the migration is half done, by design

`6d9475d2` fixed the in-game HUD and the `ui.rs` layout layer. The
hand-placed screens still share the bug class:

- **C13-7a — every LIST screen is now measured.** One table in
  `hud::tests::a_settings_row_of_text_fits_in_a_settings_row_of_space`
  covers settings, deck, controls, trials and academy: a row of text
  (`font::GH * T_BODY` = 0.0574) must fit its row of space with 5% clear.
  Two were failing that — **CONTROLS and TRIALS at 0.058, about 1% clear
  or a third of a pixel at 1080p**. Not overlapping, but no visible gap
  either, so the list read as a block. Both raised to 0.062.
  `ui_k` clamps at 1.0, so these are worst-case margins.

  **A REAL OVERLAP WAS FOUND ON THE CHASSIS CATALOGUE.** `catalog_menu`
  draws at `T_BODY * ui_k(a)`, and `ui_k` clamps to 1.0 for any aspect at
  or above 1.70 — so at 16:9 a 0.0574 glyph sat in a 0.052 row and the
  rows overlapped, on every normal widescreen display, on the screen you
  pick a bot from. Raised to 0.062. It survived two sweeps because the
  constants were not in the table AND `catalog_menu` did not use them —
  it repeated the literals, so `CAT_ROW_H` and the draw agreed only by
  coincidence. `no_row_pitch_escapes_the_clearance_table` now scans for
  both: an untabled pitch, and a draw that has gone back to a literal.

  What is still unmeasured is the screens that are NOT uniform lists —
  the pause overlay, the garage and the map picker place labels at
  hand-chosen coordinates, which is where the rasteriser is the right
  tool and an inequality is not.

- **C13-7a** — pause / garage / tweaks / academy / maps use `row_pair`
  at pitches authored for the wrong metric, and each has a mouse
  hit-test mirror in `main.rs` that must move with it. These want the
  panel migration, not another round of hand-retuned literals.
- **C13-7b** — an active trial/lesson banner and the combo readout can
  still both occupy top-centre. Neither knows about the other.

## 8. Two habits this session earned the hard way

- **A test written after the fix proves nothing until you put the bug
  back.** Four of the ~19 tests added this session did not fail against
  the regression they were supposed to guard. The worst: a pan test
  asserting `power == gain²`, which is satisfied by dumping every sample
  into the left channel — it passed against a pan that did not pan.
  Another: a whole-table bound of 0.70 that the reverted formula slid
  under. Both were rewritten to assert the mechanism, not a side effect.
- **When a test fails and you expected it to pass, suspect the test's
  MODEL before its threshold.** `valuestruct.rs` first measured value
  separation as a linear luminance difference, which declares every dark
  scene flat and every bright one structured — backwards for this game.
  The failing tests were right; the metric was wrong. It is log₂ ratio
  now. Same shape as CLAUDE.md §5b's corollary, one level up: not a bad
  assertion, a bad quantity.

## 9. rustcheck false positives, and the real bugs they were hiding a real one behind

A `./verify.sh` sweep flagged four projects as PROBLEMS. Three were the
checker, not the code — all in `resolve_mod_files()`:

- Attribute lookback checked only `lines[n-2]` for `#[path]`/`#[cfg]`.
  Every real `#[path]`-redirected mod in this tree also carries a second
  attribute (usually `#[allow(...)]`) directly above the `mod` line, so
  the lookback landed on that and missed the `#[path]` above it. This
  alone was voiceterm's 10 warnings and krapper's 2.
- Nested inline `mod outer { mod inner; }` resolved `inner` against
  `<file>/inner.rs` instead of `<file>/outer/inner.rs` — ambient-learner's
  `egui_lib.rs: pub mod ui { pub mod egui; }`.
- A file that is a crate root only via Cargo.toml `[lib] path = "..."`
  (not named main.rs/lib.rs) was still resolved as a plain submodule dir
  — same ambient-learner file, root-gated this way specifically because
  `required-features` is silently ignored on `[lib]`.

Fixed in `b3d0e09f`, tests in `339707d7`.

**The fourth, rusty-board's `mod layouts;`, was real** (`42c83d28`):
main.rs still declared a plain `mod layouts;` pointing at a file that
stopped existing when the layout layer moved to `crates/board-layouts`;
lib.rs was updated at the time (`pub use board_layouts as layouts;`) and
main.rs, a separate crate root, was not. Fixed to match. Unverified by
compilation — no GTK4 in this sandbox.

## 10. snapshot-cam: `.show(ui.ctx(), ...)` was building a second CentralPanel

Not a deprecation nit. eframe 0.34 made `App::ui(&mut self, ui: &mut
egui::Ui, ...)` the required entry point — it already hands you a `Ui`
for the frame's content area, and eframe's own doc example draws into it
with `CentralPanel::default().show_inside(ui, ...)`. snapshot-cam instead
pulled the `Context` back out of that `Ui` and called the deprecated
top-level `.show(ui.ctx(), ...)`, which builds an independently
laid-out second CentralPanel nested inside the one eframe already gives
you. The comment already on this code correctly diagnosed why the OLD
shape didn't compile under eframe 0.34, but the fix that landed used the
wrong replacement method — `.show(ctx, ...)` still typechecks (deprecated,
not removed), so nothing forced the second half of the migration.
Fixed to `.show_inside(ui, ...)` (`a71eca64`). Layout correctness itself
is unverified — no display in this sandbox.

## 11. tomescope: two navigation mechanisms built, neither wired — not touched

`cargo-ailite check` on apps/tomescope reports `textures`, `nav` and
`prev_screen` on `TomeApp` as never read — confirmed by grep, not just
the lint: none of `self.textures`, `self.nav`, `.nav.` appear anywhere
outside their own construction. `nav: Nav` (`uikit_egui::Nav`, the shared
page+subpage state machine with `go_page`/`push_subpage`/`back`/
`title_bar`) is constructed with `Nav::new(4)` and never called again.
`prev_screen` is set once at init to `Screen::Library` and never read or
reassigned. Navigation in this app is instead entirely explicit
`self.screen = Screen::X` assignments (a handful of "Go to Library" /
"Go to Stats" commands plus a couple of hardcoded returns to Library on
closing the reader) — there is no "go back to whatever was open before"
concept anywhere for `prev_screen` to serve, and no drill-down UI for
`Nav`'s subpage stack to track.

**Not fixed.** This is the C12_TODO pattern of "component built
carefully, tested [transitively, via TomeApp's own struct], and never
called" (§Session summary, C12_TODO.md) rather than a bug with an
obvious mechanical fix — deleting the fields is safe compile-wise but
guesses that the intended feature (probably: a real back button, and/or
drill-down into flashcard decks or library shelves via `Nav`) was
abandoned rather than just unfinished, and TextureCache in particular
(cover-art texture caching, from `library.rs`) sitting unused suggests
cover thumbnails may simply not be wired into the library screen yet
rather than the whole field being vestigial. Reading `draw_library_screen`
and `library.rs`'s `TextureCache` API against whatever the pre-egui-port
predecessor did with cover art would settle which of the two it is;
that reading was not done this session.

`egui::CentralPanel::default().show(ctx, ...)` also appears 9 times
across `main.rs`'s `update()` — genuinely fine, not a repeat of §10's
bug, because `update(&mut self, ctx: &egui::Context, ...)` only ever has
a bare `Context`, never a `Ui`, so `show_inside` isn't available to it.
eframe still calls both `App::update` (deprecated, default no-op) and
`App::ui` (required) every frame in that order, so this compiles and
runs; it is technical debt (migrate the `update()` body into `ui()`,
which touches every `draw_*_screen` call in the match) rather than a bug,
and out of scope for a warnings-driven pass. Left as-is; not silenced
with `#[allow(deprecated)]` either, since that would hide the next
person's reason to do the real migration.

## 12. Correction to §10: the show/show_inside deprecation is not a rendering bug

§10 (this file) characterized snapshot-cam's `.show(ui.ctx(), ...)` as
"building a second, independently laid-out CentralPanel" — that's
overstated. Reading egui 0.34's panel.rs closely: the root `Ui` eframe
hands `App::ui()` and a top-level `Panel::show(ctx, ...)` call both
target `LayerId::background()` and cooperate through Context-level rect
bookkeeping (the same mechanism panels have always used to divide up
screen space across multiple `.show(ctx, ...)` calls in one frame). The
root Ui doesn't draw anything of its own in either app, so there's no
actual conflict — this renders correctly, not corrupted.

The real, narrower reason to prefer `show_inside(ui, ...)` from inside
`ui()`: the root Ui's own `available_rect_before_wrap()`/`min_rect()` get
recorded into pass state at the end of `run_ui_dyn` for the NEXT pass,
and if the app never advances the root Ui's own cursor (because
everything is drawn via ctx-based top-level panels instead), those
values describe an empty Ui rather than the actual on-screen content.
Plausible knock-on effects: window auto-sizing, or anything else reading
that bookkeeping — unconfirmed, not chased further.

Net: the snapshot-cam fix (`a71eca64`) is still correct and worth
keeping — it matches eframe's own documented pattern and cannot make
things worse. But it should have been described as a forward-looking
correctness/API-migration fix, not "fixes a rendering bug", and that
overclaim is why studio-app's six identical-looking `.show(ctx, ...)`
call sites were deliberately NOT migrated the same way this session
(`efbc3783`) — that file's `ctx` extraction is a documented, deliberate
choice threaded through much more code than six panel calls, and the
risk of a wider unreviewed refactor outweighs a correctness benefit that
turned out to be smaller than first claimed.

## 13. studio-fx: Halftime has a documented fade mechanism it never calls

`cargo-ailite check` on apps/studio flags `Halftime::STOPPED`,
`Halftime::FADE_FROM` and `Halftime::stop_gain` as dead code in
`crates/studio-fx/src/playrate.rs`. Confirmed by grep, not just the
lint: `Halftime::process()` never calls `stop_gain`, and no test
exercises stop/fade behavior for `Halftime` (only for its sibling
`TapeStop`, which does ramp a `rate` down to 0 and IS tested ending in
silence).

The doc comment on `STOPPED`/`FADE_FROM` is unambiguous about intent —
"the rate at which the level begins fading toward it" — but `Halftime`'s
`process()` doesn't have a continuous decelerating rate the way
`TapeStop` does; it does a fixed jump-back-and-read-at-half-speed every
window, with no notion of "rate_now" to feed `stop_gain`. Two readings:
(a) this is copy-paste scaffolding from `TapeStop` while `Halftime` was
being written, abandoned because the mechanics don't map, and should be
deleted; or (b) `Halftime` is meant to eventually decelerate to a full
stop (matching "the trap halftime" sound design more closely — a beat
repeat that runs down rather than repeating forever) and the wiring was
simply never finished.

**Not fixed.** This is real-time audio DSP with no way to verify "does
it still sound right" in this sandbox beyond the existing crossing-count/
RMS-style tests, and picking (a) or (b) wrong either deletes a documented
intentional feature or half-wires audio behavior nobody asked for and
nothing tests. Reading whatever pre-port reference (if any) inspired the
"trap halftime" comment, or asking whoever wrote it, would settle which.

## 14. bot-arena: a whole git-history-mining subsystem, unwired and untested

`cargo-ailite check` on apps/bot-arena/egui-frontend (which pulls in the
main `bot-arena` lib) flags 14 functions in `src/engine.rs` as dead code.
One (`sim_fitness`, line 326) is isolated. The other thirteen form a
single coherent, self-contained pipeline clustered at lines 2599–2986:

`extract_filepaths`, `weighted_vocab`, `tokens`, `diff_metrics`,
`git_recent_commits`, `git_commit_diff`, `changed_lines`,
`diff_similarity`, `diverse_commits`, `best_match`, `progression_refs`,
`lcs_lines`, `merge_versions`.

Read together they're a git-diff-based genome merger for whatever
evolutionary/genetic-programming loop drives this arena: pull recent
commits, tokenize and diff them, score similarity/diversity, pick a
diverse reference set, and merge two "versions" of code by walking their
LCS anchors and coin-flipping which side's hunk wins between anchors.
Confirmed zero callers anywhere — not `src/`, not `egui-frontend/`, not
even a test (`grep` across both trees, not just the lint).

**Not touched.** This is a bigger, more self-contained candidate for
wiring than tomescope's Nav or studio-fx's Halftime — it's pure
string/git logic (no audio/UI feel to misjudge) and 13 functions is
enough surface that SOME integration point in the arena's
mutation/selection loop plausibly wants exactly this. But guessing where
to call `merge_versions` from without understanding the rest of the
arena's evolution loop risks bolting on a feature nobody asked for, with
no test to confirm the wiring is even correct. Worth a dedicated look:
find where the arena currently does mutation/crossover (if there's a
simpler existing scheme it uses instead) and decide whether these
supersede it, complement it, or were an abandoned direction.

Fixed in the same pass, no risk: a stray `mut` on a closure in
`merge_versions` that's never reassigned (`6777efa4`) — trivial and
independent of whether the function itself ever gets called.

## 15. apps/snapclone/rust did not compile at all — five independent errors, now fixed

Unlike every other app swept this session (warnings at worst),
`apps/snapclone/rust` — a ~40-crate workspace — failed to `cargo check`
outright. Five genuinely independent compile errors, in five different
crates, none related to each other except by being in the same
workspace and none previously caught (this workspace has no CI signal
visible to `verify.sh`, which is source-tree-static and doesn't compile
anything). Fixed in five separate commits (`5c4cc8c1`, `a966c1b6`,
`414a3fe7`, `a094f6f4`, `6bd4ad72`, `9a6b8673` — the last bundles three
missing-dependency fixes together since they're the same class of bug):

1. **snapclone-storage**: a local variable named `display` collided with
   `tracing::field::display` inside the `debug!` macro's own expansion —
   confirmed with a from-scratch repro outside this repo, so this is a
   general `tracing` macro-hygiene gotcha (never name a local `display`
   or `debug` near a tracing macro call), not specific to this code.
2. **snapclone-core**: `AppCore::new` moved `evt_rx` into `Self` and then
   used it again in the return tuple. The struct field was simply wrong
   — nothing ever reads `self.evt_rx`, and the crate's own doctest says
   the caller owns it.
3. **snapclone-session**: an `if let Some(u) = relay_url` pattern against
   a plain `String` field that could never type-check as written — dead
   code from (probably) an earlier version where the field was
   `Option<String>`. Also found and removed a genuinely-dead duplicate
   `NetworkBin::of(ip)` computation right next to a `hints_for()` call
   that already computes the identical value internally.
4. **snapclone-piggyback**: `[u8; 64]` under `#[derive(Serialize)]` —
   serde's array impls are a fixed enumerated list topping out below 64.
   `snapclone-mailbox` hit this exact wall earlier and already
   established the fix (hex-encoded `String` instead of a raw array);
   applied the same pattern here rather than inventing a new one.
5. **snapclone-vcard**: `Vec<&str>::first()` gives `Option<&&str>`, and
   `.to_owned()` on that resolves to the blanket `impl<T: Clone> ToOwned`
   at the outer `&str` level rather than descending to `str`'s `ToOwned
   -> String` impl — `.to_string()` (via `Display`) resolves through the
   reference correctly instead.
6. **snapclone-relay / snapclone-databus / snapclone-network-diag**:
   three separate missing-Cargo.toml-dependency/feature-gate bugs
   (`base64`, `futures-util`, `chrono` never declared; a binary using an
   optional dependency with no `required-features` gate on its `[[bin]]`
   — the same silently-broken-by-default shape CLAUDE.md §3 already
   documents for `[lib]` targets, just on `[[bin]]` instead).

None of these are related to each other in cause — the common thread is
just that this workspace had apparently never been fully `cargo check`'d
end to end before. `cargo-ailite check` over the whole workspace now
finishes clean; targeted `cargo-ailite test` runs on every touched crate
(23 + 12 = 35 tests across snapclone-storage/core/session/piggyback/
vcard/network-diag/relay/databus) all pass, 0 failed. The full
`cargo-ailite test` across the ENTIRE workspace (all ~40 crates,
including heavy ML deps like tract-onnx) was not run to completion in
this sandbox — it exceeded a 280s budget mid-compile without erroring,
just slow. Worth a dedicated longer run next session to be sure nothing
outside the six crates touched here regressed.

## 16. skydribble: 10 remaining dead-code findings, not chased individually

After the fixes in `135dbb61`/`387c91db`/`1e23a0c1` (duplicate CLI arms,
a misplaced/never-checked Fairphone-vs-desktop platform-classify
diagnostic, two computations superseded by documented rewrites, and
assorted `mut`/unused-var mechanics), `cargo-ailite check` still reports
10 warnings, all genuine dead code with no nearby comment explaining
whether each is superseded, unfinished, or speculative:

`terrain_mesh` (main.rs, unused fn), `GFX_TIERS` (config.rs, unused
const — but the game clearly has graphics tiers somewhere, since
`VideoCfg` exists; worth checking if tier selection reads a different
list), `VideoCfg::ray_dist` (unread field — sounds ray-tracing/reflection
related, might be a stub for a feature not yet built), `Source::label`
(gyro.rs, unused method — likely meant for a settings UI that doesn't
call it), `Trick::Backboard` (replay.rs, unconstructed enum variant —
one trick out of presumably several that the detector doesn't fire for
yet), `Session::done` (academy.rs, unread field — sounds like tutorial
completion tracking that nothing reads back), `Analyzer::summary`
(style.rs, unused method), `Flick::hint` (sim.rs, unread struct field —
probably meant for on-screen text that isn't drawn), `Body::{scale,
rot, spin}` (world.rs, unread fields on what's presumably meant to be a
general transform+physics body type), `World::surface_below` (unused
method — sounds like it answers "how far to the nearest surface below
this point," plausibly meant for spawn placement or fall-through
prevention that doesn't call it yet).

Ten unrelated findings is enough that most are probably genuinely
per-feature unfinished work rather than one pattern, unlike bot-arena's
13-function cluster (C13_TODO §14) which reads as one coherent unwired
subsystem. Logged rather than triaged one-by-one — this is real-time
game code with no way to verify "is this the right fix" without playing
it, same caution as the physics deletions in this same pass.

## 17. apps/snapclone/rust and system/snapclone/rust are ~36,000 lines of near-duplicate workspace

The biggest duplication found this session, by a wide margin. `diff -rq`
across the two full ~40-crate workspaces (apps/snapclone/rust: 36,833
source lines; system/snapclone/rust: 36,022) returns only 20 differing
files total, and only 2 files that exist in one tree and not the other
(both in `apps/`: `snapclone-presence`, the whole crate, and
`snapclone-group/tests`). Everything else — every crate name, every
module, essentially the entire dependency graph — is the same code
forked at some point and never reconciled.

**Confirmed independent convergent bug fixes.** All five of the real
compile errors fixed in `apps/snapclone/rust` this session (commits
`5c4cc8c1`, `a966c1b6`, `414a3fe7`, `a094f6f4`, `6bd4ad72`, `9a6b8673` —
see §15 above) were found ALREADY FIXED in `system/snapclone/rust`,
each with its own comment independently describing essentially the same
root cause:

- `snapclone-storage/db.rs`'s tracing-macro `display` collision — fixed
  there as `display_name` (I used `shown_name`; same bug, different name).
- `snapclone-core/app.rs`'s double-owned `evt_rx` — fixed there with a
  comment reading almost like a paraphrase of mine: "A struct field
  storing a value that's also returned is a compile error... fixed by
  not double-owning it."
- `snapclone-relay`'s missing `base64`/`futures-util` deps — fixed
  there, with a comment noting the crate "had never compiled."
- `snapclone-databus`'s missing `required-features = ["bin"]` gate — the
  fix comment explicitly compares it to "the same class of bug the rest
  of the fleet guards against."
- `snapclone-network-diag`'s missing `chrono` dep — fixed there too.

(`snapclone-piggyback`'s `[u8; 64]` serde issue, `snapclone-session`'s
type-mismatch, and `snapclone-vcard`'s `&&str` ToOwned issue weren't
individually diffed here but the same pattern almost certainly holds —
worth confirming.)

**What this means:** `system/snapclone/rust` is the ahead, working copy;
`apps/snapclone/rust` was the stale fork that had drifted behind and
accumulated the now-fixed bugs independently. The two fixes are
functionally identical but textually different (variable naming,
comment wording), so a naive `diff`-and-overwrite in either direction
would lose real information from whichever side is discarded, and doing
it by hand file-by-file for ~40 crates is exactly the kind of tedious,
error-prone work a dedicated pass should do carefully rather than as a
side effect of a bug-hunting session.

**Not attempted here — this needs its own session.** Unifying two
~36,000-line, ~40-crate workspaces safely requires: deciding which
directory is canonical (or extracting a genuinely shared crate set that
both `apps/` and `system/` path-depend on, mirroring the fix just
applied to skydribble/blackstar-rs's vendored crates, but at roughly
30x the scale); reconciling `snapclone-presence` and
`snapclone-group/tests` (apps-only) into whichever side wins; and
verifying the full test suite on both sides post-merge, which this
session was never able to complete for even one side within the
sandbox's timeouts. Flagging this as the single largest LOC-reduction
and compile-speedup opportunity found this session — bigger than the
skydribble/blackstar-rs vendor fix (11,656 lines) by more than 3x — but
explicitly NOT something to attempt without a dedicated session and a
plan for verifying both call sites (whatever consumes `apps/snapclone`
as a GUI app vs whatever consumes `system/snapclone` as a daemon) still
work after unification.

## 18. scene-kit: spatial/textsurface migrated, the rest confirmed still diverged

Following up on shared/scene-kit's own lib.rs doc comment ("logscope
still carries its own copy; it migrates here at the next scene
co-change"): content-hashed every file and found `spatial.rs` and
`textsurface.rs` were byte-identical between logscope-egui and
scene-kit, with no cross-dependency on scene-kit's other modules.
Migrated those two (commit `2d1741ca`) — logscope-egui now consumes
`scene_kit::{spatial, textsurface}` via `pub use` instead of its own
copies, -656 lines, verified compiling clean.

Checked the other four modules scene-kit's own doc names as diverged
(mesh3d, gpuscene, texgen, glow_scene) rather than assuming: `diff`
against logscope-egui's copies shows 100–360 changed lines each,
confirming they're genuinely not safe for the same mechanical swap.
Reconciling those needs someone who can verify the visual/rendering
result (per-vertex UVs, the inscription material, the wired `u_touch`
response) actually still looks right in both consumers after merging —
not a compile-time-only check like the two modules just migrated.

## 17b. apps/snapclone/camera-bridge vs system/snapclone/camera-bridge: same drift, smaller scope

Noted 2026-09-01 while reusing its probe: `capture.rs::parse_camera_nodes_from_json`
computes the backtrack window from a `search`-relative offset but slices
`json` with it, so after the first `Video/Source` match the window for
later nodes points at the wrong region. It worked in the field because the
first node was the one wanted. The frontend's `camera_enumerate` scans
structurally instead; port that back when the two crates are reconciled.

A follow-up check on §17: `camera-bridge` is a small, standalone
workspace (`[workspace]` with no members, deliberately kept out of the
big rust/ workspace — its own Cargo.toml explains why: it drags in
gstreamer/tract-onnx that a UI crate shouldn't link). At ~4,500 lines
across 6-7 files, `diff -rq` shows it's almost entirely identical
between `apps/` and `system/` — except `view.rs`, where `apps/` is
meaningfully AHEAD, not just textually different:

- **Desktop-webcam fallback**: `apps/`'s version tries an ordered list
  of camera-source candidates (libcamera pipewire nodes, then v4l2
  devices) via a new `source_plan`/`capture` module pair, falling
  through to the next candidate on failure. `system/`'s version still
  builds one hardcoded pipewire pipeline string and has no fallback —
  exactly the failure mode the newer code's comments describe fixing
  ("the pipewire candidate fails fast on a desktop and the v4l2 one
  behind it takes over").
- **Phone GPU crash fix**: `apps/`'s version clamps wgpu device limits
  to what the adapter actually supports (`device_limits`), with three
  unit tests pinned against the specific failure it fixes
  (`LimitsExceeded... max_color_attachments: requested 8, allowed 5` on
  a phone GPU — the same crash the comment says logscope also hit on
  Asahi). `system/`'s version has none of this and would hit the same
  crash on that hardware profile.

So this isn't the same "which side happened to fix bugs first"
situation as §17 — here `apps/snapclone/camera-bridge` is unambiguously
the more-correct, more-complete version, and `system/snapclone/
camera-bridge` is missing two real fixes for hardware failure modes
(not fixed independently on both sides this time). Given the standalone
workspace's small size, this one is a much more tractable candidate for
a future session to actually reconcile (likely: copy `apps/`'s view.rs,
source_plan.rs, and capture.rs additions into `system/`'s copy, or — if
`system/snapclone` still exists as a maintained target — better to
retire it in favor of a single canonical camera-bridge referenced by
both, same shape as the shellkit/scene-kit/peek migrations done
earlier this session). Not attempted here: still needs someone who can
verify camera-bridge/pipewire behavior on real hardware (a phone GPU
and a desktop webcam) rather than compile-check alone.

---

# Session 2026-09-01 (late): hard dedups + field-report backlog

## 19. voiceterm real_pty: zsh prompt redraw garbles the grid (root cause NOT fixed)

Fleet failure: `a_tab_lands_on_its_column_stop` and
`colour_codes_from_a_real_program_leave_nothing_on_the_screen` both fail
with a screen showing only `➜  ~ =` and no command output.

Reproduced locally by pointing `$SHELL` at a themed zsh (the sandbox
default has `$SHELL` unset, so `PtyHost::spawn` falls back to /bin/sh and
all three tests pass — which is why this never showed up here before).
Two things were found and fixed along the way (commit `0c012e0c`): OSC
sequences were being typed onto the grid as literal text, and the test's
fixed-duration sleeps were too short for a slow login shell. **Neither
fixes this failure.**

The actual cause, traced by dumping the raw pty bytes and feeding them
through `Parser` directly: zsh's line editor draws its prompt with a
reverse-video placeholder (`ESC[1m ESC[7m # ESC[27m ...`), a run of
spaces to the terminal width, then `\r`-and-redraw, plus `ESC[J` / `ESC[K`
erases and `ESC[?1h ESC= ESC[?2004h` (DECCKM + keypad-application +
bracketed-paste). Feeding the captured 436-byte real sequence into the
parser produces interleaved garbage — `➜/➜`, ` e`, ` t`, `~c~` across
successive rows — i.e. the cursor ends up in the wrong place and
subsequent output lands scattered instead of on one line. So the grid is
mis-tracking the cursor through some combination of the erase-in-display /
erase-in-line / carriage-return / wrap interaction, not merely printing
stray characters.

Fixing this properly means auditing `Grid`'s cursor model against a real
VT spec for: `ESC[J`/`ESC[K` with implicit param 0, the deferred-wrap
("phantom column") state after writing the last column, and what `\r`
does to the pending-wrap flag. That is a genuine terminal-emulator
correctness task with real regression risk to every app embedding
uikit-term, and it wants its own session plus a corpus of captured real
shell sessions as fixtures — not a quick patch. It is also very likely
the same underlying defect as the "voiceterm panes can't be used like
konsole" field report (§20), so it is worth doing once, properly.

## 20. voiceterm-egui panes still unusable as a real terminal

Standing field report, still open. Almost certainly the same root cause as
§19 — the grid mis-tracks the cursor through a real interactive shell's
prompt redraws, so anything beyond simple line-at-a-time output smears.
Do §19 first; re-test this after.

## 21. logscope as an agent harness: no time awareness, small models loop

Field report with a captured transcript. Two distinct problems:

**No time awareness.** The assistant has no notion of elapsed time or of
the current date. Transcript shows "keep working for the next day" being
accepted and then answered immediately, and the fan status line
(`189 rounds left`) counting rounds rather than time. Minimum fix: inject
the current timestamp into each turn's context and surface wall-clock
elapsed in the fan status, so a model can at least know that time passed
between turns.

**Small models loop instead of doing work.** The transcript is a
llama-class model asked to "rewrite smite in bevy and rust" producing
four nearly identical "Scene Management" sections in a row, each
restating the same non-compiling skeleton, drifting into "Beavy", and
never touching the actual repo. The harness let it run ~10 minutes of
that unchecked. This is not fixable by prompt alone; the harness needs
loop detection (e.g. n-gram overlap against the previous k assistant
turns, or a repeated-heading check) that halts the fan and reports
"model is looping" rather than burning the round budget. Also worth
gating: a round that produces no tool call and no file edit should not
count as progress.

## 22. gitmind-egui fleet management never connected to the Fairphones

Field report: openssh-server installed via apk on the phones, screens on,
same wifi, but the devices never appeared, so cross-compilation to them
could not be tested at all. Nothing was verified end-to-end here — the
discovery path, the SSH path, and the cross-compile path are all
untested against real hardware. Needs a session with the devices in
hand and, first, a way to see WHY discovery fails (does it broadcast? do
the phones answer? is it mDNS, a fixed port scan, the DHT rendezvous?).
Right now there is no diagnostic output to reason from.

## 23. Camera enumeration on postmarketOS is wrong in several ways

Field report from a Fairphone. All of these are separate:
- Device names (`qcom-iris-decoder`, `qcom-iris-encoder`, `msm_*`) render
  as a flat list that **runs off the page**; should be a dropdown.
- The decoder/encoder nodes are not cameras at all and should be filtered
  out of a camera picker, or at least sorted below real capture devices.
- Default selection is wrong: it lands on `/dev/video0` and **hangs**.
- Advertised format is 640x480 at a low frame rate on a 4K sensor, so
  format negotiation is either not querying or picking the first entry.
This overlaps the `apps/snapclone/camera-bridge` source_plan work
(C13_TODO §17b) — that crate already has ordered-candidate probing that
tries libcamera pipewire nodes then v4l2 and falls through on failure.
Whatever fix lands should reuse that, not reinvent it.

**2026-09-01, partly done.** Root cause of the first three items was one
thing: `camera_enumerate` read `/dev/video*` only and never asked PipeWire,
while `Device::id` was documented as the PipeWire node name. On the FP5 the
sensors are libcamera devices with no v4l2 capture node, so the list WAS
the decoder/encoder nodes. `camera_enumerate::pipewire_devices` now runs
`pw-dump` (the bridge's own probe) and feeds every `Video/Source` node to
`camera_select::device_from_node`; the bridge's `virtual-cam-*` outputs
rank first, the raw sensors follow, and the v4l2 read is the fallback for
a machine with no session. Pinned on an FP5-shaped dump fixture. NOT yet
verified on the phone. Still open: the picker as a dropdown (§24), and the
640x480-on-a-4K-sensor format negotiation — `pipewiresrc` takes the
session's default format unless the pipeline states caps, and
`device_kit::source_plan::pipewire_pipeline` states none.

## 24. Layout/overflow bugs across apps (Fairphone AND desktop)

- **Settings panels flow over the bottom navbar and over each other**
  instead of establishing bounds and scrolling. Both platforms.
  **snapclone: done 2026-09-01** — one ScrollArea at the page dispatch,
  keyed by page (aec364ac), after the gesture rule learned that a
  container yields to the page along the swipe axis (614718e9). Other
  apps: same fix applies wherever a page is a bare stack; not done there.
- **logscope portrait at 300% scale overflows left and right.** 200% is
  "a bit better". 300% is common on these compositors, so it is the case
  to design for, not an edge case.
- **logscope landscape: the bottom bar is cut off** — the app overflows
  past the bottom rather than splitting content into sections that fit.
  The suggestion in the report is good and worth following: use the
  dashboard-metrics machinery in reverse, to turn one page that does not
  fit into either a page of elements that do fit, or appropriately-named
  sub-tabs.

## 25. logscope Models tab: trusted-devices field takes one character

Another instance of the long-standing single-letter input bug (see the
earlier entry on this). Workaround used in the field was pasting. Also
reported alongside it: after pasting a device key, the desktop machine
could not chat or list models and was labelled
`added ejz2w7sg2wmnab0e as a BUILD worker — it may see source, not
conversation. Unverified until it announces itself with a key; raise it
then.` So the trust/announce handshake never completed. Two bugs stacked:
the input field, and the announce path.

## 26. aperture / studio: unfinished open flows

- **studio** does not use a native file picker for `.studio` files.
- **aperture** cannot browse UP into higher-level Rust projects, and gets
  stuck in Python once a python test script inside a Rust crate is opened
  (the project-type detection latches and never re-evaluates).
- **aperture** still has no tap-and-hold gesture for the command palette.

## 27. voiceterm VT correctness — konsole is now vendored as the reference

Supersedes the "audit against a real VT spec" note in §19. A konsole
checkout was supplied specifically for this. The parts to read, in order:

- `src/Screen.cpp :: displayCharacter` — the deferred-wrap model, and its
  comment states the rule uikit-term is most likely getting wrong:
  *"VT100 does wrapping BEFORE putting the character... We indicate the
  fact that a newline has to be triggered by putting the cursor one right
  to the last column."* uikit-term has a "phantom column" notion but it is
  not clear it defers the wrap the same way, and this is exactly the state
  that a `\r` in the middle of a prompt redraw interacts with.
- `src/Screen.cpp :: clearToEndOfLine` / `clearToEndOfScreen` /
  `eraseChars` / `clearImage` — the erase family, with konsole's handling
  of the implicit param-0 case.
- `src/Screen.cpp :: setLineProperty` and the `wrapped` line flag —
  konsole tracks wrappedness PER LINE, which is what lets it walk back
  through a wrapped run. uikit-term has no equivalent, and reflow was
  already noted as out of scope in `Grid::resize`.
- `src/Vt102Emulation.cpp` for the parser table.
- `src/autotests/ScreenTest.cpp` and `Vt102EmulationTest.cpp` — the shape
  of the fixtures to copy. Capturing a few real zsh/bash prompt-redraw
  sequences as fixtures (the method that found the bug: dump raw pty bytes
  to a file, feed them to `Parser` directly, diff the grid) is the way to
  make this tractable.

The concrete reproduction is already written down in §19: a themed zsh
prompt redraw produces interleaved garbage (`➜/➜`, ` e`, ` t`, `~c~`
across rows). Fix that and re-test §20.

## 28. logscope wgpu backend should be one importable crate, not per-app

Requested. `apps/logscope/crates/gpu-gemm` is a real GPU compute engine
(kernels, memory planning, residency probing, sparse/streaming) that other
apps will want and that will not benefit from diverging. Today it is a
logscope-internal crate reached through `logscope-egui`'s `backend.rs`,
which mixes engine concerns with that app's UI state.

Shape to aim for, consistent with what worked for shellkit/scene-kit/peek
earlier this session: promote `gpu-gemm` to `shared/` (name it for what it
does, not for logscope), and put the API other apps consume behind a
narrow trait — load a model, submit a prompt, stream tokens, report
residency — so an app does not link the kernel internals. `backend.rs`
becomes one implementation of that trait rather than the only door.

Do this AFTER §29, because the residency/estimator work touches the same
files and merging two refactors of gpu-gemm would be miserable.

## 29. Small models refused on a machine with room — the f32 embedding term

Reported: llama3.2-1b-instruct at q2_k and IQ1_S (~500 MB files) refused
on a Fairphone with 8 GB and free memory, against a ceiling the message
attributed to a completely different machine.

Half fixed. The message now names its own provenance (commit in this
session) so "which machine does this number describe" is answered in the
message. What is NOT fixed is the arithmetic, and the prime suspect is
named in the new message text:

`store::estimate_resident_for_load` prices `token_embd.weight` and
`output.weight` at **4 bytes per parameter unconditionally** — see
`is_f32_kept` in `estimate_resident_from_tensors`. For llama-3.2-1B that
is 128256 x 2048 x 4 = ~1.0 GiB for the embedding alone, i.e. more than
the entire q2_k file weighs on disk, and it dominates `plan_q4()` for any
small model with a large vocabulary. Modern small models are exactly the
case where vocab dwarfs the hidden size, so this term is wrong in the
worst place.

To settle it, someone needs to answer the loader question I could not
from reading: does the loader actually keep those two tensors at f32, or
does it hold them at the file's quantisation like the rest? If the
latter, the estimator should price them from `type_nbytes` the same way
the IQ1_S native path already does, and the refusal disappears on its
own. If the former, the fix is in the loader, not the estimator.

Also worth checking while in here: `resident_budget_bytes` uses
MemAvailable, which is right, but on a phone under memory pressure that
can be far below MemTotal — a model that fits in RAM overall can still be
refused. Whether that is correct behaviour or too strict is a judgement
call, but it should be a deliberate one.

## 30. logscope networking: join from a phone, drive models on the desktop

Requested end state: open logscope on the Fairphone, join the desktop
(M2 Ultra), type, and choose which models run there. Blocked in at least
three separate places, all already recorded:

- §22 — gitmind fleet discovery never found the phones at all, and there
  is no diagnostic output explaining why. Whatever discovery logscope uses
  should be made to SAY what it tried.
- §25 — the trusted-devices field took one character. Fixed this session
  (stable widget ids), so pairing should at least be typeable now.
- The announce/verify handshake: a machine added by id alone is capped at
  `Trust::Build` and stays "Unverified until it announces itself with a
  key". The reporter got exactly that message and then could neither chat
  nor list models. So `register_keyed` — the path that raises a peer past
  Build once it presents a key — is either not wired to the announce
  listener or the announce never arrives. That is the next thing to trace,
  and it is testable without hardware: two processes on one host, one
  registering the other by id, then announcing.

Model SELECTION across machines is a further step behind that: `hive.rs`
has the placement matrix and `workerkit.rs` the capacity model, but
nothing yet lets a phone pick which of a remote box's models to load.

## 31. switcheroo: field observation — connecting alone switches the plug off

Reported after the first switcheroo commit: connecting to the WSP100 over
plain Bluetooth causes it to turn OFF, with no command sent. Worth taking
seriously for two reasons.

Practically: if a bare unauthenticated connect reliably powers the load
down, then "off" is already reachable without HAP, and a deliberate
connect-then-disconnect is a legitimate (if blunt) implementation of one
half of the switch. It should NOT be wired up as if it were a real off
command without saying what it is — but it is worth confirming whether it
reproduces, because a reliable half is more useful than nothing.

Security-wise this is a genuine finding: any device in radio range can
apparently power-cycle the load without pairing. That is the accessory's
flaw, not ours, but it is worth writing down.

Also captured from the same report: the reporter uses no iOS devices, so
the objection in HOMEKIT.md about stealing the pairing from the Home app
does not apply to them — which changes the calculus. Implementing HAP
pairing becomes viable rather than destructive. The suggested design is
worth keeping: let ONE known node on the bo-suite fabric hold the pairing
and act as an oracle for the others, rather than every client pairing
independently. That matches how a Home hub works and sidesteps the
one-pairing-at-a-time limit for everything except the hub itself.

---

# Fleet run 2026-09-01 (late) + voiceterm direction

## 32. Fleet failures, triaged. Three are NOT our code.

- **beatspace / beatspace+features** — `error: instruction requires:
  fullfp16`, emitted from `gemm-common-0.19.0/src/simd.rs` inline asm
  (`fmla v0.8h, ...`). A third-party crate emitting ARM fp16 SIMD that the
  target does not declare. Fix is a build-config decision, not a code one:
  either add `-C target-feature=+fp16` for the aarch64 target in
  `.cargo/config.toml` (correct only if every target device really has it —
  the Fairphone's SC7280 does, an older ARM board may not), or pin/patch
  gemm. Deliberately not guessed at, since getting it wrong produces
  binaries that SIGILL on the device rather than failing to build.
- **fao-codec / fao-codec-egui-frontend** — `dct_roundtrip`
  (`in=-3, back=-1.176`), `dct_simd_matches_scalar`, `dct_dc_is_mean` all
  failing in `fao_codec_core/src/dct.rs`. The scalar/SIMD disagreement plus
  a roundtrip that loses more than rounding says the SIMD path is wrong on
  this architecture, which given the beatspace finding above is very likely
  the same ARM story. Check whether the SIMD path is gated on a target
  feature it is not actually getting. Real bug, ours, worth doing.
- **ambient-learner** — one failure, `security::tests::test_pairing` at
  `src/security.rs:516`. 129 others pass. Small and self-contained.
- **bot-arena** — four `engine::tests::*` failures
  (`experience_log_dedups_identical_fixes`,
  `clean_repo_passes_and_broken_fails`, `evaluate_real_noop_is_full_reward`,
  `deny_and_allow_dirs_enforce_lists`). Note these are in the git-diff
  version-merging subsystem recorded in §14 as having zero callers — so
  either the tests exercise dead code (in which case §14's deletion
  question answers this too), or something does call it and §14 is wrong.
  Settle §14 first.
- **studio** — three `studio-video` probe/thumbnail failures at
  `crates/studio-video/src/lib.rs:415`. Almost certainly a missing or
  differently-built ffmpeg on the runner rather than a logic error; confirm
  before chasing.
- **logscope / logscope+features / gpu-gemm / bo-suite /
  voiceterm-egui-frontend** — carried over, see §19 and earlier entries.

## 33. voiceterm-egui chrome: the rest of the density work

Part 1 landed (narrow surfaces start with the tab column collapsed and the
keycaps row closed). The structural half remains, and `main.qml` says
exactly what to mirror — these are the constructs to match, not invent:

- `Drawer` — the file picker. egui equivalent: an overlay `Area` that
  slides in, not a mounted `SidePanel`.
- `Popup` (with `closePolicy: Popup.CloseOnEscape`) — settings,
  favourites, voice, ask-AI, the peer panel, the setup wizard. All of
  these are `egui::Window`s at most, and several are currently panels.
- `SlidePopup` — the tap/slide marking menus (link actions at
  main.qml:1737, per-tab actions at :1766, pane long-press at :1780). The
  egui port already has `slide_menu`/`gesture_menus`; the point is that
  these are the ONLY way that material appears in Qt, never a fixed row.

Concretely: the keycaps/gesture-chip row should become a `SlidePopup`-style
overlay reachable by a gesture from the bottom bar, rather than a third
permanent band. That deletes a whole bottom panel on every device, not just
narrow ones.

## 34. voiceterm mail: talk to Proton directly, using hydroxide as the reference

A hydroxide checkout was supplied for this. It is a Go bridge exposing
Proton Mail over standard IMAP/SMTP/CardDAV by implementing Proton's API
client-side, and it is the right reference for two separable pieces:

1. **The API/auth layer** — hydroxide's `protonmail` package: SRP login
   (Proton uses SRP, not a bearer password), 2FA, the mailbox-password
   step that unlocks the key ring, then OpenPGP key handling for
   encrypt/decrypt. This is the part that is genuinely hard and where a
   port must follow hydroxide closely rather than improvise.
2. **The surface it presents** — hydroxide's own answer is "run a local
   IMAP/SMTP server and let any client talk to it". Worth seriously
   considering taking that same shape rather than embedding a Proton
   client in voiceterm: a `voiceterm-mail` daemon speaking IMAP on
   localhost means the existing mail code keeps working unchanged, the
   crypto lives in one auditable place, and every other app on the fabric
   (and any third-party client) gets Proton access for free. The
   alternative — a Proton-specific backend inside voiceterm's mail view —
   is less work once and more work forever.

Recommend (2). Check the existing `apps/voiceterm/src/mail.rs` +
`mail_view.rs` to see how much of it is already IMAP-shaped; if it is,
this reduces to "point it at localhost and write the daemon".

Licence check before porting anything: hydroxide is MIT, so lifting
structure and algorithms is fine with attribution. Do not paste Go
verbatim into a comment and call it a port.

## 35. voiceterm voice features from the Qt predecessor

Not started. The QML side has `ReplVoiceGlow.qml`, a voice `Popup` driven
by wake-word signals (main.qml:2038 — `voice_pop_wake_event`, inline
command acceptance, an ask-AI path on a second signal bit), and a
`workflow_playback` intent confirmed through that popup (main.qml:2190).
`uikit-egui` already has `repl_voice_glow`, so the visual half is
partially lifted; the wake-signal plumbing and the intent confirmation
flow are not. `apps/voiceterm/src/voice_intent.rs` and `editor_voice.rs`
are the non-Qt logic to reuse.

## 36. Fleet run 3 — what moved, what is left

Fixed this round: **ambient-learner** (pairing signature — was broken, not
flaky; see the commit) and **voiceterm** (`qml_gen` id counters raced
across tests, and the failure mode was duplicate QML ids, not just a red
test). **fao-codec-egui-frontend**, **gpu-gemm** and **bo-suite** stopped
failing on their own between runs — worth a glance to confirm that was the
id/layout work and not luck.

Still open, with what is known:

- **gitmind** — NEW. `cargo test -p gitmind-egui --doc` fails with
  `E0463: can't find crate for uikit_egui` / `settings_kit`, pointing at
  `crates/gui-egui/src/pages/push.rs:384`, which is ORDINARY CODE, not a
  doctest. Both crates are unconditional path deps in that Cargo.toml, so
  the manifest is not the problem. That means the crate itself fails to
  build under whatever configuration the doctest pass uses — a feature
  set, or a target dir where those rlibs are absent. Attempted to
  reproduce here and the build did not finish inside the sandbox's time
  limit, twice, so this is UNCONFIRMED. Next step is cheap: run
  `cargo test -p gitmind-egui --doc -v` on the fleet box and read which
  `--extern` flags rustdoc is actually passing.
- **beatspace / beatspace+features** — unchanged, and NOT ours:
  `gemm-common 0.19.0` emits ARM fp16 asm (`fmla v0.8h`) the target does
  not declare. See §32; still a build-config decision that wants a device
  to validate against.
- **fao-codec** — `dct_roundtrip` (`in=-3, back=-1.176`),
  `dct_simd_matches_scalar`, `dct_dc_is_mean`. Scalar and SIMD disagree
  and the roundtrip loses far more than rounding, so the SIMD path is
  wrong on this architecture. Given the beatspace finding, first thing to
  check is whether the SIMD path is gated on a target feature it is not
  actually being given. Ours, real, and the most valuable one left.
- **bot-arena** — four `engine::tests::*` failures, all inside the
  git-diff version-merging subsystem that §14 recorded as having ZERO
  callers. Settle §14 first: if it is genuinely dead the tests go with it,
  and if it is not then §14 is wrong and needs correcting.
- **studio** — three `studio-video` probe/thumbnail failures at
  `crates/studio-video/src/lib.rs:415`. Suspect a missing or
  differently-built ffmpeg on the runner rather than a logic error;
  confirm before chasing.
- **logscope / logscope+features** — carried over; see §19.
- **voiceterm-egui-frontend** — carried over; the real_pty VT work in §19
  and §27 is the blocker.

## 37. Two bugs from the supplied run log (both fixed), and one that is not

Fixed in this session (see the gpu-gemm commit): `prewarm` built a
dense-only cache and panicked on any hybrid model — that is the whole of
why Qwen3.8-27B "loaded" and then the worker died. And the engine printed
two contradictory RoPE lines about the same model in the same run.

**Not fixed: the buffer-overrun validation error that preceded the panic.**

    Copy at offset 0 for 8192 bytes would end up overrunning the bounds of
    the Source buffer of size 8

logged twice as "device error (non-fatal, logged)" immediately before the
prewarm panic. A source buffer of **8 bytes** where 8192 were expected is a
factor of 1024 — that reads like a length being passed where a byte count
was wanted, or an empty/placeholder buffer reaching a copy. Note that
prewarm was running on a cache with missing recurrent slots at the time, so
this MAY be a downstream symptom of the same bug and may simply vanish now.
Check the next run's log before chasing it: if it is gone, it was the
cache; if it persists, it is its own defect and the 8-vs-8192 ratio is the
lead.

**Also from this log — the GPU budget constant is being flagged by the
engine itself.** It now advises "GPU budget 48.0 GiB is a CONSTANT, not a
measurement... the constant has been wrong by 2.6x on this hardware", and
separately the arena hits a wall at 24 GiB of mapped slabs against a
compiled-in 31 GiB clamp. `kernel-validate --gate-vram` exists and measures
the real ceiling; nothing consumes its result yet. That is the missing
piece — see §29, which is the same "a number describes a different machine"
problem from the phone side. Wiring the probe's measurement into
`resident_budget()`/the clamp would fix both ends at once, and `probe_key`
+ `ResidencyProbe::explain` are already written for exactly this and have
no callers.

## 38. meta_muse-glimmer — a new architecture was supplied, unimplemented

Config only (no weights): `meta_muse-glimmer/`, `model_type`
"muse_glimmer", `MuseGlimmerForConditionalGeneration`. What the engine
would need, read off config.json:

- **Multimodal.** A vision tower (`vision_config`: hidden 1536, its own
  `layer_types` mixing `window_attention` and `full_attention`) plus a
  projector (`projector_hidden_size` 4096 → `out_hidden_size` 6144, gelu),
  with `image_token_id` 200092 / `video_token_id` 200091 splicing embeddings
  into the text stream. The engine is text-only today; this is the largest
  single gap.
- **Interleaved sliding/full attention.** 52 text layers, `layer_types`
  counts 39 `sliding_attention` + 13 `full_attention`, `sliding_window`
  2048. Per-layer attention kind, like the Gated DeltaNet hybrid routing
  already in `decode_step_full` — that per-layer dispatch is the right
  precedent to follow rather than a new mechanism.
- **Per-layer RoPE theta.** `layer_rope_theta` is an ARRAY, with 0 entries
  (no RoPE) on some layers. The engine stores one `rope_dims`/`base` for
  the whole model, so this needs per-layer rope state — and note §37's
  finding that even the single-value reporting was inconsistent.
- **Logit softcapping** (`final_logit_softcapping` 20.0),
  `qk_scale_factor` 3.87, `output_multiplier`, `post_norm_eps` 1e-8
  distinct from `rms_norm_eps` 1e-5. Each is a small arithmetic change but
  each silently produces fluent nonsense if missed.
- vocab 202048, hidden 6656, 32 heads / 2 KV heads, head_dim 128.

Scope this honestly before starting: the vision tower alone is comparable
to the existing text engine. A staged plan (text-only path first, refusing
image tokens by name; vision second) is the only version that lands.

## 39. `:ws <path>` → "summarize the codebase": one cause found, one still open

**Found and fixed:** the model was being reloaded on every request. The run
log shows Llama-3.2-1B loaded TEN times in one session. `Cmd::Load` had no
already-resident guard, while `Cmd::SetBackend` five lines above it does —
added for precisely this reason. Each reload drops the KV prefix tree, the
warmed pipelines and any prefix a long prompt already paid for, so no turn
could build on the previous one. Fixed this session.

**Still open — the two failures are different and should not be conflated:**

- *Llama-3.2-1B "can't be useful".* A 1B at Q4_K_M summarising a codebase
  is close to the model's ceiling regardless of harness. Before blaming the
  engine, check what the prompt actually contains: `ctxplan.rs` decides
  which files fit the budget, and a summary built from three truncated files
  will be useless no matter how good the decode path is. The diagnostic to
  add is one line saying how many files were considered, how many fit, and
  what fraction of the context the prompt used — right now nothing reports
  it, so "the model is bad" and "the prompt was empty" look identical.
- *The Qwen model "blows up" on the same task.* Not characterised in any log
  supplied so far — no panic, no OOM, no error text. Needs a run that
  captures it. If it is the 27B hybrid, note that recurrent state cannot
  seek, so trie/fleet prefix reuse is refused for that model (the loader
  says so) and a long `:ws` prompt therefore re-prefills every turn with no
  reuse at all — which is the same wound as the reload bug, from a
  different direction, and would show up as time rather than a crash.

Next step for both: instrument the `:ws` path to report prompt token count,
files included vs skipped, and context utilisation. Both questions become
answerable from one log line, and neither is answerable without it.

## 40. Vectorization: 29 candidate loops, none acted on

`maintenance/tools/vectorize_audit.py` now lists them (default score >= 4).
Four are in the real CPU kernels; the rest are in CLI/diagnostic code and
are almost certainly not worth touching. The four:

- `cpumatvec.rs:146`, `:184`, `:630` — Q4_K/Q8_0 dequant matvec inner loops,
  each flagged for a single float accumulator plus paired bounds-checked
  indexing. `:184` is the one the log shows moving 0.72 GB.
- `deltanet.rs` — one loop, relevant because the recurrent mixer is
  CPU-routed and runs per layer per token on hybrid models.

The accumulator finding is the interesting one and the fix is NOT free:
splitting into partial sums changes the summation order, so the result can
differ in the last bits. For a dequant matvec feeding attention that is
almost certainly fine, but it is a numerical decision and should be made
deliberately, with the existing A/B gates used to confirm output stays
within tolerance rather than assumed.

Measure before and after. The tool is triage; it proves nothing.


## 41. ctxplan.rs has no callers — the context planner was never wired

Found while chasing the `:ws` question. `ctxplan.rs` (265 lines) computes
exactly the thing that was missing: which files fit a token budget, whole
vs peeked vs skipped, with `used_tokens`, `looped_in()`, `peeked()` and
`skipped()` already implemented. **Nothing in the crate calls it.**
`grep -rn "ctxplan::" src/ --include=*.rs` outside its own file returns
nothing.

So the prompt-building path that DOES run (`workspace_block` /
`agent_context`) budgets by a coarse `ModelClass` and byte caps, while a
per-file relevance-ranked planner sits beside it unused. That is the same
"built, never wired" pattern already logged for tomescope (§ earlier) and
bot-arena's engine.rs (§14), and this one has direct user-visible cost:
the chat path shipped a directory listing where a planner would have
chosen files.

Next step is a judgement call, not a mechanical fix:
- If ctxplan is the intended design, wire it into `workspace_block`
  (it needs a token estimate per file and a real budget from the model's
  context window — `Budget::for_context` already takes exactly that).
- If it was superseded by read_harness's class budgets, delete it, because
  a second unused answer to a question the codebase has already answered
  is worse than none.

Decide before adding anything else to either. Do NOT leave both.

## 42. Model inventory — what the fleet can actually be tested against

From the operator's `~/models` listing, so future work stops guessing what
is available. Relevant to arch coverage:

- **llama3**: 1B-Q4_K_M, 3B-Q2_K, 3B-UD-IQ1_S, 3.3-8B-Q4_K_M. The IQ1_S is
  the one that exercises the native IQ1 path (see the loader's IQ1_S
  transcode notes).
- **qwen3**: Qwen3-14B-UD-IQ1_S, Qwen3.5-9B-Q4_K_M, Qwen3.8-27B-UD-Q4_K_M
  (the 48/65 Gated DeltaNet hybrid), plus a `3p6` directory.
- **DeepSeek-R1-0528-Qwen3-8B-Q4_K_M**, deepseek-coder-6.7b.
- **Falcon-H1-Tiny-90M-Coder** — 24/24 Gated DeltaNet, i.e. a FULLY
  recurrent model at 90M. That is the cheapest possible regression test for
  the hybrid cache path that crashed prewarm; it loads in seconds.
- **gemma4** e2b-it — exercises the Gemma family features (weightless V
  norm, GeGLU, (1+w) norm) the glimmer work leans on.
- **diffusion/llada-8b-instruct**, staging/llada2mini — the LLaDA paths.
- **meta_muse-glimmer** — config only, NO weights. So glimmer work cannot
  be end-to-end tested yet; the arch registration and swa_pattern tests are
  as far as it goes without a converted GGUF.
- qwen2.5-0.5b-instruct-q8_0, TinyLlama-1.1B-32k-Q2_K — small enough to
  iterate the harness against quickly.
- whisper: ggml-base.en / large-v3-turbo / tiny.en, plus a q4_k turbo.

Two things follow. First, **Falcon-H1-Tiny-90M is the right smoke test**
for anything touching recurrent state — it is 90M and fully recurrent, so
it catches hybrid-cache bugs in seconds rather than on a 27B. Second, the
glimmer implementation has no checkpoint to test against, so it should not
progress past the registry/refusal stage until one exists.


## 43. §14 settled, and it was pointing at the wrong thing

bot-arena's `engine` module is **not dead**. main.rs, heroes.rs and
inspect.rs call it throughout (`run_engine`, `cleanup_workspace_temp`,
`all_edit_ops`, `container_count`, `load_checkpoint`). §14's "13 functions
with zero callers" describes a SUBSET of engine.rs, and the four fleet test
failures have nothing to do with it — so the question §14 posed ("do the
tests exercise dead code, or is §14 wrong?") had a third answer: both
claims were about different parts of the same file.

The failures were environmental: two of the four need `python3 -m pytest`,
which the runner does not have, so `evaluate_real` scored 0 on everything
and the assertions read as engine bugs. Both now skip with a named reason
(fixed this session). The other two pass locally.

**Still open from §14 proper:** whether that 13-function git-diff
version-merging subsystem is genuinely unreferenced. That is now a separate,
smaller question than it looked, and deleting it is a normal dead-code call
rather than something that might break the four tests.

**Worth doing on the fleet box:** install pytest. Those two tests are the
only end-to-end coverage of `evaluate_real`, so skipped is a real loss even
though it beats a permanent red.

## 44. Third instance of the shared-mutable-state race — worth a lint

fao-codec's DCT basis was a `static mut` behind a check-then-set Relaxed
`AtomicBool` (fixed this session, §32's "ARM SIMD" guess was wrong). That
makes three this session, all invisible until something ran tests in
parallel, all with a real consequence beyond a red test:

1. board-layouts — a process-wide `CodeMode` atomic → wrong keyboard layout.
2. voiceterm `qml_gen` — id counters reset by a concurrent test → duplicate
   QML ids, which is a parse error in QML.
3. fao-codec `dct` — half-initialised cosine basis → wrong transform.

The shape is always the same: process-wide mutable state, no
happens-before, and a reset/init that races. Two of the three were "fixed"
by making the state thread-local or once-initialised rather than by locking
the tests, which is the right direction — the invariant becomes structural.

Worth a small check in `maintenance/tools`: flag `static mut`, and flag
`AtomicBool` used as a check-then-set init guard (a `load` followed by a
`store(true)` with writes between, on a Relaxed ordering). Both are
mechanical to detect and neither has a legitimate use in this tree —
`static mut` is a hard error in edition 2024 anyway, so the codebase will
have to move regardless.


## 45. §44 done — the lint exists, and 5 soft findings remain

`maintenance/tools/shared_state_audit.py`. Catches all three of this
session's races by shape; `--self-test` includes the qml_gen counter
verbatim as it was before the fix, so the tool is checked against the bug
it exists for rather than only against invented cases.

Five findings left, all the same soft class: a mutex-protected global with
a free-function reset.

- `rusty-board/src/keyboard/character_draw.rs:536` — `clear_strokes()` over
  a global `STROKE_BUFFER`.
- `rusty-board/src/ui/keyboard_view.rs:295` — `reset_swipe_latch()`.
- `rusty-board/src/ui/window.rs:180`, `:353`.
- `voiceterm/src/event_signal.rs:112`.

No data race (the mutex handles that), but the logical one is real and is
exactly what bit board-layouts: one test's reset lands inside another
test's sequence. Each wants a judgement call rather than a sweep — for a
GUI singleton like the stroke buffer, process-wide is arguably correct and
the tests should serialise; for anything a test drives directly,
thread_local makes the invariant structural. Do not batch-convert these.

## 46. Remaining fleet failures after this session

Closed: ambient-learner (pairing crypto), voiceterm (qml_gen race),
fao-codec (DCT race), bot-arena (pytest missing → now skips honestly).

Still open, in the order I would take them:

1. **beatspace / beatspace+features** — `gemm-common 0.19.0` emitting ARM
   fp16 asm the target does not declare. Needs a device decision: add
   `-C target-feature=+fp16` for aarch64 only if every target device really
   has it (the Fairphone's SC7280 does; an older ARM board may not), or pin
   gemm. Getting it wrong gives binaries that SIGILL rather than fail to
   build, so this wants the operator, not a guess.
2. **gitmind** — `--doc` fails with `can't find crate for uikit_egui`
   pointing at ordinary code, not a doctest. Both are unconditional path
   deps, so the manifest is fine and the crate must be failing to build
   under whatever config rustdoc uses. Unconfirmed; the cheap next step is
   `cargo test -p gitmind-egui --doc -v` on the fleet box, reading which
   `--extern` flags rustdoc actually passes.
3. **studio** — three `studio-video` failures at `lib.rs:415`. Suspect a
   missing or differently-built ffmpeg on the runner; confirm before
   chasing, since the fao-codec lesson this session was that "three tests
   fail together" usually means one shared cause, not three bugs.
4. **logscope / logscope+features** — carried; §19's VT work is the blocker.


## 47. §41 settled — ctxplan is wired, not deleted

The decision was "wire or delete, do not leave both". Wired. The planner
was never wrong; it was missing an input. `ctxplan::describes_project`
supplies the relevance it needed, and `workspace_block` now uses the
planner instead of a coarse byte cap.

What this does NOT do: `agent_context` still uses the class budgets. That
is defensible — the agent path can actually read files on demand, so it
needs a starting set rather than a considered selection — but the two
paths now differ in how they choose, and that should be a deliberate
choice rather than drift. Revisit if the agent path starts looking thin.

Also worth noting for whoever tunes this: `describes_project` is path-shape
only, deliberately. The obvious next signal is the user's actual question
("summarize" vs "why does X fail") — `Section` and per-section relevance
already exist in the planner and nothing fills them either. That is the
same missing-input shape one level down, and the same warning applies:
decide before building a second scorer.

## 48. Verify-by-running is what caught the last two bugs

Recording this because it is a pattern, not luck. Two bugs this session
were found by running a thing against real data rather than by reading it
or by a test passing:

- The ctxplan wiring looked right and its tests passed. Running it over
  `apps/switcheroo` showed one markdown file eating 70% of the budget.
- The `repo_tree` depth-first order looked fine and its existing test
  passed (that test only checked boundedness, not order). The failure only
  appears when you ask what a 40-entry cap actually contains.

Both are cases where the test suite was checking the wrong property. Worth
doing for anything that SELECTS or RANKS: run it over a real repo and read
the output, because "it returned something" and "it returned the right
something" look identical to an assertion that only checks the former.


## 49. studio fleet failures were an ffmpeg CAPABILITY gap, not a missing binary

My §46 guess ("missing or differently-built ffmpeg; confirm before
chasing") was half right and the half that was wrong is the instructive
part. A `has_ffmpeg()` guard already existed — it checked that `ffmpeg
-version` and `ffprobe -version` succeed. The fixture encodes with libx264
and aac, which many distribution builds omit, so the binaries answered
happily, the guard passed, and the fixture's `.expect()` panicked. The
failures then looked like probe/thumbnail bugs.

Two fixes, both this session: the guard now queries `-encoders` for what
the fixture actually needs, and two tests that never called the guard at
all now do (which is why the fleet saw a handful of failures rather than a
whole-module skip).

**Verified in both directions**, which is what turned a guess into a
diagnosis: against the real ffmpeg here all 13 pass, and against a stub
ffmpeg reporting only mpeg4/pcm they all skip cleanly — where before the
fix that same stub produced exactly 2 hard failures.

Same class then found and fixed in studio-media (2 tests, one needing
libvorbis).

**The generalisable lesson, worth applying elsewhere:** the dangerous
dependency is not a missing binary — that is easy to check and usually
already checked. It is an OPTIONAL COMPONENT of a binary that is present.
`ffmpeg` exists but cannot encode h264; `python3` exists but has no pytest
(§43, bot-arena, same session). Both produced fleet failures that looked
like application bugs. A guard should check the capability the test uses,
not the command it invokes.

Swept apps/ for the shape: 50 unguarded external-binary tests, 46 of them
`git` (a hard requirement of the repo, safe). Remaining singletons —
`unzip`, `getconf`, `sh` — left alone deliberately: the first two are POSIX
baseline and the `unzip` one is a cross-check arguably better failing
loudly than skipping. Not every external dependency deserves a guard.


## 50. The 1B was never the problem — agent tool surface had three disagreeing lists

From the 487deac transcript, and this closes the "is the 1B useless or is
the harness broken" question from §39 in favour of the harness.

`RUNTIME_TOOLS` advertises ten tools to the model. `diagnose()` accepted
four. `parse_one` could build nine (no `git_log` arm, despite the enum
variant, wire name, display form and executor all existing). Three lists,
one checked pair. A 1B emitted 13 tool blocks in a turn and every one was
refused for naming a tool the same prompt had offered.

Fixed this session, with agreement tests in both directions
(advertised→validates, advertised→constructible), verified by reverting the
fix and watching them fail.

**Still open from the same transcript, in priority order:**

1. **Names with an inserted space** — `write_ file`, `read_ file`,
   `git_ log`, `tool_ quoted`. Correctly-spelled `git_log` appears in the
   same run, so this is probably the model, not our detokenizer. Cheap
   hedge worth doing regardless: normalise whitespace inside a tool name
   before lookup. A model that wrote `read_ file` meant `read_file` and
   there is no tool it could be confused with.
2. **`tool_quoted_quoted`** — something is doubling. Appears alongside
   "prose inside the fence" rejections, so the fence language tag may be
   being re-wrapped. Trace where `tool_quoted` is emitted.
3. **The retry loop is not converging.** The transcript shows the same
   malformed call repeated a dozen times with the reminder text growing
   each round ("Reply with EXACTLY one line, nothing else"). The ladder in
   `line_calls.rs` exists for this; it is either not escalating or the
   escalation is not reaching the model. A loop that repeats an instruction
   the model has already failed twice is spending context to no purpose —
   the same finding as §21's small-model looping, from the harness side.
4. **`edit` missing `find`** was a real schema mismatch in that run and is
   now a required-field message rather than "unknown tool", which is at
   least actionable by the model.

## 51. Performance: the engine already knows what to do and says so

The 487deac run is 12.69 tok/s on a 1B, and the log diagnoses itself
better than a profiler would:

- `roofline — 19 GB/s of 800 spec = 2% of the decode ceiling; NOT
  bandwidth-bound ... the engine is the lever`
- `sync 87760ms of 90710ms (GPU-sync-bound)`
- `7800 submits (7/tok)`, `96% of submits were forced by PASS BOUNDARIES`
- `decode is 23.6x SLOWER PER TOKEN than this model's own prefill`

That last comparison is the useful one because it needs no bandwidth
measurement. The log's own recommendation is `LOGSCOPE_SPEC_DRAFT=lookup`
(model-free prompt-lookup drafting, identical output at temperature 0),
and it notes it pays exactly on repetitive text — repository reading, code
with recurring identifiers — which is precisely the `:ws` workload.

Worth trying before any kernel work: it is an env var, the output is
provably identical at temp 0, and the phase-balance figure says the
bottleneck is "nothing in flight to cover memory latency", which is what
speculation fixes. `LOGSCOPE_PASS_FUSE` is RETRACTED and must not be
reached for — it corrupts AR decode and changes diffusion output.


## 52. Flags that are the only route to performance — autospec, and the rest

Operator: "there are too many flags for a lot of folks. they are good but if
it's the only way to get the performance then not so much."

Landed: `gpu_kernels::autospec` decides speculative drafting from
measurement (temperature, decode/prefill phase ratio, remembered acceptance)
with `store::spec_accept_{get,put}` persisting the acceptance. Policy and
store only — **the wiring is deliberately a separate change**: call
`autospec::decide` where the decode path currently reads
`LOGSCOPE_SPEC_DRAFT`, and feed `spec_accept_put` from the live
`spec::Acceptance` at end of turn. Small change, observable effect, wants a
machine that can measure it.

On "maybe I have to clear my autotune cache": probably not for this — the
autotune cache holds tile choices, and speculation was never in it. But the
question is fair and the answer is in `store.rs`: each cache carries an
EPOCH in its filename (`autotune-v6.tsv`, and the new `spec-accept-v1.tsv`)
precisely so that changing a kernel invalidates its measurements without
anyone being told to clear anything. If a cached verdict ever looks stale,
bumping the epoch is the fix, not a manual delete.

**The remaining flags worth the same treatment**, in rough order of how much
they cost someone who never finds them:

1. `LOGSCOPE_ARENA` — the run log shows it set manually. If the arena is a
   win, it should be the default and the flag should turn it OFF.
2. `LOGSCOPE_FAST_DECODE` — described in its own log line as "bit-identical,
   decode-speed only". Something bit-identical and faster has no business
   being opt-in.
3. `LOGSCOPE_PREFILL_TILED` — same shape, wants the same measurement.
4. `LOGSCOPE_MATVEC_SG` — subgroup matvec, auto-DISABLED on Asahi for a real
   reason (hangs). Already a measured decision; leave it.
5. `LOGSCOPE_PASS_FUSE` — RETRACTED, corrupts AR decode. Must stay off and
   must never be auto-enabled. Recorded here so a future sweep does not
   mistake it for another free win.

The principle to apply, and the one autospec follows: a flag is defensible
when it selects between outcomes that genuinely differ (sampling, precision,
risk). A flag that only chooses between "fast" and "slow" with identical
output is a missing default, and its existence means the fast path is off for
everyone who did not read the source.


## 53. Physical UI: the gap between Mistfall (shipping) and Observatory (reference)

Reference shots supplied. The running app and the target differ in ways that
are mostly NOT styling, which is the useful finding — they are things a
renderer is failing to do:

1. **Graph labels collide.** Fixed this session as pure geometry
   (`graph_labels::place`), NOT yet wired to the renderer. Wiring is small:
   measure, build `Want`s, call `place`, draw the survivors and a "+n more".
2. **The left nav.** Observatory uses an icon + title + one-line subtitle rail
   ("GRAPH / Explore connections"). Mistfall uses a crowded bottom tab strip
   with nine equal-weight tabs. The subtitle is what makes the reference
   legible to someone who has not memorised the tabs.
3. **Panel language.** Observatory's carved-stone frames are consistent across
   every panel; the shipping app applies `carved_frame` to some and not
   others. `theme::carved_frame` already exists — this is application, not new
   code. The earlier "carved-frame sweep" commits did part of it.
4. **Context cards carry a score** (0.98, 0.86) in the reference. We now HAVE
   that number — `ctxplan::describes_project` produces exactly this relevance,
   and the planner already decides whole/peek/skip. Surfacing it in the
   Context panel would make the selection legible instead of invisible.

(4) is the one that connects to this session's other work and is probably the
highest value: the user currently cannot see WHY a file made it into the
prompt, and the answer already exists as a float.

Also: the scratch_code graph (§ this session) wants a view, and the
Observatory GRAPH tab is the obvious home — nodes with `parent` edges is
exactly the shape that panel already draws.


## 54. Physical UI: what landed, and the honest gap

Landed this session: three mansion palettes (Travertine / Obsidian /
Lattice), `uikit_egui::lux` (sheen, bracket, Weight), sheen + gilt brackets
wired into `theme::carved_frame`, and `anim_secs_for`.

**The gap I cannot close from here.** Everything above is verified as maths
and invariants — smoothstep has no corner, brackets scale off the short side,
heavy settles slower than light, alpha stays in range. None of that is
evidence it LOOKS expensive. Three specific things want eyes on a screen:

1. **Sheen alpha (currently max 26/255).** Deliberately low — the effect
   should be noticed as the surface looking right, not as a visible gradient.
   If it is invisible on a real display, raise it; if it reads as a smear,
   the band half-width (34% of panel width) is the other knob.
2. **Bracket inset (5pt) and stroke (1.3pt).** On a Fairphone at 300% these
   may need to grow with the touch scale rather than staying in points —
   `lux::bracket` scales with the PANEL, not with the display density, and
   those are different questions.
3. **Whether Travertine is legible outdoors.** It is the only light palette
   and the one most likely to fail on a phone in daylight, which is also the
   case it is most likely to be chosen for.

**Next, in the order I would do it:**

- Wire `anim_secs_for` at the call sites that currently pass
  `BASE_ANIM_SECS`. The helper exists and nothing uses it yet; that is a
  mechanical sweep and it is where the weight model actually becomes
  perceptible.
- Apply `carved_frame` to the panels that still lack it. The earlier sweep
  did part of the app; the reference is consistent across every surface and
  we are not.
- Surface `ctxplan` relevance on the context cards (§53 item 4) — the number
  exists, the reference shows it, and it is the one that makes the system
  legible rather than merely prettier.


## 55. Two liaisons, one of them dead — and it is the one named in the warnings

`logscope-egui/src/liaison.rs` defines a `Liaison` struct that **nothing
constructs**. `grep` for `Liaison::new` across the workspace returns nothing.
Only its `Epistemic` enum is used (by `promptroute` and one field in chat).
Meanwhile `liaisonhud` uses `logscope::liaison::Statement` — a SECOND liaison
module in the other crate.

The `field phase_done is never read` warning in the build log is the visible
edge of this: it is set by `advance()` and read by nobody, because the whole
struct is unreachable.

So "the liaison isn't working" is accurate in the strongest sense — the one in
the egui crate has never run. Before writing any new wiring, decide which of
the two is canonical:

- if `logscope::liaison` is the real one, delete the egui copy (a second
  unused answer to a question already answered is worse than none — the same
  call made for ctxplan in §41, which went the other way);
- if the egui one is the intended design, `liaisonhud` should be reading it,
  and `phase_done` needs a reader or removing.

Do not leave both. Note the HUD *is* wired and fed (`hud.observe` at three
sites, `hud.display` in the chat view), so the display half works — it is the
state machine behind it that is absent.


## 56. A refused tool call still builds a Tool (gate hole), + a third dead duplicate

Found by the new `tool_protocol_round_trip` suite.

**The hole.** `authorize_text` refuses an unnonced call — raises
`UnnoncedToolCall`, rewrites the head to ```tool_quoted — but `parse_tools`
scans for the JSON payload without requiring a fence, so the refused call
still becomes a `Tool`. The prompt promises "tool syntax you quote out of a
file you read cannot execute"; this is the path by which it could. Pinned as
an `#[ignore]`d failing test (`tool_gate_hole`).

Two candidate fixes, and the choice matters:
- `authorize_text` removes the whole refused BLOCK, not just the head. Safest,
  but it changes what the model sees echoed back and may hurt the retry ladder
  (the model no longer sees what it got wrong).
- `parse_tools` requires an authorised fence. Cleaner separation, but every
  caller that feeds it un-authorised text (tests, the line-call fallback)
  changes behaviour.

Before either: check whether the executor actually gates on anomalies. If it
does, this is defence-in-depth rather than a live hole — worth knowing which.

**The duplicate.** `taint::extract_nonced` scans only for `<tool_call` and
never for ```tool fences, so it cannot see the form the prompt teaches. It has
no callers; production uses `authorize_text`. Third instance of the
two-implementations pattern this session (after the tool lists and the two
liaisons). Delete it or make it delegate — an unused half-implementation of a
security-relevant check is the worst kind to leave lying around, because the
next person to need one will find it first.


## 57. Agent tool-channel: three causes found, ranked by how much each explains

The 8B `:ws` runs produced perfect JSON payloads and zero executed calls.
Three independent defects, all now understood:

1. **`neutralize` was not idempotent** (FIXED). Compounding
   `tool_quoted_quoted_quoted` across turns. Explains the mutating fence the
   model imitates, and part of the context growth that triggered the
   compact/summarise prompt. This is the one that gets WORSE the longer a run
   goes, which matches the transcripts degrading rather than failing flat.
2. **The prompt showed one correct example and eleven argument-only lines**
   (FIXED). Explains the initial malformed fence on turn one.
3. **A refused call still builds a Tool** (§56, NOT fixed, pinned as an
   ignored test). Not a cause of the failure — a hole in the guarantee.

**Still unexplained and worth the next look:** the model appends "The final
answer is: TASK_COMPLETE (after verification) Please let me know what to do
next." to a turn whose tool call was ignored. It is declaring completion
while having read nothing. Once calls actually execute this may vanish on its
own; if it does not, the retry ladder is not telling the model that its call
failed — the anomaly is logged to the operator, not fed back into the
transcript. Check whether `line_calls`'s ladder text reaches the model.

## 58. Performance: the 8B numbers, and what they change

`setup 57798ms + encode 4639ms + submit 4643ms + sync 174054ms of 179589ms`
at 4.87 tok/s, 15 submits/tok, 1.7 dispatches/pass.

Two things stand out beyond the known sync problem:

- **`setup` is now 57.8s of 179.6s (32%)** — it was negligible on the 1B.
  Nothing has explained this line yet and it is the second-largest bucket.
  Worth finding what "setup" covers before optimising anything else, since a
  third of wall time is a bigger prize than most of the kernel list.
- **"134.3% accounted"** — the buckets sum to more than wall time, so at
  least two of them overlap (sync is probably counted inside setup, or vice
  versa). Until that is resolved the split cannot be trusted to prioritise
  by, which makes fixing the accounting a prerequisite rather than a nicety.

The readback-backoff fix from earlier this session is NOT in these numbers
(the run predates it). The next run's sync figure is the measurement.


## 59. repo_tree: wrong in both directions before it was right

Worth recording as a pattern, not just a fix. `repo_tree` shipped
depth-first (all 40 entries in one deep branch, no top level), then
breadth-first after my change (all 40 at the top, no source), and is now
depth-balanced. Each of the first two had a test that passed, because each
test asserted the property that the OTHER failure violates:

- "a tight cap keeps the shallow files, not one deep branch" — passes under
  BFS, and passes under balanced.
- "a tight cap reaches the deep files too" — the one that was missing, and
  the one the operator found by using the thing.

A selection function needs its tests to bracket the answer from both sides.
One-sided assertions on a ranking or sampling function will happily certify
the opposite mistake. Same lesson as §48's verify-by-running, from the test
side rather than the observation side.

**Still open on the same report:** the operator also saw the model emit paths
like `./logscope/ crates/gpu- gemm/Cargo.toml` — spaces inserted mid-path,
matching the `read_ file` / `git_ log` space-insertion noted earlier. Now
that the tree will actually contain deep paths, it is worth checking whether
those spaces are the model's own tokenisation or something in our rendering
of long paths. If the tree line-wraps anywhere, that is the first suspect.

> **Numbering note.** Two sessions appended here concurrently and both used
> the same numbers: the snapclone session's §24/§35/§36 are different items
> from this session's. Neither has been renumbered — the commits reference
> these numbers by name, and silently shifting them would break every
> reference in the log. The snapclone block below is fenced off instead.

# --- snapclone session, 2026-09-01 (separate numbering) ---

## 35. NAT traversal in the egui port: the loop is wired, not yet field-run

2026-09-01. `NatTraversal.cpp` is what got the Qt build across CGNAT:
STUN from the punch socket (a mapping is per socket, so the announced port
must be that socket's), a 60×150 ms punch volley then one every 20 s to
hold the carrier's mapping, and registering the peer's REAL source address
from the first punch that lands. The egui port had the strategy
(`nat_punch`), the server list (`stun_servers`) and the codec (`nat-kit`)
and ran none of them. `nat_traversal.rs` is the loop as a sans-io state
machine with the C++ timings pinned on a fake clock; the binary drives it
on the voice socket, which now outlives a call so its mapping does; the
record carries the STUN-mapped punch port (`dht_announce::with_public`).

Not done here, in order of value:
- No real NAT in the sandbox: the state machine is tested, the wiring is
  type-checked, nothing has crossed a NAT in this tree yet.
- `nat_punch::classify` (symmetric vs cone, needs two servers' answers) is
  still not driven, so `Strategy::Relay` is never chosen and there is no
  relay to choose — same as the Qt roadmap.
- Punches go to call participants and snap recipients only; the Qt build
  also punched on `connectToPeer` for text. The conservative halving is
  plumbed (`initiate(.., conservative, ..)`) but the binary passes false:
  no power-context signal is read yet.

## 36. snapclone UI, 2026-09-01: what landed and what is next

Landed: swipe-vs-widget ownership in the shared bridges (one fix, every
app); pages bounded and scrolling; a prioritised hint line (phone: short,
above the tab bar; desktop: long, under the toolbar); double-tap flips
the camera; the phone's camera picker is a dropdown; feed tiles no longer
own drags; columns on a wide screen (the kit's fit + a no-wrap window);
the trackpad detent turns pages. All headless-frame tested, none seen on
a screen.

Next, in order: the peers page as a table on desktop and cards on the
phone (the hint pattern — one decision, two renderings — applied to a
page); long-press a tab for its quick action; hold-to-preview a tile. The
QR square is the identicon the Qt build also drew (`qr_render` says so);
a real encoder is a separate decision, ~300 dependency-free lines if
taken.

### §36 addendum — the goodies, 2026-09-01

Wired this pass (each was ported, tested, and reached from nowhere):
- **Received snaps can be opened.** `SnapArrived` dropped the frame's bytes
  as it built the feed entry, so no received snap could ever be viewed.
  `Snapclone::received` holds them (48 / 96 MB, oldest first); a tap on a
  tile opens through `open_snap_routed`, by kind.
- **Hold on a tile opens the snap menu** (`open_snap_menu` / `apply_menu`,
  `menu_ui` now drawn); "Conversation" is one of its entries, which is what
  a hold used to do directly.
- **Swipe up on the viewfinder sends** (`Composer::begin_drag/end_drag`):
  opens the recipient picker with "Send" armed.
- **The help manual is drawn**, with its filter and its scrolled/interacted
  distinction (`help_interacted`, `scrolled`, `set_filter`); "help" in
  Settings opens it; it closes itself on idle as designed.
- `close_video` is the one close path; `ambient_snap_arrived` on arrival;
  `Gossip::purge_stale` daily on the expiry tick; `set_talk_prominent`
  follows the shell; Escape dismisses every modal.

Still unwired, and why — decide per item, don't wire blind:
- `face_consent::{grant,revoke}`: methods on the crop STORE, which the app
  does not instantiate; the app holds the `Consent` enum. Wire when a
  detector produces crops.
- `reticle::set_state`, `video_viewer::{note_frame,note_error}`,
  `voice_playout::note_decoder_unavailable`, `peer_session::note_sync_reply`:
  each needs a signal the binary does not yet raise (a focus result from
  the camera, frames/errors from a video backend, a decoder, a sync reply).
- `prepare_send`, `publish_landmark`, `attach_photo_hashes`, `open_playlist`:
  need the send receipt, landmark fingerprints and a thread playlist to be
  computed somewhere first — features, not call sites.
- `set_speed`, `set_auto_sync`, `set_mobile_display_debug`, `set_dwell_armed`,
  `set_long_press_ms`: settings surface without a control; a settings row
  each, when the settings page is redone (§24's "sections that fit").
- `modal::{dismiss_layer,dismiss_topmost}`: library API; `dismiss_all` is
  used.


---

## 60. Merged the snapclone session (c14unified-019) — and it closes part of §23

23 commits, merged at `0adf560ce`. Common ancestor is `origin/main`; the only
file both sessions touched was this one (append/append), and the two blocks
are kept side by side — see the numbering note above, since both sessions
independently used §24/§35/§36 for different items and renumbering would
break every commit message that references them.

**Closes two items from §23 (camera enumeration on postmarketOS):**
- "device names run off the page; should be a dropdown" — done by
  `a2cc0b969`, which also adds double-tap to flip.
- "default selection lands on /dev/video0 and hangs" — addressed by
  `fcd55ce67`, which asks PipeWire for cameras BEFORE reading `/dev/video*`.
  That is the right order: the decoder/encoder nodes that were cluttering the
  list are v4l2 devices, and PipeWire knows which are real capture sources.

Still open from §23 and worth re-testing on the phone now: the 640x480 /
low-framerate advertisement on a 4K sensor. Format negotiation was not part
of this session's work.

**Verified after merge:** uikit-egui 353 tests (my 349 + their 4, no
interaction), snapclone-egui 1605, snapcore 112, dht-kit 32. All green, no
conflicts outside this file.

Worth noting for the fleet-testing pass: this session ported `NatTraversal`
(STUN on the punch socket, volleys, mapping maintenance) and made the DHT
bind the listen port and bootstrap from remembered peers. That is directly
relevant to §22 — gitmind fleet discovery never finding the Fairphones — and
may be the missing piece there, or at least the place to look first now that
there is real NAT code to point at the problem.


## 61. The "bright pink theme" run: what it proves

The model was patient, not blocked — the operator confirmed replies do arrive.
So the failure is quality, not plumbing, and the run is a clean diagnosis:

- **It never read a file** (`0 file(s)`, plan `4 unverifiable`). It invented
  a `themes` array in a Cargo.toml.
- **The harness pointed it there.** Query-blind ranking put both Cargo.tomls
  at 0.90 and `palette.rs` at 0.17. Fixed this session.
- **It could not have found palette.rs anyway** — that run predates the
  depth-balanced `repo_tree` (§59), so the tree it saw was top-level only.
  Both fixes are needed together and neither is in the running build.

**Ruled out, so nobody re-chases it:** the garbled identifiers are not a
tokenizer bug. `uikit-egui` appears correctly beside `uikit - egui` in one
line, and `drop_report()` — which prints on any encode/decode drop — is
absent from the log. It is confabulation.

**Still open and now the most interesting question:** the loop guard fired at
9 turns, which is the right outcome, but the model spent those turns
rewriting the same wrong file. Once it can see and read the right one, re-run
this exact prompt — it is a good regression case because the answer is
verifiable (a new palette constant, an enum arm, and the `all()` count test
that will fail if the arm is missed).

The `patch mode: no patches applied — the reply was prose` message is working
as designed and is worth keeping; it is the only line in the run that told
the operator plainly that nothing had been written.


## 62. Merged the games bundle (c14unified-010); two pre-existing example failures

16 commits, 39 files, ZERO overlap with this session — no conflicts. blackstar
vehicle combat and the district/world loop, unendlich's generator ring and
behavior field, `perfbisect`, and `games/shared/worldhost`.

`worldhost` is the piece that matters for the engine port: a small trait of
world questions (ground height, clearance, line of sight, standing spots,
steering obstacles) and deliberately nothing else — physics excluded because
skydribble's is coupled to its shape list and unendlich has its own voxel one.
Two implementors intended, `worldhost-unendlich` already stubbed.

Verified after merge: worldhost 2, blackstar-rs bins 1497, perfbisect 8. All
green.

**Two pre-existing failures, confirmed against the merge base in a scratch
worktree rather than assumed:**
- blackstar-rs EXAMPLES fail `E0433: cannot find 'anim' / 'orient' /
  'hardpoints' in crate`. The same class fails at the base (example
  `botcheck` there, `rigpose` after). They pull `src/*.rs` in via
  `examples/../src/` and reference crate-root paths the example's own root
  does not declare. The shellkit de-vendoring earlier in this session is a
  plausible origin and is worth checking before assuming it is ancient.
- `unendlich-engine`'s `opus_decode_check` example: `cannot find 'audio' in
  unendlich_engine`. Same shape.

Neither blocks libs or bins, but both make `cargo test` red in those crates,
which hides real regressions behind known noise. Worth fixing for that reason
alone.

## 63. Compaction staging is manual BY DESIGN — decide whether it should be

The operator saw the compaction prompt appear in their text box and read it as
a leak. It is not: both call sites do `self.input = teacher_turn(...)` and the
note says "Send it; the briefing becomes the working context". Human-in-the-
loop, so you see what will be folded and pick the model that folds it.

But the "summarize, don't squeeze" checkbox reasonably implies something
automatic, and the operator expected it to be. That is a real UX mismatch and
it is a DESIGN question, not a defect:

- if staging is right, the checkbox and the note should say so — something
  like "stage a compaction for review" rather than a setting that sounds like
  a policy;
- if it should be automatic, the teacher turn needs to run on its own
  generation and substitute `staged_context` without touching the composer,
  and the operator needs an undo, because a bad briefing silently replaces
  real history.

Do not leave it ambiguous. Fixed separately this session: the plan now refuses
to stage at all when there is too little to fold (§ the refresh commit), which
removes the most confusing instance without answering this question.


## 64. §56 gate hole CLOSED — and it was live

The prior question is answered: nothing gates on `anomalies`. The one
production caller logs them with `eprintln!` and parses the text anyway. So a
tool call quoted out of a file the model read COULD execute; it was not
defence-in-depth.

Fixed conservatively — `authorize_text` removes the whole refused block and
leaves `REFUSED_MARKER`. The alternative (make `parse_tools` require an
authorised fence) stays unimplemented on purpose: cleaner in shape, but it
changes every caller that feeds un-authorised text, and this was not the path
to take the elegant option on.

**Overridden a prior deliberate design choice**, recorded because it should
not be discovered by surprise later: refused content is no longer "preserved
for the human to read" inside the model-facing text. It is preserved in the
anomaly snippet, which is the operator-facing channel. Same goal, right
channel — and the old behaviour was the vulnerability.

**Follow-up worth doing, now cheap:** `parse_tools` requiring an authorised
fence would make this defence-in-depth rather than a single point. Not
urgent now that the payload never reaches it, but it is the difference
between one correct function and a system that stays correct if that
function regresses. Do it when the callers can be audited properly.

**Also still true and unaddressed:** the executor ignoring `anomalies`
entirely is its own smell. Nothing currently needs it — the refusal is
enforced upstream — but a caller that receives a list of security anomalies
and only prints them will eventually be given one that matters. Consider
making the return type harder to ignore.


## 65. Mobile input: the machinery is right, the ADOPTION is the gap

Three field reports, and they are not three bugs.

**1. Palette does not fit (aperture).** Fixed, plus a sweep — hard widths of
460/520/516/340 clamped to `dialog::dialog_width`. Two Window-based sites
remain (av1tuner `default_width(400)`, studio `default_width(460)`); they need
`ctx` at the call site, which is a slightly larger edit.

**2. Input fields do not override swipe page gestures.** The mechanism EXISTS
and is correct: `gesture_layer::drag_owner` returns `Owner::Widget` when a
TextEdit is selecting, a slider is dragging, or the focused widget's rect
contains the press origin, and both `GestureLayer` and `touch.rs`'s
recognizer suppress on it. It arrived with the snapclone merge (9e1b62bc7).

**The gap is that almost nothing uses it.** `pager::` has exactly ONE consumer
in the whole tree — `apps/snapclone/egui-frontend`. Every other app that
swipes pages does it by hand and therefore does not get the widget-ownership
check. That is the fix: migrate the hand-rolled swipes onto `pager`/
`GestureLayer`, or at minimum have them call `drag_owner` before treating a
drag as a page change. Do NOT re-implement the check per app — that is how
this diverged in the first place.

**3. One character on the Fairphone, STILL.** My widget-id work (68 + 89 call
sites, plus `text_field` in the kit) did not fix it, so the id hypothesis was
wrong or incomplete. Given (2), the leading candidate is now that the
page-swipe recogniser is consuming the touch that would give the field focus —
which would make (2) and (3) the same bug, and would explain why it is
Fairphone-specific (a touch device) and why stable ids changed nothing.

Test for it cheaply before building anything: on the device, open a field
inside a NON-swipeable view (a modal, or an app with no pager) and type. If
that works and the same field inside a paged view does not, the gesture layer
is the cause and (2) fixes both. If both fail, the id hypothesis was wrong for
a different reason and the next suspect is the on-screen keyboard's input
method, not egui ids.


## 66. §65 partially closed — logscope's swipes now check drag ownership

Both `total_drag_delta()` sites in chat.rs now consult
`gesture_layer::drag_owner` before treating a drag as a swipe, with a
source-level regression test that fails if a third drag read is added without
a guard (verified by deleting one).

**The test to run on the Fairphone**, in this order, because it distinguishes
three hypotheses cheaply:
1. Type into the logscope chat composer. If it now takes more than one
   character, the swipe recogniser was the cause and the id work was a red
   herring — say so and stop looking at egui ids.
2. If still one character, type into a field in an app with NO swipe handling
   (switcheroo has none; its palette-free UI is a clean control). If that
   works, something else is competing for the press. If it also fails, the
   cause is below egui — the on-screen keyboard's input method — and the
   next step is capturing what events actually arrive, not more egui changes.

**Remaining adoption gaps** (same fix, other apps): every app that swipes by
hand rather than through `pager`. `pager::` still has ONE consumer,
snapclone. Grep for `total_drag_delta()` across apps/ — after this commit
logscope's two are guarded; check aperture, voiceterm and studio next.

## 67. Still open on logscope-egui, in the order I would take them

1. **`setup` is 32% of wall time on the 8B** (57.8s of 179.6s) and nothing
   has explained that line. Second-largest bucket after sync.
2. **The profile buckets sum to 134%**, so at least two overlap. Until that
   is fixed the split cannot be trusted to prioritise by, which makes it a
   prerequisite rather than housekeeping.
3. **The retry ladder does not converge** (§57): the same malformed call
   repeated a dozen times with the reminder growing each round. The ladder
   text does reach the model, so escalation is happening and not working.
4. **Two liaison modules, one dead** (§55) — decide which is canonical.
5. **`ctxplan` relevance on context cards** (§53 item 4) — the number now
   exists and the reference design shows it; surfacing it would make the
   selection legible instead of invisible.
