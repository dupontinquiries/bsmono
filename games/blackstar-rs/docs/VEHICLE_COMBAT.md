# VEHICLE COMBAT — hostiles in the world, cables, the two registers, scale

Written at the end of the c14 session that put the wave in the world. What
is built, what it is for, what is still a note. Nothing here has been seen
on a screen; every number is a labelled guess in the module it lives in.

## What is built

**The field** (`field.rs`). One `enemy::Enemy` per living fighter of the
current wave, slot-indexed against the fight's roster. The fight keeps
authority on HP (`Fight::external_on_b_slot`); the field owns geometry,
line of sight, fire direction and FORM. `Brain::Hostile{slot}` puppets on
the bot list carry it into the renderer, the section damage and the shot
marks. Hostile fire is its own `shots::Shots` pool marked against the
player. The player's body dying during Engage concedes the sortie. The
aim-agnostic strike bridge is gone: a miss is a miss.

**Vehicle mode is the closing move.** A far foe folds (still for
`FOLD_S`, the window to shoot), drives to a LOCKED flank point beside
you, unfolds on arrival, fights. Spark-hunters never fold. Passing
through you in vehicle form is a ram; you ram back in vehicle form at
speed. A Hijack swarmer shoots from the saddle while closing.

**The two registers** (`weapons::Register`). Heavy forms (PHOTON,
TITANIUM, TSUNAMI) stagger a body when one hit is worth `STAGGER_FRAC`
of its health, and stall a fold in progress. Fast forms (WATER, METAL,
WIND) suppress by volume: spread up, advance down, draining when the
fire stops. A body hit mid-fold is caught between forms and takes
`MIDFOLD_EXPOSURE` on everything until it is through. This is the
Dark-of-the-Moon / War-for-the Foundry blend the brief asked for, as
rules rather than as a tone note.

**The cable** (`cable.rs` is the tether; `field.rs` makes it moves).
- *The pull.* A rail rank (`cable_troop`: Blackstar's Dreadwake ranks
  and the Hijack harvester) throws a line at a standing player in the
  open and reels. You are dragged toward it unless you outweigh it, in
  which case it drags itself to you. Folding to vehicle drops the line.
- *The tow.* Your GRAPPLE, in vehicle form, hooks the nearest foe in the
  hood cone. It is dragged behind the car, cannot act, wears road
  damage by the second, and is exposed.
- *The whip.* Turn hard and the body on the end swings; let go above
  `WHIP_MIN_MS` and it flies. It lands staggered, and whatever it lands
  on takes the same impact. A body heavier than the car reverses the
  trade and yanks the car.
- *The hood gun.* Projectile verbs fire fixed forward from the saddle,
  impulse scaled by speed (`saddle_mul`): the faster you are going the
  harder you hit.

**The scale finding** (`grandeur.rs`, `--grandeur`). The source
material's distances written down in one unit — the occupant height —
and the game's own numbers measured against them. Headline: engagement
and reach sit at about NINE TIMES the reference in body-heights, because
everything was authored around a body that is a car. The module refuses
to retune; it prints the table. The architecture rows (archway 4 oh,
corridor 2.5 oh, hangar 20 oh, district 250 oh) are the ones to author
FROM, because a number in oh survives whichever scale fix is chosen.

**The white boxes.** With a real exterior model loaded the room framing
was still drawn from outside. It now draws only when aboard (or when
there is no exterior and the procedural loft needs it).

## What the frame now shows (second pass, same session)

The renderer is the game; these are reasoned from the draw path and the
shader, not seen:

- **Tracers.** Both shot pools flew invisible rounds — nothing drew a
  `shots::Live`. Every live shot is now a stretched emissive sphere along
  its velocity, element-tinted (`element_glow`), hostile rounds pulled to
  a hot red so whose fire it is reads at a glance.
- **Cables.** A thin box between chests, the rain-streak primitive; the
  tow line warmer than a hostile's. `Field::cables()` is the seam.
- **Contact shadows, cheap version (C13-2a).** A flattened dark disc on
  `ground_h` under the player and every bot, shrinking and fading as the
  body leaves the ground.
- **Motion weight (C13-2c).** Hit-stop on rams, whip landings and
  staggers (`Game::hitstop` holds the accumulator); shake proportional to
  damage taken; the lens widens up to 9 degrees boosting at road speed
  in vehicle form (a negative `fov_punch`).

## Hardpoints — the fittings, read off the model in the loader

`hardpoints.rs`. The vehicles are models with their guns, nozzles and
canopies baked in; `tools/find-mounts.py` read some of that offline into
a JSON the game never loaded, in a space the loader's rescale and
re-orientation would have had to be undone against. The in-engine pass
runs on the same oriented per-prim points `turret::find` walks, so the
answers are in draw space. Guns (thin along Z, tip in the front half or
on the deck, RAISED off the surface under the muzzle half — with the
barrel's own sleeve prims excluded; the last two rules are what
separates a barrel from a roof rail, and each was found on the roster),
thrusters (round at the rear face, flaring aft, not on the ground line),
a cockpit (compact, high, forward, centred), popouts on the actual top
surface when nothing is gun-shaped, and a FORWARD HINT: nozzles opening
at the nose with no forward gun to contradict them say the hull is
loaded backwards.

Used by: the saddle's trigger (a real muzzle, alternating a pair), the
player's boost (one flame per nozzle, at the exit, along its aim),
hostile vehicles' flames and fire origin, and the startup print
(`bots: <name> ... forward hint -1.0 <- FITTINGS SAY THIS HULL IS
BACKWARDS`).

`--hardpoints` runs it over the 254 hulls in `assets/bots` headlessly and
`--hardpoints <name>` narrates one. Where the roster stands after this
session's iteration: 91 hulls with guns (8 of 26 tanks; 12 of 27 cars,
which is the remaining false-positive class — curved roofs read as a gap
under a rail), 93 with thrusters, 156 with a cockpit, 10 called
backwards: `flyer_alphastar`, `flyer_resonator`, `flyer_wayside` among
them — the first two have their nozzles at -Z in the oriented frame,
which is either a wrong orientation guess in `model.rs` or a hull posed
tail-first. Not applied to the orientation search; reported for it.

## Where the seams are

- `field::Field::step(dt, &Player, ground, los) -> Vec<Event>` is the
  whole contract. The shell applies events; the field never touches the
  player or the renderer. `raidflow` can drive the same field for raid
  troops when its own kill bridge retires — nothing in `field.rs` knows
  it is a sortie.
- `Sortie::strike_slot(slot, dmg, elem, Register)` is the one aimed door
  into the fight: exposure, ring and the register's answer all happen
  there.
- `cable::Cable::step(dt, &End, &End, Mode) -> (Pull, Fate)` is the
  tether with no opinion about who the ends are.

## Not done, in order

1. **A cable you can see.** `FxKind::Cable` exists inside the weapon fx
   and is not reachable from `tick_field`; a hostile's line on you is a
   burst and a toast. Needs a persistent line primitive in `fx.rs`.
2. **Vehicles in raid mechanics.** Two concrete proposals against
   `raidflow.rs`'s own debts: SYNC plates can be held by a body you
   THROW onto them (the whip as the seat-holder the squad ghosts stand
   in for), and AIR nodes can be reached by a squad-mate's whip. Both
   are `field::Event::Thrown` landing on a `raidrun::Beacon`.
3. **Open spaces and travel.** The field musters inside `SITE_AHEAD_M`
   of the LZ and nowhere else, so the rest of a map stays open — but the
   maps are skydribble's eighteen (`docs/UNWIRING.md`) and the ship parks
   at the origin. The vehicle side of the game wants a map authored in
   oh from the `grandeur` rows: a district of 250 oh with a highway
   through it. `procgen::World` is the place; the bridge to
   `world::World` is the known gap.
4. **The scale decision.** `--grandeur` and `--rescale` together are the
   brief for it. Not a wiring job.
5. **Atmosphere.** Not touched. C13 §2 has the list; contact shadows are
   the largest lever and none of it can be judged without a screen.
6. **Transform glitches.** The fold strips are per-chassis data; the
   field now shows them on hostiles too, which will make the bad ones
   visible from further away. `foldaudit` is the tool.

## Verification

`cargo test --no-default-features --bin skydribble` under
`tools/fastcheck`. The new tests pin the described behaviour (a foe that
folds, drives, flanks and unfolds; a ram that lands once per pass; a
tow that drags and a whip that throws; a stagger, a suppression, a
stalled fold; the headline scale ratio as a finding). Several were
written first and failed on the previous behaviour — the flank spiral
and the melee standoff were both found that way. No GPU, no controller,
no audio here.
