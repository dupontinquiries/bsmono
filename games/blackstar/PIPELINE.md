# Model pipeline

Eleven tools turn a folder of downloaded models into animated, detailed, installed
game assets. This is the order they run in, what each needs, and the traps.

All tools live in `tools/`. They find each other automatically, tolerating
versioned filenames (`part_harvest_v9.py` etc.) and picking the highest version.

---

## Order

```
extract_models.py     zips        -> folders (names preserved = classification)
obj_diagnose.py       folder      -> how is this model structured?  (read-only)
library_shrink.py     library     -> smaller library, still re-processable
process_models.py     models      -> single-mesh .iqm  (geometry only)
texture_export.py     models      -> per-part .iqm + textures + scanner tags
part_harvest.py       models      -> part catalog (+ exemplar meshes)
detail_apply.py       one model   -> rib banding from a catalog
limb_split.py         one model   -> articulated limbs (+ optional detail)
vehicle_rig.py        one vehicle -> wheels that roll
model_metadata.py     library     -> classification JSON
install_models.py     parts       -> engine assets + generated tables
```

Nothing after `extract_models.py` works on zipped input. That was the first thing
that went wrong: `process_models` reported "no models found" because everything was
still inside `.zip` files.

---

## Typical run

```bash
# 1. unpack, preserving zip names as folder names
python3 tools/extract_models.py .

# 2. see what you have before trusting any of it
python3 tools/obj_diagnose.py "path/to/a/model" 
python3 tools/obj_diagnose.py . --all

# 3. shrink (destructive -- dry run first, and try ONE category)
python3 tools/library_shrink.py . --target 15G
python3 tools/library_shrink.py bots --apply -j 24

# 4. geometry + textures
python3 tools/process_models.py . --quality ultra --out-iqm iqm
python3 tools/texture_export.py . --out-iqm parts --out-tex parts/textures \
        --cap 1024 --tex-format auto --force

# 5. harvest a part catalog (category-balanced, or the generator starves)
python3 tools/part_harvest.py donors --out detailcat --tol 0.20 \
        --min-tris 50 --emit-per-category 60 -j 24

# 6. build the kit
python3 tools/limb_split.py --batch bots --out kit --detail detailcat
python3 tools/vehicle_rig.py --batch vehicles --out kit

# 7. install (engine_src, not the project root)
python3 tools/install_models.py --iqm-dir iqm --parts-dir kit \
        --engine /path/to/cybertron-cmake-project/engine_src \
        --tuning model_tuning.json
cmake --build build -j24
```

---

## Settings that were measured, not guessed

| setting | value | why |
|---|---|---|
| `--tri-budget` | 250000 | must exceed the top export tier; 20k measurably capped ultra (32k) |
| `--tol` (harvest) | 0.20 | 0.01-0.12 finds ~0 cross-model reuse; 0.30 over-merges |
| `--rib-stretch` | 4.0 | uniform scaling shrank ribs to nothing: 10% -> 45% coverage |
| rib catalog | `roboskelcat` | 44.6% coverage at 35k tris, vs detailcat 27.5% at 156k |
| stride amplitude | clamp 34 deg | unclamped reached 102 deg -- cartwheeling |
| foot counter-rotation | 0.85x | keeps the sole within ~8 deg of level |
| JPEG for colour maps | q92 | 6.4x smaller than PNG, PSNR 35-38dB |
| PNG for normal maps | always | JPEG on normals = visible shading artifacts |

---

## Known limits

- **No cavity to open.** These models are layered panels, not shells around a void.
  Measured on Jazz: 1,550 faces through the torso with 714 *behind* the front
  layer. Cutting the outer skin exposes the next skin. Ribs must be built *on* the
  surface, which is what `detail_apply` does.
- **Detail parts do not transfer between models.** Functional parts (wheels,
  treads, struts) reuse 16x and appear across many vehicles; detail parts (blocks,
  greebles) reuse 1.5x and are effectively per-model.
- **Greebles are not normal-aligned.** They anchor to shell vertices but do not
  orient to the surface, so they read as scattered rather than attached.
  `--greebles` defaults to 0 for this reason.
- **Limbs are rigid segments.** Thigh/shin/foot with single-axis pivots. No hip
  abduction, no ground contact, no IK.
- **Wheel detection is shape-based.** It finds real wheels on cars, and also finds
  round greebles on robots. Verify before installing a bot's "wheels".

---

## Recurring failure modes

**Truncated names silently merge two models into one.** This has bitten in the
browser list, a batch shell command, and the display layer. Symptom: a model you
expected is simply absent, with no error. Names are now made unique at every stage,
and long ones keep their *tail* (the `LMB`/`RIB` suffix is what distinguishes a
variant).

**Wrong `--engine` path.** It must point at `engine_src`, not the project root.
`install_models` now refuses rather than writing to a stray folder -- which
previously produced two "nothing changed in game" sessions.

**Unwelded meshes yield zero parts.** If every triangle carries its own vertices,
connected-component analysis sees each triangle as an island. A skeleton reference
produced 115,386 one-triangle components. `part_harvest` now welds first.

**Multi-part OBJ models.** Some downloads split one model across hundreds of
`model_N.obj` files, and some of those re-declare the same vertex and face blocks.
Parsing one file gives a fragment; concatenating them all duplicates the mesh ~26
times. Both are handled, but run `obj_diagnose` if a model looks wrong.

---

## What has never been verified

Everything above is checked by measurement -- geometry lands where intended,
hierarchies compose, names are unique, builds are clean. **None of it has been
checked by eye.** The one time visual feedback arrived, it revealed that ribs were
sealed inside an opaque chest and the only visible additions were scattered
debris -- neither of which any measurement had caught.

The visibility raster (`ribs frontmost N%`) exists specifically because of that,
but it answers "is this geometry in front" and not "does this look right".
