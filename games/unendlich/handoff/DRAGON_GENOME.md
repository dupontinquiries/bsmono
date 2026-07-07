# DRAGON GENOME — the viewer's parametric space IS the gene range
Decision (user): the morphospace exposed in `dragon_forge_3d.html` is the canonical dragon genome.
Every slider's [min,max] is the gene's expressible range; a genome is those values normalised to
[0,1]; expression reproduces the phenotype the viewer shows. The viewer is the authoring tool — a
"copy genome" export can dump any dialed-in dragon as a gene vector (offered).

## Genes (normalised 0..1 unless noted) → viewer slider → phenotype
BODY
- bulk, neck_len, neck_arch, tail_len, leg_len, wing_span, wing_spread — torso/limb proportions.
- pose_bias (viewer "Pose") — resting lean from quadruped→reared; gait modulates around it.
SKULL / FACE (Darwin's-finch beak morphospace)
- muzzle_len, jaw_depth, snout_curve(−hook..+hook), brow, cheek, teeth, crest — bone-box face.
- horns (quantised 0..4 pairs), horn_sweep(−fwd..+back).
ELEMENT (discrete: Wood|Water|Air|Dust|Fire|Storm)
- drives base colour + emissive regions (eyes/neck/spine; r7 `emissive`) + breath/effect type.
- crossings via `hybrid_rules` (r9); the `Signature` link is `dragon_signature` (r2).
MATERIAL (new — this stream)
- armor → reflectivity + specular power (shader; core `armor_reflect`). More armor = glossier/harder.
- wing_opacity → membrane alpha: 0 = glassy/translucent, 1 = opaque leathery (core `wing_opacity`).
- scale_gloss → micro-spec on scales.
ANIMATION (new — this stream; see ENTITY_ANIMATION.md)
- breathe_rate, breathe_depth — chest swell.
- vocal_expressiveness — how far mouth/neck move on sound.
- blink_rate, claw_curl_tendency, ripple_amount — idle/secondary motion.

## Inheritance & expression
- breeding: per-gene blend via `egg_genome::breed` (r8); element via `hybrid_rules` (r9); quantised
  genes (horns, element) pick a parent rather than averaging.
- expression: a genome → phenotype struct; **batched** for fast mob spawning via `batch_gene`
  (r14, bit-parity microopt) so a swarm spawns without per-mob scalar cost.
