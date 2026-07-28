# The Rust port — status and the GPU path

The port advances in verifiable slices: each reads the SAME data files the Python
game ships and reproduces facts the Python suites already proved.

| slice | module | state |
|---|---|---|
| 1–8 | `data` `sim` `match_sim` `skirmish` `battler` `tournament` `net_skirmish` | tech/economy, match sim, skirmish, battler, tournament, netplay |
| 9 | `render` | software rasterizer: the papercraft facet language |
| 10 | `origami` | card models — builtin parts **and** the data-driven `parts.json` vocabulary |
| 11 | `gpu` | `SceneBuffers`: interleaved pos/nrm/col + u32 indices, byte-deterministic |
| 12 | `wgpu_paper` | **the portable GPU renderer** |
| 13 | `origami::model_mesh` | card models folded into real **3D geometry** |
| — | `editor` | data editor, now model- and part-aware |

## Why wgpu

The previous GPU on-ramp targeted the monorepo's Vulkan engine, which meant the
path only existed on a machine with a current toolchain and a Vulkan loader. The
fleet is *both* Metal (M2 Ultra) and Vulkan (Asahi M2), and wgpu covers both from
one source. It also has the property that matters most here: its shader
front-end (**naga**) validates the WGSL in a unit test with **no GPU, no window,
and no CI runner tax**.

The module is split so the untestable surface is as small as possible:

* `wgpu-shader` (CPU only) — the WGSL, the `CameraUniform` and its byte packing,
  and the vertex layout. Verified headlessly.
* `wgpu-render` (needs a device) — adapter/device acquisition, buffer upload,
  pipeline construction, and an offscreen render with readback.

```sh
cargo test                          # default, std-only: 50 tests
cargo test --features wgpu-shader   # + naga validates paper.wgsl: 51
cargo check --features wgpu-render  # the full device path (compiles clean on wgpu 22)
```

## Keeping the graphics style

`shaders/paper.wgsl` is written to protect the look, and three tests assert the
promises rather than trusting a comment:

* **Hard facets.** Normals arrive per FACE from `SceneBuffers` and are used raw.
  A test forbids `normalize(in.nrm)` in the fragment stage — re-normalizing an
  interpolated normal is exactly how a crease gets rounded off.
* **Paint bands.** Lambert is quantized (`fn band`), because paint on paper reads
  as steps, not a gradient.
* **World-anchored grain.** The paper fibre is sampled in world space, so it
  belongs to the sheet instead of swimming across it as the camera moves.
* **Ink edges** darken facets turning away from the eye — the drawn outline the
  software renderer gets for free.
* **No back-face culling.** A folded sheet is a *sheet*; culling punches holes in
  wings and fins.

The uniform defaults are the software renderer's own desk lighting — warm key low
and left (the candle), dim cool fill from the room — so both paths light a model
the same way.

One subtlety worth recording: wgpu clip space is 0..1 in Z (Metal/D3D style), not
GL's -1..1. `CameraUniform::set_camera` builds the projection accordingly and a
test projects a near-plane and a far-plane point to prove it — this is the classic
tutorial-paste bug that shows up as z-fighting or an empty frame.

## Models and the editor

`parts.json` grew from 13 to 23 parts, adding the siege/cavalry/naval/caster
vocabulary the RTS silhouettes needed: `ram` `catapult` `hoof` `oar` `mast`
`lantern` `scroll` `shieldwall` `antler` `ribcage`. Because parts are **data**,
both the Python game and the Rust port gained them at once, and packs can add more
the same way.

Two tests keep that honest: every registered part must actually fold into
geometry (a typo'd `kind` or an empty `points` would otherwise ship as an
invisible unit), and the new parts must each fold to a *different* footprint —
two parts that render identically are a spec bug, not a style choice.

The editor learned the same vocabulary: `parts` lists everything (builtin and
data), `parts <name>` describes one, and `model <card>` prints a card's fold
recipe part by part. All three read the same `ModelBook` the renderer uses, so
the editor can never disagree with what will actually be drawn.


## Slice 13 — the models become geometry

Slice 10 taught the software renderer to draw card models, but only as a
*screen-space* pass: `render_scene` painted them over the finished 3D frame.
That left the models stranded on the CPU. `build_scene`'s mesh — and therefore
`SceneBuffers`, and therefore everything wgpu could ever draw — contained only
the generic diamond stand-ins. The prettiest part of the game could not reach
the GPU path at all.

`origami::model_mesh` folds a recipe into actual geometry: every part becomes an
extruded papercraft solid — front facet, back facet, and the side walls that
close the fold. Depth is deliberately shallow (a fold has thickness; a sheet is
not a box), the far facet is shaded darker so it reads as a fold rather than a
cutout, and every facet stays flat because `SceneBuffers` derives one normal per
triangle and that is the entire look.

`render::build_scene_with_models` uses it to build a battlefield where units ARE
geometry, so one mesh describes the whole field and every renderer — software,
wgpu, or whatever comes next — draws the same thing. Units whose spec names no
card keep the diamond, which is why the stand-in stays in the code: it is the
honest fallback, not dead weight, and a test asserts it still appears.

Four tests pin the fold itself (real triangle count, genuine thickness and
height, unknown cards declining cleanly without leaving geometry behind,
different recipes producing different meshes) and two pin the scene — including
the one that matters most: a folded battlefield survives the trip into the
interleaved GPU buffers with unit-length per-face normals.

A note on test honesty: the scene tests originally early-returned when the sim
produced no units, which made them pass while asserting nothing. They now place
fighters directly and assert the field is non-empty first — a test that cannot
fail is worse than no test.
