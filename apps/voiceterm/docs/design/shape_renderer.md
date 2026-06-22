# 2D Shape Renderer — design + multi-turn plan

Status: **DRAFT / planning**. No renderer code exists yet as of
v0.1.590. This doc captures the direction from the CAD/block-document
conversation, scoped DOWN to something real and incremental.

## The decision (what we are and are NOT doing)

We are building a **basic 2D shape renderer** whose first real use cases
are physical-design artifacts: **panel cutouts and phone back covers** —
flat parts with holes, patterns, and outlines, expressed in real
millimetres and exported as SVG.

We are explicitly NOT (now, maybe never) building: a parametric geometry
kernel, boolean CSG, 3D extrusion, a GPU vector pipeline, or a plugin
system. Those appear in the source conversation as "easy next steps."
They are not — each is a product. They are out of scope and stay out
until the basic renderer has earned them.

### Why this scoping is the whole point

The risk with the source vision is a fascinating architecture pulling
the project away from its actual, nearly-working differentiator
(voice-first terminal + web). This plan guards against that by:

1. Picking ONE concrete, finishable target (2D SVG of flat parts).
2. Putting the renderer behind a **seam** (a trait) so today's simple
   renderer and a future heavy one are interchangeable — the rest of
   the app never depends on which renderer is behind the seam.
3. Reusing what already exists instead of rebuilding it.

## What already exists (do NOT rebuild)

The source doc assumes we need to "evolve Mermaid into a typed graph."
That work is **already done** in the tree:

- `niftyplot::parse(source) -> GraphDoc` — a real parser with `Shape`
  (Rect, Circle, Hexagon, Diamond, Cylinder, ...), `Node`, `Plane`,
  `Constraint`, `NodeGeomKind`, `PlaneGeomKind`. The spec
  (docs/niftyplot-spec.md) was authored with "hardware mm" in mind.
- `graph_layout` — force-directed + grid layout, bounding box.
- `mermaid::parse` — a second, simpler graph parser (interop).
- `note_cards` (CardKind enum + CardDelta) — the block-document backbone
  the source doc's "step 16" asks for, in cousin form.

So the **parser layer the source calls "the full renderer piped in as
the parser" already substantially is niftyplot**. The new work is the
RENDER side, not the parse side.

## The seam (the core architectural commitment)

One trait. Everything renders through it. The model is always a parsed
`niftyplot::GraphDoc` (or a small `ShapeDoc` projection of it — see
phase 1). Renderers are swappable backends.

```rust
/// A backend that turns a parsed shape model into some concrete output.
/// The app depends ONLY on this trait, never on a concrete renderer.
pub trait ShapeRenderer {
    /// Render the doc to the backend's output form (SVG string for the
    /// first backend; later: a GPU scene, a 3D mesh, an STL, ...).
    fn render(&self, doc: &ShapeDoc) -> RenderOutput;
    /// Stable id for settings / selection ("svg", "vello", "wgpu3d").
    fn id(&self) -> &'static str;
}

pub enum RenderOutput {
    Svg(String),
    // future: Scene(GpuScene), Mesh(Stl), ...
}
```

The first and only backend for a long time:

```rust
pub struct SvgRenderer { /* dpi/scale, stroke defaults */ }
impl ShapeRenderer for SvgRenderer { ... }   // pure: ShapeDoc -> SVG string
```

Why a trait and not just "a function": the seam is the promise. When a
heavy renderer (Vello/wgpu) eventually arrives, it implements the SAME
trait, and NOTHING upstream changes — the document model, the voice
mutations, the QML preview binding all stay put. The simple renderer is
the entrypoint; the trait is the socket the full renderer plugs into
later. This is the "keep things separated" the user asked for, encoded
in the type system.

### ShapeDoc — the renderer's input contract

`GraphDoc` carries diagram concerns (warnings, suggestions, layout
hints) the renderer shouldn't care about. So phase 1 defines a small,
pure `ShapeDoc` — a flat list of placed primitives in mm — and a pure
`GraphDoc -> ShapeDoc` lowering. This keeps the renderer's input minimal
and stable even as the parser grows.

```rust
pub struct ShapeDoc {
    pub width_mm: f32,
    pub height_mm: f32,
    pub shapes: Vec<Primitive>,
}
pub enum Primitive {
    Rect   { x: f32, y: f32, w: f32, h: f32, corner_r: f32 },
    Circle { cx: f32, cy: f32, r: f32 },
    Hex    { cx: f32, cy: f32, r: f32 },          // flat-top, for vents
    Hole   { cx: f32, cy: f32, r: f32 },          // cutout (rendered as subtraction hint)
    // grids/patterns lower to repeated primitives in the lowering step
}
```

All coordinates in mm. The SVG backend maps mm → user units with an
explicit scale, so a 158×72 mm plate renders at a known physical size.

## Multi-turn plan

Each phase is independently shippable, pure-testable where possible, and
does NOT block the existing voice-terminal work. Phases 1–3 are fully
sandbox-verifiable (no Qt). Phase 4 needs the running build.

### Phase 1 — `ShapeDoc` + the seam (pure, testable) [NEXT]
- New module `shape_doc.rs`: the `ShapeDoc` / `Primitive` types above,
  plus `ShapeRenderer` trait + `RenderOutput`.
- Pure `lower(&GraphDoc) -> ShapeDoc`: map niftyplot `Shape::Rect/
  Circle/Hexagon` nodes (with width/height/r params) into placed
  `Primitive`s. Start with explicit-coordinate / explicit-size nodes;
  constraint solving comes later.
- Tests: a niftyplot source for a plate with two holes lowers to the
  right primitives at the right mm coords.
- Ships nothing visible; it's the foundation. ~1 turn.

### Phase 2 — `SvgRenderer` (pure, testable)
- `shape_render_svg.rs`: `impl ShapeRenderer for SvgRenderer`.
  `ShapeDoc -> SVG string`. Rect (with corner radius), Circle, Hex as a
  6-point polygon, Hole as a same-fill-as-background circle (or an SVG
  mask for true cutout).
- Tests: golden-ish assertions on the SVG string (has a `<rect>` at the
  expected mm-scaled coords, N `<circle>` holes, correct viewBox =
  plate size). Pure string output → fully testable here.
- Output is a real .svg a user could laser-cut / 3D-print-trace. ~1 turn.

### Phase 3 — parametric + patterns in the lowering (pure, testable)
- Extend `lower` to expand a `HexPattern(spacing=3)` over a region into
  many `Hex` primitives, and a `BoltGrid(rows, cols)` into a grid of
  `Hole`s. This is where "cutouts and phone back covers" get real:
  vented back covers, bolt patterns.
- Incremental-recompute note: lowering is cheap and pure; the row_cache
  dirty-tracking PRINCIPLE (v0.1.574) applies if it ever isn't — only
  re-lower changed subtrees. Defer until measured.
- Tests: spacing/rows/cols produce the expected primitive counts and
  positions. ~1–2 turns.

### Phase 4 — QML preview binding (needs running build)
- A `PreviewBlock` in QML that takes the SVG string from the backend
  and shows it (Qt has native SVG via `Image` with an svg source, or
  `QtSvg`). Three-pane on desktop / stacked on mobile reuses the
  existing `_navAtTop`/compact responsive infra (v0.1.572).
- This is the first VISIBLE piece and the first that can't be verified
  in-sandbox. Dogfood-gated.

### Phase 5 — voice mutation (the actual differentiator)
- "set plate width to 160", "add a hex pattern", "move the camera hole
  left 5 mm" → resolve the target node (entity_extract::resolve_reference,
  already built + hardened) → mutate the niftyplot source → re-parse →
  re-lower → re-render. This is the SAME voice→structured-mutation loop
  as editor_voice, pointed at a shape doc instead of helix keystrokes.
- Pure core (text mutation + re-lower) is testable; the audio path isn't.

## Sequencing principle

Build the seam and the pure pipeline (1–3) FIRST and completely, because
those are verifiable here and are the foundation. Only then the QML
preview (4) and voice (5), which need the running build. At every phase
the renderer stays behind the trait, so when/if a heavy renderer is
ever wanted, it slots in at phase 2's position without disturbing 1, 3,
4, or 5.

## Explicit non-goals (revisit only after phases 1–5 ship)
- Boolean CSG / true geometry kernel (Fornjot/OpenSCAD territory).
- 3D extrusion, rotation, STEP/STL of solids.
- GPU vector renderer (Vello/wgpu) — slots in behind the trait IF the
  SVG backend is ever proven too slow, which for flat 2D parts it
  won't be soon.
- Plugin-registered components.
- Bidirectional node-graph editing (drag node → edit source).
