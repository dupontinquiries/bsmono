# Prezi-walk — fitting too-big-to-see graphs (v0.1.675)

## Problem (user, with the scattered-cards screenshot)
A mindmap/graph (e.g. TF_DOMINION_SCENE_MINDMAP.md) lays out across a huge
canvas. Zoom-to-fit → nothing readable; zoom-in → you're lost. Request: a
Prezi-style walk that fits the graph by hopping between focal points, using a
"space algorithm with connected edges and keywords to prioritize geometry
within constraints."

## Approach (src/prezi_walk.rs, 7 tests; consumes graph_layout positions)
1. PRIORITY per node = degree centrality (connected edges, normalized) +
   keyword relevance (fraction of the query's words in the node label). This
   is the "connected edges AND keywords" prioritization.
2. WALK = greedily, highest-priority unvisited node becomes a stop's LEAD;
   gather its highest-priority unvisited neighbors (up to cluster_size) into
   the stop; mark visited; repeat. Every node is framed exactly once.
3. FIT per stop = bounding box of the cluster + padding → zoom =
   min(vw/world_w, vh/world_h), CLAMPED to [min_zoom, max_zoom]. Center =
   cluster centroid. So a tight cluster zooms in (capped so a lone node
   doesn't blow up absurdly); a sprawling one zooms out (capped so it stays
   readable). This is the "within constraints" part — the viewport bounds the
   geometry shown.

Output: ordered stops "lead\tcx\tcy\tzoom\tids" → qt_method prezi_walk_for.

## QML (build-gated)
The animated camera transitions between stops are the QML layer — a
cinematic_camera path already exists (docs/design/cinematic_camera.md). The
cards canvas feeds its laid-out cards + edges + (optional) the search query
to prezi_walk_for and animates center/zoom per stop, advancing on tap/voice
("next"/"previous"). NOT yet wired into the canvas — the algorithm +
qt_method are ready + tested; the camera-drive is the next QML step.

## Tested behaviors
hub (high-degree) leads; keyword promotes an unconnected node; zoom always in
[min,max]; tiny cluster caps at max_zoom; huge spread caps at min_zoom; every
node framed once; center = centroid.
