# r68-06 · ambient occlusion — FINDINGS (scoping result, no code change)

**WID:** r68-06-shd-ao · **Files changed: NONE** (and the descriptor contract is
therefore intact). This explains why, with the evidence, and gives the correct
implementation plan so the coordinator can dispatch it properly.

## Sandbox fix (do keep this)
`glslangValidator` / `spirv-val` / `glslc` were **missing** in this sandbox
(the COMMON brief assumed them present). Installed from the Ubuntu archives:
`apt-get install -y glslang-tools spirv-tools` → `glslangValidator 15.1.0`,
`spirv-val 2025.1`. Without these, `build.rs` can't recompile shaders and the
required `spirv-val` gate can't run, so this step is a prerequisite for any
shader round in this sandbox.

## Why a good AO term can't be a `chunk.frag`-only edit here
Three structural facts (all verified in this base):

1. **The fragment normal is flat per face.** `chunk.frag` derives `normal` from
   `cross(dFdx(v_world_pos), dFdy(v_world_pos))` and then **snaps it to the
   dominant axis** (lines ~235–245). It's constant across each voxel face, so a
   normal-derivative / curvature AO only changes at the 1-pixel face boundary →
   thin dark **edge halos at silhouettes**, exactly what the brief says to avoid.
2. **No depth or normal target exists.** `chunk.frag` binds only
   `set0/bind0 CameraUbo`, `bind1 shadow_map`, `bind2 froxel_volume`
   (confirmed against `harness/shader_descriptor_baseline.txt`). There is **no
   G-buffer/depth/normal texture anywhere** in `src/shaders/*` or the render
   pipeline. GTAO/SSAO need depth (+normal) as input — none is available to
   sample.
3. **No per-vertex AO attribute.** The only vertex inputs are `v_color` and
   `v_world_pos`; there's no neighbor-derived AO coming from the mesher. In the
   fragment alone there is no neighbor/occlusion information, so a uniform
   per-face edge darkening can't tell a real concave cavity from a flat
   coplanar seam → it paints a 1 m **dark grid on flat floors** (artifact).

So every in-`chunk.frag`-only option is either a halo (derivative) or a grid
artifact (edge darkening) — both forbidden — and the screen-space path needs
infrastructure that doesn't exist yet.

## Why I did not add the SSAO/GTAO binding blind
Adding an AO texture + depth/normal targets + a new `chunk.frag` binding is
multi-system render wiring (new render targets, a new pass, pipeline-layout +
UBO + baseline changes). The brief itself warns that **silent descriptor drift
breaks the pipeline at runtime**, and there is **no GPU in this sandbox** to
runtime-validate it — `descriptor_gate.py`/`ubo_parity.py` passing *statically*
does not prove the runtime pipeline binds correctly. Shipping an unvalidatable,
runtime-fatal-if-wrong descriptor change is exactly what not to do. (Consistent
with the r62 posture: don't ship what you can't validate.)

## Correct implementation paths (pick one; I'll build the scoped version)

**Option A — per-vertex voxel AO (cheap, no textures, best ROI).** The classic
Minecraft-style smooth AO. Lives across three files, so it needs a task scoped to
them (not chunk.frag alone):
- `render/mesher.rs`: at each emitted face vertex, sample the 3 neighbor voxels
  around that corner; AO = f(#solid neighbors) in {0,1,2,3} → 0.5..1.0. Pack one
  byte into the vertex format (or steal bits from `v_color`).
- `chunk.vert`: pass AO through to a new `location` varying.
- `chunk.frag`: `ambient_term *= ao;` (multiply AO into the *ambient/indirect*
  term only — not direct sun). Tunables: `AO_INTENSITY`, `AO_POWER`, quality
  toggle; fallback ao=1.0.
- Validatable headless: it's deterministic mesher logic (unit-testable like the
  other sim cores) + a descriptor-stable frag multiply; the vertex-format change
  is the one thing to coordinate with the bin's input layout.

**Option B — SSAO/GTAO pass (screen-space, needs new infra; device-validated).**
- Add a depth (and ideally a packed normal) render target; if depth-only,
  reconstruct normals from depth in the AO pass.
- New `ssao.comp` (hemisphere kernel + range check) or `gtao.comp` (horizon
  search), registered in `src/shader_artifacts.rs`; bilateral (depth-aware) blur
  to denoise without edge bleed.
- New `chunk.frag` binding sampling the AO texture; **intentional** descriptor
  change ⇒ update Rust pipeline layout + UBO + regenerate
  `shader_descriptor_baseline.txt` + keep `ubo_parity` in agreement.
- Must be validated on the M2 by the operator (binding correctness + frame cost);
  not headless-certifiable here.

Combine either with the existing hemisphere/underside term via `min`/multiply,
as the brief suggests.

## Recommendation
Dispatch **Option A** as a `render/mesher.rs` + `chunk.vert` + `chunk.frag`
task (highest quality-per-cost, no new bindings, mostly headless-validatable). I
can implement and validate the mesher AO logic + the frag multiply here, and
clearly mark the vertex-format/input-layout change for the bin to wire. Reserve
**Option B** for when a depth/normal target + on-device validation are available.

I made no change this round to avoid shipping either a forbidden visual artifact
or an unvalidatable runtime-fatal descriptor change. Toolchain is now in place
for the next shader round.
