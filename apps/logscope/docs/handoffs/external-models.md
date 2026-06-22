# Using external (Creative Commons) 3D models — fish & friends

The renderer is procedural (meshes are built in Rust as `m3::Mesh3`). To bring in real models we now
have a minimal **OBJ importer**: `mesh3d::load_obj_str(src, color, mat) -> Mesh3` (parses `v`, `vn`,
and `f` with `v` / `v/vt` / `v//vn` / `v/vt/vn`, fan-triangulating polygons; missing normals are computed).
`mesh3d::scale3(m, sx,sy,sz)` then sizes it and the same local→world transform used by `add_fish` orients
and places it. Unit-tested in `mesh3d::tests::obj_loader_parses_faces_and_scale3_compresses`.

## Where to get CC0 fish (no attribution required for CC0; CC-BY needs credit)
- **Quaternius — "LowPoly Animated Fish" pack** (CC0): Blend/OBJ/FBX. The cleanest drop-in.
- **Poly Pizza** (poly.pizza): CC0/CC-BY low-poly models incl. fish; OBJ/GLTF/FBX, no login; has a JSON API
  (api.poly.pizza/v1.1) for scripted pulls. Check each model's licence — CC-BY requires attribution.
- **Kenney.nl**: 40k+ CC0 assets. **OpenGameArt**: CC0 low-poly fish (Quaternius mirror).
- `github.com/madjin/awesome-cc0` aggregates the above.
- NOT necessarily CC0: CGTrader / TurboSquid "free" models are royalty-free but licences vary — verify.

## How to wire models in
1. Drop `.obj` files into e.g. `crates/logscope-egui/assets/fish/`.
2. `include_str!` them (or read at runtime) and call `load_obj_str`, then `scale3` + place via the
   `add_fish`-style basis transform. Keep a `FishSpecies`-like table mapping species → model + scale + tint.
3. Record attribution for any non-CC0 model in this file.

Until models are provided, `sea_life()` builds a fully **procedural** bestiary (tuna, angelfish, clownfish,
eel, grouper, ray, lanternfish, whale) from primitives — no assets needed.
