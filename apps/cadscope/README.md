# CADscope

Touch-first parametric CAD. Analytic SDFs are the modeling truth; meshes are the viewing and
export truth. The renderer is [`games/skydribble`](../../games/skydribble)'s object pass
transplanted into an `egui_wgpu` paint callback; the app patterns (liaison, fan-out, convograph,
sanitization boundary) come from [`apps/logscope`](../logscope).

**Status: M0 complete** — the vertical slice compiles and tests green; the interactive gate needs
hardware (this workspace was built in a sandbox with no GPU or display).

## Build and run

```sh
cd apps/cadscope
cargo test                              # 36 tests, includes offline WGSL validation via naga
cargo run --release -p cadscope-egui    # the app (bin name: cadscope)
```

Requires Rust 1.96+. The wgpu backend is mandatory — `NativeOptions.depth_buffer = 32` maps to
`Depth32Float`, which is the format the `cad-view` pipeline declares. The glow backend will not
work.

## What M0 gives you

A demo document (`m0-bench`: slab, pillar, knob, ring, handle) built from forge ops, evaluated to
an SDF model, drawn as meshes on a gridded ground plane.

| input | action |
|---|---|
| drag | orbit |
| right/middle drag | pan |
| scroll or pinch | dolly |
| tap / click | pick (sphere-traced against the SDF — what you see is what you hit) |
| `h` `j` `k` `l` | orbit |
| `+` `-` | dolly |
| `f` | frame selection (or the whole model) |
| `n` / `N` | cycle selection |
| `g` | Grow mode (M2 stub) |
| `Esc` | clear selection / leave mode |

`z` and `space` are deliberately unmapped: they are the camera prefix and the leader key, reserved
for the `logscope-fan::vimmode` integration at M2.

## Crates

Dependencies point down only. Nothing below `cadscope-egui` knows about egui except `cad-view`,
which needs it for the paint callback.

| crate | milestone | what it owns |
|---|---|---|
| `cad-kernel` | M0 ✅ | SDF primitives, CSG, `Blend`, `Shell`, sphere-traced rays, `Part`/`Model`, picking. std + glam only. |
| `cad-forge` | M0 ✅ | `.cadf` op documents — names not IDs, per-op error tolerance, byte-stable round-trip. |
| `cad-view` | M0 ✅ | wgpu renderer, procedural meshes, orbit camera, the one lit WGSL pass. |
| `cad-motion` | M0 ✅ | modal key → action mapping. Pure; no camera, no scene. |
| `cadscope-egui` | M0 ✅ | the shell: window, viewport, panels, wiring. |
| `cad-liaison` | M3 | PartSpec/OpScript sanitizers, `logscope-fan` adapters, genviz progress. |
| `cad-tex` | M4 | the texturing system: material graphs, triplanar + auto-unwrap, bake, audit. |
| `cad-pool` | M5 | asset pools: manifests, budgets, per-game injection. |
| `cad-net` | M6 | forge-op replication over snapcore clusters. |
| `cad-nifty` | M6 | niftyplot emit; the `%kind: hardware` implementation (upstreams to `shared/nifty-core`). |

## Design commitments worth knowing before editing

- **The kernel never lies about distance.** `Sdf::eval` must never *over*estimate distance to the
  surface, because the sphere tracer and the M1 mesher both rely on that bound to be correct. The
  `Blend` property test pins this.
- **The forge tolerates typos.** One bad op is skipped and reported (`OpWarning`), never fatal.
  This is what makes documents safe to hand to a GUI, a collaborator, or an LLM.
- **`ObjectRaw` is exactly 256 bytes.** That is skydribble's layout, kept verbatim: a universally
  supported dynamic-offset alignment and the stride the WGSL `Obj` struct pads to. A test asserts
  it. Do not reorder the fields.
- **Shaders are validated offline.** `cargo test -p cad-view` parses and validates `shader.wgsl`
  with naga, so WGSL errors are a test failure and not a black viewport on someone's phone.

Debts and known gaps: [PARITY.md](PARITY.md). Full plan and milestones: `CADSCOPE_PROMPT.md`.
