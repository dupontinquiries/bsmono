# TODO: 2D shape renderer (cutouts, phone back covers)

Full design + rationale: docs/design/shape_renderer.md
Recorded in vision.md as a deliberate exploratory expansion.

## The one-line idea
niftyplot graph → (pure lowering) → ShapeDoc → (swappable renderer
trait) → SVG. Basic SVG renderer now; heavy renderer slots behind the
same trait later. Voice mutates the source. Use cases: panel cutouts,
vented phone back covers, bolt grids — flat parts in real mm.

## Status: PLANNED, no code yet (v0.1.590)

## Phases (see design doc for detail)
- [ ] **Phase 1** — `shape_doc.rs`: ShapeDoc/Primitive types +
      ShapeRenderer trait + pure `lower(&GraphDoc) -> ShapeDoc` for
      explicit-size Rect/Circle/Hex nodes. Pure, testable. ← NEXT
- [ ] **Phase 2** — `shape_render_svg.rs`: `SvgRenderer` impl,
      ShapeDoc → SVG string. Pure, testable (assert on SVG output).
- [ ] **Phase 3** — pattern lowering: HexPattern(spacing) → many Hex,
      BoltGrid(rows,cols) → grid of Hole. Pure, testable.
- [ ] **Phase 4** — QML PreviewBlock rendering the SVG (needs build).
- [ ] **Phase 5** — voice mutation: "set width to 160" → resolve node
      (entity_extract) → edit source → re-lower → re-render. Pure core
      testable; audio path needs build.

## Guardrails (from the design doc)
- The renderer ALWAYS sits behind the `ShapeRenderer` trait. Nothing
  upstream depends on the concrete renderer.
- Reuse niftyplot (parser), graph_layout (positions), entity_extract
  (voice target resolution). Do NOT rebuild these.
- NON-GOALS until 1–5 ship: geometry kernel / boolean CSG, 3D/extrusion,
  GPU renderer, plugins, drag-node-edits-source. They slot behind the
  trait later IF earned — they are not prerequisites and not next.
