# skydribble — expanded design brief

This document is the expanded version of the original prompt: *"a Rocket
League inspired air dribble game, open world, bigger maps generated out of
playful shapes, in Rust + wgpu, reusing the reference code, with the provided
car and ball models."*

## Vision

The joy of Rocket League freeplay without the arena walls. You, a car, a ball,
and a huge sunlit island full of toy-like geometry that exists purely to be
dribbled around, bounced off, and flown through. No timer, no opponent — the
game is the feel: keeping the ball balanced on your nose at 60 km/h, feathering
boost, flicking it through a floating ring, catching it again on the far side.

## Pillars

1. Air-dribble flow first. Every constant is tuned around the RL dribble loop:
   pop the ball, jump, feather boost under it, micro-adjust with pitch/yaw/roll.
   Anything that fights that loop (harsh drag, sticky collisions, camera snap)
   is wrong.
2. Open exploration. One seamless ~900 m island, no walls, no loading. Terrain
   rolls gently so ground dribbles survive; landmarks are visible from anywhere
   so you navigate by sight.
3. Playful geometry. The world is built from kindergarten shapes at silly
   scales — rings, balls, capsules, boxes, lollipops — in a pastel palette.
   Readable silhouettes over realism.
4. Generous, authentic physics. Real RL numbers where they're known (gravity,
   boost, supersonic, ball restitution, masses, the extra touch impulse),
   forgiving everywhere else (ground snap, soft landings, slow coast decel).

## Moment-to-moment mechanics

- Driving: throttle to 14.1 m/s, boost to a hard 23 m/s supersonic cap, yaw
  steering that tightens at low speed, powerslide that cuts lateral friction
  for drifty turns.
- Jumping: impulse along the car's up + a short hold-boost window; a second
  press is a double jump, or — with a direction held — a dodge/flip that adds a
  velocity impulse and a fixed 0.65 s tumble, exactly the RL flip cadence.
- Aerials: torque control on all three axes (pitch strongest, roll fastest),
  exponential damping when sticks are released so the car settles, angular
  speed clamp so you can't spin out.
- Ball feel: e≈0.6 bounce with tangential grab, rolling with visible spin,
  capsule-vs-sphere car contact plus the "psyonix impulse" — an extra push away
  from the car's center on hits — which is what makes nose-balancing possible.
- Boost economy: 6 big 100-pads on a ring around spawn, ~40 small 12-pads
  scattered along natural dribble lines, RL cooldowns (10 s / 4 s).

## World generation (playful shapes)

Seeded SplitMix64 → value-noise terrain (two octaves, ~3 m amplitude,
smooth-flattened inside r≈90 m of spawn) plus rejection-sampled shape
scatter in a 25–430 m annulus, minimum spacing so every shape is approachable:

- ~26 upright rings at heights 6–45 m — the aerial goalposts. Ball passes are
  detected by plane-crossing inside the hole radius, flash the ring, and score.
- 6 fat flat halos you can land on and dribble around like donuts.
- ~34 balloon spheres floating 6–30 m up — soft aerial pinball bumpers.
- ~14 buried domes — smooth hills that launch ground dribbles skyward.
- ~22 floating box platforms at 4–26 m — stepping stones for wall-less "wall"
  play and double-touch practice.
- ~16 tilted box ramps resting on the ground, 15–30° — kickoff ski jumps.
- ~22 lollipops (capsule stem, sphere head) — slalom posts and backboards.
- 5 grey plinths near spawn displaying the red show-car Dominus.

All of it is collidable through one world SDF (heightfield + analytic shape
SDFs with a 4-tap normal), so physics and visuals can never disagree.

## Presentation

- Procedural sky: gradient dome, warm dusk band, sun disc; matching exponential
  fog that thins with altitude so high aerials feel airy.
- Hemisphere ambient + Blinn-Phong sun, subtle 4 m/24 m grid on the terrain for
  speed perception, emissive boost orbs that shrink to dots while on cooldown.
- Real Sketchfab models: Octane, Fennec, Dominus (switchable), the RL ball with
  its proper texture, spinning wheels with front-wheel steer, all auto-oriented
  and auto-scaled at load from wheel-node names.
- HUD: minimal boost bar, supersonic tint; scores and speed in the window title.

## Tuning table (authoritative copies live in src/physics.rs)

| constant | value | note |
|---|---|---|
| gravity | 6.5 m/s² | RL 650 uu/s² |
| max drive | 14.1 m/s | RL 1410 |
| boost accel | 9.92 m/s² | RL 991.7 |
| supersonic cap | 23 m/s | RL 2300 |
| boost drain | 33.3/s | 100 ≈ 3 s |
| jump impulse | 2.92 m/s | + hold force ~0.19 s |
| dodge impulse | 5.0 m/s | body-frame input dir |
| ball radius | 0.9325 m | RL 93.25 uu |
| ball restitution | 0.6 | tangential ×0.714 |
| masses | 180 / 30 | car / ball |
| touch impulse | 0.3 × rel speed | the dribble-maker |

## Release polish (v1.0)

Shipped on top of the original cut: fully procedural audio (detuned-saw engine,
speed-scaled wind, boost hiss, impact pings, combo-pitched ring chimes, and a
generative pentatonic soundtrack — no audio files), a pixel-font HUD (boost,
speed, rings, combo with timer, air-dribble clock, FPS), a north-up minimap,
pause menu with controls + lifetime stats + credits, a title splash, 4x MSAA,
particles (boost trail, ring confetti in the ring's color, bounce dust, pad
sparkles, landing puffs), a `skydribble.save` file for stats and settings,
fullscreen toggle, CLI flags (`--seed`, `--fullscreen`, `--mute`), a procedural
window icon, optional gilrs gamepad support behind `--features gamepad`, and an
offline naga shader-validation test wired into `cargo test`.

## v1.1 — input, worlds, and other people

Rebindable input (logical-key so non-QWERTY layouts bind their real characters;
keyboard + mouse + controller all capture in the in-game controls screen;
classic + Dvorak presets; `:` toggles steer-as-air-roll), sixteen selectable
map styles adapted from logscope scene3d's ROOMS table (palette, fog/zenith,
terrain amplitude, shape-family weights, spawn centerpieces; terrain mesh is
rebuilt in place on switch), and multiplayer over the monorepo's fleet netcode:
snapcore-store control plane (host announces a SessionRecord, friends resolve
it by name), a bespoke 30 Hz UDP data plane with host-authoritative ball,
remote cars with name tags that really touch the ball, map/seed sync in the
WELCOME, and bsgames BSGAMES_HANDOFF_* launch integration.

## v1.2 — rays without RTX

The physics SDF was always the renderer's shadow twin; v1.2 finally lets the
GPU march it. Shapes stream into a storage buffer (world shapes + per-frame
proxies for ball/cars/pad orbs, each raster surface knowing its own proxy slot
so reflection rays skip themselves), terrain bakes into an R32F heightmap
(manual bilinear, no float-filtering feature needed), and the forward fragment
shader adds three ray-traced terms: cone-marched soft sun shadows, 4-tap SDF
AO, and a single true reflection bounce gated by Fresnel and per-material
gloss — with a per-style ground wetness so the night maps become mirror
floors. Acceleration is per-ray bounding-sphere culling into a 16-slot
candidate set; on 300+ shape maps that turns each march step from 300 SDF
evals into a handful. Sized for M2 Ultra / M4 Max class GPUs; `--gfx classic`
keeps the old flat path.

## v1.3 — the settings layer

A single Config struct now owns every tunable, threaded live through physics,
camera and renderer, edited by a data-driven menu system (rows are slider /
toggle / preset / link records with fn-pointer accessors, so a new setting is
one table line) and persisted as `cfg` lines in the save. All physics sliders
are multipliers over the RL-baseline constants, which keeps 100% meaningful.
Flip mechanics were completed: directional flip spin was already the dodge;
added flip cancels (opposite-pitch kills the x spin), flip resets (underside
contact vs ball or non-drivable surfaces restores the dodge, strict/loose
alignment, 0.3 s cooldown, event → chime/sparkle/toast/HUD pip) and the
unlimited-flips mutator. World size regenerates the map in place on leaving
the mutators page; ball radius and car scale are recomputed every frame so
sliders feel live.

## v1.4 — feel, cinema, targets

Three feel fixes: fixed-step physics now renders **interpolated** (prev/cur
snapshot lerp by the accumulator, with a teleport snap guard) which kills the
120 Hz-vs-frame-rate stutter; flips shed their spin the instant the flip
timer ends so held keys stop tumbling the car; and touching down clears all
dodge state and angular velocity with a stronger up-alignment, so the ground
blend always wins. Menus were rebuilt on glass panels with shadowed text and
gained full mouse support (hover-select, click-activate, bar-click sliders,
wheel nudges, right-click back) hit-tested against the same exported layout
metrics the HUD draws with. The renderer gained a cinematic tail — ACES
tonemap, exposure, vignette, hashed film grain, letterbox — as live VIDEO
options. And the world gained NetGoal pockets (frame bars + glowing back
panel, plane-crossing scoring with per-net cooldown, flash + fanfare +
combo feed) scattered at random sizes/heights/tilts, plus four net-centric
styles: STADIUM (paired full-size goals), SKY GOALS, CARNIVAL, AURORA.

## v1.5 — armies, assists, and a faster tracer

Landing round two: the flip now sheds ALL its spin at completion, and a
configurable landing-assist magnetizes wheels-down (scaled by proximity,
eased while the player is actively steering) so post-flip grounding stops
fighting held keys. Boost became an economy (regen / tank / unlimited).
bots.rs adds ballchase AI on the real car physics via physics::step_car_only,
with a uniform-grid crowd separator, two spawners (50v50 phalanxes on
STADIUM, 300 bronze on goalless ZEN), team tints, minimap dots, draw LOD
(wheels < 60 m, cull > 150 m) and rt-proxy LOD (nearest 20 shadow-cast). The
ray tracer gained distance LODs: step counts lerp down with primary distance,
shadows stop past 230 m, AO past 120 m, reflections gate at 150 m with
range-shrunk tmax, and upward rays skip the heightmap. Menus gained an
information hierarchy: section headers with hairline rules, gold chevrons on
action rows, width-fitted text (the binds overlap fix), and bracketed
[KEY] legends replacing prose footers.

## v1.6 — one stick to rule the air

Air steer defaults to roll (hold-key swaps to yaw; cfg toggle reverses),
which makes slide-jump drift carry into dribbles. Camera: temporal-heading
car cam (rate-limited chase, flip freeze, 0.35 s reversal deadband before
the lerp commits), degrees-based boom angle, impact shake, MMB rear view.
procars.rs builds three composite-mesh cars via new meshgen bake/merge
(analysis: the GLTF trio spans balanced/flat/long — the additions take
tall-sponge, flat-blade, round-pod extremes). Reflection fix: per-tag spec
(tires 0.15) kills the chrome-wheel artifact. Two focal builders (9: walled
arena with skylight ceiling, banks, spines, bleachers, towers, paired goals;
10: coliseum ring) bring STYLE_COUNT to 22. sound.rs gained the adaptive
sense seam (prox/tension/swarm atomics, consumed by the music). Menus moved
to soldier's design language: slate glass, cyan info ladder, per-section
inset cards.

## v1.7 — analog keys

The uploaded SDK zip ships the runtime, not the wrapper crate, so analog.rs
carries a five-symbol portable loader (libloading, per-read dlsym) with a
learned logical->HID map built from winit's dual-identity key events —
layout-independent by construction. The wind curve is the design ask
verbatim: deadzone 0.06, >=0.97 snaps to 1, otherwise floor+(1-floor)*t with
floor = 1-depth (default depth 0.45). Movement axes only; jump/boost/slide
stay digital. No Wooting hardware or GPU in the build container, so the
integration is compile-verified and the curve unit-reasoned, not play-tested.

## v1.8 — the wall update

Wall driving fell out of the existing math: the ground branch was already
normal-relative, so opening the grounded test (orientation-gated on steep
surfaces, speed-gated peel at 3.2/6.0 m/s for walls/ceilings) plus a strong
stick force was the entire physics change. Attribution: Ball.last_touch
(WHO_PLAYER/WHO_REMOTE/team codes) stamped in collide_car_ball, goals carry
it, sparta 50v50 keeps a scoreboard + airhorn. Rendering went HDR: scene
resolves to Rgba16Float, bright->blurH->blurV at half res, composite adds
bloom and runs screen_fx once (moved out of fs_scene/fs_sky); post resources
live at group(1) bindings 5-8 so pipeline layouts stay two entries; targets
rebuild on resize.

## v1.9 — the sim closes the loop

For the first time the AI is tuned by measurement: sim.rs runs bots + ball
+ world headless (physics::step_ball and ball_goal_check made public for
it) and prints style-first scores. Findings that shaped the code: the
steering sign in the shared steer helper was inverted (every bot since v1.5
turned away from its target — invisible until now); goal detection lived
only in the coupled step(); ground-phase controls fight the air (throttle =
pitch), so airborne bots get a recovery behavior; the wall-peel can wedge a
slow car on curved geometry (bots hop free). Soft-touch physics: e =
(0.10 + 0.25·soft)·bounce with soft = |vn|/8, plus 16% horizontal
speed-matching when the ball rides the hood. Sim truth at ship: pass4
3/6 goals avg 22.5 s, defender travel ~379 m, air1 pops and carries but
does not yet finish solo — the striker constants (Pop aim 65°, AirCarry
hold −0.55·goal −Y·(r+0.45), boost gates) are the live knobs.

## v1.10 — one key, two rotations

Slide became the air-roll modifier (roll_mix = slide XOR default-toggle),
with Wooting analog depth as a continuous roll/yaw blend via a raw axis
(deadzone only — the wind curve is for commands, not ratios). Retired
Action::AirRollToggle (unknown ids load-skip, so old saves are fine).
Jump buffering (90 ms, cleared by an executed dodge) plus a 0.55 landing-
frame upright snap and a slightly earlier ground catch (clearance +0.11)
answer the "jump back is slow / flip land not right" pair. Menus: type
floor 0.0072, bars selected-only, dimmer values, slogan carousel.

## v1.11 — true settling, honest turtles, real mirrors

Landing became physics, not assistance: (a) end-pivot — a car propped on
its nose or tail (center hovering ~3 cm above the grounded threshold, the
"stays at 45 deg forever" bug) gets a gravity torque about the planted end
(tau = r x F along f x -Y, gain 3.4 with pivot friction) that lays it flat
in ~0.75 s; (b) floors now require ori > 0.25 to count as DRIVING contact,
so a turtled car enters a body-contact regime instead: pushout, real
tangential friction (2.4/s), 45% attitude authority to wriggle, boost
scrapes you along, and an inverted hull past the balance point rolls toward
its wheels. The land-assist slider remains the learnable layer on top.
Reflections stopped being placeholders: the ball proxy is kind 4 (panel
bands in ball space, live rotation — it spins in the floor), cars are
body+cabin composites in their signature tints (octane orange / fennec
silver / dominus gunmetal; procars use their real prim colors), the local
car adds four wheel spheres.

## v1.12 — nine cars, roles, real passes, vsync

VSYNC toggle (Fifo vs AutoNoVsync, live reconfigure). Three cars complete
the roster at nine: LONGBOY (1.04 half-length limo plank), CUBE (0.34 cubed
chaos hitbox), SHARK (streamliner + dorsal fin). AI grew roles: one
designated ATTACKER per team (nearest to ball) presses; everyone else holds
support lanes ~11 m out — the mob spreads instead of dogpiling. Shared
prejump helper (ballistic lookahead at 0.30/0.48 s, jump when the arrival
lands 1.0-3.6 m overhead within 2.8 m) plus mini-aerials (2.5-8 m balls met
in the air with committed boost). Passers finally pass: a matured carry
re-aims the whole pop/carry/flick machine at the next teammate (lead 0.4 s)
and flicks at 2.2 m; receivers run goal-side lanes and prejump receptions.
Sim deltas vs v1.11.1: pass3 5.7 -> 8.6, defender-drill air touches 4.7 ->
8.6, air consistency way up; pass-mode goal conversion (lost to the v1.11
contact rework) remains the open knob alongside the solo air1 finish.

## v1.13 — the marble run update

From the perplexus design doc: tube + ribbon families, reusing volume,
gravity as a property of the road. Sdf::Shell (mean radius / half-length /
wall half-thickness; shader kind 5; meshgen::tube outer+inner walls +
annulus caps; broadphase reach added) makes hollow drivable pipes with
working rays inside. World.grav: Vec<GravNode { pos, down, r, tube }>;
gravity_dir() = the brief's formula verbatim (0.9 nearest + 0.1 runner-up,
tube nodes radial, fade to -Y by 1.5r). Physics: gravity is a field
(car + ball), steep/ceiling/floor gates use n . local-up, landing assist
falls along gdir, end-pivot torques about gdir. Camera stays Y-up for now —
the honest rough edge inside full loops. Focal 11 = helix + approach tubes
(3 nodes/segment at 1.55x tube radius); focal 12 = 16-slab corkscrew with
per-slab baked down, banked 0 -> 2.6 rad.

## v1.14 — the run gets strange, the camera learns gravity

Roll/yaw mix rebuilt as interpolation with hysteresis: raw slide depth
snaps below 0.15 / above 0.85 (decisive digital ends), stays analog
between, and smooths at 14/s — the regime can no longer flicker at the
actuation point, which was the "finnicky, not consistently activating"
switching. Camera: full gravity awareness — a smoothed, rate-capped
(2.4 rad/s) local-up follows world.gravity_dir, the temporal heading
lives in the up-plane, terrain clamping disengages off-world-up, the view
basis guards degeneracy, and CAMERA -> GRAVITY CAMERA (0-100%) blends
back to classic Y-up. PERPLEXUS became a real grammar composition:
approach tube -> striped helix -> summit junction -> EITHER drop shaft
into a wall-of-death DRUM (radial gravity, pad ring, tangential exit
tube) OR S-bend descent -> 12 m GAP JUMP -> three-hoop stepped FUNNEL;
plus a figure-eight tube crossing mid-field and a bare balance-beam rail
from summit to drum rim. Boost pads run every spine; emissive lane
stripes alternate segments.

## v1.15 — the jungle gym

Dvorak preset rebuilt to the requested layout verbatim (./E/O/U throttle+
steer, ,/P dedicated rolls, Z jump, V boost, A slide, SPACE ballcam; pad
face buttons mirrored). Jump depth is analog: wooting depth trims the
impulse (x0.95-1.0) and hold-thrust (x0.88-1.0) — a feathered Z takes off
shallower for dribble-pitch control, digital is exactly stock. Tubes are
glass now (alpha 0.55, both wall faces) so the camera passing through
reads; a configurable checker SCREEN DOOR (post.z, default 8%) sits in the
composite. PERPLEXUS v3: tube radius 5.8, flared rim tori at every mouth,
ground aprons, then the gym — a 16-anchor random lattice (two nearest-
neighbor links each, knuckle rims, spine pads), three sheer drop shafts,
THREE SPINNER rotors and two slow riding discs (Anim::Spin rebuilds
mat+inv_rot so physics AND rays track them), closing doors sliding across
the figure-eight (Anim::Door), pinball ZONES (Vibe jitter field / Slip 10%
grip ice / Boost strips, translucent discs), three blast-off LAUNCHER
rings (26 m/s along the arrow, flings the ball too), and three emissive
gravity-wrap orbs you can loop like moons.

## v1.15.1 — the sign of gravity

Field fixes, all four confirmed mechanisms. (1) SETTLING: the v1.11
end-pivot torque axis was mirrored (f x g instead of g x o), so gravity
pushed propped cars INTO their prop — it now derives per probe as
g x o_dir with four rim probes (nose/tail/left/right, Car gained half_w),
gain 6.5 against the 4/s air damp: any propped pose lays flat in ~0.75 s.
(2) WAVE DASH: the floor gate's tilt requirement rejected mid-flip
contact; a car in an active dodge is now exempt, and landing mid-dodge
converts the flip into +4.2 m/s along the surface (spin dies, momentum
lives) — the wavedash proper. (3) The reflection's 90-degree turn: proxy
half-extents were built length-on-X, but the car's length runs along Z —
one shared push_car_proxy now uses true axes. (4) "Not the real model":
that proxy is a five-piece silhouette per model (low body, set-back
cabin, tapered nose wedge, four wheels for the local car), proportioned
from each model's real dims.

## v1.16 — the landing matrix

`--sim land` drops the car in nine poses (flat, nose 45/90, tail 45,
side 45/90, roof, tumble, spin) from 5 m and 2.2 m, timing first driving
contact, flip restoration, and upright — with land-assist at default and
off. LAND_DEBUG=<pose> traces a pose frame-by-frame; SETTLE_DBG=1 prints
the settle gate values live. The matrix drove five fixes:

1. Assist=default rights EVERY pose mid-fall (all nine: 0.93 s) — the
   default experience was already consistent; the raw physics under it
   was not.
2. Hover stall: each contact frame's pushout lifted the probe clear, so
   settle torque duty-cycled to ~zero against continuous air damping.
   Fixed with a near-contact torque BAND (1.9x rim, linear weight) and a
   gentler normal-velocity kill (0.5).
3. Exact balance points (nose-90/side-90) have mathematically zero
   moment; numerical perfection held them forever. Symmetry now breaks
   along a HORIZONTAL body axis — a vertical pick just yawed in place.
4. The turtle was being stabilized: a signum-in-body righting scalar
   flipped sign mid-roll and fought every lean. Replaced with
   frame-independent righting (torque about up_car x up_world, monotone
   from any pose, drive 4 + 11 x inversion).
5. Clearance here is wheel-based (wheel_r + 0.06 = 0.19), not half_h —
   every fixed reach was 0.12 tighter than intended, and a tumbled car
   parked on a nose probe at d = 0.66, two centimeters past the righting
   gate. Reach is now probe-proximity (`near`), model-size-proof.

Jump restore relaxed to ori > -0.15 (90-degree poses were knife-edge at
exactly 0). Final assist-off numbers: 45-degree poses upright in
1.1-1.8 s, 90-degree in 2.1-2.8 s, roof 2.5-2.9 s, tumble 2.7-3.1 s;
flip back 0.44-0.92 s everywhere except roof/tumble (1.1-1.6 s).

Handling: base steering rate (3.0 - min(sp*0.050, 1.65)).max(1.15) —
tighter everywhere than the old 2.6/0.055/0.95 — and powerslide is a
hairpin: yaw rate x1.7 while sliding, slide grip cut to 1.3*sqrt(k) so
the rear swings.

## v1.17 — suspension and the wheel net

Three feel fixes off the back of the landing matrix. (1) WHEEL
SUSPENSION: the rigid ride (full normal-velocity kill + positional
snap) became a spring-damper — the bottom 45% of clearance is a
compression zone (spring 58 toward ride height, damping 24 down / 9 up,
hard stop at axle bind), with `Car.susp` (0..1, smoothed) bobbing the
drawn body ~4.5 cm into its compression. Landings sink and rebound,
bumps stay glued, contact reads solid. (2) GROUND-GIVES-FLIP wheel net:
the restore detector was four slim body probes — not even at the
wheels — so honest wheel brushes missed. A four-corner check at the
actual wheel positions (window wheel_r + 0.12, wheels-down-ish, costs
only while flipless) hands the flip back the moment any tire touches.
(3) Settle pace: band torque 9 -> 15, righting base 4 -> 5 — the
45-degree family now stands in ~0.6 s after first touch (1.12 s total
from a 2.2 m drop, was 1.41), 90-degree in 1.6-2.2 s, roof/tumble
2.0-2.45 s, flip back unchanged at 0.44-0.91 s.

## v1.18 — clusters, carving, coasters

PERPLEXUS v4. The world SDF learned SUBTRACTION: Shape.cut marks a
shape as a CSG cutter (solid = max(union, -nearest cutter), numeric
normals stay consistent for free, cutters skipped in draw + mirror).
That one primitive unlocks the whole turn: curved pipes are chains of
short shells whose elbow seams are carved open by joint cutters
(smooth continuous bore, no new SDF kind, no new meshes); hollow
junction CHAMBERS are glass spheres hollowed by an inner cutter with
mouth cutters where tubes attach — genuinely open holes you drive
through, down up sideways and onward.

Three clusters now: the v3 sculpture is the TECHNICAL heart (its
lattice gained real width variance — a quarter of links are 9-15 m
freight bore); a COASTER cluster (11-17 m pipes, a grand full vertical
loop at 2.4x bore radius, a swooping S, a second loop, two ground
mouths with rims, aprons and an entry launcher); and a PLAZA cluster
(three hollow chambers with interior radial gravity, chamber-to-chamber
tubes with carved wall mouths, and a ground riser INTO each chamber —
launcher-assisted entrances from below). Freight connectors (7.5-9.5 m)
triangulate the cluster hubs with rim collars and carved midpoint
joints.

Moving parts: Anim gained Orbit and Slide. Nine orbiting sky panels
circle the hubs, sliding stepping-stones shuttle the two big gaps, a
rotor sweeps the coaster summit — and a roaming MOON orbits the plaza
dragging its own gravity well (GravNode.follow syncs a node to an
animated shape every update).

Bots learned the pipes: World.routes stores drivable waypoint
polylines (coaster line, connectors, chamber ring, every ground riser);
route_locate finds whether a point is inside a route. When the ball is
in-tube, each bot rolls its tube_iq (0.35-0.95, fixed per bot) once per
5 s window — DROPOUT — and on success drives for the best mouth and
gives chase through the bore; on failure it plays the outside. Some
bots out-think the pipes most of the time, none always.

`--sim genall` generates all 24 styles headless (worldgen panic +
stats smoke test).

## v1.19 — the jump race, the grid, the leagues

THE JUMP BUG: not physics at all. The input edge was copied into
Controls and cleared BEFORE the fixed-step loop — any frame where
acc < FIXED_DT (no substep, common at high fps) ate the press outright,
a probabilistic drop that read as "sometimes takes seconds". The edge
is now consumed INSIDE the substep loop. `--sim jump` (new, permanent)
proves the physics side: press-to-liftoff worst 33 ms, 0 fails across
120 timed presses on flat and bumpy maps, idle and driving.

PERFORMANCE: (1) physics sdf() walks an XZ spatial hash (56 m cells,
shapes inserted by reach + 6 m probe margin) instead of scanning every
shape — the landing matrix reproduces BIT-IDENTICAL, so the grid is
provably transparent; (2) gravity nodes thin at generate (5.5 m dedupe:
perplexus 354 -> 216); (3) the mirror list distance-culls (hard 210 m,
small shapes out past 110 m) so the per-ray gather pays for far less;
(4) tube LOD: shells past 140 m swap to a 10-segment mesh (M_SHELL_LO,
same SDF); (5) screen-door checker halved in the shader and default
0.08 -> 0.05 — it read far too heavy on glass. The object pipeline
already backface-culls; the sky/post pipelines correctly do not.

MODES: bot modes run on ANY map (sparta no longer force-loads its
stadium). New VERSUS league: 1v1 through 5v5 — you play blue with
n-1 teammates (Striker / Defender-at-your-net / Passer lineup) against
a mirrored orange squad, kickoff rows around the ball, team scoreboard
live. Perplexus playground grew ~50%: lattice radius 34-118, 20
anchors, 72 m links, clusters pushed to 205 / -195.

## v1.20 — bake and project

The ray hot-cost was the gather: every fragment scanned the ENTIRE
shape list up to three times (soft shadow, AO, reflection) doing
bounding-sphere culls before any marching. Now the static scene is
BAKED into a candidate grid at map load: 40x40 XZ cells, each holding
the nearest 28 static shape ids within a 135 m relevance horizon
(animated shapes inflated by their travel radius so the roaming moon
stays visible in mirrors). At runtime the fragment PROJECTS into its
cell and gathers only that list plus the live dynamics (car proxy,
ball, bots — RTS[static_n..]); shadow, AO and reflection all share the
projection. On perplexus that is ~30 candidates per gather instead of
~350 — an order of magnitude off the dominant per-pixel cost, with
bit-identical physics (all sims reproduce) and the naga test
validating the new RCELLS storage binding.

Plumbing: build_rt_shapes pushes world statics FIRST in stable shape
order (the bake indexes that prefix; the old per-frame distance culls
were removed as superseded), RtIds carries static_n into G.rt.z, and
`--sim genall` bakes every style and reports average candidates per
cell. NOT shipped: the temporal history buffer (half-res reflection
target + previous-frame reprojection + disocclusion rejection). It is
the right next step but needs eyes on a real GPU to tune ghosting —
blind-shipping smear would be worse than the frames it saves.

## v1.21 — junction rooms, MONKEY RUN, weather, floodwater

PERPLEXUS NAVIGABILITY, root cause: lattice tubes ran anchor-to-anchor
straight THROUGH each other — a wall at every joint. Every linked
anchor is now a hollow glass JUNCTION ROOM (radius 1.45x its widest
tube), carved open toward each tube, with interior radial gravity;
tubes pull back to the room walls and every mouth is cut. Three links
per anchor (was two, reach 85), a grand peripheral ring loop, emissive
guide hoops every 20 m inside big bores, and two ties wiring the
lattice into the coaster summit and a plaza chamber. Perplexus:
353 -> 484 shapes, 10 routes.

MONKEY RUN (style 24, focal 13): a huge dusk-jungle canopy coaster.
Eight giant trees (210-340 m ring), four hollow treehouse chambers,
and three LONG floodable vine lines — THE CONSTRICTOR (8 m bore),
CANOPY EXPRESS (10 m, full vertical loop), THE BIG SQUEEZE (12.5 m,
loop) — built from a new quadratic-bezier `swoop` chain (alternating
banked curves, carved elbows) threading root mouth -> treehouses ->
canopies -> ground splash-out, with slingshot launchers at the roots,
banana boost straights in the bores, rope bridges, six swaying vines
(Slide), four orbiting parrots, mud-wallow slip pools and a drum
hollow. 767 shapes, 3 routes, all floodable.

WEATHER (focal-13 maps): a 90 s clear -> rain -> storm cycle. Rain
densifies fog, raises mirror wetness, and falls as ~380 instanced
streaks around the car; storms add hashed LIGHTNING (fog whites out,
exposure spikes for a tenth of a second).

FLOODWATER: every 48-80 s a vine line floods for 11 s. The tube
shapes glow (waterline via flash), a toast names the line, the water
takes ~1 s to fill — then anything inside (you, the ball, the bots)
is servo-dragged downstream at 26 m/s with axis centering and flushed
out wherever the line ends. World.current() is the force field;
route_shapes / route_names / floodable are the per-route metadata.

## v1.22 — real time, real hits, real fillets

GAME-TIME BUG confirmed: frame dt was clamped at 0.05 s, so below
20 fps the simulation ran SLOWER than wall time — lag literally slowed
the game. Clamp is 0.25 now, with a 10-substep spiral-of-death guard
that sheds backlog instead of freezing.

HITTING THE BALL: the car-ball envelope was a thin spine capsule of
radius half_h x 1.12 (~0.35 m) — no width, no nose forgiveness. Now:
height near stock (carry balance depends on it), width counts for half
its excess, +0.04 margin, and a +0.14 LUNGE margin while mid-dodge.
Push direction blends 2.5% of car velocity: hits go where you drive.

HALF-FLIP FIXED: `car.ang = dodge_ang` hard-overwrote roll every dodge
frame — the 180 roll after a cancelled backflip was impossible. Roll
is live through dodges now, and a cancel WITH roll intent (|roll|>0.2)
truncates the dodge to 0.16 s for instant control.

PERF at low quality: AO off below 45%, shadow reach/steps scale with
quality, reflections skip beyond 70 m below 35% and scale tmax/steps.

CAD FILLETS: smooth-min union (k=2.2) for the blend group — all tube
shells automatically, junction rooms explicitly — in the physics SDF
and the mirror marcher (kind+16 flags the group). Tube-into-room and
tube-into-tube joints now blend like machined fillets instead of
creasing. Junction rooms went near-clear (alpha 0.16-0.18) with a
two-hoop emissive cage, so you can SEE through every joint.

BOT TUNING, sim-driven, three iterations: the power shot (hop then
dodge THROUGH the ball) initially fired for every brain — defenders
abandoned nets, carriers hopped out of their own dribbles, and the
roll-less cancel truncation killed every flick. Final form: power shot
for Ballchase attackers only, cancel truncation requires roll intent,
carry-safe ball radius. Result: air1 3/4 goals (best ever), pass4
style 75.4 (record), defender travel back in noise range.

## v1.22.1 — the boom obeys

"Camera settings don't change the camera": the settings were wired all
along — the geometry was eating them. The boom was built along the raw
3D direction to the ball, so any high ball dragged the camera down
behind the car where the terrain floor clamp pinned it: dist, height
and angle all washed out. New `--sim cam` probe caught it in numbers
(boom_up stuck at the floor for height 0.6 AND 3.0). The boom now
lives in the UP-PLANE (flattened chase direction) at exactly cfg dist
and height — probe reads boom_up 0.60/3.00 for settings 0.6/3.0,
|boom| = sqrt(dist^2+height^2) to the centimeter — while only the LOOK
tilts up at the ball, which is the Rocket League convention.

## v1.23 — feel dials and cheaper frames

FLIP FEEL: base dodge spin toned 8.4 -> 7.4, and the rotation TAPERS —
full rate through the first 55% of the flip, easing to 42% by the end,
so flips fade out instead of cutting. INPUT EASE: player steer/yaw/roll
get a ~55 ms temporal buildup with a whisper of center-softening (the
detent) — player-side only, bots keep raw controls so the sims stay
comparable. Three new dials in the handling menu, all 0.5-1.5x in 5%
steps around a 1.0 default: FLIP SPIN (dodge rotation), STEER RATE
(ground yaw + air yaw/roll authority), INPUT EASE (buildup time).

GRAPHICS: raster distance cull — small shapes stop drawing past 250 m,
everything past 640 m (monkey run was pushing all 775 instances every
frame, hoops and vines included) — and emissive-dominant fragments
(em > 0.5: guide hoops, launch rings, orbs) skip the soft-shadow and
AO marches entirely, since glow owns their shading anyway.

## v1.24 — exact mirrors, the pinch, the pole

EXACT-MODEL REFLECTIONS: every gltf car's vertex cloud (~520 surface
points, captured at load) bakes into a 3D distance atlas — 28x18x32
voxels per model, twelve slabs stacked in one R8 texture, distances
byte-encoded — and the reflection marcher gained kind 8: box early-out,
then trilinear SDF sampling scaled by car size. Local car and peers
now mirror as their ACTUAL models; procars keep the five-piece proxy
(they are boxes anyway), bots keep proxies for budget.

THE LONG POLE: the ball was pushed into the mirror list with kind 4 —
which is not a shape, so it fell through to a rounded box with half
extents (r, 0, 0): literally a pole, with no panel bands because the
identity tag in e.w was never set. Sphere kind + explicit tag; the
mirror ball is round and spinning again.

THE PINCH: squeezed between a car touch and a world surface in the
same frame, the ball no longer chatters — total speed converts to a
kinetic exit along the wedge seam (lifted slightly off both faces),
gamified exactly like a Rocket League pinch, 0.3 s cooldown.

ROLL STACKING: roll input during a dodge was ADDING 8.5 rad/s on top
of the flip's own spin (1.5-2 rotations on a side flip). Reduced to
5.0 with a hard 8.0 cap on the total — half-flips keep their roll,
flips stop over-rotating.

Defaults per feel report: STEER RATE 0.90, INPUT EASE 1.10 (existing
saves keep their values — tap ENTER on the row for the new default).
CAMERA rows now toast a live readback of the applied value.

## v1.25 — multiplayer, proven

Skydribble already spoke the fleet control plane (netcode::multiplayer:
host announces a SessionRecord on the store, a friend resolves it by id,
the game's own UDP is the data plane) and the bsgames handoff. v1.25
finishes the experience and — for the first time — PROVES the wire.

`--sim mp`: two complete Net stacks in one process share a temp
FileStore and run the real path end to end on loopback — announce,
resolve by friend id, UDP dial, HELLO, WELCOME, three seconds of state
sync. Verdict line: MP LOOPBACK: PASS, with both peers' replicated
positions within 0.1 of ground truth (smoothing lag). The HELLO/WELCOME
protocol had never executed in-container before this harness.

Fixes it surfaced or carried: (1) HOST HEARTBEAT — records and presence
carry TTLs on the store, so an announce-once host silently fell off the
friend list mid-match; netcode gained `Multiplayer::reannounce()` and
Net re-publishes every 20 s while hosting. (2) Handoff DISPLAY NAME —
from_handoff used the raw profile id as the player name; it now uses
the "<id>|<name>" display half, so in-game tags match the bsgames
friend list. (3) Net::host_with / join_with point the fleet store at an
explicit path (what makes the one-process loopback pair possible).

The hub patch (integration/bsgames-skydribble.patch) verified current
against the reference monorepo's registry. The snapcore DHT groundwork
from the mis-aimed request stays parked — netcode::store already names
SnapcoreStore as the fleet fallback, so it lands exactly there later.

## v1.26 — five arenas and two new systems

HIGH-QUALITY PERF (the "nice tracing" pass): faces pointing away from
the sun skip the shadow march entirely — shadow only ever scales the
diffuse term, so the image is IDENTICAL and roughly half of all
fragments stop paying; and low-spec objects (f0 < 0.14) trade the full
reflection march for a sky-tinted sheen, halving the glass bill.

TWO SYSTEMS. TILES: a floor panel or door the BALL operates —
Drop tiles take hp hits then fall away (Sdf::None: the hole is real)
and count back in after 12 s; Door tiles slide open for 6 s on one
solid hit. Strikes route through the new ev.ball_bounce. BODIES: free
asteroids that drift, collide with the world and each other, and trade
momentum with car and ball by mass. Found by the new `--sim mech`
harness: bodies were double-represented — dynamic in step_bodies but
solid WALLS in world.sdf, so the ball bounced off scenery before the
shove ever saw closing velocity (each body also fought its own SDF).
Their shapes are Sdf::None now; the mesh draws, step_bodies owns the
physics. Verdict: asteroid 0.91 -> 15.93 m/s off one slam. Dropshot in
the same harness: hole opened, pit scored, ball recycled.

FIVE ARENAS (styles 25-29):
- BOXBOWL (14): the classic enclosed pitch — floor, walls, glass
  ceiling all in the smin blend group so every seam fillets into a
  drivable quarter-pipe. Built for 300-sparta chaos with nowhere out.
- DROPSHOT (15): a 12x7 team-tinted tile deck over a glowing basin,
  hp by depth, drop_pit scoring: put the ball THROUGH their floor.
- GATEWORKS (16): every net hides behind a gate tunnel sealed by a
  door only the ball can open — six seconds, then it seals. Side
  vault doors hide boost alcoves; a slow centre gate revolves.
- KUIPER (17): deep-space asteroid field — twelve mini planets (half
  with their own radial gravity), carved throughways, spinning rings,
  three launcher pads, and eight free asteroids in three weight
  classes: shove the light ones, respect the heavy ones.
- ZOETROPE (18): three concentric rings of corridor walls turning at
  different speeds and directions (Orbit anims lock position and
  orientation together) — the passages align, then they don't.

## v1.27 — the flip law, open bores, a taller world

FLIPS, REBUILT RL-STYLE. The dodge no longer applies a torque burst and
hopes: input SNAPS to a cardinal (throttle dominant -> pitch axis, steer
dominant -> roll axis, ties to pitch as in RL) and the orientation is
SCRIPTED through exactly one 2-pi turn about that single body axis,
eased out. Because the total is a full turn, a flip mathematically ends
on the pose it began with — wheels-down starts land wheels-down, air
starts finish at their entry attitude. Air roll layers on top without
stealing from the flip. Cancel freezes the turn where it is.

New `--sim flip` audits all eight input combos from ground and air,
reporting chosen axis, degrees turned, orientation error at flip close,
and landing up.y. Final numbers: 8/8 correct cardinal axis, 357-360
degrees turned, error <= 1.0 deg (ground) and <= 1.9 deg (air), up.y
1.00 everywhere. Two harness bugs found on the way: measuring after the
settle (the landing snap, not flip drift) and spawning the air case
mid-fall, where no dodge is legal because the car never touched ground.

TUBES: the real reason joints were not smooth — meshgen::tube builds
ANNULUS END CAPS, so while the CSG cutters opened the SDF, the raster
kept drawing a solid ring at every mouth. That ring was the wall.
`meshgen::tube_open` (no caps, plus a far-field LOD) now draws every
chained/junction segment, each tube end gained a BEVEL COLLAR fillet
torus in the blend group, and the smooth-union k went 2.2 -> 3.4 in
both the physics SDF and the mirror marcher.

SKY + OUTSKIRTS, every map: three altitude bands (26-48, 52-84, 88-132)
of sky pads with launchers, aerial hoops, orbiting panels and floating
islets with glow cores, plus a ring of distant landmarks at 320-520 m —
spires, drive-through arches, stepped mesas with terrace pads, spinning
ring gates, leaning beacon clusters. Density scales with the style;
the LOD and distance culls absorb the cost.

## v1.28 — the phone port

Target: `Linux fp5-black 7.1.2 #1-postmarketos-qcom-sc7280 aarch64` — a
Fairphone 5 (Snapdragon SC7280 / Adreno 644) on postmarketOS. A real
Linux box with a real Vulkan driver, so it already BUILT; what it needed
was a different game.

DETECTION (src/platform.rs): `classify(version, host, model, arm)` is a
pure function of the strings — /proc/version's build tag (postmarketOS,
qcom-sc/sdm/msm), the device-tree model (Fairphone/PinePhone/Librem) and
the hostname (fp*, *phone*, *pmos*), with aarch64 as a supporting signal.
Being pure, the EXACT target device is proven headlessly in `--sim plat`
without owning one: fp5-black classifies Handheld ("postmarketOS +
mobile SoC + device name + aarch64"), an x86 workstation stays Desktop.
`SKYDRIBBLE_MOBILE=1/0` and `--mobile` / `--desktop-ui` override.

HANDHELD PRESET (first run only — a player's own tuning is never
overwritten): quality 0.30 (AO off, short shadows, cheap reflections),
reflections off, bloom capped, screen door off, vsync forced ON (Fifo:
the monorepo's own Fairphone notes name compositor fighting as the
stutter cause), camera pulled in, touch UI on, and RENDER SCALE 0.5.

HALF RESOLUTION: the scene's MSAA, depth and HDR targets now size to
`render_scale` while the surface stays native, so the composite pass
upscales with its existing linear sampler — quarter the fragments for
the expensive passes, crisp UI and text on top. A VIDEO slider exposes
0.25-1.0 live for every platform.

TOUCH (src/touch.rs on the vendored shared `touchkit` recognizer):
a FLOATING virtual stick — it materializes wherever a thumb lands in the
left 42%, dead-zoned, x=steer y=throttle (the WASD equivalent); fixed
BOOST and JUMP pads bottom-right; a FLICK anywhere in the open area
dodges in that direction (jump edge + cardinal input, which the v1.27
flip law turns into a clean full rotation); TAP-TO-MOVE unprojects the
touch through the inverse view-projection, marches the ray to the
terrain and drives itself there, yielding the moment the stick moves;
open-area drags steer the camera. In menus a touch simply IS a cursor
move and a click, so every existing hover/click path works unchanged.
HUD gained disc/ring primitives to draw it all.

`--sim plat` proves the whole chain headlessly: device classification,
preset application, stick math (up-left -> steer -0.66 / throttle
+0.66), both buttons with their press edge, swipe-right -> flick + jump
edge, and drive-to steering. Verdict: PLATFORM+TOUCH: PASS.

## v1.29 — gyro drive

Ported from the monorepo's gyro-ball prototype (games/gyroball:
`sensor_daemon/tilt_sensor_sysfs.c` plus the reader in `v15.py`) and
reshaped for driving instead of a tilt maze — the sensor-controls plan's
step 3 ("wire sensor controls into each game's input"), for skydribble.

THREE SOURCES, auto-detected in order, all feeding one normalized
vector: (1) IIO sysfs read NATIVELY — the C daemon's glob discovery and
`_scale` handling rewritten in Rust, preferring a real gyroscope
(`in_anglvel_*`) over the accelerometer, sampled at 50 Hz, no daemon and
no pipe needed; (2) the daemon protocol on stdin — `TILT_SENSOR_V1` then
`ACCEL x y z`, so `./tilt_sensor | skydribble` still works; (3)
`monitor-sensor --accel` spawned as a last resort, whose coarse change
events become decaying impulses exactly as in the prototype. The reader
runs on its own thread behind a channel, so a stalled or absent sensor
can never block a frame.

THE DRIVING MODEL: calibrate (hold still two seconds; the average
becomes the neutral bias — this is what kills gyro drift) -> sensitivity
-> deadzone with re-normalized response -> clamp -> exponential smooth
with a settle time in seconds. On the ground, tilt STEERS. In the air it
is flick aim: mode 0 rolls and pitches, mode 1 yaws and pitches. An
activation gate runs it always, only while boosting, or only airborne.
Gyro ADDS to the sticks rather than replacing them, so keyboard, pad and
phone can all be live at once.

Menu (all saved): GYRO DRIVE, SENSITIVITY, DEADZONE, SMOOTHING, AIR
MODE, WHEN, INVERT TILT, INVERT PITCH. Toggling it on spins up the
reader and recalibrates.

`--sim gyro` verifies the whole stack headless in 12 checks: protocol
and fallback parsing, calibration removing a biased-at-rest device,
motion passing the deadzone, jitter not passing it, sensitivity scaling
with a clamped output, all three drive mappings, a closed gate emitting
nothing, and impulse kicks decaying to rest. All pass.

## v1.30 — menus you can touch

Touch reached the menus as a synthesized mouse click: a finger-down was
a left click, so any drag fired whatever it passed over, sliders could
only be tapped (never scrubbed), and the PAUSE screen was pure keyboard
(ESC/ENTER/G/O/Q) — unusable on a phone, which is where touch lives.

A real gesture model now sits in front of the menus. FINGER DOWN
selects the row under it and grabs a slider immediately (with a fat
finger band, +-0.10 clip units around the bar, so the target is bigger
than a mouse would need). FINGER MOVE scrubs a grabbed slider
continuously, snapped to that row's own step so touch lands on real
values, or slides the highlight along the list once past a 0.02
threshold. FINGER UP activates only if it was a TAP (movement under
0.03, no scrub) — which is what keeps scrolling from firing buttons —
and a downward FLICK (0.25 down, under 0.25 sideways) backs out of the
menu, the gesture equivalent of Escape.

The pause screen gained five real buttons — RESUME, SETTINGS, MAPS,
ONLINE, QUIT — centred, hover-lit, tappable, and equally clickable with
a mouse.

`--sim touch` verifies the layout math headless in 14 checks: row
hit-testing at the top, middle and both out-of-range ends, a sweep
proving every row is reachable (no dead gaps a finger can fall into),
slider mapping at both ends and the middle plus clamping, the fat-finger
band, all five pause buttons hitting their own index, misses above and
below the button row, and the row being both centred and on-screen.

## v1.31 — the power shot, measured

New `--sim shot`: drive into a resting ball at a held approach speed on
open ground, then report exit speed, LOFT, and distance travelled along
the shot line at 1 s and 3 s. (Two harness lies had to go first: the
boxed arena let a wall rebound masquerade as travel, and a straight-line
distance counted a returning ball as progress.)

Baseline said the complaint out loud — an 11.5 m/s shot went 12.8 m in
its first second and only 3.3 m in the next two — and the trace named
two causes, neither of which was the one I expected:

1. THE SHOT BALLOONED. A car meets a grounded ball from below-behind, so
   the contact normal points steeply up: exit velocity was 6.7 m/s
   VERTICAL out of 14. Hard hits now drive THROUGH — the impulse normal
   flattens toward horizontal in proportion to hit strength, while soft
   touches keep the true normal so carries are untouched.
2. BOUNCES SCRUBBED 29% of tangential speed (0.714), so any driven ball
   died on its first landing. Now 0.88.
3. THE CARRY ASSIST WAS CAPPING SHOTS. The 16%-per-frame speed match
   that makes hood dribbles balanceable applied to EVERY contact,
   dragging the ball back toward the car's own speed on power shots. It
   now fades out with closing speed and stops entirely above 6 m/s.

Result at 3 s: 16.4 -> 20.1 m (8 m/s), 40 -> 44.1 m (14 m/s), 67.5 ->
71.9 m (22 m/s), with loft roughly halved throughout.

ROLLING FRICTION, the negative result worth keeping: it was the obvious
suspect (1.8/s bleeds ~83% of a roll per second) but the sim measured
IDENTICAL travel from 0.42 to 1.8 — a driven ball spends those seconds
airborne. Dropping it to 0.42 did break something else: the bots' air
dribble collapsed (air1 style 33 -> 10 over eight trials) because a ball
that never settles never gets popped. Restored at 1.5, where air1 scores
5/8 goals and style 29.7 while shots keep every metre.

FLIPS: too quick and too linear. Duration 0.62 -> 0.78 s and the ease-out
went from quadratic to a much stronger falloff, so a flip snaps away and
then visibly settles into its landing. Still exact: 360 degrees, error
<= 1.7, wheels down.

LAND ASSIST now ships OFF. The landing matrix's default block is the
assist=OFF column: 45-degree poses upright in 1.37-1.59 s, roof 2.45 s,
flip back in 0.88-1.54 s.

## v1.32 — deep space gets dangerous

THE PHYSICS QUESTION (centroid vs surface sampling): centroid gravity is
EXACT for a sphere — Newton's shell theorem — so sampling buys nothing
on a clean planet. It is wrong exactly where this map is interesting:
carved throughways, ring decks, overhangs. Gravity is hybrid now. Far
field: centroid, as before. NEAR field (within 6 m of a surface, on
nodes flagged `surf`): blend toward the negated SDF GRADIENT — a free,
exact surface normal we already compute — so "down" points into the deck
you are actually on, whether that is the outside of a world, the inside
of a bored tunnel, or the banked face of a ring. That is the
surface-sampling intuition without the sampling.

GRADED GRAVITY: GravNode gained a strength multiplier scaled ~r^1.6, so
giants feel heavy without becoming traps (0.6x to 4.2x). Surface stick
scales with it too, up to 3.2x — heavy worlds hold you through the curve
instead of letting you drift off the horizon.

PLANETS ARE BODIES: mini worlds, two roaming moons and every asteroid
are free bodies that fall in each other's fields, collide and bounce —
vertical billiards. Sizes run dwarf / mid / GIANT (26-38 m) spread
through a 22-190 m altitude band instead of a disc, and ring worlds
carry WIDE drivable rings (tube radius 2.6-5.5 m, in the fillet group,
slowly spinning) with their own surface-following gravity node.

SKY EVENTS, both on five-minute clocks: a heavy world COLLAPSES for
fifteen seconds — its field overrides everything within six radii and
hauls car, ball and worlds inward — then DETONATES, throwing everything
outward with a 1/d shockwave and shaking the camera; and a NEW WORLD
falls in from 300 m up at 46 m/s to break the field like a cue ball.
Both announce themselves.

`--sim kuiper` verifies all of it headless in ten checks: giants
outpulling dwarfs (4.20 vs 0.60) with the far field relaxing to 1.0,
gravity pointing sideways INTO a world from beside it, planet-planet
momentum transfer (struck world at 16.7 m/s), the collapse dragging a
ball inward, the detonation firing and throwing it at 11 m/s, the
shockwave falling off with distance (38.7 m/s at 14 m vs 2.7 at 140 m),
and arrivals landing high and falling. The harness caught its own bug
first: it stepped physics without World::update, so the collapse timer
never ran — the game calls both each frame, so the sim must too.

## v1.33.0 — menus that breathe

The crowding had a structural cause, not a cosmetic one: row height was
`1.44 / n` — rows SHRANK TO FIT, so every setting added made every menu
tighter, and text that stayed legible in a 10-row menu was crammed in a
25-row one. Four standards applied:

1. FIXED ROW HEIGHT + SCROLLING VIEWPORT. Rows are a constant 0.062
   (bindings 0.049 -> 0.058), comfortably finger-sized, and long lists
   scroll rather than compress. Scrolling keeps a row of context at each
   edge, never jumps backwards, never scrolls past the end, and shows a
   scrollbar plus an "n-m OF total" counter. Hit-testing follows the
   viewport, so touch and mouse land on the right ABSOLUTE row.
2. A TYPE SCALE, one ladder for every menu: title 0.0138, body 0.0082,
   section 0.0074, hint 0.0068 — each step about 1.25x the last, and the
   body size deliberately smaller than before. Dense screens read better
   with air around modest text than with big text crammed together.
3. AN ALIGNMENT GRID. Labels share one left edge, values share one right
   edge, the action caret sits OUTSIDE the label column (so text no
   longer shifts as the selection moves), and the slider bar keeps its
   own fixed column, drawn under the selected row only.
4. PROGRESSIVE DISCLOSURE. CAR SETUP had swollen to 25 rows — a full
   screen — so the eight gyro rows moved to their own MOTION / GYRO
   screen behind a link. Every settings page now fits without scrolling
   (longest is 0.8 of a screen); scrolling remains for the genuinely
   long lists, maps (30) and bindings.

`--sim touch` grew to 23 checks plus a density report that prints every
menu's row count and page depth and fails if any page exceeds two
screens — so this cannot silently regress the next time a feature adds
rows.

## v1.34 — hex tiles and spawns that land

UNPLAYABLE SPAWNS, fixed for every map. `Car::reset` placed the car at
the TERRAIN height, so any map whose play space sits above or below the
terrain buried it — DROPSHOT spawned inside its own catch basin, under
the deck. First attempt (probe DOWN from the sky) was worse in a
different way: it found ROOFS, parking the car on BOXBOWL's glass lid
and GATEWORKS' goal lintel (tilted, up.y 0.60). The rule that actually
works rises from the terrain to the first spot that is both clear and
has HEADROOM (1, 2 and 3 m above all free) — which is the inside of an
arena by construction — plus an explicit deck hint for maps whose play
surface is raised. Bot lineups spawn through the same call.

New `--sim spawn` audits all 30 maps, reporting spawn height, clearance,
whether the car settles, and its final up.y: SPAWNS PASS, with BOXBOWL
now starting inside the box (1.24) and DROPSHOT on its deck (6.79).

HEX DROPSHOT: the deck is hexagons now — a new `Sdf::Hex` (exact
hexagonal-prism distance, folded into one sextant) in the physics SDF,
kind 6 in the mirror marcher, and a `meshgen::hex_prism` for the raster,
laid out on a proper staggered flat-top grid. The look is muted base
plus ENERGIZED FILL: each tile is a dark, low-spec hex with an inset
panel that carries no collision and exists purely to light up —
emissive rises from 0.12 to 1.0 as the tile takes damage, so a fresh
deck reads dormant and a worn one glows along its seams. Healed tiles go
dormant again.

## v1.35.0 — loops are one piece

"The loops render as hollow disk segments" was a bug, and the pipe
joining code was exactly where it lived. A full loop was built by `arc`
as 27 short straight shells — and TWO of my own earlier changes made
that chain fall apart visually:

- v1.18 put a cutter sphere at every internal elbow to "open the seam".
  Once tubes were capped that was reasonable; it is not a seam-opener at
  all, it is a hole punched clean through the pipe wall at every joint.
- v1.27 removed the tube end caps so junction mouths would stop being
  walls. Correct for mouths — but it also opened every ELBOW, so at each
  of the 27 joints you now looked straight into the hollow interior. The
  open ring ends read exactly as "hollow disk segments".

Fixed at the level the geometry deserved rather than by patching the
chain: a new `Sdf::TubeTorus` (hollow torus — `abs(torus) - thick`) with
a matching `meshgen::torus_tube` that sweeps an outer skin and an inner
bore, plus shader kind 9 for reflections. A full loop is now ONE
seamless shape with no joints anywhere, at a ring radius derived from
the bore so the baked mesh ratio always matches. The elbow cutters are
gone from every chain (they were perforating the S-bends and swoops
too), and remaining chained segments overlap 0.62 -> 0.78 so bends read
as one pipe.

Perplexus went 871 -> 654 shapes: two 27-piece chains and their 52
cutters became two shapes.

New `--sim loop` proves it per loop: one shape (a chain would be ~27),
own-surface ripple sampled at 288 angles (0.00001 — float noise), zero
holes in the wall top and bottom, and only 6-16% of the ring merged
into the entry run, which is the intended junction. All four loops
across both maps: SEAMLESS.

## v1.36.0 — the flip curve, and planets that never settle

FLIPS: "too quick, then corrected, jumpy, stops instantly" was all one
curve. Profiled decile by decile, the v1.31 easing put 30% of the whole
turn into the first 10% of the time (~1100 deg/s — the lunge), 90% into
the first half, and left the last 40% covering 5% (the apparent freeze),
after which the exact close-out snapped the remainder (the correction).
Over-correction on my part: the previous complaint was that the falloff
was too weak, and I swung far past even.

The new profile is a blend of a smoothstep and a mild ease-out:
10-11-12-13-14-12-11-9-6-2 per decile. It starts decisively without
snapping, peaks at 14% (half the old spike), and is STILL turning at the
end instead of asymptoting into a stop. Duration went 0.78 -> 0.76 s
after a sweep found the real source of the "correction": at 0.86 s a
side flip OUTLASTS the airtime, so the ground truncated it at 329
degrees and the landing snap finished the job. At 0.76 flips complete in
the air — 360 degrees, 0.0 error, with the worst case (side flips) at
353 and 2.8. `--sim flip` now prints the rotation profile every run, so
the shape of a flip is a measured thing from here on.

KUIPER DRIFTERS: five planets that fall, SQUASH flat against whatever
they land on, and spring back with 98.5% of what they arrived with —
the DVD screensaver, in three dimensions. The squash is a damped
oscillation flattening along the impact normal with a sideways bulge, so
they wobble back out through round rather than popping. Their gravity
wells travel with them (GravNode.follow), so you can chase one down and
land on it mid-flight. The anchored worlds stay put: their carved
throughways and ring decks are static geometry that cannot follow.

New `--sim planets`: 5 drifters, 12 bounces in 40 s, peak squash 42%,
returns to round, and still moving at 13.8 m/s at the end — it never
settles, which is the whole point.

## v1.37.0 — swiss cheese, ring roads, open bores

WHY THE TUBES WERE STILL HARD TO DRIVE — three causes, all measured by
a new bore survey that walks every drivable route and reports the free
radius at the axis (p10, median, share of tight spots):

1. DECORATION WAS COLLISION. Monkey Run's vine lines run through the
   treetops, and the canopy spheres were SOLID — foliage was choking the
   bore (p10 0.72 m, under the ~1.05 m a car needs). The glowing guide
   hoops inside big pipes and the bevel collars at every mouth were
   solid too: fittings pinching the passage they decorate. All three
   now draw without colliding.
2. CROSSING PIPES LEAVE WALLS IN EACH OTHER'S BORE. A union keeps both
   hulls, so every place two hollow pipes meet has a wall across the
   passage. Rather than hand-carve each junction, a ROUTE CLEARANCE PASS
   now walks every route at generation, and wherever the free radius is
   under what that pipe promises, it carves an opening (deduped, so
   overlapping samples share one hole).
3. THE FILLET RADIUS ATE THE BORE. Smooth-union at k=3.4 narrows a pipe
   interior by up to ~0.85 m. Shape-to-shape blending is now 1.8, and
   blending into the TERRAIN is 0.8.

Result: PERPLEXUS p10 1.11 -> 2.86 m and tight spots 18% -> 1%;
MONKEY RUN p10 0.72 -> 3.45 m, median 3.71 -> 8.36, tight 26% -> 0%.
Loop routes also went from 12 waypoints to 28: at 12 the straight CHORD
between neighbours sagged out through the pipe wall, and the clearance
pass then carved the loop open trying to serve a route that was never
inside it.

RINGS YOU CAN DRIVE. Kuiper's planetary rings were fat tori — a round
donut has no flat surface, which is exactly why they were not drivable.
New `Sdf::RingDeck` (flat annulus slab) with a matching
`meshgen::ring_deck` and shader kind 10: a banked track around the
world with lip rails at both edges to catch a drift. Verified in the
sim by parking a car on one — 234/240 frames grounded, stayed on.

SWISS CHEESE. 12 worlds -> 18, and each is a warren: two to four bores
on different axes, offset so they cross INSIDE the rock (the
intersections make junction caverns for free), every mouth flared into
a funnel, and two to nine blind pockets pitting the surface between
them.

GENERAL FILLETS. The blend group was pipes only; it now covers all
built form — boxes, capsules, ramps, decks, rings and pipes — so ramps
meet the ground and platforms meet pillars with the same blended
transition the tubes have. Spheres stay hard-edged, so worlds and the
ball keep their silhouette.

## v1.38.0 — dribbling is a mapping, not a wrestle

New `--sim dribble`: ball on the hood, a fixed steering input, and three
numbers — the yaw the stick COMMANDED, the yaw actually delivered, and
any pitch/roll rate nobody asked for — plus how long the ball rides and
how much of the run was even on the ground.

Two real faults, and one of my own test premises:

1. A GROUNDED CAR HAD FREE ROTATION. Pitch and roll rates were merely
   DAMPED while driving (12.5% a frame), so leftover spin from a landing
   or a bump kept turning the car for a quarter second, fighting the
   surface align and stealing commanded yaw. A grounded car's
   orientation now comes from exactly two things, as in Rocket League:
   the surface it sits on, and your steering. `ang` is cleared outright.
2. NOTHING DRAGGED THE BALL AROUND A TURN. The hood had no tangential
   friction, so a carried ball was simply left behind the moment the car
   rotated — it separated rather than rolling off. Contact friction now
   uses the contact POINT's velocity, which includes the car's yaw (that
   yaw is applied straight to the rotation, so it never appeared in
   `ang`; the car now publishes a world-space `omega` for exactly this).
   A dished-hood centring nudge rides on top of it.
3. My "flat ground" test was a rolling plain — the car was launching off
   a HILL at 14 m/s, which is what made a hard turn look like it lost
   half its steering. On genuinely flat ground the mapping was already
   exact.

Result: every case now delivers the commanded yaw exactly (-1.24 ->
-1.24, -2.05 -> -2.07), stray rotation 0.00, grounded 100% of the run.
Ball carried through a steady turn 30% -> 68% of the run, and through a
turn after landing 31% -> 60%, under a FIXED input with no corrective
driving at all.

Landings also plant instead of springing (rebound damping 9 -> 19) —
you cannot dribble on a trampoline.

The bots inherited all of it: air1 air touches 0.4 -> 22.5 and style
10.5 -> 36.5, pass2 style ~44 -> 136.8. They could always reach the
ball; now they can carry it. Power shots, flips, landings, loops and
bores all regress unchanged.

## v1.39.0 — flicks exist now

THE FLICK BUG, in three layers, each hiding the next:

1. THE HITBOX WAS A CAPSULE. Round on top, so a ball resting on the car
   had about eight centimetres of contact before it separated. Rocket
   League flicks come from a BOXY hull whose nose and roof corner sweep
   up into the ball. The contact test is now the closest point on an
   oriented box.
2. THE IMPULSE IGNORED ROTATION. The collision used `car.vel` — pure
   linear velocity — so a flipping car imparted nothing but its travel
   speed. The patch under the ball actually moves at v + omega x r, and
   that swinging hood IS the flick. Both the impulse and the push
   direction now use the true surface velocity, and the hood's spin
   transfers to the ball as well.
3. OMEGA WAS MEASURED IN THE WRONG PLACE. `omega = rot * ang` misses a
   SCRIPTED flip entirely, because the flip drives the quaternion and
   leaves `ang` at zero — and the measurement I added sat inside an
   `if ang != 0` guard, so during the exact manoeuvre that throws the
   ball it never ran and omega read stale (1.9 rad/s instead of ~8).
   It is now measured from the frame's quaternion delta,
   unconditionally, which covers steering, tumbling and scripted flips
   alike.

THE STICKY DRIBBLE: contact friction was killing 75% of the ball's slip
every frame — that welds it to the roof, kills roll-off and feathering,
and swallows any flick before it can leave. Now a capped 30%, the
speed-match assist is 10% (was 16%), and the hood dish is gentler.

`--sim flick` balances the ball on the nose at driving speed, then runs
seven real input sequences and reports exit speed, height, elevation,
gain, contacts during the flip, and what the analyzer calls it:

  FORWARD     19.6 m/s  +7.6 up  23 deg  189%   BIG
  45-DEGREE   19.6      +7.6     23      189%   BIG
  DELAYED     20.1      +7.1     21      194%   BIG  (POWER FLICK)
  TORNADO     17.8      +6.4     21      172%   works
  MUSTY       11.8      +5.8     30      114%   lofted, not yet powerful
  SIDEWAYS    12.1      +3.6     17      117%
  BREEZI      11.6      +5.4     28      111%

Forward, 45-degree, delayed and tornado flicks are now genuinely
game-worthy. The musty gets its steep shape but not its power yet, and
the sideways flick needs the ball off-centre to whip properly — both
are honest gaps, both now measurable.

THE STYLE ANALYZER (`src/style.rs`) watches the event stream and names
plays: FLICK, POWER FLICK, MUSTY FLICK, CEILING FLICK, AIR DRIBBLE,
AIR DRIBBLE SHOT, AIR CONTROL, DRIBBLE, WALL SHOT, RESET SHOT, PINCH,
FLIP RESET, AERIAL, SUPERSONIC HIT and GOAL, each scored and multiplied
by a combo that compounds while tricks are chained inside a three
second window. It judges a launch against the BALL's carried speed
rather than the car's (a backflip slows the car, which made the weakest
flick look like the strongest), and it reads shape as well as
magnitude, so a steep lofted launch is a musty and a flat fast one is a
power flick. Awards toast in-game and print in the sim.

## v1.40.0 — the menus go mobile

Three blockers, all of them the difference between "playable on a phone"
and not.

1. LOW GRAPHICS NEVER APPLIED. The handheld profile — no ray marching,
   half render scale, Fifo vsync, touch on — was only applied when NO
   SAVE FILE EXISTED. Any phone that had ever launched the game, or any
   save copied onto one, kept desktop graphics forever. The save now
   records which hardware profile it was tuned for and re-tunes whenever
   that changes. On top of that, a GRAPHICS PRESET row (AUTO / LOW /
   MEDIUM / HIGH) sets rays, ray quality, render scale, bloom and vsync
   in one tap, so any device can be forced down by hand.
2. NO DRAG TO SCROLL. Dragging moved the SELECTION, which scrolled the
   list only as a side effect. Now the content follows the finger
   directly — one row per row-height of travel — with a fling that keeps
   the list moving after you let go and settles under damping. The
   selection is left alone while scrolling, because that is what a tap is
   for. Keyboard navigation keeps its own behaviour: the viewport follows
   the selection.
3. NO WAY INTO THE SUBMENUS. Reaching MAPS or VIDEO meant a keyboard
   shortcut or walking the tree. Every menu now carries a NAV STRIP —
   BACK, SETUP, CONTROLS, MAPS, VIDEO, ONLINE — as real tappable buttons
   with hover and current-screen states, sitting below the list viewport
   so a nav tap can never also hit a row.

`--sim touch` grew to 28 checks: every nav button hitting its own index,
the strip centred, on screen and clear of the viewport, nav taps missing
rows through the real hit path, LOW genuinely cheaper than HIGH, and the
handheld profile forcing low graphics and touch from any starting
config.

## v1.41.0 — worlds that pull, mirrors that look like cars

KUIPER GRAVITY WAS BACKWARDS. Planets were flagged `tube: true`, and for
a tube "down" points OUTWARD from the axis — right inside a pipe, exactly
wrong on a world, so every planet pushed you away. GravNode gained a real
`inward` flag: tubes push out, worlds pull in. Strength came down too
(r^1.6 clamped to 4.2x -> r^1.3 clamped to 1.6x): giants still feel heavy
without swallowing a car that drifts past. `--sim loop` now checks the
sign directly — dot(down, toward centre) reads 0.99, attractive.

DRIBBLING WAS GLUE. Contact friction killed so much slip that the ball
could not fall off. Ground grip is now 0.16 (capped at 0.8 m/s) and the
dished-hood nudge more than halved, so holding a turn too long loses the
ball — the skill is back. Air carries keep their grip (0.34) because an
air dribble is a different move and stripping it there kills air
dribbles outright. Ground dribble through a steady turn: 100% -> 75% of
the run; through a hard turn 64% -> 42%; straight-line carries still
hold 100%.

WEIRD SHOT ANGLES. The push direction came off the CONTACT NORMAL, which
points diagonally off a box corner, with raw car velocity blended in on
top. Now the harder the hit, the more the push follows the car's own
heading instead — a shot goes where you point. Spin was scaling UP with
impact; it now scales DOWN (soft touches roll the ball for dribbles,
hard hits drive through), which is what stops shots knuckling.

PERPLEXUS TUBES. Overlapping shells put each other's INNER wall across
the bore, and the clearance pass only opens what it knows you drive —
the lattice links were never registered as routes, so every crossing
kept its blocker. Links are routes now, and elbow seams are carved again
at 0.96r: wider than the inner surface (0.935r), well inside the outer
(1.005r), so the blocker goes and the pipe skin stays. Bore p10 2.86 ->
3.73 m, median 5.17 -> 7.10, tight spots 0%. And it goes SKY-HIGH: 20
anchors -> 32 across three altitude tiers (top tier 110-190 m), four
links each, reach 105 m — 94 drivable routes in all.

THE MIRROR BUBBLE. The baked car field was distance-to-a-point-cloud,
which is a hollow SHELL: reflection rays passed straight through the car
and fell back on its bounding box — the bubble. The bake is SIGNED now,
using each cloud point's outward normal, over a denser cloud (520 ->
2600 points) and a finer atlas (28x18x32 -> 36x24x44), with a
grid-accelerated nearest lookup so it stays instant at load. New
`--sim carsdf` measures the baked silhouette: 0.31 / 0.29 / 0.62 against
a model of 0.32 / 0.31 / 0.64 — longer than wide, flatter than long,
within 3 cm.

## v1.42.0 — actually driving the tubes

Every previous tube fix was measured with a bore SURVEY: how much free
radius sits at the pipe's axis. That number was healthy while the pipes
were still unusable, because a car does not drive the axis. New
`--sim drive` does the obvious thing instead — it spawns a car inside
each route, points it down the pipe, holds throttle and boost, and
follows it: how far along does it get, and does it ever leave the bore?

The first run answered the complaint outright: cars fell out of 23 of
24 Perplexus routes and all 3 Monkey Run lines. Three causes:

1. TUNNELING. Shell walls were r*0.035 — 0.28 m on an 8 m pipe — while
   a car at 23 m/s covers 0.19 m per physics step. Cars passed clean
   THROUGH the pipe skin without ever touching it. (v1.37 thinned those
   walls to win back bore clearance, which is what introduced this.)
   Walls are now (r*0.10).max(0.55): thick enough that a step cannot
   skip them.
2. JUNCTIONS NARROWER THAN THEIR PIPES. Monkey Run's treehouse chambers
   were 8 m radius while the vine bores feeding them run 8-12.5 m, so
   the biggest line arrived at a wall in its own junction. Chambers are
   22 m now.
3. A NEAR-VERTICAL ENTRY. The vine lines climbed ~45 m of canopy height
   in 25 m of run — a shaft, not a ramp. A car entering at speed stalled,
   slid back and spat out of the mouth. The root mouth now starts ~75 m
   out, so the climb is drivable.

Perplexus now traverses 20-23 of 24 routes end to end (the remaining
"escapes" are cars reaching 100% and leaving through the far mouth,
which is the exit working). Monkey Run went from 3 escapes to 0 and its
longest line reaches 90% of 2.2 km.

STILL OPEN, honestly: two Monkey Run vine lines stall around 7-11% of
their length without escaping — they are being driven, just not
finishing. That is the next thread to pull, and `--sim drive` is now the
tool for it.

Also measured and REJECTED: strengthening in-tube gravity (1.0 -> 1.9)
to hold cars against the bore. It cost traversals (21/24 -> 17/24) by
slamming cars into walls, so the pipe still pulls at 1.0.

## v1.43.0 — moving platforms, and bots that learn

THE BUG UNDER EVERYTHING: animated shapes were baked into the physics
spatial grid at their STARTING cell, so the moment one travelled away it
vanished from collision entirely. Every orbiting sky panel, the roaming
moon, every spinner, door and closing gate has been a ghost to physics
since the grid landed — found by tracing a car standing on an orbiting
deck and watching `world.sdf` return terrain-only distance while the deck
was demonstrably under it. Movers are now queried directly (there are
only a few dozen) instead of through the grid.

PLATFORM RIDING. The fix that matters is not carrying the car, it is
WHERE the maths happens: grip, braking, steering and the suspension are
all written for ground that stands still, and on a moving deck that code
was braking the car to a stop in WORLD space while the deck slid out from
under it. The whole ground solve now runs in the platform's reference
frame — step in, solve as if on the floor, step out — so riding feels
like driving and jumping off hands you the platform's momentum. New
`--sim platform` measures drift in each platform's OWN rotating frame (a
rider on a spinner is supposed to move in world space): shuttle 0.74 m /
94% grounded, carriage 0.42 m / 94%, turntable 0.04 m / 100%,
paternoster 0.15 m / 91%.

CLOCKWORK (style 30): a machine you play inside — shuttling decks,
orbiting carriages, spinning turntables, a paternoster of rising steps to
a prize platform, and sweeper arms. Two design fixes came straight out of
the sim: carriages hold a constant ~5 m/s RIM SPEED instead of a constant
angular rate (the outer ring was doing 18 m/s, which no landing survives)
and outer decks are now BIGGER, not smaller.

CAMERA: MOVING PLATFORM BEND (0..1, default 0.55) adopts a smoothed slice
of the nearby platform's motion, so a fast deck reads nearly still while
you line up the landing.

TOUCH: the joystick now flies the car in the air (pitch, and roll or yaw
by the slide modifier), an on-screen MENU button opens the menu without a
keyboard, and handhelds get GYRO FLYING on by default — gated to the air,
where it helps and cannot disturb driving. Menu text scales with aspect
and truncates to its column, which is the mobile overlap fix.

AI, and this is the part worth testing yourself: bots run a small guided
(1+1) hill-climb over four aerial knobs — intercept lead, jump distance,
boost commit and aim lift — scored on exactly what we want more of, air
touches and engagement with a ball that is up, re-evaluated every nine
seconds. On top of that, COORDINATED AERIALS: when the ball is genuinely
up and moving, the attacker underneath it is busy, so the best-placed
MATE is elected finisher and flies the intercept, aiming under-behind on
the goal side so a collision becomes a carry.

`--sim learn` runs a 150 s match and compares aerial output in the first
third against the last, with learning frozen as the control. Frozen:
31 -> 17 touches (0.55x, it decays). Learning: 31 -> 58 (1.87x) over 96
episodes. Same start, same conditions; the only difference is whether the
learner updates.

STILL OPEN: two Monkey Run vine lines stall mid-route (unchanged), and
the DHT ray-streaming idea is designed but unbuilt — phone-side
reprojection of remotely traced rays is the right shape for hiding
latency, but it needs real GPUs at both ends to develop and I will not
ship a renderer path I cannot measure. The order would be: prove the
transport headless (as `--sim mp` did for multiplayer), then a half-res
remote radiance buffer with local reprojection and disocclusion fill.

## v1.44.0 — the giants, and LAN render assist

THE GIANTS (MONKEY RUN). Four avatar-scale trunks, 95-165 m tall and up
to 26 m thick, with a ROAD CARVED THROUGH THE WOOD: a helix of cutter
spheres spiralling up the inside of the bark, three smooth bulb hollows
opening off it (boost pockets big enough to turn around in), a flared
ground mouth, and a canopy exit at the top. Giant vines are slung
between the crowns at windscreen height — solid capsules, so they are
something you smack into.

The helix is derived from a DRIVABLE SLOPE rather than a turn count. The
first cut used a fixed 2-3 turns, which made the opening stretch a
60-degree shaft (the drive test stalled at 4% of the route); it now hugs
the bark at constant radius and takes as many turns as a ~1:3 climb
needs, sampled densely enough that consecutive cutters overlap.

TUBE GRAVITY now scales with the BORE. A narrow pipe holds you by
geometry, and pulling harder there slammed cars into walls (21/24 ->
17/24 traversals, measured). But in a 12 m bore a boosting car flies up
the middle and never touches a wall at all — the drive trace showed
exactly that, climbing 13 m with `og=0` throughout. Pull is now
(r/6).clamp(1.0, 2.6).

LAN RENDER ASSIST — your split, built and measured (`src/stream.rs`,
`--sim stream`). A host on the same LAN (found through the same fleet
store as multiplayer) marches the FAR field and publishes world-space
RADIANCE PROBES; the client marches only the first few metres of each
ray and, when it misses, samples a probe instead of continuing. The
residual is local, the expensive tail is remote.

Probes rather than video, deliberately: a probe grid is view-independent,
so a late packet is still usable — it degrades to a slightly stale
reflection instead of a torn frame, and the client never blocks on the
network.

Loopback numbers: 40/40 frames, 1.7 KB/frame — 417 kbit/s at 30 Hz —
with the client resolving 18% of rays locally and handing 82% to probes,
for 19% of the ray steps of a full march, at a mean channel error of
0.163 against that full march. The protocol, transport, cache,
direction blending and accuracy are all proven headless. NOT built: the
GPU path that samples the cache inside the shader, which needs real
hardware at both ends to tune.

STILL OPEN: the long Monkey Run vine lines. Perplexus traverses 20-23 of
24 routes; Monkey Run's three 900-2200 m lines still stall mid-route
(4-38%) without falling out. They are wide, steep and very long, and the
next thing I would try is banking them — rolling the bore so the driving
line is the inside of the curve rather than fighting across it.

## v1.45.0 — swinging bridges, drifting leaves, arcade totems

Three additions to MONKEY RUN, each built on machinery that already
existed — which is the nice part: platform riding (v1.43) is what makes
a swinging log a bridge rather than scenery.

SWING, a new anim: a true pendulum about a pivot, so a hung object both
translates AND tilts through its arc. BROKEN LOG BRIDGES use it —
fallen trunks slung in the vines between the giant trees, three per gap,
each on its own rope length, amplitude and phase. Drive across while it
sways, or wait for the swing to bring one to you. Verified rideable with
`PLAT_MAP=24 --sim platform`: 1.40 m drift in the log's own frame, 100%
of the run grounded. (The other movers on that map — swaying vines,
orbiting parrots — are decoration and are not meant to be stood on; the
platform PASS criterion is the CLOCKWORK set.)

BIG LEAVES: ten broad blades adrift in the canopy air as free bodies,
mass 0.8 against a car's 12, with real air drag (0.9 against an
asteroid's 0.0) so they flutter and settle rather than coasting like
rocks in space. Bodies gained their own draw scale, orientation and spin
for this — previously every body rendered as a sphere, which is why a
leaf needed them. A car shoves one aside; the ball takes a nudge off
line.

ARCADE POWER HITTERS: five totem paddles that fire the ball when it
comes within 5.4 m — always up and OUT at 46 m/s, arcade rather than
physical, with a 1.1 s recharge and a flash on the ring. World gained a
`hitters` list and `bump_ball`, applied in the ball step.

Battery after: platforms, mechanics, loops, stream, touch, flips and
dribbling all green.

## v1.46.0 — APERTURE: portals, and rays that go through them

PORTAL PHYSICS. A `Portal` is a face (position, orientation, radius) linked
to a partner. The transform is the whole of Portal's momentum rule:

    R = partner_rot * (180 deg about the portal's up) * inverse(entry_rot)

Position, velocity, orientation and angular velocity all take that same R,
so speed is preserved exactly and a fall becomes a launch. The half turn is
what makes you come OUT of the far portal rather than reversing back into
it. Cars and the ball both go through, with a 0.12 s lockout so an exit
cannot immediately re-enter its own partner.

APERTURE (style 31): clean white test chambers. THE FLING — a five-step
tower over a floor portal that exits a wall portal, aimed at an island only
reachable at speed. THE LOOP — floor and ceiling portals facing each other,
so a fall never ends until you steer out of it. THE CROSSWALK — wall to
wall at driving height, with ramps into both faces.

`--sim portal` checks the rules rather than trusting them: links mutual,
transform a pure rotation, entering a face exits the partner's face
(alignment 1.00), speed conserved (23.0 m/s in, 23.0 out), the fling real
(a 45 m fall becomes 22.9 m/s horizontal), and no runaway ping-pong when
something hovers at a mouth. All pass.

Two bugs it caught, both invisible from the maths: A PORTAL IS A HOLE —
the floor portal was sealed under the floor panel, so the transform was
perfect and nothing could ever reach it; each face now carves the surface
behind it. And the climb tower stood directly OVER its own drop shaft,
which is a lid on the fling.

RENDERING THROUGH THEM. The reflection marcher is now portal-aware: it
walks in its own frame, and when a step crosses a portal face the ray is
teleported to the partner and keeps marching (two hops, so a facing pair
terminates instead of recursing). The far side is therefore genuinely
visible through a portal wherever marched rays contribute — reflections,
and the portal surface itself. Portals ride in the uniform block (up to
four; pairs are adjacent so the shader's partner rule matches the map's).

HONESTLY: the naga test validates the shader and the transform is the same
one the physics uses and passes in the sim, but the visual result is
unverified — I have no GPU here. The primary raster view still draws the
portal face as a translucent disc; a true recursive raster pass (render the
scene from the virtual camera into a target, sample it on the quad) is the
next step and needs hardware to tune.

## v1.47.0 — DROPSHOT gets its bowl

The arena had flat glass rails: a wall to BOUNCE off, not one to drive.
That is where Dropshot's verticality lives — carry up the side, turn over
the deck, and come down on a weakened tile — so the perimeter is now a
RAMP BOWL: four side ramps at ~36 degrees, four corner ramps turned 45
degrees in plan to close it, a glass lip above the crest to keep the ball
in play, light strips tracing the rim, and longer entry ramps from the
plain. Every ramp is in the blend group, so the deck-to-ramp seam fillets
into a transition instead of a lip. The catch basin below grew to match
(74 x 50) with ten glow ribs.

`--sim mech` gained a ramp-climb check — drive off the deck into the wall
and see how high you get — and it immediately earned its keep. Ramps
flush with the deck edge measured **0.7 m of climb**: the car was hitting
each ramp's blunt END FACE and stopping dead, because a rotated box
presents a wall at its foot. Burying the toe 11 m under the deck leaves
only the slope exposed, and the same test now reads **21.1 m above the
deck** — the bowl is genuinely drivable, and the check will catch it if a
future edit flattens it again.

## v1.48.0 — the tubes get thickness, frost, and quiet

Four complaints, four causes.

NOT 3D — the mouths had ZERO THICKNESS. v1.27 removed the tube's annulus
end caps because that flat ring was a wall you drove into at every
junction; correct, but it left the outer and inner walls meeting at a bare
edge, so a pipe read as a rolled sheet rather than something with a wall.
`tube_open` now closes each end with a CHAMFER BAND joining the two
surfaces, its inner lip pulled back along the axis — so the mouth is a
tapered edge you can see the thickness of, and it catches a highlight
instead of vanishing. It is a rim, not a cap: nothing to hit.

NO TAPERED EDGES — same fix. The chamfer is geometry now, which also let
the bevel COLLAR tori at every mouth go: they existed to fake this.

FILLED WITH RINGS — the guide hoops every 20 m of bore, plus a collar at
each joint. Both are gone. Lighting inside a pipe is now a short glowing
STRIP laid along the bore wall: it lights the way without standing in it.
The clutter was also expensive — Perplexus dropped 1241 -> 981 shapes and
Monkey Run 1663 -> 1256 with no loss of anything you drive on.

NOT GLASSY — shells were alpha 0.55 at spec 0.45: coloured film. They are
now alpha 0.34 at spec 0.92, and flagged as ROUGH GLASS (params.x) so the
shader gives them a fresnel-driven reflection (0.18 head-on to 0.92 at
grazing) with the reflected ray JITTERED into a small cone per fragment.
That scatter is what reads as thick, slightly frosted glass rather than a
mirror — and it hides the banding a perfectly smooth curved surface shows.

Bores, loops, portals, mechanics and driving all regress unchanged.

## v1.49.0 — TURNSTILE, and a live view through the mouth

MOVING PORTALS. A `Portal` can now be bolted to a shape (`follow`), and
takes that shape's transform every update — so where a mouth sends you
keeps changing. Because the exit rotation is read at the moment of
crossing, timing the entry IS the mechanic.

TURNSTILE (style 32): every portal is on something turning. THE DRUM — a
spinning cylinder with a mouth in its side that comes around to face you.
THE CARRIAGES — orbiting decks with face-up portals, so a drop leads
wherever the partner has got to. THE LIFT — a portal that climbs, so
entering at the bottom of its travel and catching it at the top throw you
to different places. Verified in `--sim portal`: a moving portal's exit
direction swings 143 degrees as its machine turns.

COLOUR CODING. Each pair now has its own hue — blue/orange, green/lime,
violet/magenta, cyan/amber — and within a pair the exit end is the paler
shade, so a glance tells you both which mouth you are looking at and
which one it feeds. Checked in the sim: every pair's two ends are shaded
apart.

THE VIEW THROUGH. A portal face is no longer a coloured disc. The
fragment takes its own view ray, carries it through the pair transform,
and marches the world from the partner's mouth — three jittered samples,
averaged, so it reads as a portal rather than a hard window, then tinted
with the mouth's own colour and rimmed at grazing angles. Cars and the
ball live in the marcher's shape list, so THEY APPEAR THROUGH IT: you can
read an attack before it arrives, and a shot through a portal stops being
blind. Portal faces carry their index in params.x; the uniform block
holds eight portals, enough for both maps' three pairs.

Same honesty as before: the shader validates and the maths is the same
transform the physics uses and passes, but the visual result is
unverified — no GPU here.

## v1.50.0 — stuck no more, and two new arenas

STUCK UNDER A TURNSTILE PORTAL — a moving portal carried its face but left
its HOLE behind. The carve is five cutter spheres punched through whatever
the mouth sits in; on a portal bolted to a spinning drum those stayed where
they were baked while the mouth rode away, so the portal ended up against
solid geometry and you parked inside it. Portals now remember their cutters
with offsets in the mount's frame and move them every update. The exit also
clears properly: 1.2 m out along the face, then up to eight pushes along the
surface normal until the car is genuinely in open air.

BALL JOINTS BLOCKING TUBES — the junction chambers. Their shells were thick
enough to be a plug: the interior carve left 0.9 m of wall and each tube
mouth was cut at only 1.12x its radius. Now 0.4 m and 1.45x, so a room is a
room and its mouths are openings.

TUBES STILL NOT 3D — a perfectly smooth cylinder of translucent glass has
nothing for light to catch. The wall is now FLUTED: a shallow twelve-fold
ribbing with normals tilted along each ridge, so the highlight runs around
the pipe and it reads as round. Wall thickness went 0.10 -> 0.14 of radius
and the glass back to alpha 0.55, so there is body to see.

PORTAL VISIBILITY — the view is 35% brighter, the colour tint cut from 22%
to 9% (it was washing the image), the edge glow softened, and crucially the
march no longer passes the fragment's own distance, so the low-quality
early-out can never turn a portal into flat sky.

WYRMFIRE (style 33): a lava caldera with a wyrm coiled through it — 34
vertebrae you drive along with a dorsal ridge, fire vents every seventh
that fire the ball skyward, a head plate to land on, flowing lava that
drags you toward the middle, and six fire columns that throw you up.

BREAKWATER (style 34): concrete arena, rolling swell. Three giant wave
walls sweep the deck and CARRY you (platform riding), each pushing a wash
ahead of it, and two DRAINS take the water under the floor through glass
pipes that spit you out somewhere else. Built on a new `Flow` — a standing
current with soft edges — folded into the same force path as floodwater.
Verified: 58 flows, sample current 12.1 m/s, carries.

## v1.51.0 — your tuning pass, merged forward

Twelve patches of hand tuning, based on v1.41 while the tree had moved to
v1.50. Rather than apply them over the top (which would have reverted
platform riding, portals, the new maps and the tube work), the v1.41.0 tag
was checked out, the series replayed onto it, and the resulting delta
ported forward by intent. Four physics changes, all kept:

1. DIAGONAL FLIPS. The cardinal snap is gone: the dodge now follows the
   input VECTOR, normalised. W+A is a diagonal flip instead of a left one,
   and normalising keeps flip power constant however far the stick goes.
   (The earlier snap was built to an earlier request; this is closer to how
   RL actually reads a dodge, and it is your call to make.)
2. ROLLING SPIN WAS BACKWARDS. `n x v` spins a rolling ball AGAINST its
   travel; it is `v x n`. It also snapped to the target instantly — now it
   eases in at 0.3, so a rolling ball looks like it has mass.
3. THE PSYONIX IMPULSE, DOWN TO A TENTH. The extra push layered on top of
   the elastic impulse was doing most of the work in a hit, which is why
   touches felt like blasts. At 0.1 it shapes the hit instead of dominating
   it: exit speed is now 91-97% of approach (it was 121-125% — the ball was
   gaining energy from being hit). Shots travel less far as a result: 57 m
   at 3 s from a 22 m/s approach, down from 74. That is the trade, and the
   feel is the thing worth trading for.
4. Wider ranges throughout (FOV to 200, camera height to 24, world scale to
   10.6x, ball size to 30x, top speed to 10x) and a new default camera:
   FOV 86, height 5.0, angle -30 — a much higher, steeper chase view.

Everything still passes: portals, platforms, mechanics, loops, touch, the
shader validation, and the bots are healthier than before — with a gentler
impulse they hold the ball far longer (pass4 air touches 105, pass2 style
132.7).

`presets/` now ships the save files from those sessions, with a README on
how to load one.

## v1.52.0 — CHAMBER, and a ball you can actually lose

THE BALL IS NOT STICKY ANY MORE. Ground grip on a carried ball went 0.16
-> 0.055 (cap 0.8 -> 0.35 m/s), the dished-hood nudge to a third, and the
speed-match assist halved again. The point is that the ball now keeps its
own momentum on your roof: BOOST AHEAD and it rolls back down toward you,
lift off and it runs out in front, and the recovery is to scoop under it
from the far side. Measured in `--sim dribble` under a FIXED input with no
corrective driving: a straight line still carries 100% of the run, a
steady turn 45% (was 79), a hard turn 29% (was 42). Air carries keep their
grip — that touch is the whole move and there is no ground to catch a
mistake.

RAY-TRACE PERFORMANCE. The portal work cost more than it should: every
march STEP was running up to eight quaternion plane tests. Now one cheap
ray-versus-portal bounds test happens once per ray, and the per-step hop
check is skipped entirely unless a mouth is actually within reach — which
on the twenty-odd maps with no portals at all is now a single integer
compare. The portal face view also went from three samples to two.

LOW CEILINGS. BOXBOWL's lid was 13 m, which clipped every aerial: walls
and ceiling are now 26 m over a wider floor, and TURNSTILE's hall doubled
to 20 m.

CHAMBER (style 35): a giant chess set in a dark hall that PLAYS ITSELF.
Thirty-two pieces on 26 m squares, working through Morphy's Opera Game
(Paris, 1858) move by move from the real score — including the captures,
where the taken piece slides off the board and stays off. Every piece is
drivable furniture: rooks carry three ramps up to a battlement, knights
carry a launcher ring on the crest, bishops are angled blades, the queen
is a landing deck with a boost crown, the king a ring of pillars.

And it winds up. Every goal you score raises the board's FURY: the
interval between moves shortens and, past a threshold, pieces start moving
two and three at a time. Verified in the sim: 32 pieces, 17 moves in the
first 60 s at rest, 4 captures, and the tempo going 0.28 -> 0.53 moves per
second once the fury is up. Pieces are proper moving platforms, so a rook
crossing the board carries you with it.

## v1.53.0 — the hood is a dish

Defaults: camera height 1.21, angle 20 degrees.

DRIBBLING, DIAGNOSED RATHER THAN GUESSED. New `--sim hood` parks a ball on
a driving car and reports its position IN THE CAR'S FRAME — height above
the shell, fore/aft, lateral, relative speed, spin. Two things fell out
immediately:

1. THE BALL WAS FLOATING. The contact skin was 6 cm, so a carried ball
   rode visibly clear of the shell and its contact point sat above the real
   hood. At 2 cm it rests on the car: measured hover 0.020-0.051 m (it was
   0.060-0.337) and chatter halved to 0.02 m/s.
2. THE HOOD WAS A SLIPPERY PLATE. Last version cut grip until the ball slid
   off everything — which is not "challenging", it is just uncontrollable.
   Uniform grip can only ever give you a magnet or a marble.

So the hood is a DISH now. Within about two thirds of the way to the edge
the cradle is firm and quadratic, so small errors self-correct and you can
hold a line; past that it falls to nothing and the ball is gone. That
gradient is the balance point, and it is what makes a dribble a skill.
Fore/aft is deliberately much looser (0.9 against 3.2) because that axis is
where the play is: boost past the ball and it rolls back down the hood,
ease off and it runs out front, and you recover by scooping under from the
far side.

Carry through a FIXED turn with no corrective driving: straight 100%,
steady turn 61%, hard turn 36%, after a landing 56% — against 45/29/40
when the hood was uniformly slippery, and 79/42/72 when it was sticky.

Housekeeping: the container filled up mid-session (linker failure), so
stale build artefacts were cleared — 6 GB back.

## v1.54.0 — shots punch, carries slip, the ground is where it looks

POWER SHOTS were killed by their own fix. Cutting the extra impulse to a
flat tenth stopped touches feeling like blasts, but it treated a gentle
dribble tap and a committed 22 m/s strike identically. It now SCALES with
the hit: 0.06 + 0.55 x strength. A 22 m/s approach sends the ball out at
22.4 m/s and 60 m in three seconds (it was 19.3 and 51). Soft touches are
untouched.

THE BALL COMES OFF THE ROOF. Three things were still holding it: ground
grip at 0.10, a dished-hood pull at 3.2x, and a speed-match assist at
0.055. Grip is 0.018 with a 0.18 m/s cap, the assist 0.012, and THE DISH
IS GONE ENTIRELY — a hood that pulls the ball back to its centre line is
exactly what made carries feel magnetic. Under a fixed input with no
corrective steering: a straight line still carries 100% of the run, a
steady turn 33% (was 61), a hard turn 25%. Keeping it is your driving now.

THE TEXTURE / CAR MISMATCH, diagnosed. Physics blends built form into the
terrain with a smooth-min, but the RASTERISED meshes have no such blend —
so near any ramp, wall or platform the car was resting on a surface up to
20 cm away from the one being drawn. The blend radius drops 0.8 -> 0.25
(and the same in the shader, so reflections agree), cutting the worst case
to about 6 cm while keeping transitions drivable. Bores got better as a
side effect: Perplexus p10 2.86 -> 3.80 m and 21 of 24 routes traversed.

TUBES: in-bore gravity down from (r/6, capped 2.6) to (r/10, capped 1.5) —
it was pinning cars to the wall — and the sections are smoother: segment
overlap 0.78 -> 0.92 of half-length so joints stop reading as seams, with
the tube meshes up from 28 to 40 segments (18 for the far-field LOD).

## v1.55.0 — the ball's spin becomes real

The gap between our ball and Rocket League's was not a constant, it was
structural: SPIN WAS A SLAVE TO VELOCITY. Every bounce set the tangential
speed to 88% of what came in, then computed angular velocity FROM the
result (`target_ang = v x n / r`). Spin could therefore never change where
the ball went — no curve, no backspin, no topspin, and a dribble touch
could not put shape on the ball.

Contact is now a proper rigid-sphere impulse, which is what RL models:

    contact point sits at -n*r from the centre
    its material velocity is v + w x r_c
    friction acts on the SLIP of that point, Coulomb-limited by the
    normal impulse, and torques the ball in return
    (solid sphere: I = 0.4*m*r^2, so tangential effective mass = m/3.5)

Everything falls out of that one change, and `--sim ballspin` measures it:
from an identical entry, a bounce exits at 6.34 m/s plain, 7.67 with
topspin and 5.01 with backspin, and sidespin sends it 1.33 m/s sideways.
A sliding ball spins UP until it rolls — |w| 3.47 against v/r 3.47, no
slip — and a slide-to-roll transition costs exactly 2/7 of the speed,
which is the textbook result for a rigid sphere and a good sign the model
is right rather than merely tuned.

Also RL-accurate now: ball spin caps at 6 rad/s (theirs), which means a
ball above about 5.6 m/s CANNOT roll and must slide — true in RL too —
and ball friction sits at 0.33 rather than the 0.7 that was braking a
12 m/s roll to a stop in two seconds. Rolling resistance is 0.02, so a
rolling ball now coasts (3.5% loss per second) instead of grinding down.

One bug found on the way, in code I had added: a resting-contact line
subtracted HALF of a negative normal velocity, so instead of cancelling
the fall it ADDED upward speed every frame — an invisible ratchet that
launched a rolling ball a metre into the air each second.

Shots and dribbling are unchanged by this (22.4 m/s exit, 33% carry
through a steady turn), and the bots read the new ball fine — air1 style
back up to 38.5.

## v1.56.0 — the pieces earn their squares

Every piece is now built facing OUT from its own side and the black army
is turned through half a turn, so ramps, kickers and chutes point down the
board from both ends instead of the black half facing backwards into
itself. Checked in the sim: a white pawn's ramp reaches +5.4 z, a black
pawn's -5.4 z.

And the pieces are things to play with rather than decorated blocks. Each
one now does what the piece does:

- PAWN — a kicker, and nothing else. The front rank is a row of take-offs
  pointing down the board.
- ROOK — a castle you drive INSIDE: a doorway at the foot, a spiral bore
  carved up through the tower, crenellations at the top with gaps wide
  enough to drive out of, and a boost on the battlement.
- KNIGHT — the arched neck built as a curved quarter-pipe: ride up the
  inside of the arch and it throws you, with the launch ring at the lip.
- BISHOP — a diagonal chute, because that is how a bishop moves: a long
  banked slide running corner to corner with lips to hold you in.
- QUEEN — the hub: a ramp spiralling up the OUTSIDE of her spire to a wide
  deck with boost, so you can leave her in any direction, as she does.
- KING — a bowl. He is the slowest piece, so he is the skate park of the
  set: a sunken court with banked walls to carve around and pop out of.

CHAMBER went 201 -> 373 shapes. The board still plays Morphy move for
move, still captures, still winds up with the score (0.28 -> 0.53 moves a
second), and every piece is still a moving platform that carries you.

## Stretch goals (still not in this cut)

Shadow mapping, more shape families (arches, helices, half-pipes), ghost-replay
of your best ring run, photo mode, achievements, OpenDHT global reach for the
session store (the vendored crate's `opendht` feature is the seam).

## Reference-code reuse

Built on the `logscope-c4-273` monorepo's game kits: wgpu 22 / winit 0.29
initialization, surface configuration, depth setup and the redraw-loop pattern
follow `games/clawbound` (app.rs, mesh.rs), and the SDF-collision approach
extends the ideas in its `openworld.rs`/`physics.rs`.
