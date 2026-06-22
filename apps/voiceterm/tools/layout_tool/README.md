# layout_tool

A standalone evaluator for web2qml's graph layout. Point it at any mermaid file
and it runs the **real** parser + layered-layout engine the app ships (pulled in
via `#[path]` from `../../src`, so there's no duplicated layout code) and reports
node positions plus legibility metrics.

## Build & run

```sh
cd tools/layout_tool
cargo build --release
./target/release/layout_tool path/to/diagram.mmd
```

Accepts:
- `.mmd` files (raw mermaid source), or
- `.md` files containing a ```` ```mermaid ```` fenced block (the block is
  extracted automatically).

## Output

Default: a human-readable report —

```
graph: mips_datapath
  direction : TopDown (layered)
  nodes     : 38
  edges     : 34
  bounds    : 953 x 1662
  aspect    : 1.74  (balanced)
  crossings : 29
  overlaps  : 0

  positions:
    PC                       (  369.2,  1488.0)
    ...
```

`--json` emits positions + edges in the same JSON shape
`scripts/layout_analyze.py` consumes, so the full Python metric suite (layer
imbalance, backward edges, edge-angle consistency, label-fit, …) can post-process
the tool's output on arbitrary files — not just the baked-in corpus.

Multiple files can be passed at once.

## Why it exists

The in-repo `scripts/layout_metrics.sh` harness only runs a fixed corpus. This
tool exposes the same engine as a general CLI so any diagram — from any project —
can be evaluated for layout quality without adding it to the corpus first.
