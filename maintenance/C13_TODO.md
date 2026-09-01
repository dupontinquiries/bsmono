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

## 24. Layout/overflow bugs across apps (Fairphone AND desktop)

- **Settings panels flow over the bottom navbar and over each other**
  instead of establishing bounds and scrolling. Both platforms.
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
