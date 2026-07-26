# ART BIBLE — Cybertron: Arena of Gods
## IMAX-grade visual direction, sourced from the DOOMSDAY / DOMINION / DESTINY treatments

This document is the reference layer between the screenplay project
(`transformers_blackstar`) and the game. Every visual pass below cites the
film moment it's drawn from, states the *feeling* the shot is doing in the
film, and translates it into concrete, buildable geometry/light targets for
our engine (box/spire/glow vocabulary, `collide=False` visual layers, per-map
key/fill lighting in game.c).

Guiding rule, from the storyboard's own language: **"Minimal dialogue.
Maximum gravity."** Detail passes should add *gravity*, not clutter. One
enormous readable idea per map beats twenty greebles.

---

## 1. THE CAMERA CONTRACT (what "IMAX" means for us)

The treatments frame everything in three registers:

1. **Geological scale** — "A metallic hand, barely visible, sinking. Small in
   frame. Not heroic. Almost geological." Big forms must read from 150+ units
   away: silhouettes first, texture second.
2. **Human foreground** — "Tiny human figures running." Every map needs
   near-field, waist-height detail so the player's 1x scale has something to
   measure the monuments against. (Our low-cover pass in Updates 206-207
   accidentally built this layer; the art pass makes it *deliberate*.)
3. **A single wound of light** — each film scene has exactly one light source
   that matters: the red seam at the beach, the Array's red lens beams, the
   sun-answer over the bridge. Maps should have ONE dominant emissive
   statement, with everything else in support.

---

## 2. MAP-BY-MAP DIRECTION

### 2.1 BEACH — "Earth Splits" (DOOMSDAY, Act II)
Source: *"The ocean goes silent. … The shoreline rips open like a zipper. A
glowing red seam splits the horizon. Lava and metallic structures push
upward. A colossal eye — miles wide — flickers beneath the crust."* And the
Array scene: *"The ocean drains before the LA fissure."*

Film job: the moment the familiar becomes cosmic. Horror through scale.

Build targets:
- **THE SEAM**: one long, thin, molten-red emissive line (non-colliding)
  running the full seaward edge — the horizon "zipper." Not segments: one
  continuous statement.
- **THE EYE**: a wide, dim ember disc half-buried at the seam's midpoint,
  barely brighter than the seam — "flickers beneath the crust," so it should
  read as *under* something: ringed by dark crust plates.
- **DRAINED WATER**: the existing shoreline gets a receded second waterline
  band (pale, non-colliding, flat) seaward of the sand — the drawn-back sea.
- Everything else on the map stays civilian: the PCH, the palms, the pier.
  The wrongness of ONE red line against a normal beach is the shot.

### 2.2 KUIPER — Blackstar's flagship + THE ARRAY (BLACKSTAR storyboard)
Source: *"A massive rotating sphere of shattered Cybertronian crystal. Star
projections flicker across its surface. Deep red lens beams pierce into
space. Fragments of the Chicago transport event are still embedded in it."*

Film job: Blackstar as cosmic archaeologist — he *built something capable of
hearing it*. The Array is intellect made object.

Build targets:
- **THE ARRAY**: a shattered crystal sphere raised on a plinth aft of the
  flagship mast (crystal texture, non-colliding upper half so the silhouette
  is jagged, colliding plinth), with **deep red lens beams** — thin, tall,
  non-colliding red emissive shafts piercing upward at slight angles.
- **EMBEDDED FRAGMENTS**: 3-4 small pale-blue shard boxes (plate_blue — the
  Chicago transport-event material) socketed into the sphere's surface.
- The beams are the map's single wound of light; the existing hazard-mast
  becomes secondary.

### 2.3 CHICAGO — the aftermath + the transport-event scar
Source: the Array holds "fragments of the Chicago transport event" — so the
event itself left physical, glowing debris in the city. Dominion overview:
Earth cities are "key cities for Blackstar's reverse-interface."

Film job: the wound that never fully healed; the place the record begins.

Build targets:
- **PILLAR SHARDS**: a small field of canted pale-blue crystal shards
  (plate_blue + faint glow, non-colliding tips over small colliding bases is
  NOT needed — keep fully non-colliding, they're memorial not cover) near the
  memorial, marking where the space-bridge event tore through.
- One shard taller than the rest, leaning — the "flagpole" of the scar.

### 2.4 OUTER_RIM / PLUTO MAPS — the Solar Defense Grid [IMPLEMENTED: pluto]
Source: *"A massive Autobot-constructed weapons lattice orbits Pluto —
retrofitted Cybertronian and human tech."* Dominion: "Scattered Autobots
defend mining stations, asteroid bases, and abandoned tech."
Shipped on pluto: a 3x3 overhead truss lattice with energon weapon nodes at
the nine crossings, and retrofit material mixing (rust patch-plates + live
energon seams) on the corner towers. Future pass: extend the retrofit
vocabulary to outer_rim + shield_edge.

### 2.5 LUNA — Apollo photography (already landed, Update 199)
The existing "BLAZING key, near-black fill, black noon sky" treatment is
exactly the IMAX register; no change. Listed here so the bible is complete.

---

## 3. PLAYABLE CHARACTER DIRECTION

Source register: the films describe bots by *era and burden* — Prime
"weathered idealist," Blackstar "cold architect." Our parts pipeline
(phead/pchest/plimb/pbevel sculpts) renders every bot from shared templates,
so character detail passes multiply across the whole cast.

Standards ladder (each rung = one gen_props pass):
1. ~~Blocked forms~~ (shipped: base part meshes)
2. ~~Signature sculpts~~ (shipped: robot_head, robot_chest)
3. ~~Articulated limbs~~ (shipped Update 204: cuffs, hinge, armor strip)
4. ~~Panel language~~ (shipped: pauldrons; chest/head already carried vents,
   visor, crest from earlier passes)
5. ~~Weathering~~ (shipped: deterministic per-panel paint wear in
   robot_parts.c — hash of part slot + alt-mode, 0.94-1.06x, applied under
   the accent pass so accents read as fresh paint over worn plate). Future:
   faction-specific decals (corsair naval markings) would need texture work.

Rules:
- Detail must survive the taper: attach greebles relative to local radius
  (the Update 204 lesson), never absolute sizes.
- Silhouette first: nothing added may blur the four family head-shapes
  (Update 205). Panel detail lives INSIDE the silhouette.

---

## 4. VERIFICATION DISCIPLINE

Every art pass in this bible ships only with:
- regenerated data (`gen_props.py` / `gen_world.py`) — verify vert deltas
- `map_score.py` re-run on touched maps (visual layers are non-colliding and
  must NOT move collider scores; a moved score means a leaked collider)
- full ctest green
- an honest note in SIGNATURES.md for anything eyeballs-only
