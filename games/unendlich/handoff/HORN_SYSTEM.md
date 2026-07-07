# HORN SYSTEM — exotic horns, ability charging, obsession expression
From the user's idea-stream: "horns could have [rare] types in the 8-24 range with exotic charging
powers for abilities and weird fractal designs or parts of their obsession genomes built in."
Extends DRAGON_GENOME.md (the viewer's `horns`/`hornSweep` dials) into a functional subsystem.

## Two tiers
- **Common (0-4 pairs):** the current viewer range — simple curved horns, cosmetic.
- **Exotic (8-24, rare):** rolled rarely (rarity-gated like item affixes). Exotic horns are
  fractal/branching (antler/coral-like, recursive), and they are **functional**, not just shape.

## Horns as ability conduits ("exotic charging powers")
Exotic horns channel and **charge** an ability — an elemental breath/blast or a special power.
- charge accrues over time toward a **capacity**; both **rate** and **capacity** scale with horn
  complexity (count × branch depth). Discharge fires the ability and resets charge.
- the ability charged is tied to the dragon's ELEMENT (Wood/Water/Air/Dust/Fire/Storm) and to its
  obsession (below). This makes a dragon's horns *read* its power at a glance.

## Horns express the "obsession genome"
Creatures carry an **obsession** (a personality fixation — hoard / hunt / build / territory / song,
ties to docs/experience-brain). Exotic horn form encodes the obsession: e.g. hoarders grow dense
crystalline clusters, hunters grow forward-swept blades, builders grow lattice-like branches. The
obsession also biases behavior drives (what the creature seeks), so horns are a visible tell of an
invisible drive.

## Genome additions (normalized; quantized where noted)
- `horn_type` (quantized): tier + archetype. Common 0-4, rare exotic 8-24; archetype picks the
  branch pattern + charge affinity.
- `horn_fractal`: branch_factor, recursion_depth, sweep — the recursive geometry params.
- `horn_charge`: ability_id (element/obsession-derived), charge_rate, capacity.
- `obsession`: the fixation gene → drive weights (seek-food / hoard / build / defend-territory / sing).
Inheritance via egg_genome::breed (r8) + hybrid_rules (r9); the exotic tier is rarity-gated so most
offspring stay common.

## Pure cores (queued — R20)
- `horn_type` (sim): horn-type gene + rarity roll → archetype descriptor (tier, count, branch params, charge affinity).
- `horn_fractal` (render/geom): archetype + depth → recursive branch segments (positions/dirs/scales) for the mesh/viewer.
- `horn_charge` (sim): charge accrual to capacity + discharge → ability trigger. Deterministic.
- `obsession` (sim): obsession gene + context → drive-weight vector (which behavior dominates).

## Viewer follow-up (offered)
Extend the `horns` dial in dragon_forge_3d.html into type (common↔exotic) + count (0-24) + fractal
branch controls, and render exotic branching via `horn_fractal`, so exotic dragons are authorable.
