# ELEMENTS.md — elemental items as field brushes

The north star: an elemental item is **not a scripted effect, it is a brush
that perturbs a field**. You don't code "fire spreads" — you inject heat into
the temperature field and the existing simulation (the climate triangle,
water settling, weathering) carries it the rest of the way. This is the same
"field is authoritative" rule that governs weather and water in
`CLIMATE_PATH.md`, applied to gameplay: items write into the fields; the
world's response emerges from how those fields are coupled.

The payoff is that the elemental rock-paper-scissors is **emergent, not a
type chart**. A fire item is weak in a swamp because the humidity field
literally soaks the heat into evaporation; a water-drain item is useless in
the ocean because `settle_step` refloods the hole. Positioning and
environment become the strategy, and because fields persist and settle,
effects leave aftermath.

---

## The substrate — what an element can push

From the climate triangle (`CLIMATE_PATH.md`):

```
        temperature (scalar)
            /          \
   convection            phase change
   (warm rises → wind)   (vapor ↔ liquid ↔ ice)
          /                  \
   wind (vector) —— transport —— moisture (scalar)
```

Concrete fields/systems an item can write into, and whether they exist today:

| Field / system            | Where                              | Status        |
| ------------------------- | ---------------------------------- | ------------- |
| temperature (scalar)      | `WorldModel::temperature`          | exists        |
| humidity / moisture       | `WorldModel` humidity              | exists        |
| pressure (scalar)         | `world::pressure_sampler`          | exists        |
| water depth + settle      | `world::water_field`               | exists (inert)|
| water absorb (drain)      | `water_field::absorb_at` (ship 16) | exists        |
| weathering state          | `world::weathering`                | exists        |
| material response         | `world::material_response`         | exists        |
| weather (rain/fog/snow/⚡) | `world::weather`                   | exists        |
| seasons (temporal climate)| `world::season`                    | exists        |
| frost / subvoxel layers   | `world::subvoxel`                  | exists        |
| **wind as a vector field**| (climate triangle, planned)        | **roadmap**   |
| **erosion (flow→weather)**| water roadmap step 4               | **roadmap**   |
| **evaporation→rain cycle**| water roadmap step 5               | **roadmap**   |
| **weather fronts**        | downstream of wind vector field    | **roadmap**   |

So Water and Fire verbs are reachable now (humidity + temperature + the water
field already exist); Air and the structural side of Earth wait on the wind
vector field and erosion step 4.

---

## Water — moisture & the water field

The verb is **move moisture around**, in either direction.

- **Drain** — `absorb_at` already removes water-column depth (the ported
  `SuperSponge`). Its sibling drains the *humidity* field, which is the
  interesting one: the weather model gates clouds/fog/rain behind a humidity
  threshold, so a moisture-drain item carves a zone where weather *can't
  form* — a forced-clear bubble that spreads as diffusion pulls moisture from
  the edges. Sustained, it's a creeping drought.
- **Flood / humidify** — inject depth (it `settle`s into a pool) or raise
  humidity until fog rolls in, then rain.
- **Freeze** — a cold water item drops temperature on wet cells →
  `snow_fraction` flips, subvoxel frost forms, `material_response` friction
  drops (slippery).

Emergent counterplay: useless where `settle_step` refloods (the ocean);
strong in arid terrain; the natural opposite of Fire on temperature+humidity.

Reachable now (humidity/temperature/water field exist). Drain is shipped.

---

## Earth — terrain density & weathering / erosion

The verb is **age or harden the ground** (your "erosion weaknesses").

- **Accelerate weathering** — blocks carry a weathering state (the stone →
  gravel → sand → silt chain). Push it forward on a target wall and it
  *crumbles* — you age a fortification to dust instead of mining it, or read
  the existing weathering to find the weak seam and collapse along it.
- **Petrify** — the inverse: lower the weathering rate / raise erosion
  resistance, fossilizing terrain so water can't carve it. Defensive earth.
- **Raise / lower terrain** — perturb the density field (the same density
  that worldgen uses). Raising dams a river; lowering cuts a channel — earth
  redirects water by reshaping its floor.

Emergent counterplay: earth wins against water *structurally* (a dam) but
loses to *sustained flow* (erosion step 4: high-flow cells weather their
neighbors). A slow back-and-forth rather than a coin flip.

Partly ahead: `weathering` exists, but the flow→erosion coupling (step 4) is
roadmap, so the water-vs-earth dynamic lands when that does.

---

## Fire — temperature

The verb is **inject heat**, and the cascade is the point.

- Melts snow → water; evaporates the water field (feeding humidity that may
  rain *elsewhere*); burns off local moisture into a dry, flammable patch.
- **Convection → thermals.** Heat makes air rise (the triangle's
  temperature → convection). Once wind is a vector field, a fire item creates
  an *updraft a flier can ride* — see the dragon synthesis below.
- **Ignition** via `material_response` flammability: sustained heat over fuel
  becomes a self-propagating temperature wave, gated by humidity (wet wood
  resists). Fire spread is not scripted — it is the temperature field plus a
  fuel mask.

Emergent counterplay: weak in high humidity/water (heat dumps into
evaporation and never clears the wet ceiling), brutal in drought. Pure
opposition to Water.

Reachable now for the heat/evaporation/dry verbs (temperature + humidity
exist); the thermal→lift loop waits on the wind vector field.

---

## Air — wind & pressure

The verb is **transport**, not creation. Air doesn't make a field; it *moves*
the others'.

- **Gust / advect** — inject wind and you carry everyone's perturbations
  downwind (the triangle's wind × moisture → transport): blow your own fog,
  disperse an enemy's heat plume, scatter a storm.
- **Pressure fronts** — drive the weather front model (high/low pressure),
  summoning or dispersing storms.
- **Flight interaction** — wind feeds straight into `world::flight`: a
  tailwind is a boost, a downdraft grounds a flier. Air is the natural
  anti-dragon tool, and the pro-flight tool for the player.

Emergent counterplay: counters everyone by dispersing their fields, but
creates nothing lasting — strong tactically, weak strategically.

Mostly ahead: depends on the wind vector field (roadmap). The flight hook
(ship 20) is already there to receive it.

---

## Storm / lightning — a synthesis, not a fifth element

A storm item doesn't add a new field; it forces the *weather state*: raise
humidity + cloud cover past the storm gate (`world::weather`) and lightning
follows (rain-gated, as the model already does). A strike is a temperature /
ignition spike at a point — fire by proxy — plus the flash. It needs
moisture, so it's suppressed in a drought (a fire or water-drain item
disarms your storm) and synergizes with a humidify-first play.

---

## Counterplay is emergent (no type chart)

The interactions fall out of *which field each element pushes* and *how the
fields are coupled* — there is no table to maintain:

- Water ↔ Fire: opposite signs on temperature + humidity. Each is weak where
  the other has set the field state.
- Earth ↔ Water: earth dams (density), water erodes (flow → weathering).
- Air ↔ everyone: advection disperses the others' perturbations.

And because fields persist and settle, every effect has **aftermath** — drain
a swamp and it stays dry until rain refills it; erode a cliff and it doesn't
un-erode. The strategy is to set up the field state, then bring the element
that the state favors.

---

## Elemental armor — stat stack + field bubble

Your Ruby / Solar / Taydon armor splits cleanly across two systems:

- **Stats** → the attribute modifier stack (`world::attributes`, ship 18).
  Flat/percent resistances and bonuses go in as `Modifier`s from an
  `"armor:<slot>"` source, removed on unequip.
- **World** → a small projected *field bubble*. Solar armor carries a warm
  temperature bubble (immune to freeze, melts snow underfoot); a water set
  carries a humidity bubble (resists fire, damp trail). Same field-brush
  idea, attached to the wearer instead of an item use.

---

## The dragon as the synthesis

A fire-breathing dragon is the cleanest expression of all of this: a moving
heat-injection brush whose breath cone dumps temperature, dries the area, and
ignites fuel — the world responds through the same fields, no bespoke dragon
code. An ice-drake is a heat *drain*. The pieces compose:

- `world::pathing` (ship 21) finds the route through the cave,
- `world::flight` (ship 20) flies it with banking dynamics,
- the **breath cone** (next ship — a cone of cells from the mouth, built on
  the ship-15 raycast + ship-17 shapes) is the brush,
- the elemental field write is the effect, and a fire dragon literally rides
  its own thermals (Fire → convection → updraft → flight) once wind lands.

---

## Honest status & build order

What's reachable now vs gated:

- **Now (fields exist):** Water drain (shipped) + humidity drain; Fire heat /
  evaporation / dry / ignition; the breath-cone geometry; the armor field
  bubble against temperature/humidity.
- **Needs roadmap fields:** Air (wind vector field), Earth structural
  (erosion step 4), thermals→lift (wind), storm fronts (wind).
- **Needs the integration gate** (`blocks.rs` / `bin/` / `entity.rs`): any
  item *use* that reads picking + applies a field write in the running game,
  and equipping armor.

Suggested order, building only on what exists: (1) a generic **field-brush
op** — write a delta into temperature/humidity over a shape (reuse the
`area_mining` shapes), the primitive every elemental use shares; (2) **Fire**
and **Water (humidity)** verbs on top of it, since their fields are live; (3)
the **breath cone** as the dragon's brush; (4) **Earth** and **Air** as their
roadmap fields (erosion step 4, wind vector field) come in.

All default-off / inert until wired, like the rest of the gameplay layer.
