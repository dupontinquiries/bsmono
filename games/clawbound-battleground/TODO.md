# TODO / known limitations

Tracking doc for gaps that are known but not yet closed, and ideas that came
up in development but weren't built. Headless logic (sim, weather, map
events, wildlife, objectives, catastrophes, input resolution, audio DSP,
shader source) is covered by `cargo test` and is trustworthy. Everything
under **Never seen rendered** is the flip side of that: it compiles, the
math checks out, but no one has watched it run on a real GPU with a real
display, because this was built in a sandbox without one.

## Never seen rendered

None of this is *known* to be broken — naga validates every shader
(`tests/shaders.rs`) and the uniform layouts are size-checked — but "compiles
and type-checks" is a different claim from "looks right," and these have
only had the former:

- The stylized water surface (waves, depth shading, foam, puddles) and the
  flood-plane overlay.
- Weather visuals end-to-end: sky greying under cloud, the Long Night, rain/
  snow particle fields, lightning flashes, gust-driven canopy sway.
- The Elder Boar and Storm Drake models, their pit dressing, and their
  attack/slam/breath VFX.
- Catastrophe visuals: wildfire spread and char, earthquake fissures and
  screen shake, the flood rising, the tornado funnel.
- The feline ancestor voxel bodies at the new higher fidelity, and whether
  the per-pelt signatures (stripes, rosettes, antlers, sun disc, etc.) read
  correctly at gameplay camera distance.
- Post-processing stack as a whole (bloom, god rays, ACES, the S-curve,
  night grade, lens rain) — each piece was reasoned through individually,
  never seen composited.
- Procedural audio (`src/audio.rs`) — the DSP is tested on paper (RMS levels,
  reverb tail length, thunder decay) but has never been *heard*. If the howl
  or reverb feel wrong in practice, `acoustics_at()`'s per-place voicings
  (`src/audio.rs`) are the tuning knobs.
- The touch drag-to-aim indicator (line + confidence ring on the ability
  button) — logic is tested (`src/touch.rs`), the drawing code type-checks,
  never seen on an actual touchscreen.

**If you have a display:** `cargo run --release` and just look at it. That's
the single highest-value thing anyone can do with this repo next.

## Seen rendered (lavapipe, `--rig-sheet`), stills only

- Ancestors posed per part: a `Rear` cast on vertical planted hind legs, a
  forepaw mid-swipe, an `Arch` with the tail up, a cat on a buttress root's
  crest, a cat trotting a log's moss top, a cat mid-tumble with the head
  leading. Four of the thirteen cast signatures (Sunroar, Rake, Rainfall,
  Umbra).
- NOT seen: any of it in motion; the stalk-in-fern posture (the staged
  camera landed inside a trunk, then the re-render was not viewed); a cat on
  an actual RuinWall; the other nine signatures. `CLAWBOUND_SHEET=wall,fern`
  renders named shots only.

## Known gaps

- **High ground is real and untuned.** Auto-attack range is 3D distance
  (`systems.rs`), so a cat on a 4 u pillar cap is out of reach of a 3.4 u
  melee swing. A solid hit knocks a cat off a ridge, but only a ranged
  attacker can land one. Needs a decision: measure melee range in the
  ground plane with a height allowance, or make climbing out of reach the
  point and give melee ancestors the better leap.
- **Bots do not leap**, stand on nothing, and have not been run against the
  new wall and log capsules. The ruin wall's L has a concave inner corner,
  which is the shape that stalls a unit that paths by push-out. Run
  `simdbg` across seeds before trusting it.
- **Roots: outer slabs only.** `push_jungle_roots` registers the two outer
  boxes of each jungle-tree root (1.8 u) as walkable. The inner three
  (2.4 u and up) are still ghost geometry, and StranglerFig / MegaTree roots
  are not registered at all.
- **The ruin wall's long arm is crenellated** (1.8 to 3.6 u) and registered
  flat at 3.0 u, so paws clip or float by up to a voxel along it. The short
  arm is a true 3.0 u.
- **`Grid::mesh` still emits interior voxels** (~877k instances for the
  world). `mesh_where` hull-culls the ancestors' parts; doing the same for
  props wants its own screenshot diff.
- **Perching grants nothing** beyond position: no sight bonus, no cover.

- **bsgames handoff plays solo.** `BSGAMES_HANDOFF_ROLE/ME/PEER` are read and
  logged (`src/launch.rs`), but there's no netcode — a "join a friend" handoff
  currently starts a solo quick match instead of actually connecting the two
  sessions. Building real multiplayer (even LAN-only) is its own project.
- **Gamepad is untested end-to-end.** `cargo check --features gamepad` passes
  against real libudev headers, and the dead-zone math is unit tested, but no
  physical controller has driven it. If bindings feel off, `PB_*` codes and
  the gilrs `Button` mapping in `src/gamepad.rs` are the place to check first.
- **AI win-rate looks Blue-biased.** Traces from `cargo run --example simdbg`
  showed Blue winning most AI-only seeds. Not investigated — could be lane
  pathing, spawn offsets, or just a handful of unlucky seeds. Worth running
  `simdbg` across a wider seed range before trusting it either way.
- **Mouse-look vertical sign is a judgment call, not a certainty.** The strafe
  bug (v6) was numerically proven against the actual view matrix; the
  reported "inverted camera" was very likely the same bug felt two ways
  (turning correctly, then strafing the wrong way relative to what you're
  looking at). Pitch/yaw sign checked out under standard conventions and was
  deliberately left alone rather than risk a real regression — but it's
  still unverified with an actual mouse. The `invert_y` setting is the
  one-checkbox fix if it's wrong for a given setup.
- **`role = "app"` vs `"game"` in the monorepo manifest.** This project's
  manifest entry uses `role = "app"` because `ManifestEntry::is_runnable`
  doesn't treat `"game"` as buildable, which would make it invisible to
  gitmind's build buttons. The other game entries (skydribble, blackstar-rs,
  soldier) still use `"game"` and are therefore listed but not build-offered
  in gitmind's catalogue — a pre-existing inconsistency, not something this
  project's changes caused, but worth fixing centrally rather than having
  every new game entry independently route around it.

## Ideas that came up but weren't built

- **Skydribble-style driving joystick, ported with per-game mods, to
  unendlich and here.** Confirmed real (`games/shared/shellkit::touch`, on
  `shared/touchkit`). This game's touch stack already vendors `touchkit`
  directly (`src/touchkit/`) and adapts the anchored-gesture idea (see
  drag-to-aim, v7) — porting the rest of shellkit's specific car-handling
  conventions to unendlich is a separate, cross-repo task that deserves its
  own session with unendlich's actual physics in front of it.
- Canopy wind sway currently bends by height only; a per-species stiffness
  curve (bamboo whips further at the tip than a mega-tree trunk, which
  barely moves at all) would read better than the current single exponent.
- River life (fish, dragonflies) and ambient creatures (bird flocks, glow-
  moths) are all camera-proximity-based and stateless between frames by
  design (cheap, no bookkeeping) — a termite-mound / anthill prop that
  spills bugs when an ancestor steps near was sketched in conversation but
  never built.
- A hunting-season variant of Elder's Bounty (prey give bonus gold during a
  window instead of/alongside the boss buff) was mentioned once as a
  possible sixth map event; not designed or built.
- `Game::sell_item` refunds a flat 70% regardless of tier or how recently the
  item was bought — a "sell within N seconds for full refund" grace window
  (common in MOBAs to forgive misclicks) would be a small, safe addition.
- The ancestor-select and shop search boxes (`gridpick.rs`) don't currently
  support gamepad d-pad navigation (only keyboard arrows + mouse/touch) —
  the picker itself is input-agnostic (`move_cursor`, `point`), only the
  wiring into `draw_select`/`draw_shop` is keyboard/mouse-only right now.

## Verification commands

```bash
cargo test --no-default-features   # 209 lib + 12 + 2, no GPU, no audio libs needed (was '79': the lib tests had stopped compiling)
cargo test                          # same, plus links cpal (needs libasound2-dev)
cargo check --features gamepad      # needs libudev-dev
cargo run --example simdbg <seed>   # headless 12-minute AI match trace
cargo run -- --screenshot           # headless PNG render (needs a GPU or lavapipe)
cargo run -- --rig-sheet            # close-ups of posed ancestors, through the real draw path
```
