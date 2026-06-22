#!/usr/bin/env bash
# v0.1.740: layout-quality metrics harness. Runs the real mermaid parser +
# layered layout over the hardware-diagram corpus, dumps node positions + edges
# as JSON, then a Python analyzer computes legibility metrics and derives
# concrete "lessons learned" about how to improve the layout. Measure-first:
# this is how we decide WHAT to change before touching the live layout.
#
# Usage: scripts/layout_metrics.sh   (writes JSON + prints the lessons report)

set -euo pipefail
cd "$(dirname "$0")/.."
ROOT="$(pwd)"
CORPUS="$ROOT/tests/layout_corpus"
STAGE="/tmp/layout_metrics_stage"
OUT="/tmp/layout_metrics"

rm -rf "$STAGE" "$OUT"; mkdir -p "$STAGE/src" "$OUT"

cat > "$STAGE/Cargo.toml" <<'TOML'
[package]
name="layout_metrics"
version="0.1.0"
edition="2021"
[[bin]]
name="lm"
path="src/main.rs"
TOML

for m in mermaid graph_layout layered_layout node_id card_density; do
  cp "$ROOT/src/$m.rs" "$STAGE/src/$m.rs"
  sed -i 's|^//!|//|' "$STAGE/src/$m.rs"
done

cat > "$STAGE/src/main.rs" <<'RS'
#[path="mermaid.rs"] mod mermaid;
#[path="graph_layout.rs"] mod graph_layout;
#[path="layered_layout.rs"] mod layered_layout;
#[path="node_id.rs"] mod node_id;
#[path="card_density.rs"] mod card_density;

use std::io::Write;

fn esc(s:&str)->String{ s.replace('\\',"\\\\").replace('"',"\\\"") }

fn main(){
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut out = std::io::stdout().lock();
    write!(out,"[").unwrap();
    for (gi,path) in args.iter().enumerate() {
        let src = std::fs::read_to_string(path).unwrap();
        let g = mermaid::parse(&src);
        // build cards with the SAME density sizing the app uses (compact default)
        let dens = card_density::Density::Compact;
        let mut cards: Vec<graph_layout::CardPos> = g.nodes.iter().map(|n|{
            let mut c = graph_layout::CardPos::new(node_id::wire_id(&n.id));
            let label = if n.label.is_empty(){&n.id}else{&n.label};
            let nb = card_density::node_box(label, n.id.chars().count(), dens);
            c.half_w=nb.half_w; c.half_h=nb.half_h; c
        }).collect();
        let edges: Vec<graph_layout::LayoutEdge> = g.edges.iter()
            .map(|e| graph_layout::LayoutEdge::new(node_id::wire_id(&e.from), node_id::wire_id(&e.to))).collect();
        let dir = match g.direction {
            mermaid::Direction::TopDown=>graph_layout::FlowDirection::TopDown,
            mermaid::Direction::BottomTop=>graph_layout::FlowDirection::BottomUp,
            mermaid::Direction::LeftRight=>graph_layout::FlowDirection::LeftRight,
            mermaid::Direction::RightLeft=>graph_layout::FlowDirection::RightLeft,
        };
        let used = if dir.is_directional() {
            graph_layout::seed_grid(&mut cards, &graph_layout::LayoutConfig::default());
            layered_layout::layout_layered(&mut cards,&edges,dir,&layered_layout::LayeredConfig::default())
        } else { false };
        if !used {
            let cfg = graph_layout::LayoutConfig::default();
            graph_layout::seed_grid(&mut cards,&cfg);
            graph_layout::layout_force_directed(&mut cards,&edges,&cfg);
        }
        if gi>0 { write!(out,",").unwrap(); }
        let name = std::path::Path::new(path).file_stem().unwrap().to_string_lossy();
        write!(out,"{{\"name\":\"{}\",\"dir\":\"{:?}\",\"layered\":{},\"nodes\":[",name,dir,used).unwrap();
        for (i,c) in cards.iter().enumerate(){
            if i>0{write!(out,",").unwrap();}
            let lbl = g.find_node(node_id::strip_ns(&c.id)).map(|n| if n.label.is_empty(){n.id.clone()}else{n.label.clone()}).unwrap_or_default();
            write!(out,"{{\"id\":\"{}\",\"label\":\"{}\",\"x\":{:.1},\"y\":{:.1},\"hw\":{:.1},\"hh\":{:.1}}}",
                esc(&c.id),esc(&lbl),c.x,c.y,c.half_w,c.half_h).unwrap();
        }
        write!(out,"],\"edges\":[").unwrap();
        for (i,e) in edges.iter().enumerate(){
            if i>0{write!(out,",").unwrap();}
            write!(out,"{{\"from\":\"{}\",\"to\":\"{}\"}}",esc(&e.from),esc(&e.to)).unwrap();
        }
        write!(out,"],\"groups\":{}}}",g.groups.len()).unwrap();
    }
    write!(out,"]").unwrap();
}
RS

echo "[layout_metrics] building dumper…" >&2
( cd "$STAGE" && cargo build --release -q 2>&1 | grep -vE "Compiling|Finished" || true )
BIN="$STAGE/target/release/lm"
[ -x "$BIN" ] || { echo "[layout_metrics] BUILD FAILED" >&2; exit 2; }

"$BIN" "$CORPUS"/*.mmd > "$OUT/positions.json"
echo "[layout_metrics] positions → $OUT/positions.json" >&2

python3 "$ROOT/scripts/layout_analyze.py" "$OUT/positions.json"
