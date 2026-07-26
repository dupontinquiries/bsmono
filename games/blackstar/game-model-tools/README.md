# game-model-tools

Shared model-pipeline tooling: ingest a library of downloaded 3D model archives,
trim/compress geometry, extract real textures, classify by name, and generate
raid/patrol content. Used by the Cybertron game and reusable across the monorepo
(e.g. `games/soldier`, `apps/logscope`).

Two parallel implementations:
- **`python/`** — the reference implementation (numpy / trimesh / PIL). Full-featured:
  quadric decimation, quality tiers, glTF + OBJ, per-part texture export with the
  material scanner, parallel (`--jobs`).
- **`rust/`** — a Cargo workspace of native ports for the hot paths, no Python runtime
  needed. Faithful to the Python output (metadata output is byte-for-byte equivalent).

## Pipeline order

```
extract  ->  process (geometry)  ->  texture (parts)  ->  metadata  ->  raid content
```

1. **extract** — unzip the library, preserving each zip's NAME as its folder (the
   names carry classification metadata the later stages route on). Handles nested
   zips and is idempotent.
2. **process** — decimate to a triangle budget + compress, emit `.iqm`. glTF and OBJ.
   Quality tiers: `indie` (8k), `high` (16k), `ultra` (32k).
3. **texture** — split each model by material into per-part `.iqm`s carrying their
   real baseColor/normal/emissive/spec maps, plus a scanner that tags tires/fans/
   doors/armor for auto-animation.
4. **metadata** — classify the whole library from paths + zip names into JSON
   (category, subtype, modes, flags, notes).
5. **raid content** — parse the raid-boss lore + generate stubs into loadable JSON.

## Python usage

```bash
python3 python/extract_models.py   LIB            -j 16
python3 python/process_models.py   LIB --quality ultra --out-iqm iqm  -j 16
python3 python/texture_export.py   LIB --out-iqm parts --out-tex parts/textures  -j 16
python3 python/model_metadata.py   LIB --out metadata.json
python3 python/raid_content.py     --lore plans/raids-and-relics.md --out raid.json
```

Requires: `numpy`, and for texture/decimation `pillow`, `trimesh`, `fast_simplification`.

## Rust usage

```bash
cd rust && cargo build --release
./target/release/extract-models   LIB
./target/release/obj-iqm          model.obj out.iqm --tex skin.png
./target/release/model-metadata   LIB --out metadata.json      # or --tree tree.txt
./target/release/raid-content     --lore ../plans/raids-and-relics.md --out raid.json
```

### Rust parity status
- `model-metadata` — full parity (output matches the Python tool exactly).
- `raid-content` — lore parsing parity (authored entries).
- `extract-models` — full parity (recursive, name-preserving, nested zips).
- `obj-iqm` — OBJ load + **vertex-clustering decimation** (`--tri-budget N`) + normalize
  + vertex-normals + IQM write. Full-quality **quadric** decimation is still Python-only
  (clustering preserves silhouette but loses fine detail faster); glTF loading and the
  texture/scanner pipeline also remain Python-only for now.

## `plans/`
The design docs (`plan.md`, `raids-and-relics.md`) describing the library layout,
patrol/raid vision, and raid-boss lore.
