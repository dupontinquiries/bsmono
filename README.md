# skydribble

An open-world air-dribble sandbox inspired by Rocket League's freeplay, written in
Rust on wgpu 22 / winit 0.29. One big procedurally generated island of playful
shapes — floating rings, balloon spheres, lollipop pillars, tilted ramps, box
platforms and buried domes — with real car models, a real ball, boost pads, and
RL-style flight physics tuned for keeping the ball on your nose.

## Build & run

```
cargo run --release
```

First build takes a few minutes (wgpu). Debug profile is set to `opt-level = 2`,
so `cargo run` is playable too. Needs a desktop GPU/driver (Vulkan, Metal or DX12
via wgpu). Run from the project root so `assets/` is found.

Audio is on by default and needs ALSA headers on Linux
(`sudo apt install libasound2-dev`; nothing extra on Windows/macOS). If you'd
rather not: `cargo run --release --no-default-features` builds a silent binary
with zero extra system deps. Optional controller support:
`cargo run --release --features gamepad` (Linux also wants `libudev-dev`) —
left stick steer/pitch, A jump, B/RB boost, X powerslide + air roll, triggers
throttle, Y ball cam, Start pause.

CLI flags: `--seed N`, `--map 0..15`, `--gfx rays|classic`, `--fullscreen` /
`--windowed`, `--mute`,
`--dvorak` (apply the Dvorak binding preset), `--name NAME`, `--host`,
`--join FRIEND`, `--help`.

`cargo test` validates the WGSL shader offline with naga (the same frontend
wgpu uses at runtime), so shader typos fail in CI instead of on launch.

## Controls (all rebindable)

Every gameplay action binds to keys, mouse buttons, **and** controller buttons:
pause (Esc or Tab) then ENTER opens the controls screen — pick an action, press
the new input. Bindings are *logical* keys (the character your layout types),
so Dvorak/Colemak users bind their real keys; a full Dvorak home-row preset is
one Enter away (or launch once with `--dvorak`). DEL clears a binding. All of
it persists in the save file.

Classic defaults: W/S throttle + air pitch, A/D steer + air yaw, Q/E air roll,
Space jump / double-jump / dodge, Shift boost, Ctrl powerslide, C ball-cam,
T toss ball, B ball to front, R reset car, 1/2/3 cars, F flip model, M minimap,
N music, [ ] volume. The `:` key toggles **steer = air roll** mode, so your
left/right keys roll instead of yaw while flying (the toggle shows on the HUD).
Mouse buttons bind anywhere you like — boost on right-click and jump on
left-click is one capture each.

Fixed keys (layout-independent): Esc/Tab pause, F11 fullscreen, arrows + Enter
drive the menus, Q or X on the pause screen saves + quits.

## Menus that feel like menus

Every menu sits on a translucent glass card (borrowed straight from the
unendlich menu kit's design language) with drop-shadowed pixel text, tighter
type, and full **mouse support**: hover selects, click activates, clicking a
slider's bar sets it, the wheel nudges the selected slider, and right-click
goes back. Keyboard still does everything. The cursor hides while driving.

## Settings, mutators, flips

Pause → ENTER opens SETTINGS: **CONTROLS** (the full rebinding screen),
**CAMERA** (FOV, distance, height, angle, stiffness — the classic RL camera
menu), **ARENA + BALL MUTATORS**, **CAR SETUP**, **FLIPS** and **VIDEO**.
Arrows adjust, Enter resets a slider to its default, everything persists, and
every slider is a multiplier of the stock Rocket League numbers so 100%
always means "RL feel".

Mutators: gravity (0–260%), arena bounciness/friction, world size (rebuilds
the map on exit), ball size/bounciness/friction/weight — plus one-tap presets:
MOON BALL, PINBALL, BEACH DAY, HEAVYWEIGHT. Car setup: size, tire grip, top
speed, acceleration, boost power, jump strength.

Flips got the full treatment. Jump again in the air with a direction held and
the car genuinely flips (front/side/diagonal, with the dodge impulse);
**hold the opposite pitch to flip-cancel** (the speedflip family); and —
the air-dribbler's endgame — **flip resets**: press your wheels onto the ball
(or any surface) mid-air and the flip comes back, with a chime, a sparkle and
a HUD pip showing when a flip is banked. Options: flip strength, resets
on/off, strict vs loose wheel alignment, flip cancels, and an UNLIMITED FLIPS
mutator for pure chaos.

## Maps

Pause → G opens the map screen: sixteen procedural styles — MEADOW, CATHEDRAL,
OBSERVATORY, GREENHOUSE, CRYPT, SCRIPTORIUM, FOUNDRY, ZEN GARDEN, LIGHTHOUSE,
TEMPLE, CONSERVATORY, CAVERN, PLANETARIUM, DOJO, VAULT, HEARTH HALL — adapted
from the logscope scene ROOMS table in the same monorepo. Each style recolors
sky/fog/terrain, reweights the shape families, reshapes the hills and builds a
different centerpiece at spawn (orrery, lighthouse, great tree, colonnade, …).
R rerolls the seed; map + seed persist and `--map N --seed N` set them from the
command line.

## Real settling, real turtles, real mirrors

A nose-planted car now lays itself flat under gravity in about three
quarters of a second — see-saw physics about the planted end, not an
assist. Turtling is honest too: a roof-down car has no wheel traction; it
scrapes with real friction, boost slides it along the paint, attitude
inputs work at reduced authority so you can wriggle, and past the balance
point gravity rolls it back to its wheels. And the wet-floor reflections
stopped lying: the ball reflects with its spinning panel pattern, and cars
reflect as body+cabin silhouettes in their real liveries (wheels too, for
your own car).

## Perplexus maps: a glass jungle gym

PERPLEXUS is a full playground now: wider glass tubes (see the camera
through the wall), flared rims and ground aprons at every mouth, a tall
random tube lattice, spinner rotors, rideable rotating discs, closing
doors, pinball zones (vibe fields, ice patches, boost strips), blast-off
launcher rings, and floating gravity orbs you can orbit. VIDEO gained a
subtle configurable SCREEN DOOR. Jump is analog on a Wooting: feather Z
for a shallower hop, bury it for full height. The Dvorak preset now
matches the spec sheet exactly.

## Perplexus maps: tubes, ribbons, nodal gravity

Two maps treat the track as a 3D marble-run sculpture (twenty-four styles
total). **PERPLEXUS** is a giant hollow helix tube you drive the INSIDE of —
a new shell primitive with real SDF collision, so ray-traced shadows and
reflections work in there too — fed by an approach tube from the plaza.
**RIBBONWORKS** corkscrews a banked ribbon road around a great tower,
flat at the bottom and past-vertical at the top.

The camera knows gravity now too: a smoothed local-up follows the field
(CAMERA -> GRAVITY CAMERA blends it, 0% = classic), so corkscrewing a tube
rolls the horizon with you instead of confusing it. And PERPLEXUS grew into
a full run: striped helix to a summit junction, then a drop shaft into a
wall-of-death drum with a tangential exit, or an S-bend descent across a
12 m gap jump into a stepped funnel — with a figure-eight tube crossing
mid-field and a bare balance-beam rail for the brave. Boost pads line every
tube spine.

They work because gravity became **nodal**: each track segment carries
gravity nodes, and near them "down" is the track's down — radially outward
inside tubes (any point of the pipe wall is floor), the ribbon's own baked
normal on roads. The blend is the spec verbatim: 0.9 x the nearest node +
0.1 x the runner-up, fading back to world gravity outside ~1.5 node radii.
All the contact physics (wall driving, turtles, landing, flip resets) was
already surface-relative, and steepness is now judged against local
gravity — so the whole movement kit just works upside-down in a pipe.

## Slide is the air-roll modifier

Steer and air roll live on one key now, the way RL's classic scheme works:
in the air your steer keys TURN, and **holding SLIDE turns them into barrel
roll** — release and you're turning again. On a Wooting the slide key's
analog depth is a **blend ratio**: half-press = half roll, half yaw, so you
can lean a recovery or a tornado spin by feel. FLIPS -> "AIR STEER: ROLL BY
DEFAULT" inverts which one release gives you. (The old dedicated swap key is
gone; RollL/RollR dedicated binds remain.)

Two landing fixes: a **jump buffer** (a press up to 90 ms before touchdown
fires the instant wheels touch — before, it was eaten mid-air, which is
exactly the "my jump comes back slow" feel), and a hard partial upright on
the landing frame so the ground wins the argument immediately. Menus took a
third pass: smaller type everywhere, slider bars draw only under the
selected row, values dim harder, tighter bind rows — and the splash slogan
now rotates through the game's whole voice while you idle.

## The sim, the striker, and the games centre

**Headless AI sim.** `skydribble --sim all` (or `air1`, `pass2..4`,
`defend1`, plus a trial count) runs the real physics at 120 Hz with no
window and scores each run on STYLE, not just time-to-goal: airborne
touches, flip resets (weighted hard), passes between different bots, ball
height, defender distance traveled, and a goal bonus. This is the tuning
loop for everything below — the numbers in DESIGN.md come from it.

**Striker brain.** A tutorial-grounded state machine joins the ballchase
mob: line up behind the ball on the goal axis, hood-carry matching the
ball's speed, pop the moment it settles (jump under it, nose up, boost),
then the nose-tip air carry — hold under-behind, feather boost, tiny
touches — cradling wheels-first into the ball's belly for a flip reset and
spending the flip on the finish. A Defender brain guards a net and clears,
and Passer strikers hand the ball around for the pass drills. Current sim
truth: the four-passer mob converts (3/6 goals) and the defender gets run
ragged (~380 m per trial); the solo air-dribble finish is the open tuning
problem the sim exists to close.

**Physics defaults from the same tutorials:** gentle car-ball contacts now
deaden (velocity-dependent restitution) so the ball settles on the hood
instead of pinging off, and a ball riding on top gets a whisper of
horizontal speed-matching — hood dribbles and nose carries are balanceable
by design, for you and the bots alike.

**bsgames.** Skydribble is a games-centre citizen: it reads the
`BSGAMES_HANDOFF_*` env (auto host/join with your centre identity) and the
`--quick-match` flag (straight into the sandbox). The one hub-side change —
a roster entry — ships as `integration/bsgames-skydribble.patch` with apply
instructions in `integration/README.md`.

## Walls, war scores, and bloom

**Wall and ceiling driving.** Wheels grip any surface now, RL-style: hit a
wall wheels-first with speed and you stick — the drive basis, steering,
grip, jumps and flip resets were already surface-relative, so the whole
kit works sideways and upside-down. Gravity still pulls world-down, which
means holding a horizontal line on a wall leans on your lateral grip
(powerslide and you'll skate down it), and dropping below the peel speed
lets you fall off, exactly as it should. ARENA CLASSIC's walls, banks,
spines and ceiling ring are all drivable: wall-drive, jump, and carry the
ball off for the wall-to-air dribble this game is named after.

**Sparta scoring.** The ball remembers its last toucher, goals are
attributed, and SPARTA 50 V 50 keeps a real scoreboard (you play blue) with
an airhorn and team toasts. Your personal GOALS stat only counts goals *you*
scored now, in every mode.

**Bloom.** The renderer went HDR: the scene resolves into a 16-bit float
target, a soft-knee bright pass and separable gaussian build the glow at
half res, and a composite pass adds it back and runs the whole cinematic
grade (exposure, ACES, vignette, grain) exactly once. Boost flames, torch
towers, goal panels and the sun all halo properly. VIDEO -> BLOOM sets the
strength; 0% turns it off.

## Wooting analog wind-in

With a Wooting (or any Analog SDK) keyboard, the six movement axes go
analog with a deliberately **non-aggressive wind-in**: any real press still
lands high and a full press is exactly 1.0 — snappy stays snappy — but
feathering a key trims the output toward a floor (~0.55 at default depth, a
half-press lands near 0.75). That's the slight decrease in turn on a
non-full press that lets you tune a dribble start instead of micro-tapping
and overshooting; it composes with the powerslide-jump carry rather than
replacing it. The runtime SDK is loaded at launch and everything falls back
to plain digital keys without it (or with ANALOG KEYS off in CAR SETUP,
where the WIND-IN DEPTH slider also lives; 0% = pure digital). Bindings stay
logical/layout-proof: each key's physical identity is learned from its first
press, so it works untouched on Dvorak.

## Air steer, six cars, two stadiums

**Steer and air roll are one now**: in the air your steer keys ROLL by
default, so a powerslide jump carries its drift angle straight into an air
dribble — the setup tech this game is named for. The hold key becomes the
swap (hold for yaw), and FLIPS has a toggle if you prefer the old default.

Three procedural cars join the GLTF trio, each filling a gap the originals
map out (Octane = balanced dribbler, Fennec = flat-top readability, Dominus =
long flick plank): **BREADBOX** (taller + shorter: the dribble sponge),
**WEDGE** (longer + flatter than Dominus: the flip-reset blade), **ROLLER**
(a dome pod on a skateboard: tiny contact patch, soft round first touches).
Pick in CAR SETUP -> CAR. Tires no longer mirror-reflect (body 60% gloss,
rubber 15%) — the "chrome wheels" reflection bug is gone.

Two stadium maps (twenty-two total): **ARENA CLASSIC** — a flat soccer pitch
in a box: four walls, a ceiling ring with an open skylight (flip-reset real
estate overhead), corner banks, two half-buried spine tubes to roll up,
bleachers, corner light towers, a center circle and paired full-size goals —
and **COLISEUM**, a bleacher-ringed sand pit with torch towers and two nets.

Camera got finished: ANGLE is real boom pitch in degrees now (RL-style, -3
default), SHAKE is new and wired to landings/goals, and **middle mouse holds
a rear view**. Car cam keeps a *temporal heading* — smoothed, rate-limited,
frozen during flips, and it only accepts a full reversal after it persists
for a beat — so flips never whip the camera again. And the audio grew a
measurement seam: the game reports ball proximity, goal tension and bot
swarm each frame; the music already leans on all three (see
`sound.rs::set_sense` for the extension point).

## Bots and the sparta modes

SETTINGS -> GAME MODES fields two offline armies of ballchasing AI, running
the exact same car physics as you (with the classic bot cheat: infinite
boost): **SPARTA 50 V 50** lines up two fifty-bot phalanxes, blue vs orange,
on STADIUM's paired goals; **SPARTA 300** drops three hundred bronze
ballchasers into a goalless Zen Garden with one ball. Every bot wants that
ball with its whole heart — they chase, boost, pounce and flip into it, shove
each other apart through a crowd grid, show up as team dots on the minimap,
and the twenty nearest cast real ray-traced shadows. Bots render with
distance LOD (wheels within 60 m, nothing past 150 m) so the swarm stays
cheap. Game modes are offline-only; going online dismisses the army.

Boost economy is tunable now too: CAR SETUP grew **BOOST REGEN** (passive
units/second), **BOOST TANK** size, and an **UNLIMITED BOOST** mutator, next
to boost power. FLIPS grew **LANDING ASSIST** — a wheels-down magnetism that
kicks in while you fall near a surface with no flip active, so grounding
after a flip stops fighting you (ease it to 0% if you want raw RL rules).

## Nets: something to aim at

Most maps now grow **goal nets** — frame-bar pockets with a glowing back
panel, in every size, height and tilt: grounded goals, floating tilted sky
targets you thread from below, and on STADIUM two full-size goals facing each
other across the pitch. Bury the ball through the mouth and you get a chord
fanfare, a green burst, +2 combo and a lifetime GOALS stat. Four new map
styles join the sixteen (twenty total): **STADIUM**, **SKY GOALS** (nine
floating targets), **CARNIVAL** (mini-nets in bubblegum), and **AURORA**
(mirror-wet teal night with an orrery). Nets show as green squares on the
minimap.

## Graphics: real rays, no RTX

v1.2 ships a software ray tracer — the SOTA workaround for GPUs without RT
hardware, and a natural fit here because the whole world already *is* an
analytic SDF (that's what the ball bounces off). The forward pass rasterizes as
before, then every fragment marches the very same signed-distance scene on the
GPU: **ray-traced soft sun shadows** (cone marching, real penumbras),
**SDF ambient occlusion**, and **true mirror reflections** — one real bounce,
shaded with the same sun/emissive/fog model, falling back to the procedural
sky. The ball, your car, remote cars and the live gold pad orbs are injected
as proxy primitives each frame, so cars shadow the lawn, rings glow in the
ball's chrome, and on wet-floored styles (Observatory, Planetarium, Cavern,
Lighthouse) the whole neon world mirrors in the ground.

Per-ray bounding-sphere culling plus a baked terrain heightmap keep it fast:
tuned for Apple-silicon-class GPUs (M2 Ultra / M4 Max are the design targets —
wide ALU + unified memory love this workload; 1440p is the sweet spot, 4K is
heavier). The tracer now carries distance LODs: shadow and reflection step counts fall
off with primary distance, AO and reflections hand the far field to the fog,
and sky-bound rays skip the terrain heightmap entirely — visually near
identical, meaningfully faster at range-heavy views. `--gfx classic` returns
the flat v1.1 look for small GPUs. VIDEO settings add
the cinematic layer: **ACES filmic tonemapping**, exposure, vignette, film
grain and optional letterbox bars — all live sliders.

## Multiplayer

Online play rides the monorepo's fleet netcode (vendored in `vendor/`):
hosting publishes your profile + a session record through
`netcode::multiplayer` over the snapcore store (control plane), and the game
speaks its own 30 Hz UDP protocol on the socket that hands back (data plane).
The ball is host-authoritative; remote cars render with name tags and can
genuinely touch the ball on the host's simulation.

Pause → O: type a NAME, then HOST; a friend types your name under FRIEND and
JOIN. Joiners automatically load the host's map + seed. From the command line:
`--name kessler --host` or `--name friendo --join kessler`. bsgames can also
launch skydribble straight into a session via the standard
`BSGAMES_HANDOFF_*` env protocol.

Reach today matches the netcode crate's floor: the shared FileStore in the OS
temp dir — two instances on the same machine (or any machines sharing that
path) discover each other; the crate's `opendht` feature remains the
global-reach upgrade. The UDP data plane itself is normal networking and works
across any machines that can reach the host's address.

## The world

Everything is generated from a seed (pick one with `--seed N`; it persists in the save file):
rolling two-octave noise terrain, flattened near spawn, scattered with shapes
placed by rejection sampling. Upright rings track ball passes — thread the ball
through one and it flashes, bursts confetti in its color, chimes (pitch rises
with your combo) and scores. Chain rings within 7 s to grow the combo; keep the
ball airborne with aerial touches and the HUD times your air dribble. Big pads
refill 100 boost, small pads give 12, on RL-style cooldowns. Lifetime totals,
best combo, best air dribble, top speed and your settings persist in a plain
`skydribble.save` file next to where you ran the game.

Collision against the whole world runs on one signed-distance function (terrain
heightfield + analytic sphere/torus/capsule/rounded-box SDFs), so the car and
ball roll and bounce off every shape you can see.

## Physics notes

Scaled at 100 Unreal units = 1 m from Rocket League's known constants: gravity
6.5, max drive 14.1, boost accel 9.92, supersonic cap 23, ball radius 0.9325,
bounce e≈0.6 with tangential friction, car/ball masses 180/30 plus the
"psyonix" extra touch impulse that makes dribbles stick. Tuning constants live
at the top of `src/physics.rs`.

## Known quirks

- Nose direction and wheel tagging come from Sketchfab node names ("… - FR/FL/BR/BL").
  If a car ever drives visually backwards, tap F.
- Wheel spin direction is a sign convention; flip the `-spin` in `main.rs` if a
  model's wheels roll the wrong way.
- All sound is synthesized live (engine, wind, boost, ball pings, ring chimes,
  and a soft generative pentatonic soundtrack) — there are no audio assets, so
  there is nothing to configure and no licenses beyond the models.

## Asset credits (CC-BY-4.0)

Each `assets/<name>/license.txt` is preserved verbatim from the Sketchfab downloads.

- Rocket League ball, Octane, Fennec, Dominus — "Jako" (sketchfab.com/fairlight51)
- Red display Dominus ("dominus in rocket league") — 97Mason

`vendor/` carries two crates from the logscope-c4-273 monorepo: `snapcore`
(MIT, the fleet's Kademlia DHT store) and `netcode` (profiles / sessions /
bsgames handoff). The sixteen map moods are adapted from the ROOMS table in
that repo's logscope scene3d module.

Models are © their authors, licensed CC-BY-4.0; Rocket League is a trademark of
Psyonix/Epic — this is a fan-made, non-commercial homage.


`--sim jump` measures press-to-liftoff latency across surfaces; `--sim land` runs the landing matrix (nine drop poses, timed contact / flip-back / upright; `LAND_DEBUG=<pose>` for a frame trace). Powerslide now hairpins: yaw x1.7 while sliding with near-zero lateral grip.

MONKEY RUN: dusk-jungle canopy coasters with weather and floodwater — when a vine line floods, ride the flush.

`--sim mp` proves the bsgames multiplayer path on loopback: announce, resolve, dial, HELLO/WELCOME, state sync.

`--sim flip` audits flip fidelity: cardinal axis snap, full rotation, orientation error, landing attitude.

Handhelds (postmarketOS/Fairphone) auto-detect: low graphics, half render scale, on-screen stick + BOOST/JUMP, flick-to-flip and tap-to-move. `--sim plat` verifies it headlessly; `SKYDRIBBLE_MOBILE=1` forces it anywhere.

Gyro drive (ported from gyro-ball): reads IIO sysfs directly, accepts `./tilt_sensor | skydribble`, or falls back to monitor-sensor. `--sim gyro` verifies the stack headless.

Menus are fully touch-driven: tap to pick, drag to scrub sliders, flick down to go back, and the pause screen has real buttons. `--sim touch` checks the hit-test math.

`--sim shot` measures power shots: exit speed, loft, and distance at 1 s and 3 s.

KUIPER runs live sky events: a world collapses into a black hole every five minutes then detonates, and a new planet falls in to break the field. `--sim kuiper` checks the whole system.

`--sim spawn` audits every map's spawn point: clearance, settling, and final attitude.

`--sim dribble` checks the input-to-rotation mapping and how long the ball rides the hood.

`--sim drive` drives a car through every tube route and reports traversal and escapes.

`--sim platform` checks that cars ride moving decks; `--sim learn` measures whether the bots improve mid-match.

`--sim stream` measures the LAN render-assist split: transport, bitrate, ray-step saving and accuracy.

`--sim portal` verifies the portal transform: speed conservation, exit alignment, the fling, and no ping-pong.
