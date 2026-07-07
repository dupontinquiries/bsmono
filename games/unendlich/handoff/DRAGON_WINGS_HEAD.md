# Dragon wings + head (r685)

## Wings — FIXED (membrane added)
Root cause of "weird wings": the voxel forge stamped only the wing BONES
(base→elbow→wrist + 4 finger capsules) and the comment said "membrane is
render-only, omitted from voxels" — but the voxel model is what draws, so wings
read as bare spider legs. The HTML reference (`dragon_forge_3d.html`, lines
297–329) webs the fan with a triangle membrane. r685 adds `wing_panel(apex,a,b)`
which fills each triangle with thin cross-strips (a single box would slab an
elongated triangle) sized to the local gap, tagged with the wing-plane normal so
it binds to the wing joint and flexes. Panels: leading (base→wrist→finger0) + one
per inter-finger gap. Membrane half-thickness 0.045·bulk (≈ finger radius, so it
voxelises). Needs a build to eyeball; tune `wing_thick` / `STEPS` if too heavy.

## Head — matches the HTML reference (no blind change made)
The Rust head is a faithful port: cranium sphere + occipital plate, snout boxes +
nose ridge, lower jaw, teeth, brow ridge, cheek shields, crest, horns, and
emissive eyes — every solid + parameter matches `dragon_forge_3d.html` §331–404
(eyes at `P3(skull, sk*0.55, sk*0.18, ±sk*0.62)`, identical). No obvious geometry
bug found. To improve it I need specifics on what reads "weird": head too big/
small vs the neck? snout too long/curved? jaw gaping at rest? horns/crest wrong?
Each maps to a dial (`snout`, `jawDepth`, `snoutCurve`, `brow`, `crest`, `horns`,
`hornSweep`) or a proportion I can tune once I know the direction.
