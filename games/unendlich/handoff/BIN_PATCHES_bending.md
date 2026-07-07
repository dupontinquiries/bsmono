# BIN PATCHES — genetics-derived bending (`sim::dragon_bending`)
# Frame-loop / ability wiring; logic is in the tested lib core.

## What the genome gives you
A dragon's `element[6]` (Wood,Water,Air,Dust,Fire,Storm) projects onto the seven
ambient field channels that gate bending disciplines:
    use unendlich_engine::sim::dragon_bending as bending;
    let signature = bending::dominant_discipline(&genome);   // its strongest discipline (0..6)

## Ambient fields (optional but recommended)
Build the live `affinity_access::Fields` from the environment (weather/biome) so a
fire dragon bends best where it's hot. `env_frame` already has most of this:
    use unendlich_engine::sim::affinity_access::Fields;
    let ambient = Fields {
        h: humidity01, r: heat01, w: wind01, p: pressure01,
        l: lightning01, s: storm01, b: biome_richness01,
    };
    let env_weight = 0.4;   // 0 = innate only, 1 = environment only

## Can it bend, and how hard?
    if bending::can_bend(&genome, ambient, env_weight, discipline) {
        let p = bending::potency(&genome, ambient, env_weight, discipline,
                                 intent01, resonance_state::tier(&res), &focus_mods);
        // `p` (>=0) scales the effect: breath damage, gust force, water push, etc.
    }
- `discipline` indices: 0 humidity/water, 1 heat/fire, 2 wind/air, 3 pressure/dust,
  4 lightning/storm, 5 storm-particulate, 6 biome/wood.
- `focus_mods = bending_focus::focus(&equipped_items)` — imbued items can project
  field (unlock a borderline discipline) and add strength.

## Sustained channelling builds power
Hold a `resonance_state::Resonance` per dragon; advance it while it channels:
    res = bending::channel_step(res, &genome, ambient, env_weight, discipline,
                                stillness01, spent, damage, dt);
    // tier(&res) 0..3 feeds back into `potency` (x1.0..x2.5). Standing still and
    // bending your matched element in a matching environment charges fastest;
    // taking a hit or firing a technique (`spent`) drains it.

## Hooking it to what already exists
- Breath weapon: scale `breath_attack::damage_at(...)` by `potency` of the dragon's
  element discipline, so a well-attuned, resonant dragon breathes harder.
- A `Bending` activity can be added to `dragon_activity` later; for now trigger
  bending from player/AI intent and gate it on `can_bend`.
