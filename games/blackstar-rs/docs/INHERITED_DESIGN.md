# INHERITED DESIGN — what `game.c` documented

Forty-two systems, lifted out of the C engine's `game.c` before that tree is
retired. This is a HARVEST, not a port, and the distinction is the finding.

## Why this file exists instead of a port

`SALVAGE.md` ended on a caveat: *"`game.c` is 12,509 lines and I have read
perhaps 300 of them. There may be systems worth taking that live inside it
rather than in a named module."* There are, and reading them changed what
taking them means.

The file opens with roughly two hundred lines of design commentary — every
system named, argued for, and usually sourced to the screenplay. Underneath,
the implementations are inline against the C renderer: `AIDEN'S SKATEBOARD` is
two globals and a note that transform "hops on/off, near-double pace, board
rendered underfoot". There is almost nothing to translate. **The value is the
design, and design survives deletion in a way that renderer-coupled C does
not.**

So the code goes and this stays.

## How to read it

Grouped by what they are for. Two warnings:

**Some of these were never built.** `PALE VECTOR` has a paragraph of design and
close to no implementation; others are similarly aspirational. I tried to
classify them automatically by grepping for identifiers and the heuristic was
too crude to trust — "THE SUN ITSELF" matched on the word "the". So nothing
here is marked implemented or not, because a confident wrong label is worse
than none. Assume each is a design that MAY have code behind it.

**The prose is the C engine's own.** Lightly reflowed, otherwise untouched,
including its sourcing notes to the screenplay. Where a system quotes the
script it is quoting the same drafts `docs/STORY_BIBLE.md` settles.

## Ported so far

Done, including the skateboard:
`PALE VECTOR` (`palevector.rs`), `AMALGAM OF THE GATE` + `DISGUISE PROTOCOL`
(`amalgam.rs`), the cling/grip meter (`cling.rs`),
`HALO-STYLE ASSASSINATIONS + STEALTH` (`stealth.rs`), and
`AIDEN'S SKATEBOARD` with human scale (`board.rs`).

`CRAWLER AMBUSH` too (`crawler.rs`), built on `cling.rs`.

`THE RIP` too (`rip.rs`).

Still unclaimed: `LOW-G LOCOMOTION` and the `FRONTIER CLOAK`.

## What jumped out

Reading all forty-two, the ones with the strongest claim on the Rust tree:

- **`AIDEN'S SKATEBOARD`** — a human-scale "vehicle mode". The human campaign
  needs human traversal and here is a designed answer, attached to the LA hero
  who is already in `content::FILMS`.
- **`PALE VECTOR`** — **PORTED** (`src/palevector.rs`, `--pale`). And he was
  not unbuilt: my grep looked for the wrong identifier. `game.c` has forty-one
  references to `g_pvec_*` and a complete update loop, which is a caution
  about the rest of this list — absence of evidence in a survey this shallow
  is not evidence of absence.
- **`DISGUISE PROTOCOL`** and **`AMALGAM OF THE GATE`** — **PORTED TOGETHER**
  (`src/amalgam.rs`, `--amalgam`). They are two adjacent blocks in `game.c`
  and one system: the disguise only means anything against the thing it hides
  you from.
- **`CRAWLER AMBUSH`** — enemies scale nearby structures and pounce from above.
  Wants the grip model, which is now **PORTED** (`src/cling.rs`, `--grip`).
- **`LOW-G LOCOMOTION`** — the body flies in light gravity, pitch following the
  velocity vector, then auto-rights before touchdown. `scale.rs` and the
  procedural worlds make this reachable.

---

### WORLD REVERB

*(`game.c:106`)*

WORLD REVERB (physically-motivated, pre-rendered): interiors + canyon worlds
play WET versions of the percussive one-shots (Schroeder comb tails) -- the
hall answers back.

### HIT CONFIRM

*(`game.c:115`)*

HIT CONFIRM: every real chunk of damage the player lands answers with a tight
tick (rate-limited; per-frame DoT crumbs are filtered) -- "am I damaging it",
answered.

### KILLSTREAKS

*(`game.c:117`)*

KILLSTREAKS: kills inside a rolling 3.5s window chain into callouts. The
counter lives here (before the message globals) and the announcement is
emitted later in the frame where the HUD strings are in scope.

### DUST DEVILS

*(`game.c:154`)*

DUST DEVILS: wandering vortex columns on the desert worlds -- visible spinning
dust that shoves anything caught in the funnel.

### SCATTER

*(`game.c:173`)*

SCATTER-ASSEMBLE (fly in, spinning, staggered per letter -- a transform), then
idle with a bob + energon pulse, and every few seconds a spin WAVE ripples the
word. ----

### THE SUN ITSELF

*(`game.c:182`)*

THE SUN ITSELF: a visible disc riding the planetary sun direction -- the
anchor the god rays, glints, and lens flare all point at. Hidden below horizon
+ indoors.

### PALE VECTOR

*(`game.c:206`)*

PALE VECTOR (source: "The Ancient Relic-Keeper... cloaked. thin. almost
skeletal.") A non-combat apparition on the scavenger worlds: stands far off,
whispers if you come near, WARPS OUT if you press him. He maintains the
relics. He remembers.

### THE SEA RETURNS

*(`game.c:215`)*

THE SEA RETURNS (mission I.3): a real wave wall sweeps the beach west-to-east.
Height is life: the refuge decks + dune crests clear it; the flats do not.

### RAGES

*(`game.c:226`)*

RAGES (fog closes in, wind shoves, devils spin up, dust streaks fly, sensors
choke), then CLEARS. Every world wears its own dust.

### LOW-G LOCOMOTION

*(`game.c:234`)*

LOW-G LOCOMOTION (player direction: "auto align on land, low-g animations, any
orientation possible"): in light gravity the whole BODY flies -- pitch follows
the velocity vector, roll follows the turn -- then AUTO-RIGHTS in the final
approach before touchdown. Smoothed so orientation is continuous, never
snapped.

### AMALGAM OF THE GATE

*(`game.c:256`)*

AMALGAM OF THE GATE -- a warden-gestalt fused from every frame that ever
failed the crossing (the devastator lesson: many bodies, one hunger). Entered
from PATROL on the hollow once the bridge set is charted. Asymmetric horror:
forge 4 RELICS at the forge-stones while it hunts (forging makes you LOUD),
then carry them to the door and BANISH it. Allies draw its eye. Or you get
stuffed out.

### DISGUISE PROTOCOL

*(`game.c:272`)*

DISGUISE PROTOCOL (raid-only prop hunt): crouch + transform [LCTRL+T] to
become an OBJECT -- a cave boulder -- for up to 8s. The amalgam loses you
completely. Moving or firing breaks it. Recharges every 30s ("the gate notices
patterns").

### PATROL SIDE QUEST

*(`game.c:289`)*

PATROL SIDE QUEST -- THE FAR SIGNAL: relay fragments -> the WARDEN bounty ->
the SPACE BRIDGE opens. Walking through it charts a new region: the bridge
campaign set unlocks (campaign.unlocked jumps to the VII missions, persisted).
Repeat runs pay energon credits instead.

### BREACH MISSION

*(`game.c:309`)*

BREACH MISSION (EX_OBJ_BREACH was defined but never implemented -- IV.2 played
as a plain eliminate): a SEALED WARD at the target structure with hp; your
fire within its radius chips it. Destroying it wins, whatever still stands.

### ENCELADUS GEYSERS

*(`game.c:316`)*

ENCELADUS GEYSERS: the map's three vent mounds erupt on a stagger -- a column
of ice-bright plume that LAUNCHES anything standing on it. traversal, not
hazard. One vent per tiger stripe, ON the sulci lines (matching the map + the
real moon: Damascus, Baghdad, Cairo, Alexandria)

### CONTACT SHADOWS

*(`game.c:323`)*

CONTACT SHADOWS: a soft dark blob glued to the ground under each character,
shrinking + fading with height -- the cheap half of ambient occlusion that
grounds everything.

### CHAPTER TITLE

*(`game.c:328`)*

CHAPTER TITLE: at campaign match start the modeled font assembles the
mission's word in-world over the spawn (~5s), then SCATTERS BACK OUT (assembly
reversed).

### OBJECTS

*(`game.c:346`)*

OBJECTS -- small floating bodies with their own pull -- not the ground.
Realistic-ish: inverse-square wells (clamped), surface landing kills radial
speed, the old floor keeps a weak residual pull so low play still works. Non-
flyers get ASSIST LANES: visible dotted transit lines between bodies that
guided acceleration eases them along.

### TITAN METHANE RAIN

*(`game.c:371`)*

TITAN METHANE RAIN: showers ride the wind -- streaks fall slant-wise, splash
on the ground + the seas, and everything goes WET (glossier env reflections +
glint).

### TEXTURED PARTS

*(`game.c:444`)*

TEXTURED PARTS: some imported models are exported as N separately-textured
parts (real baseColor/normal/emissive) instead of the flat placeholder. Table
maps the imported-model index -> its part IQM files. Parts were normalized
together so they share one transform. Models not listed fall back to the
single mesh.

### PER-MODEL CORRECTION HARNESS

*(`game.c:452`)*

PER-MODEL CORRECTION HARNESS: each imported model can carry an orientation +
scale fix (they were authored in different up-axis/facing conventions).
Default identity (rx=ry=rz=0, scale=1) so nothing is made worse; tune live in-
game (arrows/pgup-dn rotate, +/- scale) and the values print for baking into
here.

### KIND-AWARE IMPORTED MODELS

*(`game.c:516`)*

KIND-AWARE IMPORTED MODELS (tables baked by install_models.py from
gen_tuning.py): g_imp_kind (0 prop, 1 robot, 2 vehicle, 3 weapon, 4 env, 5
ship) drives per-category animation; g_imp_pair links vehicle<->robot forms
for the transform.

### TRANSFORM

*(`game.c:523`)*

TRANSFORM (single-mesh models): crush-spin-swap between g_imp_pair partners.
The parts-tagged morph (atag 20) stays the rich path; this is the readable
universal one: squash + fast spin down, swap the model at the nadir, rise into
the other form.

### ARMOR REACTION

*(`game.c:642`)*

ARMOR REACTION: armor/shin-guard parts shift when the body is hit or when the
suspension takes a hard hit (crater/bump). Jolt spikes on impact then springs
back, so panels visibly absorb the blow instead of riding rigid.

### UI LOG

*(`game.c:651`)*

UI LOG: a ring buffer of recent events/changes, shown on-screen (toggle key
D). Every tuning change + notable event is appended here so it can be read
back instead of eyeballing the HUD. Also mirrored to stdout.

### NAMED FRAME VARIANTS

*(`game.c:670`)*

NAMED FRAME VARIANTS (content/bots.json "variants"): same generator, distinct
frames. Index: 0=prime (heroic; the player), 1=corsair (Blackstar warlord),
2=watch (elemental god-form). Slots that differ: head, chest, pauldron.

### ORB RUSH

*(`game.c:783`)*

ORB RUSH (grifball-like): carry/dribble the Orb into the enemy forge. Melee
only.

### STRUCTURAL SKELETON

*(`game.c:1020`)*

STRUCTURAL SKELETON: a dedicated pool for DESIGNED bones + armour, placed
deterministically per frame archetype -- the part of a build's identity the
random greebles never gave. 0..11 sphere joints, 12..29 rib slats, 30..35
armour plates (2 pauldrons + 2 knee + 2 elbow guards), 36..39 SPINAL COLUMN
segments, 40..47 HOLLOW

### LIMB STRUT

*(`game.c:1024`)*

LIMB STRUT-FRAMES (paired rails -- robotic, hollow, cybertronian), 48..49
angled CHEST

### PANELS

*(`game.c:1025`)*

PANELS, 50..51 HIP RING plates, 52..53 SHIN FINS, 54..55 ANIMATED EYES, 56 JAW
BAR, 57..58 BROW PLATES (the face rig, driven by a micro-FSM + LOD), 59..65
DEPTH PLATES (inset secondary panels on chest/abdomen/head/pauldrons/thighs),
66..73 SEGMENTED LIMB

### BANDS

*(`game.c:1028`)*

BANDS (forearm + shin armour wraps with gaps), 74..75 KNUCKLE GUARDS, 76..77
TOE CAPS, 78..79 ANKLE PISTONS (compress on landing), 80..81 BACK BOOST-STACKS
(heat-glow with thrust), 82..83 FOREARM INSET PANELS (dark machined recesses),
84..85 HEAD ANTENNAE -- layered thickness, not flat faces. (The same parts
you'd hand-craft in tools/part_editor.html, composed here from the pool.)

### CRAWLER AMBUSH

*(`game.c:1111`)*

CRAWLER AMBUSH: marked enemies scale nearby structures and POUNCE from above
-- "something is crawling the spire."

### FRONTIER CLOAK

*(`game.c:1168`)*

FRONTIER CLOAK (source material: "Autobot Frontier Cloak & Suit Systems"): on
the cold outer-system worlds the cape is mission gear, not costume -- a
nanotech shape-memory weave with reactive energon threads. Radiation SHIELDING
+ a low SENSOR signature ("stealth drape"), and an icy frontier look on the
cloth itself.

### ENVIRONMENTAL TSUNAMI

*(`game.c:1187`)*

ENVIRONMENTAL TSUNAMI (campaign ch. I.3 "outrun the sea"): a wall of ocean
that sweeps the whole beach on a timer -- flee to the refuge towers or be
swept.

### AIDEN'S SKATEBOARD

*(`game.c:1531`)*

AIDEN'S SKATEBOARD: human-scale "vehicle mode" -- a salvaged deck with energon
trucks. Transform (while human-scale) hops on/off; near-double pace, board
rendered underfoot.

### WEAPON DETAIL RIG

*(`game.c:1777`)*

WEAPON DETAIL RIG: 6 attachment parts riding the held cannon each frame --
barrel sleeve, muzzle brake, energy cell (heat-reactive), top sight rail,
underbarrel grip, rear exhaust vane. "True model" layering for the gun, per-
mode proportions.

### OUTER RIM CAPES

*(`game.c:1791`)*

OUTER RIM CAPES: on the belt maps (outer_rim, kuiper, carrion) the biggest
enemies wear tattered salvage cloaks -- the rim clans' colours -- and the
player's own cape goes full DRIFTER: longer, duller, and it snaps in the live
wind.

### ENEMY DETAIL DRESSING

*(`game.c:1797`)*

ENEMY DETAIL DRESSING: the nearest enemies get emissive EYES (kind-coloured,
LOD), and the biggest units wear SHOULDER SPIKES -- silhouette + threat read
at a glance.

### LA OPENING

*(`game.c:1817`)*

LA OPENING (mission I.2): the skyline tower COLLAPSES on cue, and the camera
falls with it -- down, down, to a kid with a skateboard. The "you play a HUMAN
now" reveal.

### ORBITAL DEBRIS

*(`game.c:1956`)*

ORBITAL DEBRIS: big slow chunks drifting + tumbling around space arenas --
ambient motion on the skyline so the world never feels static.

### PER-CORNER SUSPENSION

*(`game.c:3160`)*

PER-CORNER SUSPENSION: 4 independent spring-damper contacts (FL,FR,RL,RR).
Each corner tracks its own ground height, so a crater or bump under ONE wheel
compresses that corner and tilts the body -- what a single centre spring can't
do. Also the hook the armor/shin-guard reaction layer will read (plan: parts
respond to suspension travel + impacts).

### PREVIEW ORBIT

*(`game.c:4790`)*

PREVIEW ORBIT: drag to spin the bot, wheel to zoom. Momentum carries the spin
after release, and the auto-turntable fades back in once you stop touching it.
