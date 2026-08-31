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

## 0. READ THIS FIRST: nothing here was seen or heard

There is no GPU and no audio device in this sandbox. Every visual and
audio claim below is verified to the level of *the math is right and the
shader compiles*, and no further. Two of the four fixes change what
every frame looks like.

**The single highest-value next action is a new screen recording**, on
the same chassis and a couple of maps, so the Fresnel and value-structure
changes can be judged. Until then they are hypotheses with tests attached.

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
- **C13-2e — darks vs curiosity.** The operator wants deliberate darks
  balanced against exploration and mystery, with horror and grit in the
  lore. **The film lore was never read.** Tuning against it beats
  guessing; ask for the pointer.
- **C13-2f — exengine's unported shaders.** Named by the operator as
  still missing from the C original. Never enumerated. Someone should
  diff the two shader sets and list what is actually absent.

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
