# niftyplot view-layer: smart expansion + 3D layer rotation (v0.1.697)

User vision: "niftyplot smart expansion and pan around selected elements with
keyboard and voice jumps and 'go down'/'go up' being able to exit from voice...
the coolest thing would be to have 3d cad and say rotate and the niftyplot with
layers can assume cool orientations."

Built (pure + tested, src/niftyplot_view.rs, 8 tests) — UNWIRED, awaiting
greenlight + the QML canvas.

## 1. Smart expansion (progressive disclosure)
Topology{ids, undirected adjacency} built from a GraphDoc's nodes/edges.
- expand(focus, depth) → visible ids via BFS (depth 0 = focus; 1 = +neighbors;
  …). Deterministic (sorted neighbors).
- rings(focus, depth) → each visible node's hop distance, so the QML can fade
  outer rings.
A big graph opens one ring at a time around the selected node instead of
dumping everything. Voice/keys: "expand"/"go deeper" → depth+1; "collapse" →
depth-1; arrows pan the focus to a neighbor.

## 2. 3D layer rotation (the "rotate" vision)
Planes = layers stacked in pseudo-3D. layer_orientation(n, yaw, pitch, spread)
→ per-plane LayerTransform{dx, dy, scale, skew, opacity}: yaw fans layers
horizontally, pitch stacks them, deeper layers recede (shrink + fade + skew)
for an isometric "CAD" look. Front layer is identity.
orientation_preset(name) maps spoken words → (yaw,pitch): flat/front/reset,
fan/rotate/spread, stack/iso/isometric, tilt.

EVENT-DRIVEN (per the camera guidance): a "rotate"/"fan" command computes the
TARGET (yaw,pitch) → target transforms; the QML interpolates each plane's
transform from current to target with a SpringAnimation/Behavior, retaining
in-flight motion and re-targeting smoothly. No per-frame polling.

## Wiring plan (next, on greenlight)
- qt_methods: nifty_expand(source, focus, depth) → visible-id wire;
  nifty_layer_transforms(n, yaw, pitch, spread) → "dx,dy,scale,skew,op;…";
  nifty_orientation_preset(name) → "yaw,pitch" | "".
- Voice intents: "rotate/fan/flatten the diagram", "expand/collapse",
  "go up/down" (pan focus), "exit"/"done" (leave the graph-voice mode).
- QML niftyplot canvas: render planes as transformed layers; Behavior on each
  layer's transform; focus-node pan with arrow keys; voice mode overlay with
  an explicit exit.


## v0.1.698 update — WIRED (smart expansion + 3D rotation + voice)
qt_methods: nifty_expand(source, focus, depth) → "id:ring,…"; nifty_layer_
transforms(n, yaw, pitch, spread) → "dx,dy,scale,skew,op;…"; nifty_orientation_
preset(name) → "yaw,pitch"; nifty_planes(source) → front→back plane names.
Voice intents: RotateGraph(preset) ("rotate/fan the diagram", "isometric view",
"tilt the layers"), ExpandNode ("expand", "go deeper"), CollapseNode
("collapse", "show fewer") — all corpus-verified, no collision with the
existing FlattenGraph ("flatten"/"collapse to 2d").
QML cardsCanvas: _expandDepth/_expandFocus/_visibleRing drive per-node
visibility + outer-ring fade (tap a node to set the focus; default = hub);
_layerYaw/_layerPitch → _refreshLayerTransforms → per-plane Translate + skew
Matrix4x4 on each card, with Behaviors interpolating to the target (event-
driven). Dispatch cases rotate_graph/expand_node/collapse_node.
REMAINING: codemap in the read-only doc view (#4) — backend ready
(note_outline works on any doc) but the HTML reader-view panel is deferred (not
blind-edited); a keyboard-pan of the selected node + a dedicated graph-voice
"exit" overlay are follow-ups (voice "exit"/"close graph" already maps to
close_connections_view).
