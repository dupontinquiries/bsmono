# West-African biomes & foliage — field studies → worldgen rules

Source material supplied by the operator (2026-08-30), both in
`/mnt/user-data/uploads` this session:

1. **Bâ et al.** — *Diversité des champignons ectomycorhiziens … des arbres
   forestiers natifs d'Afrique de l'Ouest* (IRD). Used here for its Fig. 1
   phytogeographic ladder and habitat/species tables.
2. **Mühlenberg, Galat-Luong, Poilecot, Steinhauer-Burkart & Kühn (1990)** —
   *L'importance des îlots forestiers de savane humide pour la conservation
   de la faune de forêt dense en Côte d'Ivoire*, Rev. Ecol. (Terre Vie) 45.
   The Comoé forest-islands study; the richer of the two for worldgen.

## What the papers give us, in game terms

**The biome ladder (Bâ Fig. 1).** Four belts from equator northward:
dense humid forest (guinéo-congolaise) → open woodland "forêt claire"
(soudano-guinéenne) → wooded savanna (soudano-zambézienne) → wooded steppe
(soudano-sahélienne). Existing biomes already cover the ends (Jungle,
Savanna, HighlandSteppe/Desert); the distinctive missing rung is the
**forêt claire** — an open woodland that is neither forest nor savanna.
Deferred; noted below.

**Tree silhouettes (both papers + operator notes).** The savanna belt's
identity is architectural, not just botanical: umbrella-crowned acacias
(flat, wide, thin crowns over clear trunks), baobabs (the trunk IS the
tree), and a real understory stratum. → **r604, LANDED**: `world/tree_form`
with `CanopyForm::{Dome, FlatTop, Baobab, Shrub}`; Savanna now grows
FlatTop umbrellas. Dome is pinned byte-identical to the pre-r604 shape so
all other biomes are untouched.

**Forest islands (Mühlenberg et al., the core finding).** In the humid
savanna, dense forest survives as ISLANDS (~1–300 ha) plus gallery forests
along rivers. Three findings translate directly:

- *Three strata* (their Tableau III): dominant emergents (D), dominated
  mid-canopy (d), understory shrubs (o). → island vegetation is placed in
  three height/form classes, not one tree type.
- *Islands are floristically distinct from each other* — neighbouring
  islands 6–9 km apart share at most **8 %** of tree species (their
  Fig. 5). → each island rolls its OWN palette from a pool, rather than
  sampling one global mix. This is the single most game-visible rule:
  walking from island to island should feel like visiting different
  forests.
- *Termite mounds seed micro-islands* (their pioneer hypothesis: old
  termitières are mineral-rich, aerated, fire-protected). → rare 1-tree +
  shrub-ring micro-clusters scattered in open savanna, independent of the
  big island field.

→ **r605, LANDED**: pure `world/forest_islands` (island field with jittered
cell centres, per-island palettes, termite micro-clusters, D/d/o strata at
20/35/45), applied on Savanna columns in BOTH backends (CPU branch +
line-for-line GLSL twin gated by transliteration tests — the r604 lesson).
End-to-end pinned CPU-side: an island-core chunk carries ≥2× the tree
blocks of open grass under the same seed. VISUALLY UNVERIFIED in-sandbox —
both r604 silhouettes and r605 islands await the operator's on-device art
check together.

## Caveat discovered while gating r604 (the noise fork)

The tree system exists twice (Rust `cpu_generator` + GLSL
`chunk_generation.comp`). Building a worldgen-parity probe for the r604
mirror surfaced a PRE-EXISTING fork: the shader computes noise in f32, the
CPU in f64, and over land the two backends disagree on ~0.5 %+ of blocks
(surface ±1 along contours; seeds with near-origin shorelines show mass
divergence immediately; a separate deep-band mismatch exists at y-chunk 1
even at the origin — fingerprint world (0,32,0) cpu=Stone gpu=Water). The
"byte-identical" claim in `terrain_generator.rs` holds only over
boundary-free terrain (flat ocean). Multiplayer implication: join-sync's
"same seed ⇒ same terrain" carries a same-backend caveat. The r604 mirror
is therefore gated at the UNIT level (a literal GLSL transliteration test
in `tree_form.rs`), and the probe (`UNENDLICH_PROBE_PARITY=1`, `_FAR=1`,
`_SEED=`) remains as the drift instrument. r605 must add its island field
identically on both sides AND accept that column-level agreement is only
as good as the underlying fork allows.

## Deferred, with reasons

- **Forêt claire biome**: a new biome id (append-only, next = 29) touching
  classification in both backends — worth doing together with r605 so the
  island work lands in its true home belt. Not before the operator sees
  r604's silhouettes on device (art direction checkpoint).
- **Gallery forests along rivers**: needs the river/water-flow field as a
  placement input; the coupling is designed but untested. After islands.
- **Noise-fork fix**: its own project. Candidate directions, in order of
  plausibility: CPU noise dropped to f32 to match the shader (changes
  existing CPU worlds — needs a world-version gate), shader consumes
  CPU-precomputed noise tiles, or f64 emulation in GLSL (slow).
