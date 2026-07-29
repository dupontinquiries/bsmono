# CADscope

Touch-first parametric CAD. Analytic SDFs are the modeling truth; meshes are the viewing and
export truth. The renderer is [`games/skydribble`](../../games/skydribble)'s object pass
transplanted into an `egui_wgpu` paint callback; the app patterns (liaison, fan-out, convograph,
sanitization boundary) come from [`apps/logscope`](../logscope).

**Status: M2 in progress** — the kernel meshes arbitrary CSG through dual contouring and the
viewport draws it. 47 tests green plus a headless `--selftest`; the interactive gate still needs
hardware (this workspace was built in a sandbox with no GPU or display).

## Build and run

```sh
cd apps/cadscope
cargo run -p cadscope-egui -- --ref-index ../..   # index the monorepo's models/textures/docs
cargo test                              # 83 tests, includes offline WGSL validation via naga
cargo run -p cadscope-egui -- --selftest  # headless: evaluate, mesh and ray-probe the model
cargo run --release -p cadscope-egui    # the app (bin name: cadscope)
```

Requires Rust 1.96+. The wgpu backend is mandatory — `NativeOptions.depth_buffer = 32` maps to
`Depth32Float`, which is the format the `cad-view` pipeline declares. The glow backend will not
work.

## What the demo gives you

A document (`m1-bench`) built from forge ops, evaluated to an SDF model, drawn on a gridded ground
plane. Five parts are primitives on cached parametric meshes; three are CSG and go through the dual
contourer — a **bracket** (a plate with a bore subtracted), a **lamp** (a sphere blended onto a
post, so the join reads as a fillet), and **studs** (one cylinder patterned four times into a
single solid).

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

### Grow mode (`g`)

Steer a form as it grows instead of placing primitives. The grammar is
[`spine_grower.html`](../../games/blackstar/tools/spine_grower.html)'s, key for key, so muscle
memory from that tool transfers directly.

| key | effect | key | effect |
|---|---|---|---|
| `h` `l` | yaw | `[` `]` | twist −/+ |
| `j` `k` | pitch | `0` | straighten |
| `H` `L` | roll | `e` | ridge toggle |
| `w` `b` | thicken / narrow | `` ` `` | wobble toggle |
| `f` `p` | flare / pinch | `1`–`5` | section: ○ □ △ ✦ ⬡ |
| `space` | knuckle (fat station) | `t` | branch |
| `m` | mirror across X | `y` | back to parent |
| `Esc` | commit into the document | | |

The form grows continuously while the mode is active; steering keys apply while held. `Esc` writes
each spine into the document as a `spine` op — the session becomes history, not a scratch mesh.

In Normal mode, `z` and `space` are deliberately unmapped: they are the camera prefix and the leader key, reserved
for the `logscope-fan::vimmode` integration at M2.

## Crates

Dependencies point down only. Nothing below `cadscope-egui` knows about egui except `cad-view`,
which needs it for the paint callback.

| crate | milestone | what it owns |
|---|---|---|
| `cad-kernel` | M0 ✅ M1 ✅ | SDF primitives, CSG, `Blend`, `Shell`, bounds, sphere-traced rays, `Part`/`Model`, picking, and dual-contour meshing, and the spine/turtle grow model. std + glam only. |
| `cad-forge` | M0 ✅ M1 ✅ | `.cadf` op documents — names not IDs, per-op error tolerance, byte-stable round-trip. Ops: add, nudge, color, combine (union/intersect/subtract/blend), shell, pattern, remove. |
| `cad-view` | M0 ✅ | wgpu renderer, procedural meshes, orbit camera, the one lit WGSL pass. |
| `cad-motion` | M0 ✅ M2 ✅ | modal key → action mapping for Normal and Grow. Pure; no camera, no scene. |
| `cadscope-egui` | M0 ✅ | the shell: window, viewport, panels, wiring. |
| `cad-liaison` | M3 | PartSpec/OpScript sanitizers, `logscope-fan` adapters, genviz progress. |
| `cad-tex` | M4 | the texturing system: material graphs, triplanar + auto-unwrap, bake, audit. |
| `cad-ref` | M4 ▸ | the reference corpus: index models, textures and specs by name so "same volume as `puma`" resolves to a measured fact. IQM/PNG/doc parsing works today; the verbs are M4–M7. |
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
- **The mesher trusts the distance bound.** Dual contouring reads crossings and gradients straight
  out of the field; if `eval` ever overestimated, vertices would land off-surface. The selftest
  measures exactly this (`max|d|` per part, against half a cell).
- **Shaders are validated offline.** `cargo test -p cad-view` parses and validates `shader.wgsl`
  with naga, so WGSL errors are a test failure and not a black viewport on someone's phone.

Debts and known gaps: [PARITY.md](PARITY.md). Full plan and milestones: `CADSCOPE_PROMPT.md`.
