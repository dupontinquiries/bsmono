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
