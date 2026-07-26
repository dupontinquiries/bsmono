# FOSS modelling pipeline (Blender -> game)

The game's parts are IQM meshes. Everything can be generated procedurally
(`gen_spine.py`, `gen_world.py`, `gen_props.py`), but you can also hand-model assets in
**Blender** (or any FOSS DCC that exports glTF/OBJ) and drop them straight in. There are
two import paths and one "override" hook.

## Path A -- glTF / GLB (recommended)

glTF 2.0 is the FOSS interchange standard and Blender's cleanest export. It keeps normals
and UVs and bakes node transforms.

1. Model the part in Blender. Keep it roughly centred; scale is normalised on import.
2. `File > Export > glTF 2.0 (.glb/.gltf)`. Defaults are fine (Blender converts its
   Z-up to glTF's Y-up automatically, which is what the engine wants). Triangulate is
   applied on import, so you don't have to.
3. Convert:
   ```
   python3 tools/gltf_to_iqm.py head.glb --out engine_src/data/phead.iqm --tex hexmesh.png
   ```
   - `--tex NAME.png` picks the material texture from `engine_src/data/textures/`.
   - `--raw` keeps your exact coordinates (skip the unit-slot normalise).
   - `--z-up` only if you disabled "+Y up" on export.
4. Rebuild (the `data_archive` CMake target repacks `engine_src/data/` into `data.ex`).

Supports `.glb` (single binary file) and `.gltf` + external/embedded buffer, multiple
meshes/primitives (merged), and the full node hierarchy (transforms are baked to world
space).

## Path B -- Wavefront OBJ (simplest)

For a quick static shape with no rig:
```
python3 tools/obj_to_iqm.py head.obj --out engine_src/data/phead.iqm --tex hexmesh.png
```

## The override hook -- replace a procedural part

The robot is assembled from named part templates the engine loads by filename:

| file (in `engine_src/data/`) | part |
|---|---|
| `phead.iqm`   | head |
| `pchest.iqm`  | chest / cockpit |
| `plimb.iqm`   | arms + legs (tapered limb) |
| `psphere.iqm` | shoulder / joint spheres |
| `phand.iqm`   | fists |
| `pbevel.iqm`  | chamfered torso + feet |
| `spine_*.iqm` | signature back-pieces (wing-cannon, harpoon, crest, ...) |

**Export a Blender mesh to one of those filenames and it overrides the procedural part
everywhere** -- for every troop, chassis, and the cinematic styling layer still applies
on top (limb scaling, posture, visor, greebles). No code change needed. Keep the mesh
inside the ~2-unit part slot (the importer normalises to it unless `--raw`) so it lines
up with the skeleton.

## Adding a NEW model (not an override)

Drop the converted `.iqm` in `engine_src/data/`, then register it in the content layer so
the pipeline knows about it: add it to `content/part_groups.json` (see `content/README.md`),
run `python3 tools/content_build.py --root . --emit-c`, and reference the group from
`chassis.json` / `signatures.json` as needed.

## Conventions & validation

- **Up axis:** Y-up (Blender glTF export handles this).
- **Slot:** ~2-unit cube, centred (auto unless `--raw`).
- **One material** per part (the engine binds one diffuse + its auto `norm_`/`spec_`/
  `emit_` maps by name).
- **Validate** any converted part exactly like a procedural one:
  ```
  gcc tests/test_spine_iqm.c -lm -o /tmp/tsi && /tmp/tsi engine_src/data/phead.iqm
  ```
  The `gltf_roundtrip` ctest also guards the converter itself.

## Normal / spec / emissive maps

The engine auto-loads `norm_<tex>.png`, `spec_<tex>.png`, `emit_<tex>.png` beside the
diffuse. Author them in Blender/GIMP/Krita, or generate a tangent-space normal map from
a height pass with `tools/gen_texture.py ... --norm out.png`.
