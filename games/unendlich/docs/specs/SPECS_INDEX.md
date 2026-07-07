# Design specs — incoming (ingested r58)

Reference design docs (not code). They drive upcoming sim/rig work; modules built
from them should cite the relevant section. Saved as-is (docx).

| Doc | Covers | Likely future modules |
|---|---|---|
| `fauna_behaviors.docx` | Animal roster (20 species), behavioural FSMs (passive/neutral/hostile/specialist), ecological roles, drop tables | generalize the dragon behaviour cores to a fauna stack; mob AI; loot tables |
| `fauna_locomotion.docx` | Body proportions, joint topology, gaits, ground/air/water locomotion — animation-rig reference for all 20 species | generalize `dragon_forge`/`dragon_locomotion` rig + the pixel-peeper to non-dragon archetypes |
| `flora_plant_tree.docx` | Plant/tree catalogue, ground/aquatic/cave flora, growth mechanics, env requirements (H/R/W/P/L/S/B) | extend `plant_*` (root/propagation/phenology/canopy) + tree structure |
| `flora_update_genetics.docx` | Combustion & dead-wood (revised burn), mega trees, tree genetics & pollination | tree genetics/pollination module; combustion/fire-spread; replaces flora §2–4 |
| `resonance_bending_upgrade.docx` | Four architectural disciplines — Stormstrike, Heavy Brine, Thermal Phase, Information Bending — + Ryujin Triad faction | new bending disciplines on top of `gesture_resolve`/`affinity_access`; faction logic |
| `river_systems.docx` | Hydrology, 7 river archetypes, bending integration, dragon thirst, swimming + camera, shaders, terrain gen | river/hydrology sim; dragon thirst hook into vitals; swim controller; shore terrain |
| `textures_overlay_system.docx` | 16×16 natural surface tiles, environmental overlay pipeline, property-driven blending | texture/overlay selection (property-driven), tile atlas — note: rendering, mostly bin-side |

Ingest note: these augment existing systems (fauna extends the dragon rig/behaviour
stack; flora extends `plant_*`; bending extends the resonance/gesture stack; rivers
add hydrology + a dragon-thirst hook). No code changes from ingesting — they're the
spec backlog. Cite the section when a module is built from one.

## Added r60
| Doc | Covers | Likely future modules |
|---|---|---|
| `resonance_bending_spec.docx` | **Resonance Bending v2.0** — complete player discipline reference (Stormstrike, Heavy Brine, Thermal Phase, Information Bending); no hotbar/cooldown/chi-bar — resonance-driven | base spec the bending stack (`gesture_resolve`, `bending_stormstrike`, future Heavy Brine / Thermal Phase / Information Bending) builds against |
| `cave_biomes.docx` | Seven subterranean biome archetypes — physics, acoustics, survival, bending, ecology, shaders, terrain; caves as underground *climates* (geology/groundwater/geothermal) | cave biome property model; acoustics (composes `sound_masking`); cave flora/fauna placement; geothermal/groundwater fields |

## Added r64
| Doc | Covers | Likely future modules |
|---|---|---|
| `legendary_items_dragon_artifacts.docx` | Legendary items & dragon artifacts | itemization; dragon-drop tables; artifact effects on the bending/affinity stack |
| `dragon_voice.docx` | Dragon vocalization (roar/call/breath audio) | dragon voice synthesis params; ties to sound_mix + the (pending) audio backend |
| `animal_voice_lauscher.docx` | Fauna/Lauscher vocalization | per-species voice/call model; fauna audio |
