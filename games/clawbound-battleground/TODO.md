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
- Also seen: a stalker and a runner at one fern (runner in a bound with
  leaf cubes behind it; stalker low and silent), the leaf-bend measured by a
  with/without frame diff, and the authored centre from above.
- NOT seen: any of it in motion; the stalk-in-fern posture (the staged
  camera landed inside a trunk, then the re-render was not viewed); a cat on
  an actual RuinWall; the other nine signatures. `CLAWBOUND_SHEET=wall,fern`
  renders named shots only.

## Known gaps

- **bsgames handoff: the lobby can, the launch path does not yet.**
  `LanLobby::host_for_handoff` / `join_for_handoff` are tested end to end
  over a FileStore, but `app.rs` still only logs the handoff and plays solo.
  Next: on `opts.handoff`, open `Screen::Lan` and call them; have bsgames
  pass `--lan`. The `opendht` backend has never been run here.
- **How to check LAN work**: `maintenance/tools/lan_sim.sh <lanbot> [n]`
  (namespaced LAN, real broadcast), and the real binary under Xvfb with
  `--lan`. Both found things tests did not. Never `pkill -f` a string that is
  in your own command line.
- **Menus sit over bare terrain**: with no match running the world is not
  drawn at all. Pre-existing; seen in the Xvfb run.

- **LAN: a den is a roster, not a match.** Hosting binds a real game socket;
  JOIN says HELLO to it, is counted, and is dropped after 4 s of silence.
  (A den hosted from the real game was INVISIBLE until this: the app
  advertised port 0.) What follows is the older note, still true where it
  says there is no networked match.
- **LAN: discovery only.** `netcode::lan` + `lanlobby` + the LOCAL NETWORK
  screen find, host and pick a den over a real network. There is NO
  networked match: JOIN ends at the host's address. Next is lockstep input
  exchange over the game socket (the sim is already fixed-tick and seeded).
  The screen has not been rendered; UDP broadcast was not exercised (one
  interface, no peer); two copies on one machine cannot both browse.
- **The sim is measured and is not the problem**: 0.056 ms/tick for a full
  5v5, unoptimised. Do not optimise it further. Frame time on a GPU is still
  the unknown.
- **Performance is measured in instances, not milliseconds.** The hull cull
  took the static world to 296,629 instances (from roughly 877k), but no
  frame time has been taken on a real GPU, let alone a handset. Next
  candidates: merge static chunks' draw calls, frustum-cull ancestors' ten
  part draws as one, a half-res post pass, and `shaft_steps` stays 0.
- **Hitboxes**: cats are capsules now; minions, towers and bosses are still
  spheres, and nothing is swept (fine at today's 0.67 u/tick).

- **The mid Runs are stubs.** Tower range went 16 -> 24 with a 28 u clearing
  (nothing that hides or lifts a cat inside it), and the lane-parallel Runs
  authored the same day ended INSIDE the old range. Truncated to 14 u. They
  want re-routing diagonally from the Hollow out to the side jungle. Bots do
  not yet respect the larger tower zone; there is no lock-on delay before a
  tower's first shot; Top/Bot clearings were not looked at in a render.
- **Ground**: the lane/grass blend is still carried per vertex on a 1.6 u
  grid (now smooth, and scuffed per pixel). A true per-pixel lane mask needs
  lane data in the shader. River banks were not touched.
- **Meshes nobody builds in a test are untested.** `crossing_log` had two
  bugs for that reason. Other prop builders are in the same position.

- **A rolling log has not been seen.** The seated log is rendered and spans
  its sluice; the roll, the spin and a rider being thrown are tests only, and
  the SIGN of the spin in `Scene::draw` is a guess. Minions on a log that
  starts rolling are left behind rather than thrown. A lodged log never
  returns, so after both are blown out the side lanes ford the river for the
  rest of the match -- whether logs should respawn is a design call.
- **The side lanes graze their rivers rather than crossing them** (nearly
  parallel for ~25 u). The crossing is a sluice cut through the causeway,
  which works, but a lane that met the river square-on would be a better
  place for a log. That is a river-shape change (`consts::RIVER_*`).

- **God rays through the canopy: attempted, not delivered.** `shaft_light`
  (voxel.wgsl, terrain.wgsl) marches the view ray through the shadow map with
  no new bindings, and is OFF (`Renderer::shaft_steps = 0`) because both
  tunings washed the far field milky instead of drawing shafts. Under this
  canopy nearly every ray is half-occluded, so the term fires everywhere and
  8 jittered samples average to glow. What it probably wants: noise that is
  stable per pixel rather than per world position, a much lower gain, more
  steps or a half-res pass with a depth-aware blur, and tuning by eye on real
  hardware. `CLAWBOUND_NO_FX=1` plus a split-screen is how to judge it; a
  luma number alone read as "working" when it was haze.
- **Nothing in groundfx has been seen in motion**: the front crossing the
  trees, the ring bending the air, a crack closing in rain. Heat shimmer has
  not been seen at all (no fire was staged). Blast and crack counts are
  capped at 4 and 8; ability hits under radius ~3 only shake the trees.
- **The shadow pass takes neither the brush nor the blast**, so bent leaves
  and whipping crowns cast their resting shadows.
- **The fitted shadow box drops far shadows** beyond ~45-75 u in close play
  (faded, not a line). Cascades would keep both; this is one map.

- **Quiet is not wired to anyone's perception.** `Game::quiet` (stalking,
  an ability's hush, wounded) drives the body, the rustle and the leaf-bend,
  and nothing else. `wildkit`'s perception and `Blockers::concealment_at`
  exist and neither reads it, so stalking cannot yet be exploited or
  countered. This is the next thing that makes it a mechanic rather than an
  animation. Bots never stalk. Touch has the gentle-push route to a stalk but
  no toggle button.
- **The Runs' canopy hides you from yourself.** It roofs the inner paths
  against a camera raised over the lane, which is the point, and will do the
  same to your own camera zoomed out above it. Needs play: fade canopy
  voxels between the camera and the player's cat, or lower the zoom ceiling
  under cover.
- **Only mid is authored.** Top and Bot lanes and the outer jungle are still
  uniform scatter; bots do not know the runs exist.
- **A bent fern casts a straight shadow**: the shadow pass does not take the
  brush uniform.

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
cargo test --no-default-features   # 242 lib + 16 + 2 (+3 in games/shared/netcode), no GPU, no audio libs needed (was '79': the lib tests had stopped compiling)
cargo test                          # same, plus links cpal (needs libasound2-dev)
cargo check --features gamepad      # needs libudev-dev
cargo run --example simdbg <seed>   # headless 12-minute AI match trace
cargo run -- --screenshot           # headless PNG render (needs a GPU or lavapipe)
cargo run -- --rig-sheet            # close-ups through the real draw path; CLAWBOUND_SHEET=fern,map_centre picks shots,
                                    # CLAWBOUND_NO_BRUSH=1 / CLAWBOUND_NO_FX=1 render the same frame without the leaf-bend /
                                    # without blast, cracks and shafts, for diffing
cargo test plan_of_the_centre -- --nocapture   # ASCII plan of the real world's blockers
```
