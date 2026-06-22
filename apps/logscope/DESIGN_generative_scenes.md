# Design: reference-image reskin, LLM scene-components, point-cloud rooms

Three generative ideas, all flowing into the **room spec** the engine already uses (`build_room()` reads a
compact spec; that struct is the safe interchange format). Each is staged from "buildable now" to "needs a model".

## 0. The sanitization boundary (the key safety idea you raised)
Nothing external — an LLM, an image model, a point-cloud classifier — ever produces *code* or touches the scene
graph directly. They only ever emit a **RoomSpec** (JSON), which a validator turns into a sane scene:
```jsonc
{ "name": "Marble Hall", "floor": "nave", "floor_rgb": [200,194,180], "floor_mat": "stone",
  "perimeter": "rows", "perim_rgb": [224,220,208], "roof": "vault",
  "focal": "altar", "accent_rgb": [255,230,176], "props": "banners" }
```
**`sanitize(spec) -> RoomSpec`** rules: enums must be in the known set (else nearest/default); every rgb clamped
0–255; counts/sizes clamped to safe ranges; unknown keys dropped; never eval, never a path/URL/shader string.
Output is *only* the 9 fields `build_room` already takes. This makes every generator interchangeable and
incapable of doing anything but pick from primitives we wrote. **Buildable now** — it's the room table as JSON +
a clamp/match validator + a `LOGSCOPE_ROOM_JSON=<file>` loader.

## 1. Local-LLM scene components  (closest to shippable)
- A small local model (your llama.cpp backend) gets a **constrained prompt**: "emit one RoomSpec JSON for a
  '<user phrase>' workspace; fields and allowed values are: …". Grammar-constrained decoding (GBNF) guarantees
  valid JSON in-vocabulary — llama.cpp supports this directly, so the model literally cannot emit anything but a
  spec.
- Pipe through `sanitize()` → `build_room()`. A bad/garbage spec degrades to a safe default, never a crash.
- Roadmap to richer rooms: grow the spec vocabulary (more focal/prop/structure enums, sub-room lists, a palette
  field) so the LLM composes from a larger kit without us hand-authoring each room.
- API sketch: `fn room_from_spec(json: &str) -> Scene { build_room_spec(sanitize(parse(json))) }`.

## 2. Reference-image reskin / generation  (needs an image model on the backend)
Two ambition tiers:
- **Reskin (palette/material transfer) — moderate.** Analyze an uploaded reference image → extract a palette
  (k-means dominant colors) + a rough material guess (warm/cool, matte/glossy, vegetation/stone/water via simple
  classifiers) → fill a RoomSpec's `*_rgb`/`*_mat` fields. No generative model needed — just image analysis. This
  reskins an existing room's structure to match a photo. **Mostly buildable now** (image-stats, no NN).
- **Generate (img2img → scene) — ambitious.** An img2img/depth model on the backend (e.g., a small SD-turbo +
  depth, or a depth-estimation net) turns the reference into (a) a depth/normal field and (b) a layout guess
  (floor/wall/opening segmentation). We map *that* onto a RoomSpec — NOT onto raw geometry — so the output stays
  inside our sanitized primitive kit. The model proposes; `sanitize()` disposes.
- Backend API: `POST /reskin {image} -> RoomSpec` and `POST /generate {image,prompt} -> RoomSpec`. The renderer
  only ever receives a RoomSpec. The model itself runs where llama.cpp does (the existing inference backend).

## 3. Point-cloud room analyzer  (the phone-scan idea)
- Input: a point cloud (LiDAR/photogrammetry export — `.ply`/`.las`, or ARKit/ARCore room scans).
- Pipeline: downsample (voxel grid) → plane segmentation (RANSAC) to find **floor / walls / ceiling** → cluster
  the residual points → **object classification** (start with geometric heuristics: bounding-box aspect +
  height → "column / shelf / table / seat"; later a small PointNet-style net for real labels).
- Map the result to a RoomSpec: floor extent → `floor`+size, wall planes → `perimeter`/`walls`, tall thin
  clusters → columns, a dominant central cluster → `focal`, palette sampled from per-point color. Again: the
  classifier only chooses spec fields; we instance our own clean primitives at the detected transforms.
- Staged: (a) PLY loader + RANSAC planes + heuristic classify (buildable, no NN), (b) swap in a learned
  classifier later for accuracy.

## Why everything funnels through RoomSpec
- One safe surface to validate. (b) Generators are swappable + testable in isolation. (c) The renderer never
  trusts external bytes — it only ever instances primitives we wrote, at parameters we clamped. (d) Same path
  serves hand-authored rooms, LLM rooms, photo-reskinned rooms, and scanned rooms.

## Suggested order
1. RoomSpec JSON + `sanitize()` + `LOGSCOPE_ROOM_JSON` loader (no model).  ← do first
2. Local-LLM → spec with GBNF grammar (uses the existing llama.cpp backend).
3. Reference-image **reskin** via palette/material analysis (no NN).
4. Point-cloud loader + RANSAC + heuristic classify.
5. img2img **generate** + learned point-cloud classifier (the model-heavy tier).
