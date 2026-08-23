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

## v1.57.0 — half the frame was never on screen

FRUSTUM CULLING. The renderer distance-culled but never checked whether a
shape was in FRONT of the camera at all, so on a big map most of what it
submitted every frame was behind the player. Six planes are pulled from
the view-projection each frame and every shape's bounding sphere is tested
before it becomes a draw. New `--sim render` measures it across seven maps
from eight compass headings:

    ARENA CLASSIC   96 shapes -> 38 drawn   60% saved
    PERPLEXUS      486 shapes -> 149 drawn  69% saved
    MONKEY RUN     593 shapes -> 176 drawn  70% saved
    DROPSHOT       483 shapes -> 201 drawn  58% saved
    CHAMBER        259 shapes -> 93 drawn   64% saved

This is safe by construction: shadows and reflections are ray-marched from
a SEPARATE shape list, so culling the raster instances cannot remove a
shadow caster or something visible in a mirror — the usual reason engines
have to be careful here.

FAR-FIELD LOD for the two most-used primitives. Spheres and capsules are
everywhere (foliage, rocks, pillars, orbs, portal faces) and the full
sphere is 1496 triangles. Past 90 m they swap to low-poly versions:

    sphere 1496 -> 320 triangles   capsule 720 -> 240   tube 320 -> 144

Both helpers (`frustum_planes`, `sphere_visible`, `shape_bound`) are free
functions so the sim can measure exactly what the renderer does, rather
than a model of it.

## v1.58.0 — the ground stops drawing itself twice

THE TERRAIN WAS ONE MESH. 221x221 nodes, 96,800 triangles, submitted whole
every single frame — the largest draw in the game and, as one mesh,
impossible to cull. It is now built as 6x6 CHUNKS over the same grid nodes
(so it stays node-identical with the physics heightmap, which is what keeps
the car sitting where the ground looks), each with a bounding sphere and
each frustum-tested. Measured across eight headings: 96,800 -> 42,380
triangles, 56% saved, with no visible change because what is dropped is
behind you.

MOVERS ARE DYNAMICS NOW, which is a correctness fix as much as a
performance one. Animated shapes were baked into the STATIC reflection
candidate grid at their load-time positions, so a moving platform reflected
wherever it had started — on CHAMBER that is 190 chess parts mirrored in
the wrong squares. They are excluded from the bake and appended to the
dynamic suffix instead, which is scanned live. Because that scan runs
per-fragment, the list is distance-sorted and capped at 28 within 190 m,
so a busy map cannot flood it.

Both changes are exercised by `--sim render`, which now also reports the
terrain triangle budget alongside the per-map instance counts and the LOD
savings.

## v1.59.0 — TRIALS: a reason to visit thirty-six maps

The game had grown a marble run, a chess board that plays itself, portal
chambers, a dragon, a wave arena — and nothing ever asked you to go to any
of them. TRIALS is a tour: fifteen challenges across eleven maps, each one
chosen to teach the mechanic its map was built around. PERPLEXUS asks you
to stay inside the pipes, CHAMBER to ride a piece across the board,
APERTURE for a hundred and twenty metres of portal travel, BREAKWATER to
surf the swell, CLOCKWORK to be carried by the machine.

The design rule: every goal is scored from state the physics ALREADY
produces — air-dribble time, combo count, height, speed, goals, tube
occupancy, platform contact, portal distance — so a trial can never
disagree with the simulation, and adding one is data, not code.

GHOSTS. A run records your car's path at 20 Hz. Finish a trial and the
ghost is kept, so the next attempt races your best — a translucent car
running the line you ran, interpolated between samples. Bests persist in
the save file.

Presentation: a TRIALS screen (name, map, your best, and what the trial
asks) reachable from the settings root, and an in-run banner with the goal,
a countdown that reddens in the last quarter, a progress bar, the time to
beat and whether a ghost is riding.

`--sim trials` checks the layer headless in fourteen assertions: every
trial points at a real map, they span the map list, each goal type can
actually complete, running out of time fails you, records only improve,
bests survive a save/load round trip, and ghosts replay where the run
went (midpoint and clamped-past-the-end both correct).

## v1.60.0 — the moment after the goal

Rocket League's best trick is not its physics, it is the replay: the game
stops, swings a camera round, and shows you what you did. Half of why a
shot feels good is watching it again. Skydribble had none of that — the
ball went in and play carried on.

THE RECORDER runs always: a fixed ring buffer of the last twelve seconds of
car and ball state at 30 Hz. Bounded by construction, so a ten-minute
session costs the same as a ten-second one (verified: 360 frames held
either way).

TRICK RECOGNITION is the part I care about. The buffer also carries a log
of what the PHYSICS reported — flip resets, pinches, dodges on landing,
aerial touches, ball height, exit speed — so when a goal lands the game can
name what happened rather than guess. Repeats collapse (a four-second air
dribble is one entry, not two hundred), stale tricks fall out of the
window, and the rarest one leads: "FLIP RESET INTO AIR DRIBBLE", not
"GOAL". Nine tricks are recognised, weighted by rarity, and the weights
also score the moment.

THE CAMERA needs no director: it orbits the midpoint of car and ball,
frames wider when they are far apart, and pulls in as the moment lands.
Good goals (score 6 and up) play back automatically the instant they go in;
R replays your best at any time; ESC skips. Letterbox bars and a scrub line
make it unmistakably a replay, and the six best moments of a session are
kept, rarest first.

`--sim replay` checks it in fourteen assertions: the buffer stays bounded
and holds the newest frames in order, the rarest trick leads the name, a
second trick is mentioned, repeats collapse, a plain goal is just a goal,
stale tricks leave the window, the camera keeps its distance and orbits and
closes in, and playback interpolates between samples and clamps past the
end.

## v1.61.0 — the ACADEMY: the game already knew, so let it teach

Skydribble grew a lot of depth — air dribbles, flip resets, wave dashes,
pinches, portal flings — and never explained any of it. The replay system
had already solved the hard half of a tutorial without meaning to: it
recognises tricks from physics events. That makes "do a wave dash" a real
instruction, because the game can tell whether you did one.

TEN LESSONS, in the order you would actually learn them: the flip, the wave
dash, the power shot, the aerial, going up, keepy uppy, the air dribble, the
flip reset, the pinch, the fling. Each one SETS THE BOARD — car here, ball
there, moving like this — so the trick is possible on the first attempt
instead of needing a lucky bounce. The board re-arms itself when the ball
settles, so practice is continuous.

VERIFICATION IS THE POINT. A lesson passes when the same detector that
names your replays fires, not when a timer runs out. Pass one and the
moment replays itself, which is both the reward and the demonstration for
next time. There is also a coach: if you have clearly tried and missed for
twelve seconds, it says the one thing most likely to fix it — "flip later,
you want to be landing as the flip fires".

The sim caught a real design flaw here. THE FLIP and THE WAVE DASH were
both verified by the same detector, so the first lesson would pass on a
wave dash — teaching nothing. Trick gained a plain FLIP (a dodge in the
air) distinct from WAVE DASH (a dodge into the ground on landing), and
`--sim academy` now asserts that no two lessons verify the same trick.

Thirteen checks in all: lessons name real maps, carry an instruction and a
nudge, teach the hardest tricks, verify only on the detector rather than on
time, scale measured goals correctly, set the ball up when the trick needs
it up, and record progress that survives a save.

## v1.62.0 — the marcher stops doing pointless arithmetic

With culling and LOD done, the dominant GPU cost is the ray march itself:
every step evaluated the FULL signed distance of every candidate — a
quaternion rotate and shape maths each — whether or not that shape could
possibly be the nearest thing. Three changes, all inside the inner loop:

BOUNDING-SPHERE REJECTION. The shape's bounding radius is already in the
buffer, so `length(p - centre) - radius` is a true lower bound on its
distance, at the cost of one length() against a full evaluate. A candidate
that cannot beat the running minimum is skipped outright.

SORTED CANDIDATES, which is most of the win. The rejection only helps if
the minimum is already small, so the gather now inserts candidates in order
of where the ray ENTERS their bounding sphere. The nearest is tested first,
the minimum drops immediately, and everything behind it falls to the bound.

RETIREMENT. Candidates also record where the ray LEAVES their bound, so as
the march advances it walks a start index forward and stops considering
shapes it has already gone past entirely.

Plus a quality knob with real teeth: the number of candidates gathered now
scales with ray quality (4 at the bottom, 16 at the top), and the marcher's
cost is candidates x steps.

New `--sim march` mirrors the shader's inner loop on the CPU and counts
exact evaluations with and without the changes, across five maps:

    ARENA CLASSIC  132,110 -> 71,238   46% saved
    PERPLEXUS      218,477 -> 149,550  32% saved
    MONKEY RUN     126,273 -> 90,586   28% saved
    CHAMBER         69,688 -> 50,855   27% saved
    DROPSHOT       278,556 -> 223,111  20% saved

DROPSHOT is the honest hard case and worth naming: 200 flat tiles at the
same range means the bounding spheres genuinely overlap, so there is little
to reject. The optimisation helps most where geometry is spread out, which
is also where there is most of it.

## v1.63.0 — ordering the frame for the GPU

The renderer submitted one draw call per object and re-bound EVERYTHING for
each: the object's uniform slice, its texture bind group, its vertex buffer
and its index buffer. Perspective on why that matters more than it sounds:
after culling, a frame is a few hundred draws, and on a phone the state
churn around a draw can cost more than the triangles inside it.

The frame is now ordered for the hardware rather than for the code that
built it. OPAQUE geometry is grouped by (mesh, texture), so consecutive
draws share state and the buffer and bind-group changes collapse to run
boundaries. Measured on PERPLEXUS: 171 opaque draws now cost 5 mesh
switches — 34 draws per state change, where it used to change on nearly
every one.

TRANSLUCENT geometry is drawn afterwards, sorted BACK TO FRONT, and that
half is a correctness fix as much as a performance one: the glass tubes and
junction rooms were compositing in whatever order the world happened to
store them, which is simply the wrong answer for alpha blending. They now
composite in the only order that is right. Depth-ordered draws cannot also
be mesh-batched — that is the trade, and it is the correct one.

`--sim render` reports both halves separately, because they are allowed to
do different things: it asserts the opaque half batches (at least three
draws per state change) and that the glass is genuinely furthest-first.

## v1.64.0 — the flick is a throw, not a hit

A flick in Rocket League is not a collision: the ball is resting on the car
and the car ROTATES out from under it, so the hood's own surface velocity
throws it. That makes it a test of the contact model rather than the
impulse — and new `--sim flick` measures five of them (forward, 45, side,
backflip, delayed) for exit speed, launch angle, distance carried and spin.

The first run said the pace was fine (18.6 m/s) but EVERY flick left at the
same flat 9-16 degrees. Two causes, both real:

1. THE PUSH DIRECTION WAS FLATTENED. A hard-won fix against ballooning
   shots also flattened the one touch whose entire point is vertical. The
   push now blends toward the contact point's SURFACE VELOCITY in
   proportion to how fast the car is spinning, and the hood's motion is
   carried into the ball on top of the collision — the car throws the ball
   rather than hitting it. Forward flicks went 15 -> 21 degrees, delayed to
   25, with pace held at 17-18 m/s.

2. A 144 rad/s SPIN SPIKE. The car's angular velocity is derived from the
   quaternion delta between frames, and a wrapped delta read as an enormous
   rotation on side dodges — which would have flung the ball absurdly. It
   is clamped to 14 rad/s now, faster than any flip the car can perform.

Also: a ball resting on the hood no longer bounces off its own carrier
(restitution scales with hit strength), so it rides the jump instead of
being launched clear of it.

OPEN, and stated rather than papered over: the BACKFLIP flick still leaves
flat (8 degrees, 13.2 m/s). The instrumented cause is contact — once the
flip fires the ball is only in touch for 0-2 frames, so the rotation never
reaches it, and a backflip retreats from a nose carry besides. The harness
asserts the three flicks that do work and prints the backflip's numbers as
an open item, so it cannot quietly regress or be forgotten.

Dribbling improved as a side effect of the softer carry contact: a steady
turn now holds 40% of the run (was 33) and a turn after landing 52%.

## v1.65.0 — the backflip flick was fine; the ruler was wrong

Last version left the backflip flick as an open item: 8 degrees and flat,
when it should loft. Tracing the contact frame by frame settled it, and the
answer was embarrassing in the best way — the PHYSICS WAS RIGHT and the
measurement was not. The harness reported the ball's peak speed, which
happens during the carry BEFORE the flip; the throw itself is the LAST
touch. Measured there, a backflip flick leaves at 76 degrees and 9.6 m/s: a
high, slow pop, which is exactly what the move is for.

A lesson worth writing down: an instrument that measures the wrong instant
will make correct physics look broken, and three plausible "fixes" had
already been tried against a number that was never going to move.

The flick family now reads faithfully — forward 17.8 m/s at 21 degrees, 45
degree 17.0 at 19, sideways 15.9 at 20, delayed 18.1 at 25, backflip 9.6 at
76 — and all of it is asserted, including that forward flicks stay LOW and
the backflip stays high.

A REAL BUG fell out of the air-dribble work. Angular velocity is derived
from the rotation delta between frames, so setting the car's orientation
INSTANTLY — a reset, a respawn, a portal — read as an enormous spin for one
frame. That was harmless until flicks began throwing the ball with the
car's spin; now it meant a ball near a portal exit could be flung across
the pitch. `Car::sync_rot()` clears the delta, and reset and portal travel
both call it.

AIR DRIBBLES are now measured too, and they behave: with the ball on the
nose and no boost, 3.21 seconds of carry over 46 small touches with the
ball ending 1.4 m from the car; boosting, 2.78 s and 130 touches; and
over-pitching loses it at 0.91 s, which is the correct failure mode. Both
the duration and the touch count are asserted, so a future change that
turns the carry back into a punt will say so.

## v1.66.0 — flicks answer to pace and position, and the catch is a skill

Two questions this time, both about whether the model has the RIGHT
RELATIONSHIPS rather than the right numbers.

DOES A FLICK ANSWER TO SPEED AND TO WHERE THE BALL SITS? In Rocket League
both matter enormously — a flick at pace carries the car's momentum, and
the ball's spot on the hood decides whether you drive it low or pop it up.
A matrix of three carry positions against three approach speeds says ours
does, without anything needing to change:

    carry @        5 m/s     12 m/s    20 m/s
    nose         18.3/30   22.2/21   24.4/19
    middle       12.3/36   18.6/24   20.6/22
    back          9.5/37   20.6/25   22.4/22

Speed adds 13 m/s from a crawl to 20 m/s, and the position gradient is the
right way round: nose flicks are the strongest and flattest (19 degrees),
back-of-roof flicks the weakest and highest (37). That is the shape of the
real thing, and it is now asserted.

IS THE CATCH A SKILL? Every dribble starts by turning a bouncing ball into
a carry, and in Rocket League that works by MATCHING the ball's motion. A
physics that catches everything is as unfaithful as one that catches
nothing. Ours: a matched approach settles the ball in 1.96 s and rides it;
six metres per second too fast and it is gone in 0.18 s; too slow and it
never settles; a fourteen-metre drop bounces away, as it should.

Getting that measurement honest took three attempts and is the lesson of
the session. First the harness drove a station-keeping controller under the
ball, which corrected every mistake and caught everything. Then a fixed
throttle caught nothing, because the car would not hold pace. The fair test
is the one a player actually performs: hold your OWN speed, never chase the
ball, and let the approach succeed or fail on its own terms.

## v1.67.0 — tube fields stop fighting each other

PERPLEXUS now traverses 22 of 24 routes with a single escape (it was 21 of
24 with two), from one change: in-bore gravity nodes reached r x 2.4, which
on a 12.5 m bore is a THIRTY METRE field. Where two lines run near one
another those fields overlapped and "down" flipped mid-tube. The reach is
r x 1.45 now — enough to hold you against your own bore, not enough to
reach into the next one.

The drive harness also got two corrections, both cases of measuring the
wrong thing. A route is the bore's CENTRELINE and a car drives its floor,
so spawning on the axis at a ground mouth dropped the car seven metres
through the open end before it moved. And these lines are ridden rather
than crawled — several are entered by launcher and none are climbable from
a standstill — so the test now enters at pace.

MONKEY RUN, still open and now better understood. Every one of its seven
routes leaves its bore within 4-7%, spiral tree roads and vine lines alike,
while Perplexus manages 22 of 24 with identical code. A failure that
uniform across different route TYPES is not a driving problem. The overlap
theory above was the best candidate and it demonstrably helped Perplexus,
but it did not move Monkey Run at all — so the cause is something specific
to that map's geometry rather than to the gravity field. The note is
recorded in the harness for whoever picks it up, along with what has
already been ruled out: spawn placement, entry speed, bore clearance,
wall thickness, junction width and node density.

## v1.68.0 — tube gravity that plants you, and an inverted assumption

IN-TUBE GRAVITY NOW HAS A SHAPE. A flat pull cannot satisfy both ends of
the problem: weak enough not to pin you against the bore (which is what it
felt like, and why it was reduced) is too weak to plant you there in the
first place, so a car flung into a wide pipe flew up the middle at constant
speed and never touched anything. The pull is now strongest at the AXIS and
eases to a third of that at the wall — it plants you quickly and then lets
go. Perplexus holds at 22 of 24 routes with a single escape.

New `--sim routecheck` reads the world SDF at every route waypoint, and it
inverted an assumption I had been carrying for several sessions:

    PERPLEXUS   median 16-22 in a 5.6-10.6 m bore, 100% "open"
    MONKEY RUN  median 7.40 in a 7.4 m bore, 12.00 in a 12.5 m bore

Perplexus's routes are OPEN-AIR lines between anchors — its 22 of 24 is
mostly flying through nothing. Monkey Run's waypoints sit dead centre in
real bores it must thread for hundreds of metres. Monkey Run was never the
broken map; it is the harder test, and the one whose result actually means
something.

The failure there is now precisely characterised, which is the useful
output of this session even though the number did not move. The car FLIES
rather than drives — og=0 for the entire run, climbing at a constant 23 m/s
and drifting off the centreline until a bend loses it — and it never lands
on the bore wall. And crucially, HALVING the swoop curvature and sag
produced identical failure points to the digit (4/5/6/7%). A failure
insensitive to the geometry is not a geometry failure, so the next look
belongs on the harness's fall-out rule and its spawn, not on the map. All
of that is written into the harness alongside the eight things already
ruled out.

## v1.69.0 — Monkey Run was never broken; the question was

Four sessions of "the vine lines are not drivable" ended today, and the
answer was that the measurement asked the wrong thing.

Following the evidence from last time — the failure was insensitive to the
route geometry, so the harness was the suspect — the trace was run out to
the moment a car "falls out". At sixty-nine seconds it read: wheels on a
surface, SDF 0.22 (resting on a bore wall), climbing steadily at 14 m/s.
The car was DRIVING, beautifully, for over a minute. It had simply taken a
different branch: on a map whose tubes all interconnect, leaving the
nominated polyline is playing the map correctly, not failing at it.

So the harness now measures NAVIGABILITY rather than obedience:

    MONKEY RUN   92% of the run inside a bore, 54% with wheels on a surface
    PERPLEXUS    83% inside a bore,             8% with wheels on a surface

That reverses the standing conclusion completely. Monkey Run is the best
driving map in the game — its bores hold a car for nine tenths of a run and
it spends over half of that actually on a surface. Perplexus, whose "22 of
24 routes traversed" looked like the healthy number, is mostly FLYING: its
routes are open-air lines between anchors (median SDF 16-22 in a 5.6 m
bore), so a car crosses them without touching anything.

The lesson is the same one this project keeps teaching from a new angle: a
metric that measures the wrong thing will indict working code for as long
as you let it, and the fix is to ask what the player actually experiences —
"can I drive these tubes?" — rather than what is easy to count.

## v1.70.0 — the CPU, measured

Every optimisation so far has been on the GPU side, where the numbers had
to be inferred. The CPU can be TIMED, so new `--sim perf` does: worldgen,
raw SDF throughput, a full step with a squad of bots, and the per-call cost
of the three functions every car and the ball hit every frame.

The first run named the culprit immediately:

    PERPLEXUS per call: sdf 791 ns, gravity_dir 4975 ns, platform_vel 78 ns

`gravity_dir` cost SIX TIMES an SDF query, and it is called for every car,
every ball, every frame — with 300 sparta bots that is a millisecond and a
half of frame time in one function. It scanned all 705 gravity nodes to
find the nearest two.

It has a spatial index now, and getting it right took three passes worth
recording:

  * Built BEFORE the node-thinning pass, so its indices pointed past the
    end of the thinned list. Panic, immediately.
  * A full-scan fallback "for safety" when no node was nearby — which fired
    on every query made in the open, i.e. most of them, so the index bought
    nothing. It is not needed: a node is inserted into every cell its reach
    touches, so an empty cell PROVES no node reaches that point and the
    field is simply world-down.
  * Scanning a 3x3 neighbourhood, which was nine times redundant for the
    same reason — one cell is already exact — on cells so coarse (56 m) that
    a whole lattice landed in one bucket. One cell now, on an 18 m grid.

Result: gravity_dir 4975 -> 3731 ns, and the frame step with twelve bots
0.347 -> 0.202 ms on PERPLEXUS and 0.130 -> 0.098 ms on MONKEY RUN, a 42%
and 25% saving on the whole simulation step. KUIPER's planet field still
reads dot 1.00 attractive and every physics harness passes, so the index is
transparent to the game.

## v1.71.0 — movers get an index too

Last version's profile left two leads. The larger one: CHAMBER managed only
908 SDF queries per millisecond with a mere 373 shapes, because ~190 of
them are moving chess parts and MOVERS CANNOT LIVE IN THE STATIC SHAPE GRID
— they travel out of the cell they were baked into. So every SDF query
walked all of them, and 190 branch-and-distance tests cost more than the
other 180 shapes put together.

They have their own index now, rebuilt each update (190 inserts a frame is
nothing) on the same cell grid as the gravity nodes, and primed at
generation so the first frame is already fast. That last detail mattered
more than it sounds: with the index built only on update, a freshly
generated world fell back to the full walk, and the benchmark showed almost
no gain until it was primed.

Across the two indexes, measured on the same harness:

    SDF queries per ms      CHAMBER   908 -> 8298   (9x)
                          PERPLEXUS   887 -> 1455
                         MONKEY RUN  3929 -> 5772
    physics step, 12 bots   CHAMBER 0.156 -> 0.045 ms  (3.5x)
                          PERPLEXUS 0.347 -> 0.198 ms  (1.8x)
                         MONKEY RUN 0.130 -> 0.101 ms
    gravity_dir per call              4975 -> 3146 ns

Every physics harness still passes — platforms, loops, portals, mechanics,
ball spin, flicks, and CHAMBER's board still plays and winds up — so both
indexes are transparent to the game.

## v1.72.0 — electric cars, and bodies built to a higher standard

The procedural cars were assembled from hard-edged boxes and squashed
spheres for wheels, which is exactly why they read as blocks rather than
vehicles: a real body catches a highlight along every edge, and a wheel has
a tyre and a rim. Two new primitives fix that at the source — `cbox`, a
chamfered box with proper bevel bands between its faces, and `wheel`, a
tyre torus with capped rim discs.

THREE ELECTRIC CARS built on them, each with a smoked glasshouse, sill
strips along the battery floor and light bars that emit:

  VOLT    a compact hatch, short and friendly — the EV that dribbles like
          an Octane
  AXION   a low hypercar: long, flat, spoiler on struts, a single blade of
          light across the nose — the flick blade
  HAULER  an electric pickup, all shoulders and straight lines, with a bed
          the ball settles into

AND THEY DRIVE DIFFERENTLY, which is the point of adding them rather than
three more skins. An electric motor makes its torque at zero revs, so
`Drive::Electric` gets a torque curve worth 1.35x off the line that fades
to parity with speed, and gives back 6% of top end for it. Measured in new
`--sim cars`: an EV reaches 8 m/s 14% sooner (0.47 s against 0.55 s) and
tops out at 21.6 m/s against 23.0. It wins the first two car-lengths off a
kickoff and loses a long straight.

The harness needed two corrections before it could say anything true. Its
speed gates were set above the throttle-only cap — which is 14.16 m/s, RL's
1410 uu/s almost exactly — so neither car ever reached them; and it read
top speed at the end of the run, by which point the car had driven into a
wall. It now times the launch on throttle alone and the top end under
boost, because the drivetrain cap and the throttle cap are separate things.

## v1.73.0 — lofted bodies, and five electric cars

Last version chamfered the boxes; they were still boxes. The real fix is a
LOFTED HULL: cross-sections along the length, skinned between, so a body
tapers into its nose, swells over the wheel arches and pinches at the tail
the way a car does. Each section carries a roundness value that blends it
from a rectangle toward an ellipse, which is what produces a shoulder line
instead of an edge. A hull carries 240 triangles against a box's 12.

FIVE ELECTRIC CARS on that hull, each with a smoked glasshouse lofted
separately and set into the body, flared wheel arches, sill strips along
the battery floor and light bars that emit:

  VOLT    compact hatch — dribbles like an Octane
  AXION   low hypercar, spoiler on struts, a blade of light across the nose
  HAULER  pickup with a bed the ball settles into
  PULSE   rally EV on tall soft tyres with a four-lamp roof pod
  TRAM    electric shuttle: a slab of glass with an enormous roof shelf

WHEELS ARE WHEELS NOW, for every car in the game. They were squashed
spheres, which is most of why even the older models read as toys; they are
a tyre torus with capped rim discs, and because all twelve share one
builder they all got the upgrade at once. Width against height: a sphere
measures 1.00, the new wheel 0.17.

The quality checks needed correcting twice, which is worth recording. I
first asserted the hull had more VERTICES than a box — it has 7x, not the
8x I guessed, because what curves is the surface BETWEEN vertices, so
triangles are the right count (20x). And I asserted the new wheel had more
vertices than the old sphere: it has slightly fewer, and that says nothing
— a torus is not a worse shape for being efficient. The real question is
whether it is disc-shaped, so that is what the harness asks now.

## v1.74.0 — one input, one rotation

Reported: flipping forward-and-right while HOLDING POWERSLIDE flips twice,
where the same input without powerslide flips once, correctly.

The cause is that powerslide is the air-roll modifier, so with it held the
steer axis feeds two things at once. A forward-right dodge spends steer
choosing its diagonal direction, and then the very same stick arrives again
as barrel roll layered on top of the scripted flip — 5 rad/s over a 0.78 s
dodge is another 220 degrees. Hence "double".

The roll passthrough exists on purpose (it is what makes a half-flip
possible), so the fix is not to remove it but to stop double-counting: the
passthrough is now scaled by however much steer went into the dodge itself.
A diagonal dodge consumed the steer axis and gets no roll from it; a
straight forward or back dodge did not, so it keeps the full passthrough.

Measured in `--sim flip`, total rotation with and without powerslide:

    forward+right      411 deg -> 411 deg   (delta 0)
    forward            357 deg -> 357 deg   (delta 0)
    back (half-flip)   357 deg -> 512 deg   (delta 155, as it should be)

The harness needed two fixes of its own before it could tell the truth. It
spawned the car six metres up, where the first jump does nothing and no
dodge ever fires — so it was measuring free air roll rather than a flip.
And it applied the roll as `-steer`, which is zero for a pure backflip, so
the half-flip case exercised nothing: a half-flip dodges straight back and
rolls AFTER the flip has started, so the roll is its own input.

## v1.75.0 — the glass was tiled, and it was expensive

Reported: the ray-traced look went blocky and slower somewhere between
v1.44 and v1.55. Both symptoms trace to one change, v1.48's "rough glass".

THE TILING. The roughness perturbed the reflection by hashing
`floor(wpos * 7.0)` — which quantises the world into 14 cm cells, so every
fragment inside a cell got the SAME jitter direction. That is a grid by
construction. Measured by walking a line a millimetre at a time, the old
hash jumps by up to 1.998 between adjacent samples (a complete direction
flip at each cell boundary); the replacement moves by 0.0008.

A hash was the wrong tool regardless: without temporal accumulation a
per-pixel random direction is just static, and quantising it to hide the
static is what produced the tiles. Three sines across the surface wobble
the reflection CONTINUOUSLY instead, which is what slightly imperfect glass
actually does, and it costs less than the hash did.

THE COST. The same change raised glass f0 to 0.92 at grazing angles, which
sent nearly every glass fragment down the full reflection march — and on
PERPLEXUS half the screen is glass. The ceiling is 0.72 now with a gentler
falloff, and the face-on value drops from 0.18 to 0.10, which is the number
that matters: it is below the 0.14 threshold for the CHEAP sky-tinted
reflection, so a pane viewed face-on — the fragments that fill the screen —
no longer marches at all. Grazing angles still get the full treatment,
which is where a reflection reads anyway.

The portal view's per-sample hash had the same flaw and got the same
treatment: a smooth angular offset per sample rather than a random one.

`--sim render` now measures the perturbation's continuity directly, so a
future roughness experiment that quantises space will fail rather than
ship.

## v1.76.0 — a calmer Perplexus, a calmer horizon

PERPLEXUS WAS A THICKET. Thirty-two anchors with four links each meant
tubes crossed constantly, junction rooms piled into one another, and the
sculpture stopped reading as a structure. It is twenty-two anchors on a
wider ring (48-165 m) with three links each, and links shorter than 26 m
are skipped — those were the ones that tangled, since two anchors a few
metres apart still get a full-width pipe and a junction room of their own.

    shapes 944 -> 725,  routes 94 -> 53,  worldgen 21 ms -> 11 ms

The loops are still seamless, the bores still drivable (p10 3.21 m, 1%
tight), and in-bore time actually went UP to 94% — a cleaner lattice is
easier to stay inside.

PIPE GRAVITY, softer. It ran (r/10) clamped to 1.5 with an axis boost of up
to 1.7x, so a bore could pull at 2.55x world gravity. It is (r/12) clamped
to 1.15 with a 1.45x axis boost — a peak of 1.67x, about a third less. The
axis-strong shape stays, because that is what plants a car entering a wide
pipe rather than letting it fly up the middle; tuning it needs care, since
the driving-contact metric is noisy (46% here against 54% before, and a
nearby setting measured 15%).

THE CAMERA STOPS ROLLING SO HARD. The horizon leaned all the way to local
gravity at 2.4 rad/s, so the world rotated out from under you every time a
tube's "down" swung round — disorienting rather than dramatic. It leans two
thirds of the way at 1.15 rad/s now: still unmistakably inside a pipe,
without the room spinning.

## v1.77.0 — the direction of an almost-zero vector

Reported: the ball sometimes leaves in a strange direction on a hit. A
result that is only SOMETIMES wrong is a discontinuity, so new `--sim hits`
sweeps the contact and measures how much the exit direction jumps between
neighbouring samples — nudge the contact a centimetre and the answer should
move a degree, not thirty.

Four regimes came back clean: across the nose (8.6 deg worst), up the face
(7.1), from a turning car (5.6), and one frame to the next (1.7). So the
contact geometry and the box hitbox were not the problem.

The fault was in the flick throw, and it is a classic. The push direction
blends toward `normalize(v_surf - car.vel)` — how fast the hood is sweeping
under the ball from the car's own rotation — weighted only by how fast the
car was SPINNING. But near the rotation axis that vector is almost zero,
and THE DIRECTION OF AN ALMOST-ZERO VECTOR IS ARBITRARY: normalising it
hands back noise. A fast flip touching the ball near the axis could
therefore throw it somewhere unrelated to the contact.

The weight now accounts for the surface SPEED as well as the spin, so a
hood that is genuinely sweeping throws the ball and a contact sitting on
the axis contributes nothing. Measured: throw weight 0.007 on the axis
against 1.000 out at the nose, continuous in between (largest step 0.02
across the range).

Flicks, shots, dribbling and the spin harness all read unchanged, because
the term only ever mattered where it was ill-defined.

## v1.78.0 — VIPERA and AEGIS, built from a specification

Two concept cars, built from a written spec rather than by eye — and built
in the order that spec insists on: WHEEL HARDPOINTS FIRST, then the chassis
between them, then body volumes around it. That ordering is the point. It
is what stops a car ending up beautiful with its wheels somewhere
impossible, and it made the rest of the construction fall out naturally:
every fender is placed from its hardpoint, not guessed at.

VIPERA — the green streamlined car. One continuous shell: a teardrop that
starts narrow and low at the nose, swells through the front fenders and
carries visibly more mass into the rear haunches. The cockpit is a dark
canopy sunk INTO the shell rather than a cabin sitting on top of it, steeply
raked at the windscreen and blending into the rear deck. The kidney side
cavity is a real recess with an extruded metallic trim rather than paint,
the amber light blade across the nose is five recessed segments, and there
is a blade spoiler on struts, a boost aperture and a diffusered floor so it
reads from underneath during aerials. Electric drivetrain.

AEGIS — the silver armoured wedge. The interesting part is that it uses the
SAME loft generator: at seven segments a cross-section stops being a curve
and becomes flat facets, so identical machinery produces armour plate
instead of aerodynamic skin. A centre ridge runs from the front tip to the
windscreen as the car's visual axis; the flanks are three layered plates at
different angles; the headlights are triangular recesses meeting in an
inverted V; the rear quarters are big triangular structures with a central
diffuser between them. Copper turbine wheels against flat grey armour, which
is the contrast the spec asks for. Combustion drivetrain, so the two also
drive differently.

`--sim cars` measures the grammars rather than trusting them: the typical
surface turn per step is 7 degrees on VIPERA's shell against 78 on AEGIS's
plates. (The first version of that check read the WORST turn and failed —
at the sharp ends of a very flat section, three times wider than tall, the
normal swings fast even on a smooth surface. The typical turn is what
describes a surface; the worst describes its flattest slice.)

Fourteen cars now. Wheels on both are built from the shared tyre-and-rim
mesh with six radial spokes and a deep dish.

## v1.79.0 — seven imported bodies, and an EV of everything

SEVEN NEW CARS from supplied .glb files: Backfire, Breakout, Assael,
Guardian, Orion, Vulcan and X-Devil — between 12k and 131k vertices each,
so they carry far more shape than anything built procedurally here. The
loader now takes either packaging: a .gltf folder with sibling buffer and
textures, or a single .glb with everything inline. New `--sim glb` reports
what each file actually contains, because a model that silently loads with
no geometry is worse than one that fails.

EVERY CAR NOW HAS AN ELECTRIC VARIANT — twenty-three bodies, and not one of
them modelled twice. `electrify` takes a finished model and converts it,
which is the only sane way to do this: it keeps the body and hitbox exactly
as they are and adds the seven cues that say "electric" — a full-width
light bar at each end, a blanked grille where the intake was (an EV has no
radiator to feed), sill strips along the battery floor, and an ion thruster
ring with a dark throat where the pipes were. The drivetrain flips with it,
so an EV variant genuinely drives differently rather than just looking it.

ION THRUSTERS replace the flame. The distinction is physical rather than
decorative: a combustion flame is turbulent and orange because it is
burning, while an ion plume is a collimated jet of charged gas — so it
leaves the nozzle at 7-11 m/s against a flame's 2.5-4.5, spreads a fifth as
much (0.18 against 0.8), and runs white-hot at the throat through cyan into
violet as it cools. Same particle system, opposite character, and it fires
automatically for any car with an electric drivetrain.

## v1.80.0 — the GARAGE

GUARDIAN WAS FACING BACKWARDS. The loader infers the nose from the
geometry, and inference is not always right — which is invisible until you
drive it. `load_oriented` takes a flip, and the Guardian gets one.

THE CUSTOMIZER. Rocket League's exotic items are mostly textures that move.
This engine can do better, because it already ray-marches reflections: a
SKIN here is a small shader program that receives the surface point, its
normal, the VIEW DIRECTION, the clock and the car's speed, and returns an
albedo, an emissive and a spec. Because it sees the view direction it can
respond to how you are LOOKING at the car; because its spec feeds the
reflection march, a skin can hand itself over to the ray tracer entirely.

Ten finishes, each a different idea rather than a different palette:

  IRIDESCENT     hue rotates with the viewing angle — a curved flank shows
                 a whole spectrum at once, and it moves as you orbit
  ENERGY FLOW    bands running nose to tail along the body's own axis
  BREATHING      a slow cycle, seams brightest on the rise: a machine
                 idling rather than a decal
  LIQUID CHROME  albedo near zero, spec 0.95 — the finish IS the reflection
  SCANLINE       a sweep that traces the bodywork, brightest where the
                 surface turns away, so it draws the car's shape
  PLASMA         drifting cell noise in the BODY's space, so the slick
                 belongs to the car instead of sliding over it
  CIRCUIT        a lattice of traces with pulses running along them
  FORGE          heat driven by SPEED, not time: leading edges glow harder
                 the faster you drive
  MOIRE          two lattices at slightly different scales, so the
                 interference shifts with both angle and time

Skins run in the BODY's own space, so a moving car carries its finish
rather than sliding through a world-space pattern, and each car gets a
phase offset so two cars with the same skin are not in lockstep.

SEVEN WHEEL STYLES (stock, turbine, blade, mesh, disc, spindle, halo), SIX
BOOST TRAILS and SIX GOAL EXPLOSIONS — and each of those is a different
physical story rather than a recolour. RIBBON hangs almost stationary and
marks the line you drove; EMBER has real weight and falls; VOID is
emissive-zero near-black, so it reads as a hole in the scene; PRISM splits
into three spectra that separate as they spread. SINGULARITY spawns its
particles out WIDE travelling inward and only bursts outward once they have
collapsed.

2520 combinations, four slots, all cosmetic and all saved. `--sim garage`
checks every option has a name and a description, that wheel styles
actually differ, that a loadout survives a save round trip, and that
nonsense in the save file cannot panic the game.

## v1.81.0 — the bounding box is not the body

Reported: the Vulcan EV and the X-Devil EV both look wrong along one flank.

Two hypotheses were checked and DISPROVED before the real one, which is
worth recording because each was plausible:

  * Mirrored node transforms. A negative determinant flips triangle winding
    and is the classic cause of one side of a car looking wrong. Every one
    of these models is a single untransformed node — no mirroring anywhere.
  * An off-centre body. If a model's X extents are asymmetric, centring on
    the bounding box puts the body off the axis and symmetric add-ons land
    correctly on one side only. Measured: Vulcan is 0.8% off and X-Devil
    0.1%, both fine. (ASSAEL is 5.1% off — a real finding, but not the one
    being reported.)

The cause was the EV conversion, and it is a lesson about what a bounding
box actually describes. `electrify` placed its sills, bars and thruster at
`half_w` — the widest thing in the model, which on a car is the WHEELS.
Measured overhang of wheels beyond the door skin:

    vulcan 18%   xdevil 18%   orion 14%   backfire 13%

The two worst are precisely the two reported. The sills were hanging 18% of
a half-width out in the air beside the doors.

The cues are now placed on the BODY: `electrify` measures the real flank
from the model's own surface points — the 90th percentile of lateral
distance, taken along the middle of the car at door height, which excludes
the wheels — and sits the sill just proud of it. `--sim glb` reports the
overhang per model and now fails if the measured flank ever collapses back
to the bounding box.

## v1.82.0 — the other two ray systems

Every optimisation so far went into the REFLECTION march, which is only
part of what a fragment pays for: it also casts a shadow ray and four
ambient-occlusion probes. Measuring all three together showed the shadow
ray was carrying nearly as much cost as the reflection and had received
none of the improvements.

SHADOW RAYS NOW RETIRE CANDIDATES. The shadow march is monotonic in t,
exactly like the reflection, so it can walk a start index forward and stop
considering shapes it has already gone past. The machinery was written for
reflections and simply never applied here — the shadow was walking the full
candidate list at every step. 24% fewer evaluations.

REFLECTIONS MARCH AS FAR AS THEY ARE WORTH. `tmax` was a constant 260 m
regardless of how much of the pixel the reflection would actually become. A
reflection contributing a tenth of a fragment does not deserve a full-length
march — the far end of it is fog-tinted sky either way. The distance now
scales with the blend weight, which is 42% fewer evaluations on a weak
reflection and leaves a mirror-bright one untouched.

AO SKIPS ITSELF when its candidate set is empty and the fragment is clear of
the ground: four map_d calls that could only ever return "no occlusion".

New in `--sim march`: a per-fragment breakdown across all three systems, so
the next round of this work starts by looking at which one is actually
expensive rather than at the one that was expensive last time.

## v1.83.0 — the gather kept the wrong candidates

With the three marches trimmed, the remaining per-fragment cost is the
GATHER that feeds them — it runs three times a fragment and had never been
looked at. What turned up there is a correctness bug wearing a performance
costume.

The candidate list is capped (16 at full quality, fewer as quality drops).
It used to keep whichever shapes ARRIVED FIRST and drop everything after,
so what a ray could see depended on the order a cell happened to store its
contents in — and a nearer shape found late was discarded in favour of a
farther one found early. Measured on PERPLEXUS, over rays whose cell holds
more than sixteen candidates:

    first-N missed a NEARER shape on 34% of them

Those are reflections and shadows passing through geometry they should have
hit. The list now keeps the NEAREST candidates: when it is full and the
newcomer is farther than the current worst, it rejects on a single compare —
the common case — and only runs the insertion sort when the answer will
actually change. Insertion work drops 38% as a side effect, but the reason
to do it is that the previous behaviour was order-dependent.

Worth naming the pattern, because it has now happened three times in this
renderer: a cap that silently discards is not a performance decision, it is
an unstated approximation. Draw distance discards what you cannot see; this
discarded whatever happened to be indexed late.

## v1.84.0 — the imported cars get their wheels back

THE WHEELS DID NOT TURN. Every imported model arrives as ONE merged mesh,
so the game had nothing to spin or steer: seven cars rolling on frozen
tyres, which reads as a toy however good the bodywork is. Named nodes solve
this when an exporter provides them, and none of these do.

So find the wheels GEOMETRICALLY. Take the vertices in the lower half of
the car, split them by corner, and in each corner find the tightest
concentration in the SIDE VIEW — a wheel is the one part of a car that is
round and centred on its own axle, so a cluster centre and a radius describe
it well. Triangles are then assigned whole, by their centroid, so a tyre
never ends up half body, and each piece is compacted to the vertices it
actually uses (without that, every wheel carried a copy of the whole car's
vertex buffer — five uploads of 130k vertices for one model — and the pivot,
being the mean of a prim's vertices, would have come out as the centre of
the car rather than the axle).

Six of the seven split cleanly. VULCAN declines: one of its corners holds a
cluster of radius 0.04, which is not a wheel, so the splitter refuses rather
than guessing. That is the right failure — a car with static wheels is
better than a car with a piece of its door spinning — and the guard is why
the other six can be trusted.

CENTRED ON THE CAR, NOT ITS BOUNDING BOX. A car is left-right symmetric, so
its lateral centre is where its mass balances; the bounding box is not that,
and one mirror or aerial shifts it. ASSAEL measured 5.1% off, which is
visible. X now takes the centroid when the two disagree by more than 2% of
the span. Y and Z keep the box, since there is no symmetry to appeal to
along those.

`--sim glb` reports, per model, how many corners look like wheels and their
radii — so a future import that fails to split says so instead of quietly
rolling on frozen tyres.

## v1.85.0 — the wheel slot does something

THE GARAGE PROMISED SEVEN WHEEL STYLES and changed nothing: the data was
there and nothing drew it. Rims are real geometry now — a lip, a face and
either a solid disc or a set of flat spokes — and they are drawn OVER
whatever wheel a car already has, riding its own pivot and its own spin.
That is what makes the slot work for every car in the game, the imported
bodies included, without touching a single one of their meshes. Seven
distinct meshes, 72 to 108 triangles each.

That required care with the mesh registry: the rims took indices 17-23,
which is where the terrain chunks had been, so `M_TERRAIN_0` moved to 24. A
silent off-by-seven there would have drawn ground where wheels belong.

THE WHEEL SPLITTER NOW SEARCHES for its threshold instead of assuming one.
How far out a wheel sits varies between cars, and a single fixed cut caught
a piece of bodywork in one of VULCAN's corners — a "wheel" of radius 0.04.
Five cuts are tried and the first that yields four plausible wheels wins,
with a new sanity rule that matters more than the radius bounds: THE FOUR
WHEELS OF A CAR ARE THE SAME SIZE, so a set whose radii disagree by more
than 1.9x is not four wheels. That tightened the detections on Guardian,
Orion and Breakout as well.

VULCAN still declines, at every threshold — its corners genuinely do not
hold four similar wheels. It keeps a correct static body rather than a
spinning door, which is the right answer, and `--sim glb` names it.

## v1.86.0 — skins that read the game, and parts that move

SIX EXOTICS. The first ten finishes read the surface, the view angle and the
clock. These read the GAME, which is the thing a texture cannot do however
well it is animated:

  HUNTER       glows on the side facing the BALL, so the car turns its light
               toward the play as you turn
  AFTERBURNER  lights from the tail forward as boost comes on — the car is
               its own boost meter
  GHOST        translucent when you coast, solid when you work; the fresnel
               keeps the silhouette readable so it fades without vanishing
  THERMAL      false colour by how square each panel is to the way you are
               travelling: the parts pushing air go white
  AURORA       curtains driven by world height, so they hang in the air and
               drift past as you climb
  CRYSTAL      a lens rather than a mirror — the reflection is displaced
               from the world behind it

They needed a second input block: the direction to the ball IN THE CAR'S
OWN FRAME, and whether boost is running.

RIGS — a new slot, and the answer to "breathing robotic car parts". Skins
change what a surface looks like; a rig changes WHERE IT IS. The body is
already split into prims (hull, canopy, fenders, spoiler, four wheels), so
each is displaced independently by a function of its own position and the
clock — and each gets a phase from that position, because a car whose parts
all move together reads as one wobbling object rather than as machinery.

  BREATHING  panels rise and settle, slightly out of step
  FLUTTER    a fast tremor, strongest at the extremities
  UNFOLD     plates lift away from the body under boost and close when you
             lift off
  HOVER      the body floats free of its wheels and leans into the turn
  SHIVER     parts jolt on impact and damp back down

Sixteen skins, six rigs, seven wheels, six trails, six explosions: 24,192
combinations, all cosmetic, all saved.

## v1.87.0 — collections, paint, and four skins that keep score

COLLECTIONS. Choosing six slots that agree with one another is most of the
work of looking good, and it is work most people would rather not do. Nine
curated sets, each built around one idea rather than one colour:

    NOVA CORE   heat and pressure — forge paint, unfolding plates, supernova
    WRAITH      absence — ghost, hover, a void trail, everything falls inward
    CIRCUITRY   a machine that admits it — traces, a tremor, hexagons
    PARADE      loud on purpose — iridescent, prism, spectrum everywhere
    COLD FRONT  aurora over cobalt, ion thrust, one clean shockwave
    PREDATOR    it watches the ball, and it breathes while it waits
    MERIDIAN    liquid chrome, hovering, nothing but reflections
    SPECTRE     crystal facets, a shiver on contact, prism shards

Picking one writes every slot; changing a slot afterwards just leaves you on
"CUSTOM", which is how it should be — the set is a starting point, not a
cage.

PAINT, twelve colours, deliberately saturated because the skins tend to wash
a colour out and a timid palette disappears under them. The finish is a
BEHAVIOUR and the paint is whose car it is, which is why they are separate
slots. The garage shows the palette itself on that row rather than dots.

FOUR MORE EXOTICS, reading things the others do not:

  MOMENTUM       the paint keeps score: hotter the longer you keep the ball
                 alive, cooling the moment you drop it
  IMPACT         rings spread from wherever the car last hit something,
                 travelling outward in BODY space so the wave rolls across
                 the panels that actually took the blow
  CONSTELLATION  stars fixed in the WORLD, not on the paint — the body moves
                 through the field like light through a window
  MIRAGE         a travelling wave bends the surface before anything is
                 shaded with it, so the reflection swims

Twenty skins, six rigs, twelve paints, seven wheels, six trails, six
explosions — 362,880 combinations, nine of them one keypress away.

## v1.88.0 — the rings, and tubes that actually connect

THE CIRCULAR ARTIFACTS were my own chamfer. v1.48 baked a mouth bevel into
the tube MESH, which means every segment drew a ring at BOTH of its ends —
and a pipe is dozens of overlapping segments, so the bore filled with
concentric circles that appeared and vanished with the view angle.

A chamfer belongs at an OPENING, so it is its own mesh now, placed only
where a bore genuinely opens. Finding those needs two tests, and the second
only became obvious after the first was not enough:

  * no other tube continues the bore at that end, and
  * the end is not buried inside a junction room — most of PERPLEXUS's links
    end that way, and counting only the first test still produced 117 rings
    for 94 segments

    PERPLEXUS   94 segments -> 75 mouth rings (was a ring on every end)
    MONKEY RUN  188 segments -> 1

TUBES NOW CONNECT. Where two pipes crossed, the union kept BOTH sets of
walls, so the bores stayed sealed from one another: the junction looked open
and was not. A pass over the finished world finds every pair of tube axes
that come within their combined radii at a genuine angle, and carves a
sphere at the closest approach — which removes both inner walls at once.
Perplexus opens 64 such crossings, Monkey Run 16.

A tube segment is exactly a Shell shape, so both passes read the geometry
back out of the world rather than being threaded through the generators.
That is why they work on every map at once rather than on the one they were
written for.

`--sim loop` now reports rings per segment and walks the world SDF straight
through the carved junctions: 40 of 40 are open all the way through.

## v1.89.0 — the menus, fixed and measured

THE MOUSE DID NOTHING on the three newest screens, and the reason is a
one-line omission: `hover_row` ended with `_ => None`, so TRIALS, ACADEMY
and GARAGE had no hit-testing arm at all. Every click on them was correctly
computing "no row is under the cursor". They have arms now, using the SAME
layout constants the drawing uses — which were previously buried inside the
draw functions, and are shared consts precisely so the two can never drift
apart again. Clicking a garage row also cycles its option: left half back,
right half forward, since a row that cycles needs a direction and a click
has one.

OVERLAPPING TEXT. Text width scales with 1/aspect, so the same string is
far wider in NDC on a narrow window than on a wide one — and rows laid out
against FIXED budgets ("the name gets 0.52") walked straight into the value
beside them. `row_pair` measures the value first and gives the name whatever
is left after it and a gap, so the two cannot collide at any window shape.
Titles and subtitles now scale with the same factor as everything else;
previously they were fixed-size and simply ran off the panel.

The scale floor also moved from 0.62 to 0.46, because 0.62 on a tall narrow
window still left text wider than the panel containing it.

New `--sim menus` measures the widest possible row against the panel across
six window shapes from 0.55 to 2.4, and checks that a click in the middle of
a row lands on that row and that a click above the list hits nothing:

    aspect   0.55  0.75  1.00  1.33  1.78  2.40
    widest   1.02  0.76  0.73  0.73  0.70  0.53   (panel is 1.60)

## v1.90.0 — the frame counter was counting the wrong thing

Reported: 600 FPS on the counter, vsync on, a 240 Hz display, and it feels
like 30. The first two facts are incompatible — with vsync on, presents
cannot exceed the refresh rate — so the counter was measuring something
other than frames reaching the screen.

It was. It smoothed 1/dt of the REDRAW CALLBACK, which counts loop
iterations, including every iteration that acquired no surface and drew
nothing. FPS now counts PRESENTED frames over a wall-clock window, which is
bounded by the display by construction. Modelled in `--sim fps`: a loop
spinning at 600 while presenting one frame in twenty reads 600 under the old
scheme and 30 under the new one — the number the display is actually
showing.

WHY FRAMES WERE BEING ABANDONED, and the likely cause of the 30. An
Outdated surface reconfigured and RETURNED, dropping that frame entirely.
Anything that puts the surface out of date repeatedly — a compositor, a DPI
change, a resize storm — therefore becomes a loop that renders nothing while
the redraw callback spins as fast as the CPU allows. The acquire now
reconfigures and TRIES AGAIN, so a frame still gets out.

DROPS ARE VISIBLE NOW. The HUD shows a dropped-frame count beside the FPS
whenever it is non-zero. A low number with drops means the renderer is
abandoning frames; a low number without them means the GPU is simply busy.
The old counter could not tell those apart because it hid both.

## v1.91.0 — the imports find their own way up

Five of the six reports shared two causes, so they are fixed once each
rather than car by car.

ORIENTATION IS DETECTED NOW. An export can arrive in any frame and a vertex
buffer does not say which — but a car has a giveaway: FOUR WHEELS, the same
size, low down, at the corners. Every axis-aligned frame is scored on that
(plus: longer than wide, wider than tall, mass sitting low) and the best is
kept. What it found matches the report exactly:

    vulcan    240 deg about (1,-1,-1)     "on its right side"
    backfire  270 deg about (1, 0, 0)     "backside facing up and rotated"
    breakout  180 deg about (0, 1, 0)     "reversed, back facing front"
    guardian  180 deg about (0, 1, 0)     (the hand-typed flip is gone)
    orion      90 deg about (0, 1, 0)
    assael     90 deg about (0, 1, 0)

Everything above is front-to-back symmetric, so a car and the same car
reversed score identically — a coin flip on every 180-degree yaw, which is
why half of them came out backwards. A car is LOWER AT THE FRONT, so the
score now compares the mean height of each half and rewards the frame whose
nose end is the low one.

WHEELS SPIN ABOUT THEIR AXLE. The pivot was the mean of the prim's
vertices, which is right for a clean wheel and wrong for one that caught a
little bodywork: the centre shifts off the axle and the wheel wobbles. The
split already computes each wheel's circle centre, so that is carried out
and used directly.

AND THEY CATCH LESS BODYWORK. The capture disc ran to 1.05x the wheel
radius and swept in bumper and arch — Orion's bumper turned with its
wheels. It is 0.92x now, and a triangle must also lie in the wheel's own
slab along the axle, not merely outboard of centre.

AEGIS HAS VOLUME. At half the height of the others it read as a plate with
wheels; the wedge silhouette is the point but a wedge still needs body
behind it. Sections are ~60% deeper, carried in the middle where a cabin and
floor would be, and the hitbox follows.

THE CUBE HAS ARCHES. A solid box with wheels bolted to its faces has them
half-buried, which looks like a mistake rather than a joke. The lower body
is four corner posts, two sills and a spine, leaving a real opening at each
corner for the wheel.

## v1.92.0 — cars assembled from other vehicles' layouts

The supplied library ships NO GEOMETRY. It is a catalogue: 5,873 part
signatures with a class and a size, and for 178 vehicles, WHERE each part
sits. That is a compositional grammar rather than a set of meshes — and it
is the more interesting input, because instead of importing someone's car a
car can be ASSEMBLED from the layout of a tank, a mech or a rocket.

Five donors, mined and normalised. Each arrives in its own units and axis
convention, so each is treated the same way: keep the largest 54 parts (the
structure, not the greeble dust), centre the cloud, then map its longest
extent to Z, the next to X and the shortest to Y — which puts any donor,
however it was authored, onto a car's envelope.

    ROLLOUT    sci-fi tank      16 block, 9 panel      spread 0.50
    CRYOFIN    slick racer      21 ring, 9 greeble     spread 0.44
    TL-4       walking mech     18 panel, 10 block     spread 0.63
    SPEARHEAD  rocket            38 ring, 9 block      spread 0.05
    OUTBACK    sci-fi jeep      12 block, 10 greeble   spread 0.51

The donor genuinely shows through, which was the thing worth checking rather
than assuming: SPEARHEAD's lateral spread is 0.05 — it is a spine, because
a rocket is — while TL-4 comes out plated and OUTBACK comes out covered in
detail. `--sim parts` compares every pair of shape signatures and fails if
any two donors would produce the same car; the closest pair differs by 0.12.

Two things the library cannot supply, so this module does. GEOMETRY PER
CLASS: a class is a hint about shape, so blocks become chamfered volumes,
struts bars, discs and barrels round things, greebles crisp little boxes. And
A LEGAL CHASSIS: the donor decides the look, the game decides the wheels —
four, at the corners, the right size, able to steer. A donor with treads,
three wheels or none still produces something you can drive.

One case worth naming: a donor can be MOSTLY wheel-class, because a rocket's
rings are catalogued as wheels. Dropping them all left SPEARHEAD nearly
empty, so only the four lowest are treated as running gear and the rest are
drawn as the rings they actually are.

## v1.93.0 — hovercars, which play by different rules

Six hovercars, and the point of them is not that they float. Almost every
assumption the wheeled car makes is wrong for a machine with no tyres, so
three of them are replaced.

THE CUSHION. A wheeled car is held ON the surface; a hovercar is held ABOVE
it by a damped spring along the surface normal. It floats over gaps, kerbs
and ramps that a wheeled car has to climb. Getting there meant switching off
three things that exist to keep tyres on the ground — the suspension spring,
the surface stick, and the four wheel probes — each of which was quietly
dragging the hull back down to wheel height.

NO LATERAL HOLD. A tyre cancels sideways motion almost entirely; a cushion
of air does not. A hovercar keeps whatever momentum it had while the nose
merely points somewhere else, so you steer by pointing and thrusting rather
than by turning. Measured after a hard turn: a wheeled car ends up 11
degrees off its nose, a hovercar 90.

THE REPULSOR FIELD, which is the interesting one. A hovercar has no bodywork
worth hitting a ball with, so it does not hit: a cone in front of the nose
PUSHES, hardest straight ahead and falling off with distance. The ball is
never struck, it is HERDED — held ahead of you at range, steered round a
corner untouched, pressed up a wall. Verified at 2.6 m with zero contacts.
Since the force falls off with distance, a hovercar cannot hit hard by
driving hard: power comes from the PULSE on boost, which is 3.5x the steady
field (5.9 m/s against 1.7). And the ball presses back on the car, so the
field is not a free tractor beam.

Two bugs worth recording. The cushion first went inside the grounded branch,
so it stopped the instant the car floated — every chassis sat vibrating on
the contact threshold at 0.11 m whatever its stiffness. And a proportional
spring balances gravity at an OFFSET, so each one then floated exactly
g/stiffness below its ride height; carrying the weight explicitly leaves the
spring to correct error only. Heights now land on 0.62, 0.55 and 0.34 as
set.

The six differ in ride height, cushion stiffness, lateral hold and field
reach: BRUTIS leans on the ball with an enormous soft field, AKIMBO slides
like a puck, CONTINGENCY pokes with a lance, TEMPEST rides high enough to
clear things, VECTORZOID has the strongest field in the game, MATRIS is
where to start.

## v1.94.0 — [ and ] cycle cars

With thirty-odd cars, walking to the garage to try the next one is the wrong
amount of friction. `[` and `]` step through them in place.

THE LAYOUT PROBLEM is the whole of the work here. A keyboard layout moves
CHARACTERS while the physical keys stay put: Dvorak puts the bracket
characters where QWERTY has - and =, and puts / and = where QWERTY has the
brackets. Matching only the character serves the player who thinks "press
the bracket"; matching only the position serves the one who thinks "press
the key next to P". Both are matched, so both get what they reached for, and
`--sim keys` confirms the combination captures nothing else — every ordinary
key, including the / and - that Dvorak users hit constantly, passes through
untouched.

SINGLEPLAYER ONLY. In a net session the other players have been told which
car you are driving and are predicting against its hitbox; swapping it
mid-match would desync them.

The swap also lifts the car clear of the floor if the new model is taller,
rather than leaving it half-buried, and toasts the name and position in the
list so you know where you are in thirty cars.

## v1.95.0 — the garage puts the car on stage

The customizer described a car in words while the car itself sat behind the
panel. Twenty skins, six rigs and twelve paints are hard to choose between
when you cannot see any of them.

THE LIVE VIEW IS THE REAL CAR, not a thumbnail: the same model pushed into
the scene a few metres in front of the camera, on a slow turntable with its
wheels turning. Because it is genuinely in the scene it gets the real skin
shader, the real rig motion, the real reflections and the real lighting —
and none of that has to be kept in sync with a separate preview path, which
is the usual way a preview drifts out of date. It stands upright in the
world rather than in the camera's frame, since a car leaning with the camera
reads as broken.

THE PANEL MOVES ASIDE RATHER THAN THE CAR MOVING BEHIND IT. On a wide window
there is room to the right and none above, so the car goes right and the
panel takes the left. On a tall or square one the reverse: the car goes
above and the panel drops to the lower part. The dimmer follows the panel
instead of covering the screen, so the stage stays bright.

The rows, dots, swatches and blurb are all laid out from the panel's own
bounds now rather than from fixed screen coordinates, so they reflow with
it. Hit-testing follows too — the panel is in a different place on a tall
window, and a click has to land on the row it appears to be over. `--sim
menus` checks both layouts.

## v1.96.0 — hovercars get thrust that returns, and a pulse

The diagnosis was right and worth restating, because it is not obvious: a
hovercar with no lateral grip and no boost cannot change direction AT ALL.
A wheeled car with an empty tank can still steer; a hovercar becomes a
passenger. That is why they felt like a novelty rather than a role.

TWO-TIER THRUST. Underneath everything there is now a small BASE reserve
that refills faster than it drains — 0.91 s of thrust, back in 0.65 s — and
pushes gently (55% of a wheeled car's boost). Pads no longer top that up:
they fill a separate CHARGE pool which is spent first and pushes HARDER than
a wheeled car's boost (125%). So a hovercar is never without a way to
redirect itself, and a pad is still worth crossing the pitch for. Measured
over three quarters of a second of thrust: 4.2 m/s on the base tier, 9.3 on
collected charge.

THE PULSE replaces the jump, which a wheeled chassis gets from pushing
against the ground and a cushion cannot. It lifts the car 7.5 m/s straight
up in the WORLD frame, and fires an omnidirectional blast that shoves the
ball and everything else nearby away. That one move covers both gaps: the
car can leave the ground to meet a ball, and it can refuse to be knocked off
one — an opponent lining up to bump you gets pushed away instead. Physics
can reach the player and the ball, so the blast is reported as an event and
applied to the bots by the caller.

It is on a COOLDOWN rather than a resource (1.6 s), for the same reason as
the base reserve: a tool you can run out of is a tool that leaves you
helpless at the worst moment.

And it adds rather than replaces, which is what makes these redirect
machines: a ball already travelling at 14 m/s leaves the blast at 21.8.

## v1.97.0 — bodies for the assembled cars, and better axles

THE ASSEMBLED CARS HAD NO BODY. A mined layout is a set of PART POSITIONS,
not a vehicle: dropping those in on their own leaves daylight between the
pieces, which is exactly why they read as half-built. A donor's parts
decorate a body — they are not the body.

Each now gets a lofted hull first, with its section widths and heights taken
from where the donor actually put its mass, so the shape still belongs to
the donor rather than being a generic blob. A station with nothing near it
borrows from its neighbours instead of pinching the hull to a point, which
matters for SPEARHEAD: it is a rocket, it has mass at two of five stations,
and it still needs a car around it. There is a darker glasshouse over the
middle too, so the result reads as a cabin rather than a loaf. `--sim parts`
checks the hull spans all five stations for every donor.

THE AXLES WERE OFF. The pivot came from the cluster's MEDIAN, which is
dragged off the axle by whatever bodywork the cluster caught — and a wheel
that turns about a point which is not its own centre wobbles. A tyre is a
circle in the side view, so fit one: two rounds of least-squares, the second
keeping only the points that agreed with the first, which drops the attached
bodywork before the final estimate. How far that moved each axle, as a
fraction of the car's half-height:

    assael 39%   vulcan 24%   xdevil 21%   breakout 16%   orion 10%

And capture is an ANNULUS now, not a disc: a triangle well inside the hub or
outside the tread belongs to something else, which is what had brackets and
arches turning with the wheel.

FOURTEEN HOVERCARS, up from six, and the numbers differentiate them:
BLACKSTAR has the shortest pulse cooldown and fights by shoving, HARDWALL
the strongest pulse and the longest wait for it, LINEBACK the fiercest
charge tier so a pad transforms it, ELEGANCE a base reserve so large it is
never quite out of thrust, ECHOTRON the only one with real lateral hold,
FOLLY almost none at all.

## v1.98.0 — the counter that never counted, and a gradient computed twice

FPS READ 0 BECAUSE OF MY OWN CHANGE. v1.90 made FPS mean "frames that
reached the screen" and then shipped WITHOUT THE LINE THAT ADVANCES THE
COUNTER — a regex insert that failed silently while the struct fields around
it landed. So `presented` stayed at zero and the readout dutifully reported
0 FPS forever. A counter is worth exactly as much as the line that advances
it, and `--sim fps` now checks the source for that line rather than only
testing the arithmetic around it. There is also a guard: a reading of
exactly zero is not a measurement, it is a broken counter, so it falls back
to the loop rate and lets the drop count tell the real story.

THE PROFILER SENT ME TO THE WRONG PLACE, twice, which is worth recording.
`gravity_dir` costs 3400 ns and it has a spatial index, so the obvious
suspect was bucket occupancy: the worst cell held 44 nodes. Quartering the
cell area cut that to 39 and changed the cost NOT AT ALL — because 39 nodes
at ten nanoseconds each is 390 ns, not 3400. The number never fit the
theory.

The real cost is that `gravity_dir` takes an SDF GRADIENT near a surface to
decide which way "down" is, and a gradient is four SDF evaluations. The
physics ground solve has already computed the normal at that exact point one
moment earlier. Two calls, eight evaluations, one answer.

A one-entry cache on the last (point, normal) pair collapses it. Measured by
COUNTING evaluations rather than timing them — timing a debug build is
noise, counting is exact:

    27.3 -> 19.3 SDF evaluations per physics step   (29% fewer)

That count is now reported by `--sim perf` as a stable measure of physics
cost, which is a better instrument than the timings for exactly the reason
this session demonstrates.

## v1.99.0 — the history under the game, and a doc that cannot rot

THE STORY GAINED A HUMAN PREHISTORY — twenty films of secret societies on
Earth — and the question was how to put it in a game that is about
Transformers. Not as a lore menu. The universe becomes ARCHAEOLOGY: the
player finds objects, objects bear on historical claims, and a claim's
standing rises as independent finds corroborate it.

THE ENGINE KNOWS NO HISTORY. `provenance.rs` names no civilisation and would
compile in a different game; `archive.rs` holds the content. That seam is the
one that pays, because the evidence tables grow with every film and the rule
for reading them must not move when they do.

Two rules carry the weight, and both exist because the obvious version was
wrong:

CORROBORATION COUNTS DISTINCT SOURCES, not finds. Blackstar's three Array
readings share one instrument, so together they reach UNKNOWN and stop. His
own array cannot confirm itself; the Carrion wreck is what makes it mean
something. That is the lesson, not a limitation worked around.

A CLAIM NOBODY HAS MADE CANNOT BE REFUTED. The first version drew SOURCES
DISAGREE over an official record the player had never seen — found by running
the thing, not by a test. An official date is somebody's ASSERTION, and stays
dark until somebody asserts it. Now the 1996 dig layer banks silently and
lands the moment you find the paperwork, which is a better beat than the one
I set out to write.

THE ASTERI MISTAKE IS COMPUTED. `orbits.rs` is a real long-period orbit
solver — Kepler checked against Halley's published elements — because
`voyage.rs` only does circular Hohmann transfers. The difference between
their published 417-year return and the true 3,812 is a 0.312% error in
perihelion speed, and at that precision the band has NO CEILING: their own
data could not exclude "never returns". They published the optimistic end of
an open band. Every figure falls out of the solver; none is asserted.

    +/- 0.312%   P in [417, unbounded] yr   <- theirs

THE MYSTERY ANCHOR is a menu dial for how much of this the game hands you
already believing — and it is not a tutorial slider. Received knowledge
enters as ONE CONTRADICTABLE SOURCE, and the received account includes the
official record, which is false. HOMEY hands you a lie to catch; FRESH gives
you a world with nothing yet to disbelieve. Same tables, different depth.

THE SHIP IS THE MENU, so archaeology got a station: `Station::Chartroom`.
`Medium` on each finding decides which surface can show it — a hard drive is
read at the terminal holding it because it IS a hard drive, not because a
screen was assigned. Only artifacts come home to the lamp.

THE FINDING IS NOT THE READING. Three protagonists plus a villain campaign
meant four evidence tables or one; `Lens` makes a new playable character a
COLUMN rather than a table. Coverage is deliberately partial — a lens with
nothing to add falls back to the object, tested, because four voices times
thirty-five findings is mostly filler.

`intel.rs` IS GONE, folded into the findings it duplicated. Its contract
survives as archive tests, including the one that mattered: the reveal
mission is late but not the finale.

THE C ENGINE GOT THE SAME TABLES, GENERATED. `exengine/archive_gen.h` comes
from `--arch emit-c`, following the convention `content_build.py` already
sets; the ladder is implemented twice and `tests/test_provenance.c` replays
fixtures the Rust suite also asserts, so a divergence is a test failure
rather than the codex quietly disagreeing with the sim.

AND THE DOCS STOPPED BEING DECORATION. `docs/STORY_BIBLE.md` is the canonical
fiction — the thing that replaces pasting script drafts into a session — and
its inventory is machine-checked. Set equality both ways on claims, lenses,
films and strata: add content without listing it and the suite fails; keep
naming something deleted and it fails too. Verified by breaking it on
purpose. Documentation rots because nothing breaks when it does.

THE CAMPAIGN'S SHAPE GOT DECIDED AND THEN GOT A LINT. It is Call of Duty's
structure — authored set pieces introducing an extended kit one verb at a
time — and then it INVERTS. A CoD campaign graduates you into multiplayer,
which is a SUBSET: smaller maps, fewer verbs, the toys gone. This one
graduates into a strict SUPERSET, Destiny's patrol loop over procedurally
generated worlds, closer to Ark or Rust than to Destiny's fixed handful.

That claim is exactly the kind that quietly stops being true: a mechanic gets
built for one scripted mission and never wired into patrol, and nobody
notices for six hours. So `curriculum.rs` is a ledger with lints — every
mechanic introduced by EXACTLY ONE mission, the table monotone with mission
order, the kit accumulating and never shrinking, raids allowed to deepen but
never to own, and the load-bearing one:

    NOTHING IS CAMPAIGN-ONLY

verified by breaking it on purpose. `the_far_gate` is the funnel's hinge and
a test checks most of the kit is taught before it, because opening the
procedural sky onto a player who cannot use it is the failure mode. Two rows
are honestly flagged unbuilt (`clinch`, `combine`) and a test pins that list
to docs/PLAN.md's wiring-debt table so the two cannot drift.

LOS ANGELES IS ONE CITY HELD TWICE. Findings had been sited on a
DESTINATION -- seventeen of them said "earth" and nothing more -- which is
fine for a table and useless the moment a player is expected to walk
somewhere. `groundside.rs` gives them addresses, and gives each address TWO
STATES: the bunker is an operational Directorate site in the archive era and
four floors of damp in the present. A test refuses any place that reads the
same in both eras, because a place with one state is a set, not a place.

THE GEOGRAPHY ANSWERED A QUESTION THE TABLES RAISED AND NEVER CLOSED. The
three findings that contradict the official 2003 excavation date -- 1998,
1996, 1994 -- all describe THE RIDGE. `the_beach_opens` puts Unicron's
fissure on the shore below it. So the record is not falsified to hide a
building; it is falsified because somebody was digging toward the thing in
the mantle a decade early, and 2003 is the smallest lie that stops anyone
asking what for. The game never says this. `RIDGE_OVERLOOKS_BEACH` keeps the
two adjacent by test and the inference stays the player's.

ONE HOME BASE, BOTH ERAS, BOTH CASTS: the Directorate's abandoned bunker is
the human campaign's safehouse and the LA hero's foothold at the same
address. A test asserts each era has exactly one home and that they are the
SAME one -- verified by decoupling them and watching it fail -- because
quietly letting them drift apart is how the overlap would be thrown away
without anyone deciding to. Its Transformer-side twin is the ship, which is
why `Station::Chartroom` and the bunker do the same job in two campaigns.

RAIDS CARRY HISTORY NOW, and the roster turned out to have been written for
it: the nineteen wardens are already framed as "wardens, judges, archivists,
seals". `judgementor` -- ancient defendants still orbiting as asteroid belts,
one verdict never delivered -- opens a docket left open on a civilisation
older than the machine's own jurisdiction. Layer 5 evidence reachable
nowhere else, and a test asserts it shares no source with any campaign
finding so it corroborates something new rather than repeating.

The gate vocabulary grew to carry it: bare id is a mission (every existing
row untouched), `warden:<id>` is a raid clear, resolved in one place by
`archive::gate_open`. A prefix rather than a second field, because two
nullable fields invite the state where both are set.

THE EDIT LOOP WENT FROM ~200 s TO 3 s, which is the change that matters most
for everything above. This crate is ONE package of 65k lines, so the
multi-crate tricks do not apply -- `gate` finds changed crates and there is
exactly one. The lever was the profile: `[profile.dev]` sets opt-level 2
because "physics + png decode are unusable at opt 0", which is true of
PLAYING the game and false of the test suite, which runs in 0.15 s and never
touches the renderer. The loop had been paying optimizer time it could not
use.

    dev, opt 2, no incremental     ~200 s per edit/test cycle
    ailite, opt 0, incremental        3 s per edit/test cycle
    cold, once                      153 s

Measured against a real semantic change, not a comment touch. `tools/fastcheck`
pins it, with the caveat written down: the opt-0 binary is too slow to PLAY,
so it is for tests, lints and the headless drivers only.

"NO SECRET BASE" IS A LINE IN THE SOURCE, and it overruled me. Last session
I gave the human campaign a home base: one building held by both casts, the
Directorate's bunker as safehouse then and foothold now, with a test pinning
them to the same address. Then the scene the team is built out of turned out
to say the opposite -- old car, three people inside, NO SECRET BASE, no
holograms, nobody completely trusting anyone yet. A safehouse with a wall of
monitors is precisely what that scene refuses.

So the archive era has NO home base and a test now asserts it stays that
way. The present keeps one, because Aiden is a kid who stayed at a fissure
rather than an operative with a network, and a fixed GTA-style home is right
for him. The asymmetry is the design.

WHAT THE ABSENCE BOUGHT was better than the base. `tradecraft.rs`: a car you
stage out of, a CASE you carry, and a scattered relay network. The point is
that FINDING STOPS BEING FREE -- in the ship campaign a scan puts something
on the map; here it only puts it in the case, and nothing reaches the map
until it is relayed. Every relay costs reach, exposure, or fidelity: a
payphone call is one item, a hideout is eight, a contact retelling a serial
number is not a photocopy of it.

EXPOSURE IS NOT A PENALTY, IT IS A PLOT. A rooftop lamp reaches everyone who
matters and everyone who does not -- which is HOW THE OFFICIAL RECORD COMES
TO BE WRITTEN BY SOMEBODY WHO WAS NOT THERE. The payoff was already built:
`facility_built_2003` is a claim the Directorate asserted, contradicted by
three dig dates. Reporting is a move in an argument, not a save button. And
the lamp sits on the ridge, which is also where the record was falsified.

THE DOC GUARD EARNED ITS KEEP HERE. Correcting the home base broke
`the_bible_does_not_lie_about_the_mechanics` -- the doc still claimed a
shared base -- and the failure arrived in the same three-second run as the
code change, which is exactly the case the test was written for.

TERRAIN.RS HAD ZERO CALLERS AND STILL DOES IN THE GAME LOOP, but it is
reachable and checkable now. 645 lines of physics-grounded worldgen -- dune
spacing from saltation hop length, talus runout from gravity, craters that
go central-peak at the right diameter -- and nothing had ever called it,
because nothing converted a `patrol::Destination` (gravity in g, a weather
flag, a sky) into a `terrain::Ground` (m/s2, a material, air density, u*).
Two tables describing the same eighteen worlds in different units with no
function between them. `mapsim::ground_for` is that function.

Material is the one judgement in it and is stated as such: the route table
does not record what a world is made of, so it is an explicit per-world
mapping with reasoning, not a heuristic pretending to be a measurement.

THE TOOL FOUND A BUG IN ITS FIRST RUN, which is the argument for building
the headless view before the renderer. `--terrain grounds` prints a column
of air density beside a column of friction velocity, and five rows read
u* 0.25 over air 0.000. Pluto and the four exoworlds carry `weather: true`
next to a vacuum sky. A friction velocity is a property of a moving fluid;
with no fluid there is no aeolian transport, whatever the flag says. Wind is
now suppressed where air is zero, the audit reports the route rows that
claim weather they cannot have, and a test names the three rows that
motivated it. Cheap to miss in a table, obvious in a column.

Mars dunes come out 3.5x Earth's at the same sample point, which is the
relation terrain.rs's own doc comment leads with -- now fed by real
destinations instead of hand-picked constants.

THE SUITE WAS RED ALL SESSION AND I KEPT CALLING IT PRE-EXISTING. Three
failures, correctly attributed to code I had not touched, and reported that
way every single run -- which means I had been reading "3 failed" as "fine",
which is exactly how a fourth arrives unnoticed. Both turned out to be real
bugs rather than stale expectations.

LERP WAS NOT ENDPOINT-EXACT. `Traits::blend` used `a + (b - a) * w`, which
is monotonic but does not return `b` at w = 1: tank->hoverer traction came
back 0.19999999 against the hoverer's own 0.2. So a fully blended chassis
was not equal to the thing it blended into. That is not cosmetic --
`flies()` is a THRESHOLD on lift, and a chassis one ulp under the line does
not fly. Taking the endpoints explicitly keeps exactness without giving up
interior monotonicity, and both properties now have a test.

SDF_CAPACITY NEVER LEARNED ABOUT TILING. The atlas became a TILES x TILES
grid -- sixteen models sharing one depth slab, which is what let 126 models
become 1,344 and the library stop being clipped -- and `sdf_capacity` kept
computing depth-only stacking. It under-reported by a factor of sixteen.

The interesting part is WHY nobody noticed: `.max(CSDF_MAXM)` swallowed the
answer on every device. 4096 computed 256, the floor returned 1344, and the
wrong number was never visible. The function's own doc says it is "NOT YET
USED TO SIZE THE TEXTURE", so it was a stale number rather than a rendering
bug -- and that is also the reason it rotted. An unused answer is never
checked against reality. Its test asserted the pre-tiling 256 and 512 and so
could not have caught it; the replacements check the RELATIONSHIP between
capacity, slab and tiles instead of restating constants, plus the claim in
CSDF_TILES' own comment that an untiled roster needs ten times the limit.

735 passing, zero failing.

THE NUMBERS I GUESSED ARE NOW IN ONE PLACE, SORTED BY HOW LIKELY THEY ARE TO
BE WRONG. A lot of recent work is decisions nobody can check without playing
-- whether the anchor should start at 45%, whether a case of six is pressure
or annoyance, whether two independent sources is the right moment to say
POSSIBLE. I can pick and reason; I cannot verify. So the useful question is
how somebody finds the numbers afterwards, and scattered as consts across
nine modules the answer was "badly".

`tuning.rs` is a registry with three properties that matter. It reads the
LIVE value through a function rather than restating it, so the ledger cannot
drift -- and a test asserts every stated default equals the code's actual
default. It says WHERE to change each one, and for config-backed knobs a
second test proves the save-file key actually works, because "no rebuild
needed" is a promise. And it carries a CONFIDENCE column, which is the
load-bearing one: `--tune guesses` prints only the five nobody has ever
verified, which is the actionable output of a playthrough rather than a wall
of forty numbers.

BOTH GUARDS FIRED IMMEDIATELY. The round-trip test caught `flip_land_assist`
-- a config key I had invented; the real one is `land_assist`. Then it
accused `cam_grav` of being broken when the key was fine and my PROBE was
wrong: every key clamps to its own range, so `default + 1` is a silent
no-op on anything bounded at 1.0. Probing from several directions fixed the
test rather than the code, which is the distinction worth recording.

THREE FEEL CONSTANTS BECAME CONFIG on the way: `case_cap`, and the
provenance ladder's `arch_possible_at` / `arch_high_at`. The ladder gets a
`sane()` clamp so an operator who inverts the rungs cannot make standing
non-monotonic in the evidence -- a save-file edit should be able to retune
pacing, not break the inference.

FOUR DEAD MODULES JOINED UP INTO ONE PIPELINE. `terrain.rs`, `procgen.rs`
and `drop.rs` had zero callers each; `foothold` and `loot` were reachable
only from other dead code. They are not separate gaps, they are one chain
nobody had ever run end to end: procgen decides what a bridge jump lands on,
the ground seam converts it, terrain generates it, foothold says what is
worth seizing, and drop is the twenty seconds between the hangar and the
ground. `--terrain jump` and `--terrain drop` run it. Warnings 405 -> 387.

THE DROP IS BALLISTIC, SO IT IS CHECKABLE WITHOUT A DISPLAY -- which is why
it belongs beside the map tool. Steering swings the miss from 25 m to 130 m
on the same line, and running it against the GENERATED world's gravity
rather than a hardcoded 9.81 means a heavy world is a measurably harder
drop. Both are tests now, and so is the claim drop.rs opens with: where you
land is decided before you jump.

STATIONS GOT ADDRESSES, AND "MENUS OBEY THE HULL" BECAME TRUE. `stations.rs`
said that in its own doc comment and enforced it with a phase check, while
`interior.rs` had been generating a real eight-room floor plan from
`content::HUB` -- positions, doors, reachability -- with no caller at all.
Every console now names the cell it stands in, placed by what the room IS:
the armoury is the upgrade bay because that is where the chassis rigs are,
the hold is the hangar because that is the room with a deck to bolt a chest
to. A test walks the plan and fails if a console sits somewhere unreachable
from the bridge.

WHICH IMMEDIATELY FOUND A BUG UNDERNEATH. `Interior::reachable_from` walked
every link in the plan whether or not the door was passable, so a depowered
ship -- hatches shut, which `depower()` explicitly does -- still reported
the whole interior as walkable. Anything asking "can the crew get there"
got "yes, always", the one answer a failure mode exists to contradict. Split
into `linked_from` (topology, unchanged by power) and `reachable_from`
(doors respected), and the new test asserts losing power actually strands
you. Two existing tests failed on the fix and both were right to: a freshly
generated interior has its doors SHUT, and the old code simply never looked.

753 passing, zero failing.

THE CONSOLES OFFERED MISSIONS AND PRESSING THEM DID NOTHING, and the code
said so out loud: `"{order:?} -- fly it from the terminal sims for now"`.
`deck` already handled `Did`, `Refused` and `Goto` itself; the two that
escaped to the shell -- a sortie and the week's nightfall -- were formatted
into a toast and discarded. That is the whole of docs/PLAN.md's wiring debt
compressed into one line of the event handler.

They run now, through `campaign::MissionRun` -- the same deterministic
resolver `--campaign` uses -- and the clear persists. WHAT THIS DOES NOT
CLAIM: the mission is RESOLVED, not played, and the toast says
"(resolved, not flown)" rather than letting a resolved clear look like a
flown one. Flying them is the renderer's job and is still ahead. Saying so
in the toast costs nothing and is the difference between wiring and
pretending.

The decision half split out as `order_target` so it could be tested at all:
`run_order` loads and stores the save, which is untestable in a suite
sharing one working directory. The pure lookup carries the whole mapping,
and the failure worth catching lives there -- the Bridge builds rows from
MISSIONS and the shell looks them up again by id, so if those two ever
disagree the console offers a mission that silently does nothing. Which is
the state this replaced.

TWO SMALLER THINGS FOUND ON THE WAY. `parse_args` had `--mobile` and
`--desktop-ui` listed TWICE, an unreachable-pattern warning nobody had read.
And `--help` named none of the twelve headless drivers, so the only way to
discover `--arch` or `--terrain` was to read main.rs. A tool nobody can find
is a tool nobody runs; they are listed now.

757 passing, 384 warnings (from 405 at the session's start).

TWO DISCOVERY LOOPS WERE RUNNING AT ONCE. `--arch find` (scan it onto the
map) and `--arch carry` (into the case, relay it later) belong to different
campaigns and are mutually exclusive by design -- the ship makes relaying
pointless, and in the archive era nothing reaches the map until it is
relayed. Both worked, always, because NOTHING RECORDED WHICH CAMPAIGN WAS
RUNNING. I had written the tradecraft layer, the era-split places, and a
bible section about the asymmetry, and never noticed there was no current
era anywhere in the save.

`cfg story_era` is that, as a dial rather than a story flag, because
switching between them is exactly what testing both needs. Each verb now
refuses in the wrong era and names the other one.

AND I SHIPPED THE GUARD WITHOUT IT LANDING. The edit that inserts the check
into `find` did not match its target -- an earlier change had moved the
surrounding lines -- so the patch applied cleanly to nothing and the tests
still passed, because I had tested `Era::from_cfg` in isolation rather than
the driver that consumes it. It was caught by running the actual command,
which is the third time this session that the headless driver has found
something a green suite did not. A unit test on a predicate proves the
predicate; only the caller proves the wiring.

CONTENT REACHABILITY IS NOW AUDITED, which is what a playthrough needs
before it starts. Three tests: every claim is reachable WITHOUT EVER
RAIDING (a raid may deepen the history -- that is the Judgementor docket --
but must never be the only door to any of it); the deep strata are arguable
from ungated content alone; and every finding is somewhere a player can
actually stand, which for Earth means an address in `groundside` and
elsewhere means a real route destination.

761 passing.

I SAID THE STATION WORK WAS UNPROVEN AND THEN PROVED IT, and it was worse
than I expected. `menusim` has existed the whole time -- `--menu <station>`
photographs a console as text frames, on the grounds that "if the cascade
stutters here it will stutter in glass". I had added two stations' worth of
rows and never once run it.

THE CHARTROOM SHOWED THREE IDENTICAL ROWS. The label carried the standing
and left the claim id out entirely, so every node rendered as
"L2 UNKNOWN TECHNOLOGICAL RELATION — 1 for / 0 against" and a player could
not tell them apart. The test I had written checked that the standing text
appeared, which it did, three times. Leading with the claim fixes it, and
the test now asserts the node rows are DISTINCT and each names itself --
the property that was actually broken.

THE CONSOLE IS 46 CHARACTERS WIDE AND NOTHING SAID SO. It was a bare
literal at one call site in menusim. Both Bridge additions overflowed and
lost precisely the informative tail: "— teaches tra~", "the open ~". Named
it `menusim::LABEL_W` and added a guard that builds every station and fails
on any row over budget.

THAT GUARD IMMEDIATELY FOUND TWO OVERFLOWS I DID NOT WRITE -- the energon
header at 49 chars and the nightfall label at 49 -- which had been
truncating since they landed. A constraint nobody can see is a constraint
nobody meets.

The lesson is the same one as the era guard last session, and it is now
three for three: a unit test on a value proves the value, and only the
driver proves the presentation. `--menu` was sitting there the entire time.

762 passing.

THE LAST PATH WITH NO DRIVER NOW HAS ONE. `run_order` lived in main.rs's
winit handler, so the one thing the shell had just learned to do -- execute
a sortie -- was the one thing nothing headless could reach. Worse,
`menusim` printed its own copy of the same disclaimer ("the shell flies
this; `--campaign` runs it"), so the excuse existed in two places and the
capability in neither.

Moved into `stations.rs`, which already owns `Order`, and menusim calls the
same function the winit handler calls. `--menu bridge --tape a` now runs the
week's nightfall and `--tape d,a` runs the sortie, both persisting, and a
second run correctly advances to the next mission because `camp.next()`
moved. If it breaks, it breaks here first.

AND IT GAINED A TEST THAT COULD NOT HAVE EXISTED BEFORE. Three tables have
to agree for a console row to do anything: the Bridge builds rows from
MISSIONS, `apply` turns a press into an Order, and `order_target` resolves
that Order back to a mission. While the shell threw every order away a
disagreement between them was invisible by construction. The test walks the
Bridge's own rows, presses each one, and asserts every Sortie and Nightfall
that comes back resolves to something real.

763 passing.

THE DIG WAS LOADED AND NEVER WRITTEN BACK. `Shipboard::store` persisted
missions, wardens, nightfalls, chi, gear, vault, watch and hotbar, and not
`finds`. Anything recovered at a console would have vanished on the next
load -- silently, because until now nothing at a console could recover
anything. A field added to a struct and forgotten in its own serializer is
the quietest bug shape there is, and the only reason it never bit is that
the feature it broke did not exist yet.

WHICH IT DOES NOW. The archaeology layer had no in-game acquisition at all:
`--arch find` is a command line, and the Chartroom was deliberately
read-only. But going through what you brought back IS that room's job --
"a long table under a hard lamp, and everything you carried home on it" --
so it lists UNEXAMINED evidence and a press records it.

THE GATE IS THE DESIGN: you can only examine evidence from a site where you
have cleared a mission. Without that the Chartroom is a button that hands
over the entire history and the archaeology layer is an inventory screen.
Clearing `the_beach_opens` opens Earth's evidence and nothing else, sealed
findings stay sealed, and pressing one reports which claims moved.

The read-only contract narrowed rather than broke, which is the distinction
worth keeping: pressing a NODE is still read-only, because the map is
DERIVED from the evidence and is not somewhere you can write. The old test
pressed a claim id and still passes unchanged.

765 passing.

I STOPPED WAITING FOR THE NEXT SILENT SERIALIZER BUG AND WENT AFTER THE
CLASS. `finds` was added to `Save`, wired into `load`, and left out of
`Shipboard::store` for several sessions. Nothing caught it because the
serializer existed only as a pair of functions that touched the filesystem:
a round trip could not be tested without clobbering the working directory,
so it never was. **A file-bound serializer is an untestable one.**

Split `parse`/`render` out of `load`/`store` -- headless core, thin shell,
the same seam this project applies everywhere else and had somehow skipped
in the one module where a dropped field is invisible. The round-trip test
then writes itself: build a `Save` with all thirty-one fields set to
distinctive values, render, parse, compare FIELD BY FIELD so a failure names
what was lost.

TWO GUARDS AGAINST THE NEXT ONE, both verified by reproducing the original
bug on purpose:

  * add a field to `Save` and forget it, and
    `the_round_trip_test_covers_every_field` fails naming the field -- it
    reads the struct declaration out of the source and checks each field
    appears in the assertions.
  * drop a line from `Shipboard::store` and
    `shipboard_writes_back_everything_it_loaded` fails with
    "Shipboard loads sv.finds and never writes it back", which is the
    original bug reported in its own words.

That second one is the interesting shape: it diffs a function's LOADS
against its WRITES rather than testing behaviour, because the behaviour that
would have exposed the bug did not exist yet when the bug landed.

770 passing.

CONFIG WAS THE OTHER UNTESTED SERIALIZER -- sixty-five keys, no round trip.
It turned out to be CLEAN, which is worth recording as a result rather than
skipping: the read and write key sets agree exactly, and every key moves its
own line and only its own.

THE INTERESTING PART IS THE BUG SET EQUALITY CANNOT SEE. `w("cam_dist",
self.cam.height)` passes every structural check -- the key exists on both
sides, the counts match -- and quietly swaps two settings the first time
anybody saves. Catching that needs behaviour, and doing it without
reflection needs a trick: perturb exactly ONE key, render the whole table,
and diff against the unperturbed render. Only that key's line may differ.

Both halves of a cross-wire are caught, from opposite directions, and I
introduced each on purpose to check:

  * write the wrong field INTO a key -> that key's own line never moves, and
    the message names both causes because the symptom is identical to a key
    the parser drops;
  * write one field into TWO keys -> the shared key moves two lines, and the
    failure prints both of them:
    `'cam_dist' moved 2 lines: [("cfg cam_dist 4.900", ...), ("cfg cam_height 4.900", ...)]`

The probe is multi-directional for the same reason the tuning ledger's is:
every key clamps to its own range, so a single probe value is a silent
no-op on anything bounded, and the test would pass by failing to test.

773 passing. Every serializer in the crate now has a round trip.

CAN THE RAID SYSTEM SERVE THE DETECTIVE CAMPAIGN? Partly, and working out
WHICH part was the whole exercise.

`foothold.rs` asks a good question in a Cybertronian accent: two ways into a
place, trading against each other, judged against a clock, with no answer
that is right in general. Clear an LZ -- fast, cheap, exposed throughout,
and the ship has to stay. Seize a fortress -- slow, dear, and the sky stops
mattering once you are inside. That is Hitman's core question, and the
detective campaign asks it with none of the same nouns.

What TRANSFERS is the decision shape. What does not: `Body` and
`habitable_fraction` (a person in Los Angeles needs no habitability check),
the nineteen Cybertronian wardens, and `loot`'s gear grades -- the human
payload is evidence, which is `Case` and already built. A Judgementor in a
warehouse is absurd.

So `approach.rs` is the question with the nouns removed, and it has TWO REAL
CALLERS rather than one and a hope: `foothold::assess` delegates its
arithmetic and keeps its domain (reaching the site, whether that shelter
stops THIS weather), and `tradecraft::infiltrate` is the same decision in
1998. The translation is term for term -- weather window becomes how long
before somebody else arrives, a grounded ship becomes a car parked where it
can be seen.

THE ABSTRACTION DID NOT FIT AND THAT WAS THE USEFUL PART. The raid's only
currency is TIME, so its ranking maximises margin; the detective campaign's
principal cost is BEING SEEN, which is not time at all. Modelling it as
`effort` failed silently, because effort only applies when nothing is
chasing you -- so the front door could never win and could never lose. An
option that is never correct is an option that should not be on the list.
`Opening::seen_cost` is the missing axis, in seconds-equivalent, counted
under pressure. Foothold passes zero, which is why its tests never moved.

AND I PUT ONE COST IN TWO PLACES. I first modelled the front door as "does
not shelter you", which made the core reject it outright whenever a clock
ran. But once you are through ANY door you are inside and out of view; what
differs is how much of the entry was in the open and what being noticed
there costs. Shelter is about where you END UP, not how you got there.

The roof earns its place only when the service entrance is watched, which
is a per-job fact rather than a property of the building -- so it is an
argument, not a table.

785 passing.

COMMANDERS TRANSFER WITH NO TRANSLATION AT ALL, which is the cleanest
answer this exercise has produced. The five archetypes are four dials --
aggression, caution, encircle, regroup -- and nothing in them is
Cybertronian: they describe how an armed group behaves under pressure. A
Directorate strike team surrounds you for the same reason a Decepticon squad
does. So the human campaign gets `skirmish.rs` for free and the only work is
an argument about each society: the Directorate encircles, the Free State
trades and leaves, the Covenant holds because it wants the digging to stop
rather than to win a firefight, and the trio press because three people
cannot afford a long engagement.

VISION WAS NOT TRANSFERABLE BECAUSE IT DID NOT EXIST. Not in `skirmish`,
which says so in its own doc -- "the shape of a fight without the geometry
of one" -- and not in the C engine's `enemy.c`. It was the thing BOTH sides
were missing, and I had already papered over the hole: `WayIn::seen_cost`
was three hard-coded numbers, which is a model's output written down as an
input.

`perception.rs` makes detection a RATE rather than a die roll -- attention
accumulates from distance, cover, motion and how hard the watcher is looking
-- which answers the question `approach::Opening` actually needs (*how many
seconds can I be here*) in closed form. The one physical claim is the
falloff: angular size goes as 1/d, so the rate falls as the INVERSE SQUARE
of distance, and that is why "stand a bit further back" is terrible advice
at eight metres and excellent at fifty. Everything else is a dial.

AND I DOUBLE-COUNTED THE SAME COST AGAIN, in a new place. I added an
`is_watched` predicate to rule doors out, on top of `seen_cost` charging for
them -- the same fact twice -- which ruled out the front door BY DEFINITION,
since being seen is what the front door IS. Deleted, with a test left behind
so the predicate does not come back. Being noticed is a cost; whether it is
worth paying is `approach::choose`'s job, and it already knew how.

The gradient that falls out is the one worth having, and it moves with the
watcher rather than with a constant:

    30s -> the front door    60s -> the service entrance    120s+ -> the roof

Against a hunter the front door costs eight times more and the roof is free,
because fifty metres is outside its thirty-five. Distance is the only answer
to that encounter, which is the correct shape for the Jhiaxus horror.

793 passing.

SCALE: THE PROBLEM IS NOT THE CAMERA, IT IS THAT SIZE IS NOT DATA.

`model_tuning.json` carries a per-model `scale` and Optimus Prime's is
**0.4989**. Across 1,012 models the medians by kind sit between 0.5 and 1.8
-- robots 0.904, vehicles 0.530 -- which is to say every model, whatever it
depicts, is normalised into the same playable box. A Transformer and a
Camaro are the same height in-game. The camera cannot adjust to size because
nothing knows its size: there is one number per model and it means "fit this
in the box", not "this is how tall it is".

AND THE UNITS ARE NOT REAL-WORLD-RELATIVE, which is bigger. DESIGN.md's own
conversion table divides RL's uu by a hundred and calls the result metres,
so the tuning body is 0.38 m tall and 1.18 m long -- a QUARTER of a real
sedan. Drop a 1.75 m person into that world and they are one and a half
times the LENGTH of a car. `Class::EngineBody` exists to keep that body's
tuning intact while stopping it being mistaken for a statement about
vehicles, which is what it was doing: filed as a Vehicle, its thirteen-times
chase ratio gave a real car a fifteen-metre helicopter shot.

FRAMING IS PER CLASS because the right distance is a genre convention, not a
function of height. Scaling everything off RL's ratio puts a person nine
metres back. Within a class, size varies by `h^0.85` -- sublinear, because a
body twice as tall does not want the camera exactly twice as far; some of
that extra height is frame it is fine to fill. Bots are framed closer in
proportion than people and with the camera BELOW the eyeline, which is the
cheapest way to make a thing read as enormous rather than as a man in a suit.

    human       1.75 m    3.5 m back    1.9 m up
    car         1.45 m    4.1 m
    bot         8.50 m   16.0 m         4.7 m up
    2 Cal Plaza  228 m   343.3 m

The existing camera is reproduced to the centimetre at the legacy body, so
adopting this moves nothing about the current game -- asserted, because if
it were not true the change would not be additive.

ASSETS EXIST, and I was wrong to assume otherwise. The catalogue has 1,012
representatives including `imported_two_california_plaza_los_angeles` (real:
228 m, 52 floors, Bunker Hill -- verified), several brutalist blocks, city
streets, NYC low-rise, and human references: `witwicky_suit_reference`,
`humanoid_realistic_old_russian_guy_animated`, `female_human_face_reference`.

The hard part is not the models. It is that a person and that tower in one
shot is a 130:1 span and a 343 m pullback, and depth range is what actually
breaks a renderer.

801 passing.

THE DEPTH RANGE IS ARITHMETIC, SO I CHECKED IT INSTEAD OF HAND-WAVING. Last
entry ended on "a 130:1 span is what actually breaks a renderer", which was
a claim with no number behind it. `camera.rs` builds
`perspective_rh(fov, aspect, 0.08, 2600.0)` and `render.rs` allocates
`Depth32Float`: an eight-centimetre near plane, a 2.6 km far plane, and
STANDARD Z. That is the worst of the four combinations, because the
perspective divide already concentrates precision near the camera and float32
also concentrates its precision near zero -- and standard Z stacks both in
the same place.

    distance      standard      reversed      gain
       343 m      0.0820 m     0.00002 m     3934x
      1000 m      0.7312 m     0.00005 m    13407x
      2500 m      3.0281 m     0.00001 m   448939x

343 m is the pullback `--terrain scale` gives for Two California Plaza. A
facade has detail at the centimetre, so standard Z there puts the windows and
the wall in the same depth bucket. At 2.5 km it resolves three METRES.

I MEASURED IT WRONG FIRST, in a way worth keeping. The reversed-Z value was
inverted by computing `1.0 - z` in f32 and reusing the standard inverse --
and that subtraction re-quantises onto the standard grid, because near zero
`1.0 - tiny` throws the tiny away. The experiment reported the two schemes as
IDENTICAL and would have "proved" reversed Z pointless. Each mapping now
carries a direct inverse, the analysis runs in f64 so the only f32 rounding
is the one under test, and the bug is a test so it cannot come back.

AND THE STORY I WANTED WAS FALSE. I expected to find that only a pipeline
change fixes this. Measured, nudging the near plane from 8 cm to 12 cm
already buys 5 cm at 343 m without touching anything. The case for reversed Z
is not that the alternative fails; it is the SIZE of the win -- three orders
of magnitude finer, at the shipped near plane, for a flipped depth compare
and a clear value of 0.0.

NOT VERIFIED ON A GPU, and the report says so where somebody will read it.
This is the arithmetic of the depth mapping and says nothing about shadow
cascades or LOD popping, which are the other two things a 130:1 span breaks.

807 passing.

A THIRD LEVER ON DEPTH, AND IT IS THE FREE ONE. The near plane is a FIXED
0.08 m in `camera.rs`. That is correct for a 38 cm body and absurd for a
camera 343 m from a skyscraper: the perspective divide spends most of the
buffer between eight centimetres and a metre, where in that shot there is
nothing at all. `scale::near_for` scales it with the shot at 1/50 of the
chase distance, which leaves the human case at almost exactly 0.08 m -- so
close work does not move -- and gives the tower shot nearly seven metres of
near plane, which costs nothing because nothing is drawn there.

    as shipped        near 0.08 m, standard Z    0.09383 m
    near plane only   near 6.87 m, standard Z    0.00070 m   (free)
    reversed Z only   near 0.08 m, reversed Z    0.00001 m   (flipped compare)
    both              near 6.87 m, reversed Z    0.00003 m

THEY ARE ALTERNATIVES, NOT ADDITIVE, and I had written a test asserting they
compose before the report printed all four rows next to each other. "Both" is
WORSE than reversed Z alone. Reversed Z gets its precision from float32 being
dense near zero, which is where it puts the far plane; pushing the near plane
out on top redistributes the same budget and hands some back. So the
recommendation is a choice rather than a stack:

  * no pipeline change available -> scale the near plane. 130x better, free.
  * able to flip the depth compare -> reversed Z, and KEEP the small near
    plane, so nothing close is lost.

The composing assumption is now a test asserting the opposite, because it was
the kind of wrong that reads as obviously right.

809 passing.

AND THEN I LEFT THE FIX IN A MODULE NOTHING CALLED, which is the exact
failure this session has spent its time correcting elsewhere. `scale.rs` and
`depth.rs` measured a free 130x improvement and `camera.rs` went on
hardcoding `perspective_rh(fov, aspect, 0.08, 2600.0)`. Analysis nobody
applies is a document, not a change.

The near plane is now derived in `view_proj` from the actual shot distance --
`(self.look - self.pos).length()` was already there. The far plane became
`Camera::FAR_M` so the renderer and the analysis cannot drift apart silently.

THE SAFETY ARGUMENT HAS TO BE ARITHMETIC because the renderer is the one
thing here I cannot look at. The gameplay camera sits 4.9 m back, so the near
plane moves 0.08 -> 0.098 m; nothing in a chase shot is within ten
centimetres of the eye, and a test asserts the gameplay value stays inside a
narrow band while the tower shot gets its seven metres.

MY FIRST INTEGRATION TEST WAS A TAUTOLOGY. It called `view_proj`, threw the
result away, rebuilt the projection matrix itself with the near plane it
expected, and asserted the two agreed -- which proves only that
`perspective_rh` is `perspective_rh`. It would have passed with the wiring
absent. The real version undoes the view (`vp * view.inverse()`) and reads
the near plane back out of the matrix the camera actually returned, via
`n = m32/m22`. Verified by deleting the wiring and watching it fail:

    at 4.9 m the camera's own matrix has near 0.07999985, expected 0.098

812 passing.

I AUDITED MY OWN MODULES FOR THE THING I HAVE BEEN CORRECTING ELSEWHERE and
found fifteen items across this session's work that existed only to be
asserted by their own tests. A ledger whose data cannot be looked at is a
test fixture wearing a table's clothes.

Three outcomes, and the split is the point:

  * SURFACE IT where the data was worth reading and simply had no reader.
    `curriculum`'s `used_in`, `built` and `module` had never been printed;
    `--tune kit` now shows the whole ledger, and the `built` column is
    honest bookkeeping about wiring debt that belongs beside `--tune`'s
    honest bookkeeping about guessed numbers. Likewise `Society::doctrine`
    (the commander mapping was content nobody could see), `relays_at`,
    `groundside::home` and `place_of`, the size ladder's `note`, and
    `depth::near_needed_for`.

  * DELETE IT where a second path already existed. `Case::take` delegated
    straight to `take_capped` and stopped being called the moment the driver
    started reading `cfg case_cap` — two doors to one room, one of them never
    right. `curriculum::mechanic` was a by-name finder built for symmetry
    with every other table here, and nothing ever wanted one mechanic by
    name. `Class::all`, `Case::is_empty`.

  * MOVE IT where it is genuinely test-only. `RIDGE_OVERLOOKS_BEACH`
    documents a story fact for one test and now lives in that test module.

And `config`'s `case_cap` default stopped repeating `6.0` and now derives
from `tradecraft::CASE_CAP`, which is what made the constant live again
rather than requiring a fake caller.

375 warnings, from 405 at the start of the session, with far more code in
the tree than there was then.

812 passing.

COMBINE WAS THE OTHER `built: false` ROW, and unlike `clinch` it did not need
a screen. `assign`, `viable`, `combine_time_s` and `scale_vs` existed, were
tested, and were called by nothing; the curriculum ledger had been reporting
that accurately for several sessions without fixing it.

It is worth a driver because the interesting claim is checkable: NOT EVERY
SQUAD CAN COMBINE. Roles want proportions, proportions are measured off the
models, and a squad of five identical racers fails at 0.42 coherence against
a bar of 0.62 while a squad built for it merges at 0.95. `--combine grid`
prints the fitness table so the assignment explains itself, which is the
whole argument for greedy over a Hungarian solve.

MY EXAMPLES ONLY DEMONSTRATED ONE OF THE TWO RULES. Both failing squads
failed on COHERENCE, so `MIN_SYMMETRY` never decided anything the driver
printed and a reader would have had no idea what it was for. The case I had
written -- "one arm is a tank" -- fails on shape, because a tank makes a bad
arm before it makes a mismatched one. Replaced with two GOOD arms of wildly
different length: coherence 0.95, symmetry 0.25. A test now asserts each
example fails for a different reason, so a rule cannot go undemonstrated
again.

AND A GESTALT IS A RUNG ON THE SIZE LADDER, which is what made this wireable
at all. A combined body has a height, and since `scale.rs` exists that height
means something: the squad above comes out at 11.9 m and 45 t, framed 21 m
back and 7 m up, from `scale::camera_for` rather than from a multiplier
somebody typed. `Gestalt::scale_vs` had been waiting for a scale system.

`curriculum` now lists one unbuilt mechanic instead of two.

814 passing.

THE TUNING LEDGER OVERSTATED ITSELF, which is the same failure it exists to
prevent. Its doc opened with "every number I guessed, in one place" and it
covered ELEVEN OF SIXTY-FIVE config keys. A document that claims completeness
and has 17% coverage is worse than one that claims nothing, because a reader
with a playthrough's worth of notes will trust it and stop looking.

The other fifty-four are now accounted for rather than quietly absent, in two
categories that are real distinctions and not filing:

  * RL_CALIBRATED (25) -- gravity, ball and car physics, the flip rules, the
    chase camera. These are not judgment calls with a confidence level, they
    are a reference implementation's numbers, and DESIGN.md carries the
    conversion table. The right way to change one is to discover the
    conversion was wrong, not to decide it feels better. Filing them as
    GUESSES would bury the eleven that genuinely are.

  * NOT_A_FEEL_KNOB (29) -- vsync, gyro deadzone, render scale, the colour
    grade. Hardware, accessibility, and switches whose right value is
    whatever the player says. A ledger that listed `vsync` beside the mystery
    anchor would be telling somebody to go and have an opinion about vsync.

AND A GUARD SO THE GAP CANNOT GROW: every config key must be in exactly one
of the three, and the exemptions must give a reason. Adding a key without
deciding fails with the key named. Verified by adding one. A stale exemption
fails too, because a category that accumulates dead entries stops meaning
anything.

`--tune` now opens "11 chosen here, 5 never verified (25 more calibrated
against Rocket League, 29 not feel knobs at all)", which is the true shape.

814 passing.

THE C-SIDE CAVEAT IS CLOSED, and it should have been closed hours earlier.
"The CMake edits are unverified -- no cmake here" was reported at the end of
every summary that touched the C engine. It was true and it was also four
lines of shell: `pip install cmake` provides one, and the configure failure
underneath it

    Could not find a GL runtime library (GL/GLESv2/OpenGL)

is not a missing dependency so much as a missing SYMLINK. Containers ship
`libGL.so.1` and `libSDL2-2.0.so.0` and omit the unversioned names that
`find_library` wants, because those belong to -dev packages. With no root
there is no apt, and none is needed: a directory of symlinks you own, handed
to CMake as `CMAKE_LIBRARY_PATH`, plus `-L` on the linker line for the
libraries resolved by name.

VERIFIED, finally, rather than asserted:

  * `provenance.c` compiles as part of `cybertron_modules` through the real
    target, not just under a hand-rolled gcc line;
  * `test_provenance` is registered, builds, and runs as ctest #56;
  * the whole project builds -- all 65 targets including `game`;
  * 65 of 67 ctest tests pass.

THE TWO FAILURES ARE NOT MINE AND I CHECKED RATHER THAN CLAIMED IT.
`content_validate` wants five part_group meshes that are not in the tree in
any form, `.iqm` or `.obj`. `model_tuning` compares `EX_N_IMPORTED` against
the `imported_*.iqm` files in `engine_src/data`, of which there are ZERO --
and the catalogue README says why: ".iqm is built, not stored". Neither test's
source is touched by anything here.

That second one incidentally corroborates the scale work: the baked header
and `model_tuning.json` agree with each other at 1012 entries, so the 1,012
models whose `scale` values I analysed are the real roster and not a partial
read.

`tools/sandbox_build.sh` makes it reproducible, with the two expected
failures documented in the script so the next person does not spend the time
I did deciding whether they matter.

THE GENERATED HEADER HAD NO GUARD, which is the drift it exists to prevent
reintroduced one level up. `archive_gen.h` is emitted from `archive.rs` by
`--arch emit-c`, and nothing checked that the checked-in copy still matched.
Edit a table, forget to regenerate, and the C engine reads a stale roster in
silence.

`test_provenance.c` cannot catch it: it validates the header against ITSELF —
every finding links to a claim that exists, every id is unique, no dead ends.
All of that stays true of a perfectly self-consistent header describing last
week's content. A generated artifact needs a test against its SOURCE, and
consistency tests read as if they are that test without being it.

The emitter split into `render_c()` and a write, the same seam `save.rs` got
for the same reason, and a Rust test now compares the rendered text against
the file. It names the file, the line and both versions:

    archive_gen.h is stale — run `--arch emit-c`.
      line 76: on disk  { "museum_specimen", "earth", ...
      line 76: expected { "museum_specimen", "luna",  ...

Verified by moving a finding between worlds and watching it fire, then
rebuilding the C target and re-running ctest to confirm the round trip.

I ALSO LOOKED AT WIRING THE CODEX and did not. `EX_GS_CODEX` has 54 entries
and several map cleanly onto claims -- "the array" onto `edge_lattice`,
"unicron" onto `unicron_in_earth`. Showing a live STANDING there needs the C
game to know what the player has found, and it has no save beyond a config
file; the archaeology gameplay lives in the Rust fork. Wiring it would have
meant inventing a dig for a game that does not have one, which is the
fake-caller pattern with a nicer view. The bible's OPEN question stands, with
a third option now recorded that I had not seen: annotate rather than gate,
so the codex stays the reliable reference it is.

817 passing, and the C suite at 65 of 67 with both failures being absent
assets.

THREE SEATING BUGS AND A SKIPPED FRAME, from a screenshot of a bot buried to
the hips and a vehicle with limbs hanging outside it.

  1. THE SEATING GUARD DID NOT MATCH ITS OWN CORRECTION. Both sites read
     `d < self.car.half_h` and then lifted by `ride_h`. Anything sunk
     between the two was judged fine and left there.
  2. `Car::reset` ZEROED `ride_h`. Its own comment says "anything describing
     the VEHICLE rather than its motion is silently lost on every kickoff,
     respawn and map load" -- and `ride_h` was in the carry list, set to 0.0.
  3. THE RESPAWN CLEARANCE used `half_h`, so a bot spawned partly inside the
     ground before anything else ran.
  4. THE FOLD NEVER RENDERED ITS LAST FRAME. `Xform::update` flips state to
     Idle on the SAME call that `t` lands on 1.0 or 0.0, and `advance_xform`
     gated on the state afterwards -- so the endpoint pose was never written.
     Limbs stop a frame short of their sockets. That is the parts-outside-
     the-vehicle bug, and it also froze the hitbox one frame early.

THEN I FABRICATED TEST DATA THAT FLATTERED THE FIX. My seating test used
half_h 0.9 against ride_h 1.5 -- a 60 cm band -- because it made the bug look
convincing. `model.rs` has the real measurement for `analog`: 0.3775 against
0.400. TWENTY-SEVEN TIMES smaller. A 2.25 cm bug dressed as a 60 cm one, and
it would have stopped me looking for the rest of the cause, because 2.25 cm
does not bury a bot to the hips. The test now carries the measured constants
and a second test that BOUNDS what the fix can explain.

WHICH LED TO THE REAL ONE. A smooth fold pins `cars.active` to the strip's
first entry -- the vehicle -- and rewrites that model's meshes into the robot
shape each frame. The geometry on screen is a robot; the `Model` whose
`half_h` and `ride_h` would be read is a car. While the fold runs it does not
matter, because the morph overwrites the dimensions every frame. It matters
the instant anything else re-derives them: a garage switch, a car hotkey, a
reload. For `analog` that is a `ride_h` of 0.239 against feet at 0.400 --
sixteen centimetres, an order of magnitude past what the guard bug could
account for. `dims_model` maps a displayed model to the model whose
dimensions describe the CURRENT POSE, and `switch_car` uses it.

AND THE TEST FOR IT DID NOT LAND THE FIRST TIME. The patch anchored on
`mod order_tests`, which had moved to stations.rs several sessions earlier,
so it applied cleanly to nothing and the count stayed at 826. Caught by
filtering for the test and seeing "0 passed". Checking the count before and
after an edit costs one grep.

828 passing.

KEYBINDS HAD NO CONFLICT CHECK OF ANY KIND. `slot()` hands out a `Vec<Bind>`
per action and nothing ever looked across actions, so a rebind could shadow
another silently and the only way to find out was to press the key and watch
two things happen.

`clashes()` compares AFTER `norm_key`, because that is what the dispatcher
compares -- without it 'E' and 'e' look like different keys in the report and
the same key at runtime, which would make the report clean and wrong. There
is a test that feeds it a deliberate clash, because a detector that reports
nothing is indistinguishable from a clean scheme.

BOTH SHIPPED SCHEMES ARE CLEAN, which is worth stating as a result: the
overlap being seen in play is not in the defaults. It is in a save's own
binds, or it is modal.

THE OTHER HALF IS THE ONE A TABLE CANNOT SHOW. Some keys never reach the
action table at all -- the hotbar takes 1-9 directly in main.rs, the deck
takes Escape, Tab, Enter and the arrows while a console is on stage, F11 is
fullscreen. An action bound to one of those fires TWICE, once as the action
and once as the shell's own behaviour, and no amount of reading the bindings
would reveal it. `RESERVED_NAMED` plus a test now covers that side. The
defaults are clean there too: only PageDown->Transform and Space->AutoTarget
sit on named keys, and neither is reserved.

`--keys` reads from the SAVE rather than the defaults, because that is where
a player's overlaps actually are.

AND IT FOUND THREE DEAD ACTIONS. `Car1`, `Car2` and `Car3` are unbound --
the defaults comment says 1/2/3 were "freed from the legacy car-picker for
the hotbar" -- but the actions still exist and `handle_action` still
dispatches them to `switch_car`. They can never fire. Worth noting next to
the dims fix: the car-hotkey route into `switch_car` is unreachable, so that
particular trigger for the robot-seated-as-a-vehicle sink comes from the
garage switch instead.

833 passing.

DRIVING ALL SIX CONSOLES ON A FRESH SAVE, which is not the state my own
width guard was testing.

THE MEDBAY HEADING OVERFLOWED AND THE TEST FOR IT PASSED. `chi::sheet` is
written for the wide `--gear` view: an unattuned body reads "nothing yet --
the body is a stranger to all seven", fifty characters before the caller's
prefix, against a console that gives forty-six TOTAL. My guard used
`board()`, whose chi is already populated, so it saw the short branch and
never the long one. Testing the state that passes is not testing -- the same
mistake as the fabricated seating numbers, two entries ago, in a different
costume.

Worth noting the overflow was not only the empty case: at three attuned
elements the populated branch runs over too, so the fixture was one element
short of catching it by luck rather than by design.

`chi::sheet_short(&chi, max)` caps hard -- strongest attunements first, then
a "+N" tail, because what gets dropped should be what matters least. The
guard now builds every station in BOTH a played and a fresh state.

AND THE HARD CAP WAS NOT HARD. Its own fallback branch -- the one that runs
when not even a single entry fits -- returned "2 attuned", nine characters,
under a cap of eight. The branch whose entire job was to guarantee the bound
was the one that broke it. Caught by testing caps from 8 to 40 across every
attunement count rather than at the one width the caller happens to use.

STILL OPEN, and it matters for the mobile targets: `Menu::view` is a
hardcoded 12 rows and nothing sets it from the panel's real capacity. The
medbay is 24 rows and the wardeck 20, so both scroll -- but whether twelve
rows FIT is a question about the screen, and on a PinePhone it very likely
does not. That one needs a display to answer.

837 passing.

I SAID `Menu::view` NEEDED A DISPLAY AND IT DID NOT. The console's row
geometry is two literals inside the draw call -- `top = 0.66`,
`row_h = 0.072` -- against `TW_BOTTOM = -0.78`, all in NORMALISED UI space.
How many rows the panel holds is therefore arithmetic, not observation, and
the answer is the same on a PinePhone as on a desktop: twenty.

`Menu::new` defaults `view` to 12 and nothing ever reconciled the two. So
every station scrolled eight rows early, and the wardeck -- twenty wardens
plus a heading -- scrolled when its roster would have very nearly fitted.
`hud::deck_rows()` names the calculation, `Deck::fitted` applies it at all
three build sites, and the whole roster now shows at once.

THE DRIVER WAS REPORTING ON A WINDOW THE GAME DOES NOT USE. `menusim` builds
menus with `build()` directly rather than through `Deck`, so it never saw
`fitted` and kept showing twelve rows of a twenty-row panel. A driver that
does not go through the same path as the shell is a driver that will agree
with itself and disagree with the game -- the same shape as `run_order`
living in the winit handler where nothing headless could reach it. Routed
through `deck::fitted`.

That is twice now that "this needs a screen" turned out to mean "this is
geometry I have not written down yet". The first was depth precision. The
test to apply is whether the quantity is defined in normalised units: if it
is, the display cannot change the answer.

840 passing.

PERFORMANCE: `gravity_dir` COST 26.8 us PER CALL, five times the SDF it sits
on, and every car and the ball hit it every frame. The reason is one line:
the near-surface branch evaluates `sdf(p)` to decide whether it needs a
normal, then calls `normal(p)`, which samples a TETRAHEDRON -- four more
evaluations, none of them at `p`. The centre sample is taken and thrown away.
Five SDFs at 5.2 us is 26.0 us against 26.8 measured; the model fits within
3%.

`normal_from(p, d)` is a forward difference that reuses the centre: three
evaluations instead of four. Accuracy is not assumed -- a test measures the
ANGLE against the tetrahedron over four hundred points on generated geometry,
because a forward difference is biased where the tetrahedron is symmetric,
and asserts the worst disagreement stays under twelve degrees inside the
blend's own range.

AND THE FIRST VERSION MADE THE GAME SLOWER WHILE THE MICROBENCHMARK IMPROVED.
`gravity_dir` fell 22% and total SDF evaluations per step ROSE, 19.3 to 25.3,
with two maps regressing. The old path called `normal(p)`, which populates a
one-entry cache; anything downstream asking for the normal at the same point
got it free. Saving one evaluation here cost four elsewhere. `normal_from`
now serves and populates the same cache, and evaluations per step land at
18.3 -- below the baseline rather than above it.

THEN I NEARLY REPORTED A STEP-TIME REGRESSION THAT WAS NOISE. One sample said
PERPLEXUS went 1.525 -> 1.605 ms. Five samples say 1.532 to 1.580, and
worldgen for the same map swung 7 -> 14 ms between runs on work I had not
touched. The honest result:

    gravity_dir   26838 -> ~21600 ns   (-20%, spread +/-1.1%)
    SDF per step   19.3 -> 18.3
    step time      unchanged within noise

Thirteen calls saving 5.2 us each is 0.07 ms against 1.55 ms. It is a real
win on the call and on the evaluation count, and it is below the noise floor
end to end. Worth keeping -- strictly fewer evaluations at a bounded accuracy
cost -- but not the headline the microbenchmark offered.

THE ACTUAL MOBILE RISK IS ELSEWHERE: worldgen is 400 ms on MONKEY RUN and
197 ms on PERPLEXUS. On an ARM target that is seconds, and it is a load-time
hitch rather than a frame-time one.

842 passing.

WORLDGEN: 400 ms -> 89 ms, AND I GUESSED THE CAUSE WRONG TWICE BEFORE
MEASURING IT.

First guess: the gravity index, because worldgen time tracked grav node count
across the five maps. Second: the O(n^2) mouth-finding pass, whose shape-count
ratio (864^2 vs 1263^2 predicts 2.14x, observed 2.08x) fitted beautifully. A
temporary phase probe said the mouths block is 2.7 ms and the gravity index
1.4 ms. Both stories were coherent and both were wrong; the arithmetic
agreeing with a hypothesis is not evidence that the hypothesis is where the
time goes.

It is the ROUTE CLEARANCE pass: 8.6 ms -> 183.7 ms, 175 of the 190. Routes x
segments x steps, each a full min-SDF over all 864 shapes, and its own comment
says why -- "brute-force distance (the spatial grid is not built yet)".

THE OBVIOUS FIX IS WRONG. The single call site only asks `raw(..) < want`, so
stopping as soon as something is nearer looks free. It is not: the combination
is not a min. Cuts SUBTRACT (`d.max(-dcut)`) and can carve a point clear again
after a near shape has been seen, so an early-out would open routes through
solid geometry.

Reject on REACH instead, which is sound -- a shape farther than its own
bounding radius cannot change the result -- and is the same test the runtime
grid already applies in `sdf()`, so the route check now agrees with the physics
SDF rather than being subtly stricter than it.

AND THE MARGIN MATTERED. At +2 m the worlds CHANGED: PERPLEXUS 864 shapes
became 856, MONKEY RUN 1263 became 1260, because `smin` reaches past a
bounding radius. At +10, matching the runtime grid's own +4 reach and +6 probe
budget, a five-map fingerprint (shapes, rings, routes, grav, summed positions)
is bit-identical. That fingerprint is the only reason I know this is an
optimisation and not a map change.

    PERPLEXUS   197ms -> 52ms
    MONKEY RUN  400ms -> 89ms

Which matters on ARM, where 400 ms is seconds and it is a load hitch rather
than a frame cost. Two tests pin it: the margin against the blend radius it
has to cover, and worldgen determinism for a seed, without which a future perf
change could alter the map silently.

844 passing.

THE TRIO AND THE VILLAIN HAVE POWERS NOW, and the mechanic came out of the
archaeology layer rather than being invented next to it.
`human_interface_unstable` was already a claim in `archive::CLAIMS`, already
corroborated by an autopsy file describing tissue damaged by working
CORRECTLY and by a rotation schedule cycling staff out sooner every revision.
That is a specification: a human power is a WITHDRAWAL AGAINST A BODY and the
balance never refills.

The Watch is deliberately the other thing — one ability, a form selector,
mastered before the story starts, costed but not degrading. Two power systems
that differ in kind rather than in numbers.

BURDEN IS PERMANENT, WHICH IS A REAL COST TO THE DESIGN and worth stating
rather than glossing. A per-mission meter is the easy version and it makes
the interface a stamina system: spend, sleep, spend again, and the fiction's
central claim stops being true. Permanent means leaning on a power in Film 3
makes you worse in Film 12, which can read as punishment for playing. The
mitigation is that the arithmetic is VISIBLE -- costs are quoted in months of
remaining service because the ledger is a calendar, and `--powers` prints
what each use leaves behind before it is spent. A cost you can see is a
decision; a cost you find afterwards is a trap.

A FAILING INTERFACE COSTS 1.5x, past 80% spent, which is the trap the
rotation ledger describes: the last of it goes fastest, which is what the
Directorate kept discovering and shortening the schedule over.

AND THE ARCHIVIST'S READOUT IS HIS MOTIVE. He starts at 86% -- failing, 1.3
years left, and one use of `unmake` takes him to 0.3. His argument for
destroying every fragment is a number rather than a monologue, and a test
asserts he is the most spent of the four AND that the dearest power in the
table is his.

851 passing.

BILLY'S FINALE WAS ANOTHER LIBRARY NOTHING CALLED. `converge` and `resolve`
were written and tested with no caller, which matters more here than for most
dead modules: the interesting thing about the scene is not that it succeeds,
it is WHAT IT REFUSES -- a true nova (the arriving allies are in the radius),
too small a draw, and a Watch not holding Photon Man (the held draw goes out
broad instead of aimed). Three refusals that each state a piece of the
fiction, and no way to read them short of the source.

AND DRIVING IT FOUND SOMETHING NOBODY DECIDED. The treatment leaves Teleport
and NewSun deliberately open; the mechanics do not. Photon Man costs almost
the entire resolve pool -- 5% left against a 16% shift -- so straight after
the strike Teleport REFUSES and NewSun does not. The leap only becomes
possible if the Morpher survives about two more seconds.

That is not a bug and it is better than a decision: the ending now depends on
whether the scene gives him a beat, and NewSun is what happens if it does
not. It falls out of two constants in `watch.rs` that were set for other
reasons. Pinned with a test, because a tweak to either would silently rewrite
which ending the story can have.

I would not have found it by reading. The driver ran both branches, one came
back refused, and the question was whether that was my harness or the game.

852 passing.

THE SHIP TABLE AND THE CAMERA HAD NEVER ASKED EACH OTHER ANYTHING.
`ship::HULLS` carries real lengths -- 28 m for a hawkdrop up to a THREE
KILOMETRE carrier, and the comment says "widesky really is a 3 km carrier and
it really should feel like one". `scale.rs` topped out at Two California
Plaza, 228 m, and I closed that entry saying a person and the tower was "the
depth range the renderer has to survive".

It is not. The carrier is thirteen times the tower, and framing it puts the
camera 3,069 m back against a far plane of 2,600. The ship would be entirely
behind it.

    ark        1600 m   camera 1799 m
    widesky    3000 m   camera 3069 m   PAST THE FAR PLANE

Neither table is wrong; they were written apart. STATED AS A TEST RATHER THAN
FIXED, because the fix is a decision with three defensible answers -- move the
far plane (and pay for it in depth precision, which `--terrain depth` can
price), frame carriers on a different curve, or accept that a 3 km ship is
never shown whole and only ever seen in pieces. That wants somebody who can
look at all three, and picking one quietly would be worse than leaving it
named.

The test asserts the carrier does NOT fit and that everything smaller does, so
the boundary is real rather than the whole class being broken, and a second
test joins every hull to a framing so the two tables cannot drift apart again.

854 passing.

THE JOINS ARE WHERE THE BUGS LIVE, so I went looking for them instead of
waiting to trip over the next one. Three times this session two correct
tables produced a wrong answer where they meet: findings against
destinations, the posed model against the displayed one, hulls against the
frustum. The common shape is a FREE STRING pointing into another table, with
nothing checking it lands.

`Boss.arena` is one. `chi::arena_element` matches on it and falls through to
`Element::Water`, so "frostlien" compiles, runs, and trains the wrong element
for a whole raid. The rule that makes the fallback a policy rather than a
hole: an arena is either explicitly mapped, or it is a real patrol
destination and the generalist default is deliberate. Anything else is a name
that exists nowhere. Two more tests cover the reverse — a mapped arena
nothing fights at, and whether the fallback is still reached by anything at
all, because a default nothing uses is dead code wearing a policy's clothes.

`Mission.champion` is worse. `Mission.dest` has been validated since it
landed; `champion` sits FOUR LINES AWAY in the same struct and never was --
and `campaign.rs` writes it straight into the final wave,
`*wave.last_mut().unwrap() = c`, with no lookup on the way. A typo does not
fail, it puts a troop id nothing knows about at the head of the mission's
climax. Now checked, along with the champion not being one of the player's
own classes.

AND MY FIRST VERSION REPORTED IT BADLY. The typo probe came back as an
`unwrap` panic from the second test rather than the named message from the
first, because that test did `find(..).unwrap()` on the same id. Whichever
test the harness happened to run first decided whether you got a diagnosis or
a backtrace. Changed to `let Some(..) else { continue }` so an unknown id is
reported by the test whose message explains it.

860 passing.

`Mission.site` AND `groundside::Place` ARE TWO NAMESPACES FOR THE SAME
GROUND. Missions name a locale in prose -- "the beach", "the foundry", "the
vault" -- and places are ids -- `the_beach`, `the_observatory`. Nothing
joined them, and nothing could.

That gap had already made something I built too coarse. `unexamined` gated
evidence by DESTINATION, so clearing any single Earth mission opened all
seventeen Earth findings at once, including an observatory and a museum the
mission never went near. My own design note for that gate says "you can only
examine evidence from somewhere you have actually been", and "been to Earth"
is not "been everywhere on Earth".

`Place::reached_by` is the join. Gating on it turns one unlock into a
progression:

    the beach                       5 findings
    + the vault                    13
    + the foundry, the trench      16

AND MY TEST COULD NOT HAVE CAUGHT IT. It asserted that clearing an Earth
mission opened "earth's evidence" and that nothing leaked from another world
-- both true of the coarse gate and of the fine one. A test that only checks
SOMETHING happened cannot see that too much happened. It now asserts the
count is strictly less than every Earth finding, and that each unlocked item
sits at a place the cleared mission actually reaches.

Two more guards on the join itself: every place is reachable by a real
mission (a place nothing reaches holds evidence nobody can ever examine), and
no single mission reaches every place, which is the coarse gate expressed as
a property rather than a story.

862 passing.

`Finding::source` IS THE HIGHEST-CONSEQUENCE FREE STRING IN THE PROJECT and
nothing constrained it. It is the INDEPENDENCE KEY: two findings sharing one
are two pages of one document and corroborate each other not at all. The
whole standing ladder rests on that string matching.

A typo does not fail. It SPLITS one body of evidence into two, and the map
then reports corroboration the player never earned -- a claim reaching
POSSIBLE or HIGH-CONFIDENCE off a single wreck read twice. The most important
invariant in the archaeology layer was held up by spelling.

`archive::SOURCES` closes the namespace, thirty-three entries, each saying
what the body of evidence actually IS -- which the ids do not, since
"scholarship" and "the_record" are both institutions and are not the same
institution.

THREE GUARDS, because a closed namespace is not enough on its own:

  * every key cited is declared -- catches a key that matches NOTHING;
  * every declared key is cited, with a real description -- catches a typo's
    other half, and a body of evidence that got deleted;
  * NO TWO KEYS ARE ONE CHARACTER APART -- catches the case the closed
    namespace cannot, a typo that accidentally lands on a second real entry.
    `luna_scar` against `luna_scan` compiles, passes the first two, and
    quietly turns one launch strip into two independent observations.

MY FIRST PROBE OF THE THIRD GUARD WAS WRONG. I RENAMED a source to test it,
which leaves one entry rather than two, so no pair exists and the guard
correctly said nothing. Adding one is the actual failure shape. Worth writing
down because "the guard did not fire" and "my probe did not create the
condition" look identical from the outside, and I nearly recorded the first.

865 passing.

I CALLED THE MYSTERY ANCHOR "A MAIN-MENU DIAL" AND NEVER PUT IT IN A MENU.

It has been in `--tune` since it landed, in `config.rs`, in the save file, in
the story bible, and described in three separate summaries as a setting on
the main menu. It was reachable only by hand-editing `skydribble.save`. So
were `story_era`, `case_cap` and both ladder thresholds -- every knob I added
this session.

Describing something as a setting does not put it in the settings. That is
the same failure as `run_order` living where nothing could call it and
`terrain.rs` having no caller, except this time I wrote the claim down
repeatedly and never checked it.

`tweak_rows` menu 8, STORY, linked under SETUP: the anchor, the campaign
selector, both ladder thresholds, and the case capacity, grouped under
headers that say what they do rather than what they are called.

THE GUARD IS THE INTERESTING PART. `--tune` and the settings tree are two
tables about the same thing and nothing joined them, which is the pattern
this session keeps finding. The test walks the menu tree from the root
following `Link` rows, drives every slider and toggle it finds to both ends,
and asserts each ledger knob MOVES. Not "appears in a list" -- moves, through
the same setter the UI calls.

    'story_anchor' is in the tuning ledger and no menu row can move it —
    it is a setting only in the sense that the save file has a line for it

Verified by deleting the link and watching it fire. A second test asserts no
settings menu is orphaned, because a page nothing links to is a page nobody
can open, and STORY was one missing line from being exactly that.

867 passing.

TOWARD THE RENDERER GAP: `raidrun.rs` IS THE CONTRACT AND NOBODY HAD RUN IT.
`skirmish.rs` calls it "the contract for the raid renderer" in its own doc.
It had ONE reference in the tree. It is handed unit positions and a clock and
hands back EVENTS and BEACONS, and it never moves anything -- a GROUND slam
returns the velocity to apply, a grapple returns the impulse, because who
applies them is the shell's business.

That makes the event stream a specification for renderer work that can exist
before there is a window, which is exactly what the gap needs. `--raidrun
<boss>` steps a real plan at 120 Hz and prints it.

THREE MISTAKES GETTING THERE, all mine, all instructive about the contract:

  * FIRST RUN: 72,000 ticks, two events. It read like a broken sim. The
    contract exposes THREE shell inputs and I was calling none of them --
    `credit_kill` (a quota clears when the combat layer says something died,
    never on its own), `champion_fell`, and `request_grapple`. A state
    machine fed nothing does nothing, correctly.
  * `advance()` RETURNS `over`, NOT "ok to continue". I read it as the
    latter, so the driver stopped after phase one and looked like a
    single-phase raid. The name reads either way at the call site, which is
    why the fix carries a comment rather than just a sign flip.
  * The units started far from the objectives and the quota cleared before
    they arrived, so a GROUND boss produced no SLAM at all. Pacing in the
    harness, not the sim -- but indistinguishable from a dead mechanic until
    you look.

Now: three phases, and the anti-air ceiling slams flyers at -22 m/s,
eighty-eight times in phase one.

    5.01s  ANCHOR    encounter 0 — champion down
   42.12s  SLAM      unit 0, vy -22.0
   85.01s  PHASE CLEARED

Every Slam and Grapple in that list is a velocity the shell applies and the
camera has to follow. That is the renderer's half of the contract, stated as
a list, which is as close to a spec as this can get before somebody has a
window open.

867 passing.

WHAT THE SHELL HAS TO BUDGET FOR, measured rather than assumed. The raid
contract is now runnable, so the questions a renderer actually asks of it can
be answered before anybody opens a window:

  * PEAK EVENTS PER TICK IS FOUR, across all nineteen wardens driven to
    completion. Not "under a bound I picked" -- I tightened the assertion
    until it failed to find the real figure, and pinned that, so a regression
    shows as a changed number rather than as a ceiling that quietly had room.
    Four is nothing; the shell can absorb it without a queue.
  * THE BEACON LIST IS STABLE WITHIN A PHASE. A renderer keys draw objects
    off the index, so a list that changes length mid-phase forces a rebuild
    and a list that reorders draws the wrong marker in the wrong place.
    Asserted for all nineteen.
  * ALL NINETEEN CAN BE FINISHED. A renderer built against a contract that
    contains one uncompletable encounter finds out late.

The drive feeds all three shell inputs and applies the velocities the sim
returns, so it exercises the loop the way the shell will rather than the way
a unit test would.

869 passing.

THE CAMPAIGN'S HALF OF THE SAME CONTRACT. `MissionRun::step` has existed
since the module landed and EVERYTHING called `resolve`, which loops it at
0.1 s and throws the events away. So the per-tick stream -- the thing a
renderer actually consumes -- had never been looked at, only summed. Same
blind spot as `raidrun`, reached from the opposite direction: there the
contract had no caller, here it had one caller that discarded the interesting
part.

    campaign  peak 3 events per tick, all fifteen missions terminate
    raid      peak 4 events per tick, all nineteen wardens finishable

Both figures found by tightening the assertion until it failed, so a
regression reads as a changed number rather than as headroom nobody was
using.

A second test walks the wave stream, because a renderer keying an intro
animation off `WaveStart` needs it in order with no gaps, and needs never to
see a `WaveCleared` for a wave that never started.

AND IT CAUGHT MY ASSUMPTION RATHER THAN A BUG. I asserted zero-based wave
indices; both emit sites push `wave_i + 1` on purpose, so the event reads
"wave 1 of 3" to a player. The fix is not shifting an index past the failure
-- it is asserting the convention, `started.first() == Some(&1)`, so the next
person reading the enum does not have to go and check either.

872 passing.

THE FOLD AUDIT WAS REPORTING 3.5 MILLION FAULTS AND ALL BUT SIX PERCENT OF
THEM WERE THE MODELS BEING MODELS.

`--fold sweep` said 251 of 254 chassis were broken. Checking one showed why:
every fault was at **t=0.00**, the untransformed vehicle pose, before
anything had moved. A tank's road wheels sit inside its hull; that is how a
tank is built. The check measured prim overlap against ZERO.

The module's own doc anticipated this -- `OVERLAP_TOL` exists so "prims that
SHOULD touch (a shoulder in its socket, a panel in its recess)" are not
flagged -- but a fixed tolerance cannot cover a wheel fully inside a hull,
where the relative depth is 1.0.

THE BASELINE IS THE AUTHORED POSES, not zero. The question is not "do these
overlap" but "do they overlap MORE than they do at rest in either end pose",
because a part passing THROUGH another during transit is the fault and
construction is not. Same reasoning as `redistribute_length_to_height`, which
already caps mid-fold overshoot against the endpoints rather than against
nothing.

    3,538,315 faults  ->  213,172
    every fault at t=0.00  ->  faults start at t=0.04, mid-fold

AND THE TEST FOR IT WAS TESTING THE WRONG THING. `a_prim_passing_through_
another_is_caught` built two prims that "start apart and END on top of each
other" -- its own doc comment. That is not a sweep-through, it is a broken
end pose, and under the new rule it is credited as construction. A SWAP is
the real case: apart at both ends, crossing at t=0.5, so every bit of the
overlap is transit. Fixed the fixture rather than the rule, and added the
inverse -- a prim that never moves relative to another must never be
reported.

STILL 251 CHASSIS WITH FAULTS, and now that is a real number rather than
noise: a straight vertex lerp from vehicle pose to bot pose moves parts along
chords, so they cross. That is the actual state of the transformation
animation engine, and the sweep now ranks which chassis fold worst instead of
burying it under three and a half million non-events.

873 passing.

A TOP-TEN CANNOT TELL YOU WHETHER TEN THINGS ARE BROKEN OR EVERYTHING IS, and
those want opposite responses: fix ten models, or fix the fold. `--fold
sweep` printed only the ten worst, so the shape of the problem was invisible
even after the fault count became meaningful.

    spread: worst 2497, p90 1382, median 795, p10 392, best 58
    NOT ONE of the 251 folds cleanly, and the best still has 58 faults.

MY FIRST VERSION OF THE SUMMARY GOT IT BACKWARDS TWICE. The list is sorted
DESCENDING, so `counts[len/10]` is the ninetieth percentile and I labelled it
p10 — and then drew the conclusion FROM the labels, which is how correct data
produces a confident wrong answer. It also judged "fold or models?" on a
ratio between worst and median, which said "a long tail" while every single
chassis was faulty. The right question is not the spread, it is whether ANY
chassis folds cleanly. None does.

SO IT IS THE FOLD. `Morph::sample` lerps vertices, so a part travels the
CHORD between its two poses — the straight line — and any part whose start
and end sit on opposite sides of another goes through it. Nothing in the
pipeline routes around anything. Two prims demonstrate it: a part swinging
across a stationary one faults in transit and never at an endpoint, and the
same swing with the obstacle moved off the chord is clean.

That is the real state of the transformation animation engine, and it is a
design question rather than a bug list — an arc, a per-part detour, or
ordering the fold so parts move out of each other's way. Fixing the worst ten
chassis would change nothing about the other 241.

874 passing.

PORTING BEGINS: `warp_wave` AND `spearcon` OUT OF THE C TREE. Both came from
the film scripts rather than from the code, and both are one idea -- something
will happen at a place, at a time, and nobody can call it off. So the clock is
shared (`hazard::Fuse`) and the two mechanics are thin over it.

THE WARP WAVE is the better of the two and the reason to port carefully. The
Ice-Clone leap is silent and undetectable; the only evidence is a gravity wave
arriving SECONDS LATER at the landing point. The jumper knows it is coming and
can warn allies, cannot stop or shape it, and IT IS INDIFFERENT TO WHO IS
STANDING WHERE IT LANDS. A consequence you cannot aim, arriving after the
fact, is an unusual thing to hand a player and it is exactly the
Watch-bearer's register: the power is real and it is not obedient. A test
pins that the landing point is fixed at leap time and nothing afterwards
moves it.

A DEFECT DID NOT SURVIVE THE PORT. `ex_spearcon_update` writes detonations
into a caller-supplied buffer and returns how many went off -- incrementing
that count whether or not it had room, so a caller iterating `0..n` over a
shorter array reads past the end. Returning a `Vec` makes the failure
unrepresentable rather than fixing it, which is the right trade in a language
that can, and the C behaviour is recorded as a test asserting all sixteen are
accounted for.

Also: a second leap RESETS the pending wave rather than stacking one, which
the C original gets right for a reason worth writing down -- there is only
one of you, so there is only one arrival pending.

883 passing.

THE CODEX CAME ACROSS, all fifty-four entries, EXTRACTED RATHER THAN
TRANSCRIBED -- a parser over `lore.c`'s initialiser emitting Rust, because
fifty-four entries of somebody's actual writing is exactly the material a
hand-copy quietly corrupts. A test asserts the count and that no line is long
enough to suggest a mangled parse, since the line breaks are the writing's own
rhythm rather than word wrap.

AND IT ANSWERED AN OPEN QUESTION WITH AN OPTION I HAD NOT SEEN. The bible
asked whether the codex should become something you excavate -- dimming until
earned -- or stay a reliable reference. Both readings assumed a choice. It
annotates: five entries name a claim, and reading one prints what the EVIDENCE
currently says beside what the ENTRY says, with the entry readable either way.
`--codex unicron` shows the settled account and then "[POSSIBLE SHARED ORIGIN]
2 for / 0 against" underneath, which moves as the dig does.

Five of fifty-four, with a test refusing more than a quarter: an annotation
that fires on everything is decoration, and the case worth building for is
the one entry the evidence ends up DISPUTING.

887 passing.

THE BRAIN SEAM IS ACROSS — `combat_json` and `commander_ai` out of the C tree
into `brain.rs`, and it was the most valuable thing in there because it is not
a feature. Its author's own framing: a commander FSM emits THE SAME COMMAND an
LLM would, so the two are interchangeable behind one interface and the
deterministic layer is a strict subset of what the model layer can express.
`--brain wire` prints both and the JSON is byte-identical.

THE DOCTRINE TABLE WAS ALREADY HERE. `content::COMMANDERS` carries the same
five archetypes and the same four dials -- aggression, caution, encircle,
regroup -- that the C loaded from `commanders.json`. This tree had the data
and not the FSM, which is why the port is a hundred lines and not a thousand.

THREE THINGS ARE LOAD-BEARING AND EASY TO MISS:

  * THE SIGNALS. A snapshot carries aggression, recent damage, pressure and
    kills alongside the positions. A brain that only sees positions plays
    chess; one that sees HOW YOU ARE FIGHTING can react to you, and that is
    the entire reason the loop is worth closing.

  * THE TICK ECHO. The C describes it as ordering. For a model provider it is
    the whole safety story: inference takes hundreds of milliseconds, the
    frame cannot wait, so replies arrive stale as a matter of course, and
    orders answering a battle two seconds gone are worse than none.

  * TOLERANT PARSING IS A REQUIREMENT. A model asked for JSON returns JSON
    wrapped in prose, or a code fence, or a `<thinking>` block, roughly
    whenever it feels like it. A strict parser is a provider that fails
    intermittently for reasons the player experiences as the enemy going
    stupid. Tested against four wrappers and a made-up verb.

EVERY FAILURE FALLS BACK RATHER THAN STALLING -- declined, unparseable, stale,
or empty, the FSM answers instead, and a test walks all five paths asserting
the squad never stops taking orders. And the model is trusted for verbs and
never for the tick, or one wrong echo poisons freshness for every later reply.

LOGSCOPE IS THE INTENDED PROVIDER and the dependency is one method:
`trait Completion { fn complete(&mut self, prompt: &str) -> Option<String> }`.
Deliberately that small so this crate takes no dependency on an inference
stack -- `apps/logscope` is a Rust LLM workspace with a `gpu-gemm`
wgpu->Vulkan engine and a local GGUF path, so it can serve this on the same
devices the game targets, but an Ollama call or a fixture satisfies it
identically and the game cannot tell.

899 passing.

THE HIVE IS ACROSS, and it is the enemy that does not fight like an army.
`corruption.rs` has always modelled what corruption IS and never how a
corrupted swarm DECIDES, which is why the infected have been fighting like
soldiers with different stats.

CRAWL, DANCE, FEED — borrowed from bees rather than from armies. Below
critical mass the infected stay dispersed and creep, so a lone scout does not
give the swarm away. One that gets close to prey emits a waggle dance that
RECRUITS whatever is near it — stigmergic, not a broadcast: nothing commands
anything, and the swarm self-organises because each individual reacts to what
is beside it. Enough recruited and it tips.

The consequence worth having is that KILLING THE SCOUT WORKS. There is no
commander to decapitate, but there is a dance to interrupt, and if the prey
gets back outside the radius the whole thing subsides to crawling rather than
chasing. A player who works that out has learned something the enemy's
statistics never told them.

A `match` COST ME THE PACING AND A TEST CAUGHT IT. The C runs its three state
checks as sequential `if`s, not `else if`, so the tick a scout arrives the
hive enters DANCE and falls straight through to count recruits — tipping to
FEED in that same tick if the pack is already gathered. A `match` looked
equivalent and is not: it spends a tick per transition, so a swarm the player
walked into took a second to notice them. Found by
`recruitment_is_by_proximity_not_by_headcount` reading zero on both sides of
its comparison, which is the kind of failure that is only legible because the
test compared two things rather than asserting one.

Fixing it then broke the arc test, correctly: with six clumped infected and a
ravener's feed mass of two, crawl->feed really is one tick and the dance is
never observable. The test had encoded the buggy pacing as the specification.
Replaced with two that check what is actually true — a gathered pack tips
instantly, and a SPREAD one dances and keeps dancing, waiting for numbers it
does not have. That waiting is the mechanic.

`--brain hive` shows all four strains against a closing player, and the
strains genuinely differ: the stalker never gathers four within eleven metres
and dances to the end, the lure tips at thirty.

909 passing.

CLEARING THREE DEAD ITEMS FOUND A REAL BUG, which is the argument for
treating an unused-code warning as a question rather than a chore.

`Fsm.squad` came across from the C's `squad_id` and was read by nothing. The
lazy fix is to delete it. But asking what it was FOR exposed that `choose`
averaged strength over EVERY live unit on the field — so a commander whose
own squad was being wiped out kept pressing on the strength of somebody
else's. `mine()` filters to the squad, a negative id still means the whole
field for the single-commander case, and a test builds two squads (one at 8%
health, one untouched) and asserts their commanders disagree. Reverting the
filter makes it fail.

The other two were smaller and got the same treatment rather than deletion.
`ZONE_MID` was defined and never emitted, which meant a PRESS committed only
its front rank — a probe with extra steps; the middle now follows the front
in. `Strain::parse` had no caller, so `--brain hive <strain>` takes one.

910 passing, and no dead code in either brain module.

THE TUNING LOOP IS ACROSS, AND THE ARTIFACT IS NOT. `tools/tune_fsm.py`
evolves transition matrices for a Markov behaviour FSM -- eight per-enemy
movement behaviours, a matrix per context and vehicle archetype. THIS TREE HAS
NO SUCH LAYER, so porting it meant building a system with no consumer and then
a tuner for it. What transfers is the loop: a headless sim, a fitness
function, a mutation search. And that has an immediate consumer, because
`tuning.rs` says out loud that five numbers here have never been verified and
`brain.rs` carries a comment admitting its posture gates are guesses.

TWO SIM BUGS, BOTH CAUGHT BY TESTS COMPARING TWO THINGS rather than asserting
one. The player kited at 9 m/s against a squad that moves at 7, so nothing was
ever caught: encircle read 0.048 for a surrounder and 0.049 for an aggressor,
and the search could not tell the doctrines apart because THE FIGHT NEVER
HAPPENED. A fitness function measuring a fight that does not happen scores
noise very precisely. Bounded the arena and slowed the player below the squad
-- fights happen in places, and a player who can retreat infinitely is not
being encircled by anybody.

Then the surrounder still scored LOWER on encirclement than the aggressor,
because my FLOCK_SPREAD pushed every unit sideways -- which ROTATES a
formation without spreading it. Units starting on a ring already cover it, so
orbiting only took them further out. Each unit now gets its own slot on the
circle, which is what "spread out around a point" meant.

And every context returned identical gates, which looked like the weights
being decoration. They were not: the search used ONE FIXED SEED, so every
context explored the identical candidate sequence. Seeded from the context
name -- still deterministic, and now they look somewhere different.

THE ANSWER IS A NEGATIVE RESULT AND THAT IS WORTH HAVING:

    context       hurt    mind    scat provoke     score vs shipped
    skirmish      0.34    0.29    0.38    0.51     0.878     +3.3%
    swarm         0.52    0.39    0.30    0.62     0.673     +2.4%
    hunt          0.56    0.52    0.40    0.60     0.831     +1.5%
    hold          0.49    0.52    0.42    0.59     0.877     +1.4%

By the criterion the tool prints for itself -- a couple of percent is noise --
the shipped gates are fine. One detail is worth an eye though: every context
wants `mind` at or below 0.40 and skirmish wants 0.29, which would put
`surrounder` (caution 0.30) above the bar and make it state-sensitive. That is
exactly the insensitivity I flagged when the FSM landed, arrived at
independently by a search that did not know I had complained about it.

NOTHING IS WRITTEN BACK. The fitness weights are themselves guesses -- better
argued than the numbers they judge, and still guesses -- and an optimiser that
edits the source is an efficient way to bake a bad objective function into a
game.

916 passing.

GAME.C IS A DESIGN CORPUS, NOT A CODEBASE TO PORT. The salvage survey ended on
a caveat -- 12,509 lines, perhaps 300 read, systems might be hiding inside it
rather than in named modules. There are FORTY-TWO of them, and reading them
changed what taking them means.

The file opens with two hundred lines of design commentary: every system
named, argued for, mostly sourced to the screenplay. Underneath, the
implementations are inline against the C renderer. `AIDEN'S SKATEBOARD` --
a human-scale vehicle mode, which is exactly what the human campaign needs --
is two globals and a note that transform "hops on/off, near-double pace,
board rendered underfoot". There is almost nothing to translate.

So `docs/INHERITED_DESIGN.md` harvests all forty-two with their own prose.
The design survives deletion in a way renderer-coupled C does not.

AND SOME OF THEM WERE NEVER BUILT. `PALE VECTOR` -- the relic-keeper who
whispers if you come near and WARPS OUT if you press him, who maintains the
relics and remembers -- has a paragraph of design and close to no code. I
tried to classify implemented-vs-not by grepping identifiers, and the
heuristic was too crude to keep: "THE SUN ITSELF" matched on the word "the".
Nothing is labelled, because a confident wrong label is worse than none.

The ones with the strongest claim here: the skateboard, Pale Vector (whose
register is the archaeology layer's exactly), DISGUISE PROTOCOL (a prop hunt
falling out of the transform verb the game already has), AMALGAM OF THE GATE
(a gestalt fused from every frame that ever failed -- and `combine.rs` now
knows how to build a gestalt), and LOW-G LOCOMOTION.

PALE VECTOR IS ACROSS, and the first thing to say is that I was wrong about
him. The `game.c` survey listed him as documented-but-unbuilt on the strength
of a grep that looked for the wrong identifier; the file has forty-one
references to `g_pvec_*` and a complete update loop. Noted in
INHERITED_DESIGN.md as a caution about the rest of that list.

THE SHAPE IS THE CHARACTER. Three distances and nothing else: outside thirty
metres he is scenery, inside it he talks on a cooldown, inside twelve he
leaves for forty seconds. That is a complete design, because the only way to
get everything he knows is to KEEP YOUR DISTANCE FOR A LONG TIME -- and the
instinct that works on everything else in this game, close and press and
take, is precisely the one that costs you. Holding at 20 m gets all four
lines in 41 s. Walking in gets one.

WHAT THE C HAD NOWHERE TO PUT: his four lines are archaeology claims spoken
aloud -- "the lattice is stirring", "the array saw it first, no one wanted to
look", "i maintain what was meant to stay buried". That engine had no
provenance layer, so they were flavour text. Here they are TESTIMONY, and
`provenance.rs` already had the right bearing for it: `Mentions`, from a
single source. A whisper puts a claim on the board at UNKNOWN and moves it no
further.

That is right twice over. He is one voice, so he could never be corroboration
alone; and he is a rumour rather than an artifact, so the map should record
that somebody SAID this and not that it is so. A test asserts every claim he
touches lands at exactly UNKNOWN and that his support count stays zero, and
another asserts he still MATTERS -- without him those nodes are not merely
unproven, they are invisible.

Two flaws found by running it. The walk-in demo started at 25 m, so he
appeared already inside press range and withdrew before saying anything --
a demonstration of nothing. And two of his lines concern the edge lattice, so
his testimony listed it twice: harmless to the assay, which counts distinct
sources, and it would have READ as corroboration. One witness saying a thing
twice is one witness.

924 passing.

THE AMALGAM AND THE DISGUISE ARE ONE SYSTEM, and porting them together is the
only way either makes sense: a boulder is not a mechanic, it is an answer to
something, and the amalgam is the something. Forge four relics while it sweeps
the hollow -- forging makes you LOUD -- and the disguise buys eight seconds
against a thirty-second recharge, because the gate notices patterns.

It composes almost entirely out of things built here for other reasons.
Detection is `perception::rate`. The disguise is cover that expires. The
gestalt is `combine.rs`. So the port is mostly the LOOP, and the loop is one
trade: THE ONLY WAY TO MAKE PROGRESS IS THE THING THAT GETS YOU FOUND.

FOUR MODELLING BUGS, EACH FOUND BY THE ENCOUNTER BEING UNPLAYABLE:

  * IT DRIFTED AWAY WHEN UNSURE, which is not hunting -- and with a starting
    range of 40 m against a watcher that sees 35, the encounter never began.
    That is `perception`'s own conclusion biting its first consumer: distance
    is the only answer to a hunter, so a hunter that keeps its distance is not
    a threat. It sweeps now.
  * THE SWEEP WAS SIX SECONDS long, the whole hollow crossed and recrossed in
    less time than one forge. A patrol has to be longer than the thing it
    interrupts.
  * ATTENTION WAS A RATCHET. It could only fall outside the amalgam's senses,
    and the sweep never leaves them, so suspicion rose monotonically and every
    strategy eventually lost. There was no way to shed it -- which means there
    was no stealth, only a countdown wearing one.
  * THERE WAS NO QUIET ACTION. Moving at sixteen metres still nets suspicion,
    so a player caught out by the sweep could only spend the disguise or be
    found. Crouching fixed it, and the source implied it all along: the
    disguise is "crouch + transform", so crouching was already a state.

AND I TUNED IT IN CIRCLES, inverting which strategy won four times over. That
is the signal that the objective was mine rather than the game's, so the
balance number is registered as a guess and the TEST WAS WEAKENED to match
what I can actually defend: somebody can win, and reading the sweep is never
worse than ignoring it. Pinning a difficulty I cannot playtest would encode my
guess as a requirement.

The safe forging distance is DERIVED from the constants rather than typed,
because a magic threshold would stop meaning anything the moment the balance
number moved -- and that number is admitted to be one that will.

    forge blindly, hide early     Caught   — 2 relics
    forge blindly, hold it back   Caught   — 2 relics
    read the sweep, crouch near   Banished — 4 relics, never found, 78s

935 passing.

THE GRIP METER IS ACROSS -- `game.c`'s cling system, four multiplied terms and
a clock. Vertical faces drain slow, UNDERSIDES drain fast, heavier builds
drain faster, crawler alt-modes grip far better, and ice is slick.

WHAT MADE IT WORTH PORTING rather than reinventing is that every term already
had a home here. Orientation is the surface you latched to; build is
`chassis_traits`' bulk; alt-mode is the chassis category; and the surface is
`terrain::Material` -- the module that already knows ice from basalt because
`mapsim` needed it to decide where dunes form.

That last one is the change I made. The C reads a per-map `g_surface_grip` and
computes `drain *= (2 - grip)`. Here slickness is DERIVED from the material,
because a second per-map opinion about what a surface is made of is a chance
for the two to disagree -- and the terrain module is already the authority on
that question for dunes and talus.

THE ASYMMETRY IS THE MECHANIC, and the table says it in one glance:

                   metal    basalt  regolith       ice  soft ice
    wall, light     32.3      30.8      23.9      19.6      18.0
    wall, crawler   67.3      64.1      49.9      40.8      37.4
    under, light     7.3       6.9       5.4       4.4       4.0
    under, heavy     4.7       4.4       3.5       2.8       2.6

Thirty-one seconds on a face and seven beneath one. A wall is somewhere you
can stop and think; a ceiling is a move you have already committed to. A test
pins both halves of that -- a wall must give more than fifteen seconds and an
overhang fewer -- so retuning cannot quietly collapse the distinction into a
single kind of surface.

AND THE TABLE IS NOT THE MECHANIC. The driver first printed hang times and
stopped, which left the whole stateful half -- recover, fall, lockout -- dead
outside its own tests. Running an actual traverse fixed that and said the
thing the table cannot:

    9 m of ice overhang, 1.4 m/s
      heavy    GRIP FAILED at 3.9 m ... 14 identical falls; it reaches 4.0 m
      crawler  across in 6.4 s, no falls

THE METER IS NOT A RESOURCE YOU SPEND, IT IS A DISTANCE YOU CAN REACH. A span
longer than your hang time is not hard, it is SHUT, and the answer is a
different frame rather than a better attempt. Fourteen identical failures at
the same 3.4 m is the game saying so.

A `release()` came across and nothing called it: letting go of a wall is the
same event as arriving on the floor, and a separate method invites the two to
disagree about the regrab lockout. Deleted, with a test pinning that a
voluntary release does not lock you out the way a fall does.

944 passing.

STEALTH IS ACROSS, and the design under the pitch is better than the pitch.
`game.c` sells it as Halo-style assassinations. What it actually has is TWO
SENSING CHANNELS WITH OPPOSITE COUNTERS.

SIGHT is range, a facing cone, and line of sight — beaten by geometry. Get
behind something.

SPARK RESONANCE is the Corrupted and the apex frames feeling a spark WORKING:
firing, sprinting, abilities. No cone, no line of sight, THROUGH WALLS, at a
range that grows with how loud you are — beaten by stopping.

So the instinct that saves you from a sentry, duck behind a wall and keep
shooting, is the one that lights you up for a hunter. `--stealth` prints the
trap in two lines: behind a wall, still firing, the sentry senses `No` and the
corrupted senses `Resonance` from 112 m. A player who has learned one enemy
has learned the wrong lesson about the other.

`perception.rs` has always modelled the first channel and never the second.

I ALSO CLAIMED THE TWO MODULES COMPOSED AND THEY DID NOT. `sight_rate`
delegated to `perception::rate` and nothing called it: `sense` treated being
inside the cone as INSTANTLY SEEN, so the delegation was a decoration and the
warning said so. Sight is a rate now — inside the cone is permission to start
noticing, not the noticing — and a test pins that being seen takes time,
that distance costs four times more of it, and that sneaking buys some.

Likewise `Spark::act` and `decay` were dead because the driver printed static
loudness values. Running the loop instead shows the thing the table cannot:
one burst at thirty metres goes unfelt in 0.8s, and the hunter keeps coming
for another 3.5 — so stopping is necessary and is not sufficient.

955 passing, zero warnings in the module.

AIDEN'S SKATEBOARD IS ACROSS, and the design line is three words while the
design is in two numbers elsewhere in `game.c`: a human moves at 0.45 of a
bot's pace, and on the board at 0.85.

THE SCALE GAP IS A MOBILITY GAP. `scale.rs` established that a bot is five
people tall and a tower is a hundred and thirty; those are rendering problems.
This is the gameplay one -- a world built at Transformer scale is not merely
large for a person, it is a commute:

                                            bot    foot   board
    a warehouse floor                        4s     10s      5s
    the hollow                              14s     32s     17s
    a patrol zone                           64s    143s     76s

The board is a FIX AND NOT A POWER-UP: 85% of a bot is still slower than a
bot. It makes the map the size it already was for everyone else, and a test
pins that it stays under 1.0 -- if it reached parity the answer to human scale
would be "stop being human", and the campaign's whole register goes with it.

THE VERB IS THE SAME KEY AT BOTH SIZES, which is why the C hangs the board on
TRANSFORM: at human scale folding is disabled and the board takes its place,
so the verb means "become the thing that can cross this" either way. That is a
better unification than a separate mount button, and I would not have found it
from the design comment alone.

AND A DEFENSIVE GUARD SURVIVES THE PORT. `game.c` snaps the avatar back to
robot whenever scale is toggled, with a comment explaining that toggling
mid-transform would strand you as a human-scale VEHICLE -- posed flat or
upside down, with mode changes locked. It is unconditional so it doubles as a
recovery for any stuck transform, and both directions are tested.

Height and camera come from `scale::LADDER` rather than being restated here,
so this module and the size ladder cannot disagree about how tall a person is.

962 passing, zero dead code in the module.

CRAWLER AMBUSH IS ACROSS, built on `cling.rs` -- the climb is a crawler's grip
used offensively, and `usable()` asks that module whether a face will hold for
the length of a climb rather than deciding separately. Two systems that
disagreed about who can hold a wall would be worse than either.

THREE THINGS MAKE IT HORROR RATHER THAN A GIMMICK, and all three are in the C:

  * A CAP OF TWO. Not a swarm from above. Two is a threat you can look for and
    still fail to find; a ceiling of them is a crowd, and a crowd is not
    frightening. Twenty crawlers and a single spire still peaks at two, and a
    test walks thirty seconds asserting it never exceeds the cap.
  * THREE WARNINGS, THE LAST ONE LATE. "Something is CRAWLING the structure",
    then the eyes catch the dark for 0.45s, then the dive -- which is 0.85s
    long, so the final warning is shorter than the thing it warns about.
  * THE POUNCE LEADS NOTHING. Flight time is fixed and the arc is solved once,
    at launch, against where you were standing then. Standing still is a hit;
    running puts you 5.1 m from where it lands. A crawler that tracked you
    would be unfair; one that predicts badly is a puzzle.

TWO BUGS OF MINE. `pounce_landing` returned the PERCH's height as the landing
height, so a dive off an eighteen metre spire reported landing eighteen metres
in the air -- caught by a test comparing it against a target on the ground,
and the neighbouring ballistics test passed throughout because it integrated
the arc properly. And `Event::Nothing` was a variant `step` can never return,
since it hands back a Vec of what actually happened; deleted rather than given
match arms, because a state that cannot be constructed beats one every caller
has to remember to ignore.

The glint was asserted by a test and invisible in the driver, which made the
printed trace look like the encounter has two warnings rather than three.

971 passing, zero dead code in the module.

THE RIP IS ACROSS -- six characters expressed entirely in the velocities of
somebody else's parts, which is a lovely way to say something about a player
without a line of dialogue. Merciful barely moves them (speed 2.6); thrash
throws everything (22.0); surgical lifts without throwing; precision makes a
tight ring and the scout BLINKS to the flank rather than charging, because
arriving rather than travelling is that whole archetype.

AND THERE IS A SEVENTH THE DESIGN COMMENT DOES NOT LIST. Buried next to its
condition: a big frame that gets its hands on a small enemy does not scatter
it, it takes it in both hands and TEARS IT IN TWO along your own shoulder
line. Prime will not -- mercy holds -- and the beast will not, because it
already has its own idea. Everyone else will.

What makes it read is COHERENCE. Every other style is a scatter; this one is
the absence of one, two halves each moving as a single piece. Measured at
1.00 against 0.13 for thrash.

MEASURING THAT TOOK TWO GOES. Coherence over the whole body in three
dimensions scored `Merciful` as high as a jazz rip -- its parts barely move
sideways, so they all point straight up, and "all going up" reads as "all
going the same way". Measured horizontally over the whole body a jazz rip
scores ZERO instead, because its halves cancel. The metric that actually
distinguishes it is horizontal coherence WITHIN ONE HALF, which is just the
fiction stated arithmetically: each half moves as one piece.

I also gave `Victim` an `Apex` variant out of habit from `stealth.rs`, where
an apex frame is a real case because it turns an assassination into a wound.
Here it changes nothing -- the override asks only whether the victim is small
enough to pick up, and an apex answers no exactly as a standard does. A
variant that only ever takes the other branch is a distinction the code does
not make. Deleted.

981 passing, zero dead code in the module.

THE BOTS ARE A CENSUS, NOT 259 CHARACTERS. `tools/meta/bots.json` documents
its own weakness with unusual honesty -- 89 distinct hooks across 259 bots,
fifty-six grimorphs sharing one line, "placement metadata wearing a costume,
not character" -- and the obvious fix is to write 178 more hooks. That is the
wrong fix. A world with 259 named personalities has no crowd in it, and the
crowd is what makes the named ones legible.

So `census.rs` reads the DISTRIBUTION, and the distribution turns out to say
things nobody typed, because the placement rules were written against the
film material's geography:

  * ALL FIFTY-SIX UNRECORDED SHAPES ARE ON CYBERTRON AND EVERY ONE IS
    CORRUPTED. Their shared hook is "a shape the coalition has no record of",
    which is a provenance statement rather than a character note -- and
    `substrate_sample` already says from the other side that Cybertron's
    architecture predates its own record of inventing it.
  * EVERYTHING ON EARTH COULD BE PARKED. All 46 are Coalition, all cars,
    trucks, racers and helicopters. No grimorph, no spaceship. Concealment as
    a demographic fact.
  * TEN OF ELEVEN WORLDS ARE MONOCULTURES. Only the outer rim is contested,
    which is why the boundary is where the story keeps happening.

It reaches the map as `Supports` from ONE source -- a headcount is a real
observation, unlike Pale Vector's testimony which only ever `Mentions`, but
it is still one body of evidence. Everything it touches lands at UNKNOWN and
waits for a dig, and a test asserts exactly that. Counting bots is
suggestive; it is not evidence twice over.

AND I ALMOST DESTROYED A MODULE DOING IT. I wrote the new file to
`src/roster.rs` without checking, and there was already a 484-line
`roster.rs` -- the chassis loader that decides which models fit in GPU
memory. `git checkout` restored it and the census moved to `census.rs`. The
compiler caught it immediately (`the name roster is defined multiple times`)
because my own sed had added a duplicate `mod`, but had the name been free it
would have compiled and I would have silently deleted a working system.
Checking whether a filename is taken costs nothing.

AND THE DRIVER MADE A POINT I HAD NOT PLANNED. Listing the frames that stand
out, per world, prints:

    earth        car_bruiser_aka_special_ops_racer
    cybertron    none -- every frame there is one of a crowd
    outer_rim    raidboss_judgementor_aka_large_walker_tank

CYBERTRON HAS NO INDIVIDUALS. Fifty-six frames and not one of them is
anybody. That is the same fact as "a shape the coalition has no record of",
said a second way, and it is what the corrupted ARE -- a place where the
distinction between one frame and another stopped mattering. I did not write
that; the roster did, and the census only had to read it out.

Also: `judgementor` on the outer rim is the warden whose docket
(`undelivered_verdict`) is the raid finding that carries Layer 5. The census
and the archaeology layer already share their best characters.

989 passing, zero dead code in the module.

KUIPER AS SPACE: THREE COUPLED CHANGES, NOT ONE. The asteroid field is still
skydribble's demo map -- eighteen worlds of radius 7 to 38 with real gravity
wells and a black hole every five minutes, and it is the best map in the set.
"Make the planets bigger" turns out to touch three systems, and `space.rs` is
the arithmetic for all of them before anything touches worldgen. Measure, then
wire, which is the order that worked for the near plane.

LOD HAS TO BE ANGULAR. A distance rule is right when everything is the same
size, and these radii already span 5.4x. What decides detail is how big a body
LOOKS -- `2·atan(r/d)` -- so a giant at four hundred metres and a dwarf at
seventy-four deserve identical treatment, and a distance rule gives them
opposite ones. Measured: THE TWO RULES DISAGREE ON 24 OF 27 CASES.

GRAVITY DOES NOT SURVIVE A RESCALE, and this is the one that would have bitten
silently. `world.rs` tunes wells as `(r/15)^1.3` clamped to 0.35..1.6, with a
comment recording that the exponent was pulled down from 1.6 so the field
"no longer swallows a car that drifts past". That is written around a fifteen
metre reference:

     scale      dwarf      giant  variety left
        1x       0.37       1.60           98%
        2x       0.91       1.60           55%
        5x       1.60       1.60            0%

Past about five times every well pins to the ceiling and the field has NO SIZE
VARIATION LEFT -- the exact opposite of what making the planets bigger was
for. The fix is not a new exponent: the reference radius has to scale with the
field, because the tuning was always a statement about a world's size relative
to its NEIGHBOURS. Scaled that way it is unchanged at 100x, and a test asserts
both halves.

AND A LONGER VIEW COSTS DEPTH, which `depth.rs` can price:

     10x reach 2100m, far plane 5250m -> standard Z resolves 4.891 m
                                         reversed Z resolves 0.00007 m

At ten times, standard Z cannot resolve a METRE at the far side of the field.
The fix already exists and is free.

THE FLOOR IS ONE FIELD AND IT IS NOT ARITHMETIC. `MapStyle` KUIPER carries
`amp: 0.15`, so an asteroid field is generated over a terrain grid like every
grounded map -- worlds the size of buildings hanging above a hill. Removing it
means `ground_h`, seating, spawn clearance and the camera's terrain clamp all
have to agree there is nothing to stand on, and I have not done it. There is a
test asserting the floor is STILL THERE, so removing it arrives with a failing
test rather than quietly never happening.

998 passing, zero dead code in the module.

"CYBERTRON HAS NO INDIVIDUALS" WAS A DATA ARTIFACT AND I READ IT AS POETRY.
Last entry celebrated the census finding fifty-six frames with nobody among
them, and said the roster wrote it rather than me. It did not. The ids are
`grimorph_part4` through `grimorph_part88` -- PIECES OF ONE EXPLODED MODEL,
listed in `bots.json` as if each were a bot. Cybertron does not have
fifty-six anonymous residents. It has none, and a parts bin standing where
they should be.

That is a better reason to design some than the one I invented, and the
correction is pinned in both modules rather than quietly edited out.

SO THE FRAMES ARE DESIGNED FROM THE PLANET, not from a mood board. Every
Cybertronian design starts at "angular and mean"; this one starts from what
this codebase already knows the world IS, and the answers fall out:

  * `Material::Metal` at grip 1.00 -- `cling.rs` calls it "purpose-built
    handholds everywhere" -- so WHEELS ARE A SPECIALISATION, not a default.
    Anything can climb. A rolling frame is the constrained one, which is what
    makes choosing wheels say something.
  * No weather, so nothing needs sealing. Open frames, exposed mechanism.
  * `Sky::Lattice` is a built ceiling, so flight is bounded and vertical
    travel is climbing between levels. Hence a `Railed` gait, which no
    Earth-derived roster would have.
  * Nothing erodes it, so fine articulation survives forever.

RESEARCH GAVE THE REGISTER AND THE FAILURE MODE. Cybertronian alt-modes are
angular and unconstrained by Earth physics, and armoured openly because at
home there is nobody to hide from. And the failure is named outright -- a bad
one "looks like a mishmash of robot parts" -- so `alt_mode_is_coherent` is
that warning turned on my own writing, and the driver flags any frame that
trips it.

Twelve frames, varied along the axes the world provides rather than colour
and attitude: gait (all five used), height (2.4 m to 15 m), and purpose (no
two built for the same job -- asserted). SPANWRIGHT is four hundred years
into a span and has not been told the war happened. TOLL still sounds an hour
nothing remembers the meaning of. SMALLHOURS is the only frame that can look
a person in the eye, which nobody designed on purpose. OFFCUT assembled
itself out of salvage -- the parts bin stood up, which is the artifact
answering back.

The claims are checked against the systems rather than asserted: a clinging
frame holds a Cybertronian overhang more than twice as long as the same bulk
without the gait, per `cling.rs`, and the smallest frame is measured against
`scale::LADDER`'s human.

AND A STANDING NOTE ON THE CATALOGUE. It contains many third-party rips of
copyrighted characters (`imported_both_transformers_fall_of_cybertron_*` and
friends). None is a reference for any of this -- the grammar is a set of
physical consequences of a metal world with no weather, and there is a test
asserting no frame borrows a name from that pile.

1006 passing, zero dead code in the module.

THE GRAVITY FORMULA HAS FOUR THINGS WRONG AND I HAD FOUND THE SMALLEST ONE.
`space.rs` analysed the clamp on `node.g` and concluded the tuning does not
survive a rescale. True, and the least of it. Measuring the field itself:

  1. SURFACE PULL DOES NOT SCALE WITH THE WORLD. A 19:1 spread of radius
     becomes a 4.6:1 spread of pull, because both clamps bite. A two metre
     platform pulls 0.35 where a uniform sphere gives 0.13 -- 2.6x too hard,
     which is the reported complaint exactly: at close range everything is a
     planet. Constant density makes surface gravity PROPORTIONAL TO RADIUS,
     which is what the `^1.3` exponent and its clamps are approximating badly.

  2. THERE IS NO FALLOFF UNTIL TWICE THE SURFACE. `w` is pinned at 1 while
     `d < 1.95r`, so the field is FLAT from the centre out past the surface
     and then decays linearly. From a 38 m world the engine reads 1.60 at
     both 38 m and 74 m; the truth falls 2.53 -> 0.67. Real gravity peaks at
     the surface and goes as 1/d² above it, and falls LINEARLY TO ZERO below
     it, which the engine also does not do.

  3. THE VOID PULLS. Beyond every well the field relaxes to 1.0 toward world
     -Y. On a grounded map that is correct and deliberate -- it is the planet
     you are standing on, and the nodes are local overrides. In kuiper it is
     an invisible floor, which the map also literally has.

  4. AND IT ONLY EVER LOOKS AT THE NEAREST NODE. Between two 20 m worlds 240 m
     apart the engine reports 1.000; the real sum is 0.074 and the two pull
     opposite ways. A field of planets should have Lagrange points and this
     one cannot.

`gravity.rs` carries the correct model -- shell theorem, both regimes -- the
engine's model beside it, and the crossover where they agree. NOTHING IS
PATCHED. `world.rs`'s constants are load-bearing for eighteen grounded maps
where the flat near field is what makes a carved deck driveable, and the fix
is a per-map ambient setting plus a real falloff. That is a decision, not a
better exponent.

1015 passing, zero dead code in the module.

A VOXEL GAN'S CRITICS, AND A DECODER BUG FOUND ON THE WAY IN.
`apps/beatspace` has a working DCGAN over mel spectrograms -- generator and
discriminator hand-built from `Var`s so checkpointing keeps BatchNorm's
running statistics. That half ports to 3D directly and is not the interesting
part. A voxel GAN on its own MAKES BLOBS: a discriminator learns "does this
look like the corpus", the corpus is a thousand vehicles, and the cheapest way
to satisfy it is a vehicle-shaped lump. Nothing in that loss knows a frame
needs two arms.

THE CORPUS DECODER WAS WRONG. `.svxr` representatives are seven bytes per
voxel -- three u16 coordinates and a palette index -- and
`tools/solve_placements.py` read them at `off + i * 4` while unpacking six
bytes, so every cell after the first came from a sliding window across two
records. On `imported_grimorph_part4`: stride 4 puts 191 of 647 voxels inside
the 0..32 grid, stride 7 puts all 647 inside it, and forty random
representatives are all exactly `nvox * 7` bytes. Anything ever solved against
representatives rather than originals was aligning to a cloud two thirds of
which was out of bounds. Fixed there, with an assert, and the Rust decoder
carries the same note.

MY SKELETON CRITIC WAS WRONG TOO, AND THE DATA SAID SO. I built one that
splits a volume into components and asks whether they form a body -- torso,
head above it, limbs in pairs -- and IT SCORED REAL MODELS BELOW NOISE, 0.413
against 0.494. The reason is in the corpus, not the weights: a representative
is ONE PART, normalised to its own grid with its own `lo` and `step`, so a
frame is many files with no shared origin. Placement was normalised away. A
critic that needs relative position cannot run on this data at all, and tuning
it would have been fitting weights to a false premise. The reference does
exist -- `tools/mounts.json`, positions "in the vehicle pose's own space" --
and wiring it is a separate job.

WHAT THE CORPUS DOES SUPPORT WAS ALREADY THERE. `tools/part-labels.json` holds
FOURTEEN CLUSTERS measured across the whole library, with shape features and
the member names that fell into each: `rib, link, face` (396 parts, paired),
`muscle, piston, shin` (530, paired), `pelvis, spine, clav` (95, paired). That
is the cross-bot part vocabulary, derived rather than authored, and it knows
about MIRRORING -- the one piece of skeleton information that survived
normalisation. A generated part is plausible if it lands near one of those.

AND THE HONEST RESULT IS A WEAK ONE. Real parts beat a density-matched blob by
about five percent, 0.81 against 0.76. Not the twenty-five I first asserted,
and the test now pins BOTH ENDS -- it fails if the margin vanishes and it
fails if it exceeds 1.5x, which would mean this note is out of date. Coarse
shape features are not very discriminative because the library's own clusters
are broad enough to admit a blob. That is the argument FOR the learned half:
the discriminator does the heavy lifting, and these two exist to stop it
settling for a lump.

NOT BUILT: the training loop. It needs GPU hours this environment does not
have, and an untrainable thousand lines is worse than none.

1022 passing, zero dead code in the module.

THE MAPS ARE DELETABLE, WHICH UNBLOCKS THE GRAVITY FIX I HAD JUST RULED OUT.
`gravity.rs` ended on "nothing is patched -- `world.rs`'s constants are
load-bearing for eighteen grounded maps". True, and about a premise that
turned out to be negotiable. Recorded rather than edited out, because it is a
thing worth noticing about my own caution: I treated an existing constraint as
a fixed one.

THE CAMPAIGN STACK IS ALREADY INDEPENDENT, and not loosely -- it does not
import `world.rs` at all. `terrain` imports nothing; `survival` imports `sun`;
`procgen` imports foothold/raid/sun/survival. `procgen::World` is a DIFFERENT
TYPE from `world::World` sharing no code, `Weather` lives in `survival.rs`,
and `Material` does not appear in `world.rs` once. So the three things worth
keeping -- weather, terrain building, material fidelity -- are already outside
the thing being deleted.

WHAT `world.rs` IS: 4,835 lines, of which 44% (2,156 lines, the focal arms at
813-2969) is per-map content. The rest is skydribble SANDBOX -- SDF, shapes,
portals, tiles, currents, the ball and goal machinery. Eight modules import
it: six renderer, one headless driver, one bridge. None is campaign logic.

AND `sim.rs`'s 115 REFERENCES ARE THE EASY CASE. I flagged that count as
deciding whether the job is mechanical or a rewrite. It is the headless bot-AI
harness behind `--sim` -- "real physics at 120 Hz, no window, no GPU, no
audio" -- so it is a DRIVER and needs one world to run in, not eighteen.

THE TUBES ARE INVERTED, and that is most of why they never felt right. Spin
gravity is `a = ω²r`: ZERO ON THE AXIS, maximum at the wall. The engine
computes `0.50 + 0.95·(1 - d/(r/1.45))` -- maximum on the axis, weakest at the
wall. The comment history records both symptoms and neither diagnosis:

    "a car flung into a wide pipe simply flew up the middle at constant
     speed, never touching anything"

That is CORRECT. You are weightless on the axis of a spinning habitat. It was
read as a failure, the fix made the axis strongest, and then "at 1.7x it
pinned" -- of course it did, it was flinging you outward hardest from the one
place that should be still. `true_tube` is the right profile and it is the
same shape as a sphere's interior, for the same reason: nothing outside your
radius contributes.

They are also 147 mentions from line 25 to line 4635, which is not a system
but a thread woven through everything. A rewrite should start from the
corrected profile and from putting the tube IN ONE PLACE.

`docs/UNWIRING.md` has the order of work.

1025 passing.

THE BENCH — authoring frames in the game rather than in a second GUI.
`[` and `]` already cycle the chassis, so the authoring keys sit beside them:
`0` rerolls, `s` saves. Both were free — checked in `main.rs`, not assumed.
`BLACKSTAR_CHECKPOINT_FILE` points at a trained generator.

A CHARACTER IS A SEED, NOT A MESH, and everything follows from that. Stored as
voxels, sharing one means sending a model file; stored as a seed it is
`bsc1-3034b47aacce8c59-lib-spanwright` — short enough to paste into a message,
and rerolling costs nothing.

The price is that a seed only means something against the checkpoint that made
it. So the code carries the checkpoint's fingerprint, and importing against a
different one is REPORTED RATHER THAN RENDERED:

    slot 0, but it needs checkpoint m96cf0626
    and this one is m45ea62ca. NOT rendered

A shared character that quietly comes out different is worse than one that
refuses. It is still KEPT in the slot, though — you do not lose a friend's
character because you have not downloaded a file yet.

IT HAS TO WORK WITH NO CHECKPOINT, because most people will not have one and
the feature cannot be a dead button for them. With none, the roll comes from
the parts library: same keys, same slots, same export, a narrower space. That
is not a consolation prize — `--voxel` measured the hand-built critics
separating real parts from noise by five percent, so a trained generator has
to be genuinely good before it beats picking real parts, and until then THE
LIBRARY IS THE BETTER GENERATOR.

SAVING REFUSES WHEN FULL rather than overwriting. Losing an authored character
to a keypress is the one failure this feature cannot afford, and "the bench is
full" is something a player can act on. Two tests: one fills every slot and
asserts the ninth save returns None with the count unchanged, one clears a
slot and saves into it.

The fingerprint hashes the PATH, not the file. Reading a multi-gigabyte
safetensors at startup to decide whether a share code matches is not a trade
worth making; a path collision gives one wrong verdict, hashing the file makes
every launch slower for everyone. Written down where the decision is, because
it is the kind of thing that looks like an oversight later.

1037 passing, zero dead code in the module.

TRAINWATCH — I built the consumer and no window into the thing producing it.
`bench.rs` takes a checkpoint; nothing watched the run that makes one.

DELIBERATELY NOT A SECOND TRAINPAGE. `beatspace/src/trainpage.rs` already
models the loss curve, the trend, the stall check and the log ring, and those
are domain-free. What it cannot have is the half that matters here.

LOSS CANNOT SEE MODE COLLAPSE. A GAN can sit at a perfectly healthy loss while
emitting the same frame every time, and that is invisible in `d_loss` and
`g_loss` — it is only visible in the SAMPLES. For voxels there is a way to see
it without eyes: `voxel::CLUSTERS` is fourteen part classes measured off the
real library, so a healthy generator lands across several and a collapsed one
lands in one. The driver runs two watches with LITERALLY IDENTICAL loss curves
and reports "learning" for one and "MODE COLLAPSE" for the other.

THE FROZEN DISCRIMINATOR ALREADY HAD A NUMBER and I only had to read it.
`beatspace/src/model.rs` records the failure from having had it: "d_loss
pinned at 2ln2, g_loss at ln2, flat for forty points, after a vertical fall. A
frozen input-independent D scores exactly there." That is detectable, and
detecting it beats plotting it. `Health::FrozenDiscriminator` checks both
constants and the flatness, and its message says WHY — no unit below the
gate's threshold can receive a gradient — because the fix is to restart from
before the fall, not to tune anything.

ORDER MATTERS AND IS TESTED. A dead gate also looks like mode collapse and
like a stall. Reporting "mode collapse" there sends somebody to tune diversity
when the discriminator has stopped depending on its input, so frozen is
checked first and a test asserts the diagnosis is not shadowed.

COVERAGE IS THE ANSWER TO "WHAT CAN IT MAKE YET", and it comes free: the
critics that shape the reward are the readout, so `observe` is the only bridge
and nothing else needs instrumenting. On real volumes it grades cleanly —
dust 0.00, forming 0.20, solid 0.61 — and dust reading zero is a diagnosis
rather than a hole, since a generator emitting specks forms nothing big enough
to judge.

1050 passing, zero dead code in the module.

A FAMILY PER PART CLASS, AND AN ASSEMBLER WITH ITS OWN SCORE. One GAN making
whole bots has to learn anatomy and geometry at once, which is exactly what
sank the skeleton critic: a representative carries no part IDENTITY and no
placement, so there was nothing to check anatomy against. Fourteen
generators, one per measured class, gives every part an identity — and then
the pairing is a separate problem with a separate score, which is the half
that was impossible before.

THE ENGINEERING LIMIT IS THE DATA, MEASURED. `part-labels.json`'s classes run
49 to 836 examples. At a floor of 200, TEN OF FOURTEEN can train their own
generator and the rest sample the library — the same fallback `--bench` uses,
because forty-nine examples is not a training set. There is also exactly ONE
leg family against six arm families, which is a real gap in the corpus rather
than in the design.

REGIONS CAME FROM THE MEASURED `up` BIAS AND THAT WAS WRONG. I preferred it
because it is a measurement and the member names looked authored, and argued
the point in a test. The names are not authored -- they are what the harvester
labelled parts as against the source skeletons, and a 27-word vocabulary with
explicit left/right (`thighl`, `clavr`, `shinl`) carries body-region
information a mean vertical bias cannot. The two disagree badly: `up` put
cluster 0 in the head and its members are 42% torso and 0% head; it put
cluster 8 in the head and that one is 90% torso.

AND A DISTRIBUTION, NOT A HOME. Leg parts are spread across NINE OF FOURTEEN
classes -- 35% of one, 19% of another, 12% of a third -- because the clusters
are SHAPE families and a thigh and a clavicle look alike. Collapsing that to
"cluster 12 is the leg one" throws away most of what the data knows, and the
apparent "only one leg family" gap was an artifact of my own projection rather
than a hole in the corpus.

THE PENALTY IS THEN PRICED BY RARITY, and normalised against the BEST
AVAILABLE family rather than against perfection. That normaliser is necessary,
not a refinement: no cluster is predominantly an arm -- the best is 35% -- so
pricing against 1.0 charges every arm in the library for being an arm, and a
plain frame came out at 0.24 with a penalty of 0.61. The question a soft bound
should ask is "how much worse than the best available choice".

HARD AND SOFT, AND THE LINE IS THE DESIGN:

  HARD where the assembly cannot exist or cannot be JUDGED. No torso is
  nothing to attach to. Two heads is a slot filled twice. A part outweighing
  its frame is a physical claim. One arm and no other side is hard for a
  subtler reason -- there is no second side, so the symmetry checks have
  nothing to compare against.

  SOFT where it is unusual and might be excellent. A leg-class part used as an
  arm, mismatched pairs, a pair-family part used alone. Every one is
  somebody's favourite bot.

    a plain frame                0.449
    legs used as arms            0.287
    mismatched arms              0.212
    no torso                     REJECTED
    one arm                      REJECTED
    a head heavier than the body REJECTED

AND THE SOFT BOUND IS OUT-ARGUABLE, which is the point. Legs-as-arms loses to
a mediocre plain frame at part quality 0.55 and 0.75, and WINS at 0.95. The
legs themselves now cost nothing, because cluster 12 IS the best leg family --
and the head is the honest weak spot, since no family is head-ish at all. Two
tests pin the calibration from both ends: a penalty that changes nothing is
decoration, and one that always decides is a hard bound wearing a discount.
No quality rescues a torso-less pile.

1060 passing, zero dead code in the module.

`guts` IS 39% OF THE LIBRARY AND I CALLED IT A TORSO. Chasing the "no family
is head-ish" finding one step further turned up a third mapping error, and the
largest: 1,733 of the library's 4,455 parts are named `guts`, and mapping that
one word to Torso made almost every cluster read as "mostly torso". Which is
why the pricing came out nonsensical and needed a normaliser to look sane —
the normaliser was compensating for a bad input.

THE TREE ALREADY HAD THE CLASSIFICATION AND I INVENTED MY OWN INSTEAD OF
LOOKING. `tools/find-mounts.py` carries an argued INTERNAL list, written after
a search for gun barrels kept returning struts:

    INTERNAL = ("guts", "bone", "muscle", "cable", "link", "piston",
                "vertebra", "rib", "coil", "core", "keel", "greeble",
                "spine", "gyro")

Using it changes the answer completely. FOUR OF THE FOURTEEN FAMILIES ARE
94-99% INTERIOR -- they generate nothing but guts. Only a handful carry real
anatomy.

WHICH IS A BETTER ARCHITECTURE THAN THE ONE I PROPOSED, and it came out of the
counts rather than out of me: the family is not fourteen equal generators, it
is a few anatomical ones plus an INTERIOR-FILLER generator.

AND THE STING:

    family  7 is  82% anatomy and has   95 parts — TOO SMALL TO TRAIN
    family 11 is  70% anatomy and has  122 parts — TOO SMALL TO TRAIN
    family  0 is  57% anatomy and has  356 parts — trains

THE TWO MOST ANATOMICAL FAMILIES ARE THE TWO SMALLEST, both under the floor,
and the classes that CAN train are mostly the ones full of guts. That is the
real constraint on a per-part family, and it is a corpus problem rather than a
design one -- which is worth knowing before spending GPU hours on it.

I also deleted `home_region`. It returned the mix's argmax, and once `guts`
was classified correctly that answer is "trim" for twelve of fourteen: a
column saying the same thing twelve times is noise with a heading. The driver
prints anatomical SHARE and best-region instead. Kept as a note, because the
argmax is the obvious thing to reach for and is wrong here for a reason
specific to this corpus.

1062 passing, zero dead code in the module.

AND THE CORPUS CONSTRAINT DISSOLVES, which is better than reporting it. Last
entry ended on "the two most anatomical families are the two smallest and both
are under the floor — a corpus problem rather than a design one". Two levers
turn out to fix it, and only the second one works.

MIRRORING IS FREE AND EXACT. Eleven of the fourteen classes have
`mirrored >= 0.9`: their parts exist as a left and a right, so reflecting a
left thigh does not APPROXIMATE a right thigh, it produces one, and that part
is genuinely in the library's distribution. Free data rather than a
regularisation trick.

IT IS ALSO NOT ENOUGH. Family 7 goes 95 to 190 against a floor of 200 — short
by ten, of a number I invented — and family 11, the other anatomical one, does
not mirror at all. A test pins both, so if the corpus or the floor moves it
says so.

POOLING BY ROLE IS THE ANSWER. A cluster is a SHAPE partition and training
wants enough examples of a FUNCTIONAL thing; there is no reason an arm
generator should care that one arm-ish cluster sits slightly apart from
another. Pooled and mirrored, every role clears the floor:

        role     pooled    trains?
        head        325        yes
       torso        383        yes
         arm        690        yes
         leg        762        yes
        trim       8165        yes

Head was the worst case at 163 and more than doubles.

SO THE ARCHITECTURE IS NOT FOURTEEN SHAPE GENERATORS AND IT IS NOT FOUR. It is
FIVE ROLE GENERATORS — head, torso, arm, leg, interior — and the shape
clusters are how you SAMPLE within a role rather than what you train. That is
a smaller, better-fed family than the one proposed, and every step of the
argument came out of counting rather than out of me.

1064 passing, zero dead code in the module.

THE BALL-CHASER AS A MISSILE. `bots.rs` steers three hundred cars at a ball on
CPU at frame rate and the structure ports directly — a target with a velocity,
a chaser with a turn limit, a uniform grid so a pile stays a pile. One thing
has to change and it is the steering law.

Ballchase aims at `ball.pos + ball.vel * 0.25` and drives at it. That is PURE
PURSUIT WITH A FIXED LEAD, and for a car chasing a ball it is right: the lead
is short because the car is slow, and constantly re-aiming looks eager, which
that module's doc calls "tuned for comedy". For a missile it tail-chases.

PROPORTIONAL NAVIGATION turns in proportion to how fast the LINE OF SIGHT is
rotating rather than to how far off-axis the target is. If the bearing is not
changing you are already on a collision course and should not turn AT ALL,
which is the exact opposite of pure pursuit and the whole reason it wins.

    crossing       pure pursuit       proportional
          0°    hit 7.49s 2997m    hit 7.49s 2997m
         30°       MISS by 82 m    hit 7.06s 2823m
         90°      MISS by 491 m    hit 4.47s 1787m
        180°    hit 2.50s 1000m    hit 2.50s 1000m
                          2 of 7            7 of 7

Pure pursuit only manages the two shots where there is no crossing to lead.
And the agility table is the practical half: at a 90° crossing PN hits at
150 m/s² where pursuit still misses by 112 m at 250. A CHEAPER MISSILE HITS
WITH IT, which is what matters if there are going to be hundreds of them the
way there are hundreds of cars.

TWO BUGS OF MINE, AND THE SECOND IS THE INSTRUCTIVE ONE.

I integrated steering as `vel += a*dt` and renormalised, which is fine while
`a·dt` is small against the speed and nonsense when it is not: at a turn limit
of 100,000 the added vector dwarfs the velocity, so renormalising left the
missile flying along its own lateral acceleration. PURE PURSUIT MISSED WITH
INFINITE AGILITY, which cannot happen and was the tell. Lateral acceleration
is a turn rate, `ω = a/v`, and rotating by `ω·dt` is exact at any magnitude.

Then both laws still missed by a hundred metres and I nearly reported that as
a finding. The DEFAULT ENGAGEMENT WAS GEOMETRICALLY IMPOSSIBLE: launching from
300 m at 250 m/s is 1.2 seconds of flight, in which a 90 m/s² missile turns
24.8° against 25.6° of lead required. Nothing could have hit it. Checked with
five lines of trigonometry rather than by tuning until the numbers looked
better, which is what I would have done an hour earlier in this session.

1072 passing, zero dead code in the module.

AND A SALVO HAS THE OPPOSITE PROBLEM TO A CROWD. `bots.rs` earned its
reputation at three hundred cars by keeping them APART — a uniform grid so "a
300-spartan pile stays a pile instead of a singularity". Missiles inherit that
and get a worse version of it.

PROPORTIONAL NAVIGATION IS DETERMINISTIC. Same target, same speed, same turn
limit, same solution — so a salvo launched together converges on one point and
arrives as a single warhead. Measured: twelve missiles land within 1.2 m of
each other.

A SEPARATION GRID CANNOT FIX THAT, which is the interesting part. By the time
they are close enough to push apart the shot is over, and pushing fights the
guidance rather than shaping it. The fix is upstream — give each missile its
own AIM POINT around the target so they are solving different problems from
launch:

    aim spread  hits/12  impact spread
            0m       12           1.2m
           10m       12           8.9m
           25m       12          23.0m
           60m       12          57.0m

The spread costs nothing: twelve hits at every setting, and a test asserts
spreading cannot halve the hit rate — a spread that does that is not a spread,
it is a miss distribution.

AND THE CPU CLAIM HOLDS. 300 intercepts in 80 ms, all 300 hitting. PN is a
cross product and a dot product per missile per tick, which is far less than
the car physics the ball-chaser was already running at that count.

1077 passing, zero dead code in the module.

MANY LIMBS, A TIGHTER CLAMP, AND A SECOND AXIS TO EARN A PLACE ON.

The old limb rule was that a limb needs its opposite side, which forbade
everything interesting — three arms, six legs, a radial thing with no sides at
all. Many are allowed now and the clamp got tighter in exchange: a ceiling,
and an evenness rule on SIDED limbs only, since an odd number of centre-mounted
ones is a radial body rather than a missing part.

BUT THE REAL PROBLEM WAS THAT SCORING ON CORPUS MATCH ALONE IS A MACHINE FOR
PRODUCING THE AVERAGE BOT. Anything novel is by construction less like the
library, so that term can only ever punish it. A freak needs somewhere else to
earn its place, and physics is the honest one — a six-legged thing matches
nothing and stands up better than a biped, which is a real advantage the
matching score cannot see.

               frame   support    margin      score   topple @1g
               biped      1.00      0.79      0.513        1.29s
     top-heavy biped      1.00      0.59      0.467        1.32s
         radial quad      1.00      1.56      0.775        1.56s
      radial hexapod      1.00      2.11      0.775        1.80s
 four legs, one side      0.00      0.70      0.589        1.04s

The hexapod has no head, no arms and six legs from one family, and it BEATS a
textbook biped. Four legs clumped on one side score 0.00 support, because
evenness is measured as the length of the mean bearing rather than by counting
feet.

GRAVITY FEEDBACK makes the same frame a different design per world. An
inverted pendulum falls in about `sqrt(h/g)`, so a top-heavy build has 1.32 s
on Cybertron and 2.15 s on Mars — unusable at home and fine on the rim.

A CHECK WRITTEN FOR THE OLD RULE REINTRODUCED THE EXACT BIAS. "A pair family
used alone" fired on any CENTRE-mounted limb, so the radial hexapod paid it
six times — 0.72 of penalty — and scored 0.055 against the top-heavy biped's
0.467. Alone has to mean one of its kind on the whole frame.

AND MAX_LIMBS IS NOT WHAT STOPS CENTIPEDES. Measured: a radial quad and a
radial hexapod score IDENTICALLY, because stability saturates at a margin of
1.5 and four evenly spread legs already clear it. Saturation removes the
incentive; the clamp is a backstop against absurdity. Pinned in a test,
because two mechanisms that look like they do the same job invite somebody to
delete the one that is carrying it.

1083 passing, zero dead code in the module.

A PLAYTEST SAID THE GAME FEELS MOSTLY THE SAME, and it is right. No walking
robot, no landing, no playable mission. Measured in `main.rs` — the actual
game loop — `patrol` has ZERO references, `procgen` ZERO, `foothold` ZERO. The
campaign stack is complete, tested, and not plugged in.

The pattern is worth naming: I built what could be verified without a screen,
and verifiable-without-a-screen turns out to correlate almost perfectly with
NOT IN THE GAME. Thirty-odd `--flags` now, and the number of things you can do
by pressing play has not moved. `docs/PRIORITIES.md` is the honest accounting
and the order to fix it — starting with one mission end to end, badly, because
every piece exists in isolation and none has ever run in sequence with a
renderer attached.

TWO BUGS FROM THE PLAYTEST PASTE. `--arch map <node>` silently ignored its
argument and printed the whole map, which reads as the map refusing to filter
rather than as the wrong verb; it routes to `why` now and says so. And
`--gravity` printed "three things wrong" above four of them, left over from
before the nearest-node finding.

ROCKET LEAGUE ASSETS GONE: `dominus`, `dominus_red`, `vulcan.glb`,
`xdevil.glb` — 9.4 MB, and a licence question nobody wants in a game with its
own 1,012-model catalogue. Substituted with procedural chassis rather than
removed from the list, because `cars.models` is INDEX-ADDRESSED by `cfg car`
and deleting an entry silently renumbers every car after it along with every
saved loadout.

1083 passing.

THE STARMAP — `patrol` IS REACHABLE FROM INSIDE THE GAME. Taking my own
priorities doc seriously rather than adding another driver: stations ARE in
the game loop, the airlock was the one hub room without a console, and
`patrol::ROUTE` had eighteen destinations that were tested for years and
invisible while playing.

It lists them all with gravity and surface material, gated on the missions
that reach them — clearing `the_pluto_line` opens a third. Unreachable ones
are GREYED RATHER THAN HIDDEN, because `menu.rs` already argues that "why
can't I press this" beats a row that is not there, and a starmap with holes
in it is a worse map.

AND IT SAYS WHAT IT DOES NOT DO. Pressing a destination reports "course laid
for EARTH — 1.00g, Sandstone, weather. flying it is not wired yet." A console
that implies the loop is finished when it is not is worse than one that
admits it, and a test asserts the string still says so.

THE WIDTH GUARD PAID FOR ITSELF. My new footer row was 47 characters against
a 46-character console and the test I added several sessions ago caught it
before it shipped as a truncation.

Two mistakes of my own along the way: I counted destinations by matching "g "
in the label, which also matches the "N of M plotted" heading and read
nineteen out of eighteen; and I reached for `Row::dim` and `Row::note`, which
do not exist — `disabled()` does, and its doc comment is the argument for
greying rather than hiding.

1087 passing.

THE STARMAP FLIES THE COURSE NOW, in simulation. Pressing a destination runs
`patrol::Destination -> mapsim::ground_for -> drop::Drop` in sequence — the
first time those three have run together rather than as three separate
headless drivers. Earth: "down at 7 m from the mark at 45 m/s".

A test asserts the destination's physics REACH the outcome — a sixth of a g
arrives slower than a full one — because otherwise `ground_for` is decorative
and the three modules are still three modules with a console in front of them.

AND MY OWN TEST FROM LAST SESSION FAILED, CORRECTLY. It asserted the console
said "flying it is not wired yet". Wiring it broke that, which is the
assertion doing its job: a claim about what a feature does NOT do has to be
revisited when it starts doing it, and a test is the only thing that makes
that happen on purpose rather than by somebody noticing.

DEEP SPACE IS A DOCK, NOT A LANDING. Three of the eighteen destinations have
gravity at or near zero — `outer_rim` and `shield_edge` are DeepSpace,
`carrion` is an asteroid field at 0.02g. A drop needs something to fall
toward, and forcing one loops to the timeout and reports an infinite miss.
Found by the test that flies every destination: "outer_rim never landed at
all". The route table is RIGHT and my assumption that every destination has a
surface was not.

1090 passing.

"WALKING AROUND AS A ROBOT" IS NOT A WIRING JOB, and finding that out was
worth more than wiring `Avatar` in would have been.

`apply_car_dims` already threads a `size` through every dimension, so the
plumbing exists. But `cfg car_size` clamps to 0.55-2.0 and a bot needs 22.4x
against the engine's 0.38 m tuning body — and raising the clamp is not the
fix, because at 22x every constant is wrong by a DIFFERENT EXPONENT:

      constant        now      correct   naive (xk)    error
          GRAV       6.50         6.50       145.39    22.4x
       CAR_MAX      23.00       108.78       514.47     4.7x
   BOOST_DRAIN      33.30         7.04       744.87   105.8x
         M_CAR     180.00   2014552.25      4026.32     0.0x

Froude: length k, time sqrt(k), speed sqrt(k), ACCELERATION 1, mass k^3.
Gravity was never wrong — it is an acceleration, and accelerations are
scale-free — so the intuitive "multiply everything by k" makes a heavier
world rather than a bigger one, and a bot that accelerates twenty-two times
too hard.

AND I GOT THE SECOND HALF WRONG FIRST. I asserted Froude PRESERVES
body-lengths-per-second and wrote a test saying so. It does not: speed goes
as sqrt(k) and length as k, so relative speed goes as 1/sqrt(k) and big
things are relatively SLOWER — which is why an elephant is not as quick for
its size as a mouse. The car's 61 bl/s becomes 12.8, so the scaling does most
of the work.

Not all of it. A cheetah manages about 4 bl/s and is the fastest thing on
land, so the scaled bot is still three times that and doing 392 km/h. The
exponents are right and the INPUT is not: scaling tells you how a number
changes with size, and cannot tell you the number was never a robot'"'"'s to
begin with. The feel numbers have to be re-derived.

`--rescale` has the table. `PRIORITIES.md` now says a day of work and a
playtest rather than a switch.

1098 passing.

THE SHIP IS A PLACE NOW, and it was the right thing to build before the scale
work: none of `--rescale`'s problem applies indoors. The rooms are authored,
the scale is fixed, and moving between them is a position and a walkable
check.

`interior.rs` built a complete walkable ship — eight rooms from `hub_plan.h`,
corridors, doors that seal and unseal, an SDF, a walkable test, fixture
positions — and `main.rs` referenced it ZERO TIMES. `stations.rs` gave every
console a `cell()` naming its room, and nothing ever read it. So the ship was
a diagram with a menu bolted to it.

The missing piece was one lookup: `room_at`. A walkable ship with no way to
ask "where am I" cannot drive a console, which is what "referenced zero
times" meant in practice.

BEING IN THE ROOM IS WHAT OPENS A CONSOLE. I declared a three-metre activation
radius out of habit and deleted it unused: the room rule needs no tuning,
cannot put two consoles in range at once, and makes the answer to "why can't
I use this" always "you are not in there", which a player can see. A radius
would have added a number to get wrong.

AND POWER FAILURE FINALLY DOES SOMETHING. From the bridge with the doors
shut, 1 of 7 consoles is reachable and six are stranded by name.

TWO THINGS I EXPECTED WRONG, both about doors being animated rather than
boolean. A freshly generated ship has every door at travel 0, so a new
`Aboard` reached exactly one room — correct, and my test read "1 vs 1" and
concluded the sealing was broken. Then `depower` sets doors CLOSING rather
than closed, so asking immediately gave "7 vs 7". Both fixed by running the
doors out; a ship you have just boarded is sealed, and unsealing is what
boarding means.

1106 passing.

A SORTIE FLIES TO ITS DESTINATION NOW. "Resolved, not flown" was the honest
label on the Bridge for as long as there was nothing to fly. There is: a
mission names a `dest`, `patrol` knows that world's gravity and surface, and
`drop.rs` flies the fall — so the chain runs

    MISSIONS.dest -> patrol -> mapsim::ground_for -> drop -> MissionRun

and the report reads "the_pluto_line — clean drop on PLUTO — nightfall
CLEARED". The board advances; two clears in and the Bridge offers
`the_scavenger_king`.

AND ARRIVING BADLY COSTS SOMETHING, which is the part worth having. A
scattered landing takes up to 35% off the engagement's power — the first
thing in this game where getting somewhere badly has a consequence at the far
end rather than being scenery.

`MissionRun::arrived_badly` is a METHOD, not a public field. `power` is
resolved once against the mission'"'"'s recommended energon and the invariant is
that nothing moves it afterwards except a NAMED cost — a landing, a burn.
Opening the field would turn that invariant into a convention, and the
compiler told me it was private, which was the right answer.

A test asserts every one of the fifteen missions goes somewhere `patrol`
knows, because a mission whose dest is off the route would silently skip the
drop — which looks like the feature being off rather than the data being
wrong.

1110 passing.

THE SHIP HAS GEOMETRY NOW. `interior.rs` gave a ship you can WALK in and
nothing to LOOK at: its SDF feeds collision, `world.sdf` is a physics query,
and the renderer draws meshes. So the rooms have had positions, sizes, doors
and fixtures all along and never a triangle among them.

`hull_mesh` is one bevelled box per room and per OPEN corridor, at their real
positions — 1,020 triangles for a 70 m hull, which is nothing. Deliberately
the shell and not the dressing: enough to stand in and see where the doors
are.

SHUT CORRIDORS ARE OMITTED, and that matters twice. A sealed door should not
have a tunnel drawn through it, and the mesh then AGREES WITH
`reachable_from` about what is passable rather than being a second answer to
the same question. Sealed, the hull drops from 1,020 triangles to 600, and a
test asserts the direction.

AND A STALE DOC, FOUND ON THE WAY. `roster.rs` opens by explaining that
`CSDF_MAXM` is 126 models and that 222 chassis at five poses is 1,105 against
that budget, which is why a selector exists. The constant is now 1,344 —
tiling turned a depth-only stack into a grid — so THE WHOLE ROSTER FITS, at
1,110 against 1,344.

The code was never wrong: `SDF_BUDGET` is DERIVED from the constant and moved
with it, which is precisely what its own comment predicted would matter. Only
the prose rotted, and it rotted invisibly because a selector that never has to
refuse looks identical to one that does. Corrected on top rather than
rewritten — the reasoning is still why the selector exists and is no longer
why it is needed — with an assertion that fails if the budget shrinks again.

1113 passing.

THE UPLOAD PATH, AND WHERE I STOPPED. `Aboard::upload_hull` registers the
ship mesh with `Gpu::add_mesh` and keeps the slot, re-uploading when the doors
move because a sealed corridor is not drawn and the geometry has to agree with
what you can walk through.

It type-checks, and that is worth stating precisely rather than hoping:
`wgpu` is NOT feature-gated in this crate, so `render.rs` is compiled by the
same `--no-default-features` build the tests run against. The path is real
code the compiler has seen; what it has never done is run on a device.

NOTHING CALLS IT, and I left the warning rather than inventing a caller. The
remaining work is two lines — call it once on boarding, and `push` the slot
with `Mat4::IDENTITY` in the draw loop the way the terrain chunks already do.
Both want eyes on the result. Writing them blind would turn "not yet drawn"
into "drawn, possibly wrong, and nobody can tell", which is worse.

What IS checked here is the bookkeeping around it: the slot starts empty and
survives walking, because getting that wrong means re-uploading a thousand
triangles every frame and it needs no GPU to catch.

1114 passing.

THE HARNESS WAS FINE AND I BLAMED IT TWICE. Two sessions running, I tried to
reach the Bridge's sortie row with `--menu bridge --tape d,d,a`, got the
nightfall row instead, and concluded the tape could not reach arbitrary rows.
Worked around it both times rather than looking.

Selection WRAPS. The bridge has two selectable rows, so the second `d` returns
to the first. `d,a` is the sortie, and it always was.

That is worth recording as a habit rather than a fact: I hit an unexpected
result from my own tool, formed a theory about the tool, and did not spend the
thirty seconds to test the theory — twice. The check that settled it was three
tapes side by side, which is the thing I should have run the first time.

WITH IT USED CORRECTLY, THE CAMPAIGN RUNS:

    the_pluto_line     — clean drop on PLUTO — sortie CLEARED
    the_red_extraction — clean drop on MARS  — sortie CLEARED
    the_beach_opens    — clean drop on EARTH — sortie CLEARED
    what_jhiaxus_grew  — clean drop on EARTH — sortie CLEARED

Five in sequence, the board advancing, each reading its own destination. That
is `PRIORITIES.md` item 1 — "one mission end to end, badly" — done in
simulation, and a test now pins it by driving the log directly so it does not
depend on menu navigation.

1115 passing.

THE SHIP IS IN THE SCENE. Uploaded once at launch, drawn every frame at the
world origin, one draw call of about a thousand triangles — 26 m across and
10.3 m tall.

AND THE OBVIOUS TRANSFORM WAS WRONG. I lifted the hull by `-deck_y`, which is
where you STAND — but the rooms extend below the deck, so the ship sat 4.9 m
UNDERGROUND. On screen that looks like nothing drew at all, which is the worst
possible failure because it points at the draw call rather than at the
transform, and it would have cost a playtest to find.

A test caught it before the build finished: take the mesh's own lowest vertex,
apply the lift, and assert the floor is above ground. `ground_lift` is the
mesh's floor rather than the deck.

That is the argument for the arithmetic checks in general. I could not look at
this, so the only way to know the ship was not buried was to compute where it
lands — and the computation is three lines.

1116 passing.

THE SHIP IS SOLID. `World::park` folds the interior's SDF into the world
query — one `Option` field and five lines at the end of `World::sdf`. The hull
has been walkable since `interior.rs` landed and the world knew nothing about
it, so you drove through scenery.

BROAD-PHASED ON THE HULL BOX, because `sdf` runs per contact per frame and
`sdf_local` walks every cell and every link. A test asserts the reject path is
at least three times cheaper than the full query, so the common case — being
nowhere near the ship — costs almost nothing.

AND BOTH TRANSFORMS READ `ground_lift`. The draw call and the park use the
same function, with a test that the mesh floor and the collision origin agree.
Two transforms for one object is exactly how a hull you can SEE and a hull you
can HIT end up a metre apart, and that failure is invisible until you walk
through a wall that is drawn beside you.

1119 passing.

FOUR MORE JOINS, AND EACH ONE EXISTED ON BOTH SIDES ALREADY.

`V` OPENED THE ARMORY UNCONDITIONALLY. Not a placeholder — there was no way to
ask where the player was, so any console was as good as another. Now the ship
is parked and solid, `interior::room_at` turns a position into a room, and
`Station::cell()` has named each console's room since the stations landed.
Outside the ship it says "no console here" rather than opening one: a hub you
can reach from anywhere is a menu.

TAB CYCLED ALL SEVEN STATIONS regardless of sealed doors, which made `depower`
cosmetic — the whole point of a door that shuts is that it stops you. It walks
the reachable set now, falling back to all of them when the player is not
aboard, because being outside should not silently empty the list.

ENTERING A ROOM IS AN EVENT, not a state. The room is a fact every frame and a
HUD line that reprints it sixty times a second is noise; only the transition
is worth saying, and what is worth saying is what you can DO there — "TRADE
DECK — press V for THE CHARTROOM".

AND THE BORROW CHECKER CAUGHT ME REPEATING A MISTAKE THE FILE ALREADY
DOCUMENTS. The Enter arm has a comment explaining that it resolves its order
OUTSIDE the deck's borrow, because acting on the deck needs the shell. I put
the reachable-console lookup inside the same borrow. Hoisted, with a note
pointing at the neighbour that already said so.

1119 passing.

THREE MORE JOINS, AND THE DOORS HAD NEVER MOVED.

`step_doors` existed, `Door::update` animated travel, `passable()` read it —
and NOTHING TICKED THE CLOCK. Every door in the game has been frozen at
whatever state it generated in, invisibly, because the ship was scenery and
nobody could walk into one.

THE CAMERA HAD TO COME IN. A 4.9 m chase boom is fine on a field and wrong in
a corridor: the eye ends up outside the hull looking at the back of a wall,
which reads as the ship VANISHING rather than as a bad clamp. It walks the
boom in until the eye is in open volume, after the boom and before the shake,
because it is a correction to a position rather than an input to one.

`camera.rs` takes a CLOSURE — "is this point clear" — rather than a reference
to the interior, so it does not learn about ships. That needed `Interior:
Clone`, since a closure cannot borrow the world it is stored beside; eight
cells and nine links is cheaper than a refcount and much cheaper than teaching
the camera a new concept.

AND THE SHIP WOULD HAVE VANISHED ON A MAP CHANGE. `World::generate` returns
`parked: None`, so `load_map` silently unparked the hull — which would still
DRAW, because the mesh slot lives on the gpu, and stop being solid. A hull you
can see and walk through is worse than one that is not there: it reads as
collision being broken. `repark` is now the single place that parks and both
world-building paths call it.

I ALSO ASSERTED SOMETHING FALSE ABOUT MY OWN FUNCTION. The door test claimed
`unseal` animates. It sets `travel = 1.0` outright — it is "the ship is
powered", a state, not an animation. What animates is `depower`, and the test
now measures that instead.

1121 passing.

THE SHIP IS MEANT TO BE TRANSFORMER SCALE, AND THE FIX WAS THE UNIT.

The playtest note was that the car is 0.38 m and the rooms are sized for a
person, so the doorways would be meaningless. Measured: a 70 m hull is 184
BODY-HEIGHTS for the body this engine actually flies. Cavernous, and the
obvious response — make it bigger for bots — is the wrong direction with the
same mistake in it.

"Transformer scale" is a RATIO, not a length. A bot's ship should be as many
bot-heights long as a person's ship is person-heights. So the hull is
`HULL_HEIGHTS = 40` occupants long: 15 m today, 70 m for a person, and 340 m
the day `--rescale` makes the player an 8.5 m bot. Nobody has to remember to
change it, which matters because the whole point of that rescale is that a
hundred numbers move at once and the ones that get missed are the ones written
in metres.

At the current body that is a ship SIX METRES ACROSS and 2.4 m tall — about
five car-lengths and six car-heights, which is something you can drive around
in rather than a cathedral. A test asserts the ratio holds at all three
scales and that the plan does not lose rooms at the bottom of the range: a
lander is a cramped carrier, not a smaller one.

1123 passing.

THE STARMAP TAKES YOU THERE NOW. Plotting a course that changes nothing is a
line of text; pressing a destination loads the map for it and closes the
console, because you are not at the chartroom any more.

`Order::Depart` carries the destination and the arrival report the console
already computed, so the shell does not fly the drop a second time to know
what to say. It goes UP to the shell for the same reason `Sortie` does —
loading a map needs the gpu and a fresh `World`, neither of which a console
has.

STYLES ARE MATCHED ON PHYSICS, NOT ON NAME. None of skydribble's eighteen is
Mars, so this is a stopgap with its name on it — but a style picked for its
fog colour would have to be re-picked the moment the styles change, and one
picked for gravity and surface material will still be roughly right. Ice
worlds get AURORA, worked metal gets FOUNDRY, low-gravity dust gets
PLANETARIUM. A test asserts every destination resolves inside the table and
that they do not all collapse to one — eighteen worlds sharing one map is
worse than no starmap.

DOORS NOTICE YOU. The ship has only ever been all-open or all-shut, so a
doorway was never a MOMENT — you had a corridor or a wall, never a door that
saw you coming. The radius is in hull terms rather than metres, for the same
reason `HULL_HEIGHTS` exists.

AND THAT EXPOSED TWO COPIES OF THE SHIP. Doors animate on `world.parked` and
the hull mesh was built from `aboard.ship`, its template — so the drawn hull
would have disagreed with the one you collide with, which is exactly the "see
it here, hit it there" failure the lift shares a function to avoid. The parked
ship is the truth; the mesh is rebuilt from it, and only when the set of
carved doorways changes.

THE COMPILER NAMED BOTH PLACES A NEW ORDER HAD TO BE ANSWERED, and `menusim`'s
own comment named the risk: "the one path with no headless driver was the one
the shell had just learned to execute". So Depart reports what the shell WOULD
load rather than being left silent.

1124 passing.

THE FLOOR BUG, AND WHY MY EARLIER FIX DID NOT TAKE.

Reported: "they still go through the floor in game". Found: `step_car`'s
per-frame ground contact reads

    let clearance = car.half_h;

while `ride_h` — how far a frame's FEET hang below its origin — sits right
there unused. Every contact test in the driving loop measured to the body's
half-height: grounded, the compression zone, the pushout. A legged frame sank
by exactly the difference and kept driving, because as far as that line was
concerned it was resting on its waist.

I FIXED THREE SITES EARLIER AND MISSED THIS ONE, and the shape of the miss is
the lesson: the three were SEATING — spawn and reset — and this is the driving
loop. So the car was placed correctly and sank the moment it moved, which
reads as the fix not working rather than as a fourth site.

NO TEST COVERED IT. The suite has a `bot_shaped()` fixture and tests that
`ride_h` survives a reset; nothing exercised the per-frame path. A fix that
names a bug in its commit message and leaves a fourth call site is exactly
what an untested invariant looks like.

ALSO: A STARTUP PROOF LINE. "The game runs nearly exactly how it did 100
messages ago" could not be distinguished, by reading, from a change that does
not execute. `[ship] hull 15.2 m, 8 rooms, slot Some(n), parked at 0.0,1.7,0.0`
on stderr settles it in one line, and I should have added it before the first
playtest rather than after the second.

1125 passing.

PARITY TESTS FOR THE FRAME LOOP, AND MY FIRST ONE GUARDED NOTHING.

The bug class is one invariant at many call sites with one missed. There were
EIGHT copies of `if ride_h > 0.0 { ride_h } else { half_h }` — three seating,
the driving loop, two in `main.rs`, two in tests — and a test that GREPPED
`physics.rs` for the literal string to confirm one site still had it. That
guard was a confession: an invariant you police with a text search is one you
should have extracted. It is `Car::stand_h` now, with one definition, and the
grep is replaced by a check that nobody open-codes it again outside tests.

THEN THE FRAME-BY-FRAME TEST. `step_car_only` is the same function the shell
calls at the same fixed step, so a hundred frames of driving is the real loop
rather than a model of it.

AND IT DID NOT CATCH THE BUG. I reintroduced `clearance = car.half_h` on
purpose and the suite stayed green. The threshold asked for half the stand
height: correct code rests at 1.44, BUGGY code rests at `half_h` = 0.90, and
both clear 0.75. I had checked that the fixed code passed and never that the
broken code failed — which is how you write a test that guards nothing, and I
only found out because I tried to break it.

Set against the bug instead — a frame seated at its WAIST rests at `half_h`,
so the line is between the waist and standing — it now fails with

    the frame sank to 0.90: it stands 1.50 tall and its waist is at 0.90

which names the bug in the failure. Verified in both directions this time.

1127 passing.

## Stretch goals (still not in this cut)

Shadow mapping, more shape families (arches, helices, half-pipes), ghost-replay
of your best ring run, photo mode, achievements, OpenDHT global reach for the
session store (the vendored crate's `opendht` feature is the seam).

## Reference-code reuse

Built on the `logscope-c4-273` monorepo's game kits: wgpu 22 / winit 0.29
initialization, surface configuration, depth setup and the redraw-loop pattern
follow `games/clawbound` (app.rs, mesh.rs), and the SDF-collision approach
extends the ideas in its `openworld.rs`/`physics.rs`.
