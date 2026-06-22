warning: function `is_graph_id` is never used
  --> src/node_id.rs:66:8
   |
66 | pub fn is_graph_id(id: &str) -> bool {
   |        ^^^^^^^^^^^
   |
   = note: `#[warn(dead_code)]` (part of `#[warn(unused)]`) on by default

warning: variants `Expanded` and `Literal` are never constructed
  --> src/card_density.rs:20:5
   |
18 | pub enum Density {
   |          ------- variants in this enum
19 |     Compact,
20 |     Expanded,
   |     ^^^^^^^^
21 |     Literal,
   |     ^^^^^^^
   |
   = note: `Density` has derived impls for the traits `Clone` and `Debug`, but these are intentionally ignored during dead code analysis

warning: associated items `from_token`, `token`, and `next` are never used
  --> src/card_density.rs:26:12
   |
24 | impl Density {
   | ------------ associated items in this implementation
25 |     /// Parse the inline hint token (from the QML toggle / a voice command).
26 |     pub fn from_token(s: &str) -> Density {
   |            ^^^^^^^^^^
...
34 |     pub fn token(&self) -> &'static str {
   |            ^^^^^
...
44 |     pub fn next(&self) -> Density {
   |            ^^^^


========================================================================
LAYOUT METRICS  (per graph)
========================================================================
       name |     nodes |    aspect | crossings | backward_ | layer_imb | node_over | label_cut | disconnec | edge_angl |    groups
  cpu_pipeline |         6 |      9.54 |         0 |         4 |       1.0 |         0 |       0.0 |         0 |      89.4 |         0
  full_adder |        10 |      0.45 |         2 |         0 |       1.5 |         0 |       0.0 |         0 |      27.7 |         0
  lhc_dataflow |        10 |      0.45 |         0 |         0 |       2.4 |         0 |       0.0 |         0 |      31.6 |         0
  mips_datapath |        38 |      0.39 |         4 |         1 |      1.26 |         0 |       0.0 |         7 |      42.9 |         2
  optical_link |        11 |      6.54 |         0 |         0 |      1.64 |         0 |       0.0 |         0 |      15.3 |         0
  power_supply |         8 |      5.22 |         0 |         1 |      1.75 |         0 |       0.0 |         0 |      51.3 |         0

========================================================================
LESSONS LEARNED
========================================================================

 1. [HIGH] Aspect ratio is extreme on 4/6 graphs (cpu_pipeline, mips_datapath, optical_link, power_supply) — layouts read 'flat and long'. Target ~1.0-1.8. Cause: layer_gap/sibling_gap ratio + single-chain graphs stretch one axis. Fix: cap the flow-axis extent or scale gaps to graph shape; for long chains, allow wrapping a chain across columns.

 2. [HIGH] Layer occupancy is lopsided on 1 graphs (worst lhc_dataflow: max layer has 3 nodes vs mean 1.2). Source/control nodes pile into one layer. Fix: place in-edge-less nodes adjacent to their single successor instead of all at rank 0.

 3. [HIGH] Subgraph groups parsed but NOT drawn (mips_datapath(2)). The reference's legibility comes largely from labeled group boxes (Data Memory, Write Registers). Fix: render a titled container rect behind member cards AND bias the layout to keep members contiguous within their layers.

 4. [MED] High source-node fraction on full_adder, mips_datapath (>25% have no incoming edge). Many are control signals joined by undirected '---'. Fix: treat '---' (Line) edges as placement hints so signals dock beside their consumer, not at the top rank.

 5. [MED] Backward edges present (cpu_pipeline(4), mips_datapath(1), power_supply(1)) — feedback loops (CPU stall/forward, power feedback) point against flow and read as errors. Fix: detect cycles (we already find back-edges for layering) and render them as visually distinct curved 'return' edges.

 6. [MED] Disconnected nodes float with no edges (mips_datapath(7)) — usually parse misses (breakout/wire nodes) or label-only decls. Fix: drop truly isolated synthetic nodes, or grid them in a footer instead of scattering.

 7. [LOW] Edge angles vary widely (worst cpu_pipeline: σ=89.4°). A clean layered look has mostly parallel edges. Fix: orthogonal edge routing (L-shaped) or align nodes to their parent's cross-axis position when single-child.

 8. [LOW] 2 graphs have high edge-length variance (CV>0.6) — some edges span many layers. Fix: insert virtual 'dummy' nodes on long edges (standard Sugiyama step we skip) so edges step through intermediate layers and stay short + routable.

 9. [LOW] Add dummy nodes for multi-layer edges (skipped Sugiyama phase) — biggest single lever for both crossing reduction AND short routable edges.

10. [LOW] Consider per-graph gap scaling: small graphs (full_adder) want tighter gaps; large ones (mips) want more — a fixed 90px layer_gap over-stretches small graphs.

11. [INFO] No node overlaps in any corpus graph — within-layer packing is collision-free.

  → 11 lessons across 6 graphs.
