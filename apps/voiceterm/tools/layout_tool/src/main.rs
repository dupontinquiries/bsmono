//! layout_tool — standalone graph-layout evaluator.
//!
//! Runs web2qml's real mermaid parser + layered layout over ANY mermaid file
//! (.mmd) or markdown file with a ```mermaid block (.md), then prints node
//! positions and a set of legibility metrics (aspect ratio, edge crossings,
//! bounds, layer count, overlaps). The layout modules are pulled in directly
//! from ../../src via #[path], so this tool always evaluates the SAME engine
//! the app ships — no duplicated layout code.
//!
//! Usage:
//!   layout_tool <file.mmd | file.md> [--json]
//!   layout_tool --help
//!
//! Without --json it prints a human-readable report; with --json it emits the
//! positions + edges as JSON (the same shape scripts/layout_analyze.py expects,
//! so the full Python metric suite can post-process it).

#[path = "../../../src/mermaid.rs"] mod mermaid;
#[path = "../../../src/graph_layout.rs"] mod graph_layout;
#[path = "../../../src/layered_layout.rs"] mod layered_layout;
#[path = "../../../src/node_id.rs"] mod node_id;
#[path = "../../../src/card_density.rs"] mod card_density;
#[path = "../../../src/group_layout.rs"] mod group_layout;

use std::process::exit;

fn esc(s: &str) -> String { s.replace('\\', "\\\\").replace('"', "\\\"") }

/// Pull mermaid source out of the input. For .md, extract the first fenced
/// ```mermaid block; for .mmd (or anything else), use the whole file.
fn extract_mermaid(path: &str, raw: &str) -> String {
    if path.ends_with(".md") || raw.contains("```mermaid") {
        let mut out = String::new();
        let mut inside = false;
        for line in raw.lines() {
            let t = line.trim_start();
            if !inside && t.starts_with("```mermaid") { inside = true; continue; }
            if inside && t.starts_with("```") { break; }
            if inside { out.push_str(line); out.push('\n'); }
        }
        if !out.is_empty() { return out; }
    }
    raw.to_string()
}

struct Laid {
    name: String,
    dir: String,
    layered: bool,
    cards: Vec<graph_layout::CardPos>,
    edges: Vec<graph_layout::LayoutEdge>,
    labels: Vec<String>,
    boxes: Vec<group_layout::GroupBox>,
}

fn run_layout(path: &str, src: &str) -> Laid {
    let g = mermaid::parse(src);
    let dens = card_density::Density::Compact;
    let mut cards: Vec<graph_layout::CardPos> = g.nodes.iter().map(|n| {
        let mut c = graph_layout::CardPos::new(node_id::wire_id(&n.id));
        let label = if n.label.is_empty() { &n.id } else { &n.label };
        let nb = card_density::node_box(label, n.id.chars().count(), dens);
        c.half_w = nb.half_w; c.half_h = nb.half_h; c
    }).collect();
    let edges: Vec<graph_layout::LayoutEdge> = g.edges.iter()
        .map(|e| graph_layout::LayoutEdge::new(node_id::wire_id(&e.from), node_id::wire_id(&e.to)))
        .collect();
    let dir = match g.direction {
        mermaid::Direction::TopDown => graph_layout::FlowDirection::TopDown,
        mermaid::Direction::BottomTop => graph_layout::FlowDirection::BottomUp,
        mermaid::Direction::LeftRight => graph_layout::FlowDirection::LeftRight,
        mermaid::Direction::RightLeft => graph_layout::FlowDirection::RightLeft,
    };
    let group_members: Vec<Vec<String>> = {
        let mut gm: Vec<Vec<String>> = g.groups.iter()
            .map(|gr| gr.members.iter().map(|m| node_id::wire_id(m)).collect())
            .collect();
        gm.sort_by(|a, b| b.len().cmp(&a.len()));
        gm
    };
    let used = if dir.is_directional() {
        graph_layout::seed_grid(&mut cards, &graph_layout::LayoutConfig::default());
        layered_layout::layout_layered_grouped(&mut cards, &edges, dir,
            &layered_layout::LayeredConfig::default(), &group_members)
    } else {
        let cfg = graph_layout::LayoutConfig::default();
        graph_layout::seed_grid(&mut cards, &cfg);
        graph_layout::layout_force_directed(&mut cards, &edges, &cfg);
        false
    };
    let labels: Vec<String> = cards.iter().map(|c| {
        g.find_node(node_id::strip_ns(&c.id))
            .map(|n| if n.label.is_empty() { n.id.clone() } else { n.label.clone() })
            .unwrap_or_default()
    }).collect();
    let name = std::path::Path::new(path).file_stem()
        .map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "graph".into());
    let placed: Vec<group_layout::PlacedCard> = cards.iter()
        .map(|c| group_layout::PlacedCard { id: c.id.clone(), x: c.x, y: c.y,
            half_w: c.half_w, half_h: c.half_h }).collect();
    let groups: Vec<group_layout::Group> = g.groups.iter()
        .map(|gr| group_layout::Group { id: gr.id.clone(), title: gr.title.clone(),
            members: gr.members.clone() }).collect();
    let boxes = group_layout::compute_group_boxes(&placed, &groups);
    Laid { name, dir: format!("{:?}", dir), layered: used, cards, edges, labels, boxes }
}

/// Count edge crossings by segment intersection (same approach as the harness).
fn count_crossings(l: &Laid) -> usize {
    let pos = |id: &str| l.cards.iter().find(|c| c.id == id).map(|c| (c.x, c.y));
    let segs: Vec<((f32, f32), (f32, f32))> = l.edges.iter()
        .filter_map(|e| Some((pos(&e.from)?, pos(&e.to)?))).collect();
    fn ccw(a: (f32, f32), b: (f32, f32), c: (f32, f32)) -> bool {
        (c.1 - a.1) * (b.0 - a.0) > (b.1 - a.1) * (c.0 - a.0)
    }
    let mut n = 0;
    for i in 0..segs.len() {
        for j in (i + 1)..segs.len() {
            let (a, b) = segs[i];
            let (c, d) = segs[j];
            if ccw(a, c, d) != ccw(b, c, d) && ccw(a, b, c) != ccw(a, b, d) { n += 1; }
        }
    }
    n
}

fn bounds(l: &Laid) -> (f32, f32, f32, f32) {
    let mut minx = f32::MAX; let mut miny = f32::MAX;
    let mut maxx = f32::MIN; let mut maxy = f32::MIN;
    for c in &l.cards {
        minx = minx.min(c.x - c.half_w); miny = miny.min(c.y - c.half_h);
        maxx = maxx.max(c.x + c.half_w); maxy = maxy.max(c.y + c.half_h);
    }
    if l.cards.is_empty() { return (0.0, 0.0, 0.0, 0.0); }
    (minx, miny, maxx, maxy)
}

fn count_overlaps(l: &Laid) -> usize {
    let mut n = 0;
    for i in 0..l.cards.len() {
        for j in (i + 1)..l.cards.len() {
            let a = &l.cards[i]; let b = &l.cards[j];
            let ox = (a.x - b.x).abs() < (a.half_w + b.half_w);
            let oy = (a.y - b.y).abs() < (a.half_h + b.half_h);
            if ox && oy { n += 1; }
        }
    }
    n
}

fn print_json(l: &Laid) {
    print!("{{\"name\":\"{}\",\"dir\":\"{}\",\"layered\":{},\"nodes\":[",
           esc(&l.name), l.dir, l.layered);
    for (i, c) in l.cards.iter().enumerate() {
        if i > 0 { print!(","); }
        print!("{{\"id\":\"{}\",\"label\":\"{}\",\"x\":{:.1},\"y\":{:.1},\"hw\":{:.1},\"hh\":{:.1}}}",
               esc(&c.id), esc(&l.labels[i]), c.x, c.y, c.half_w, c.half_h);
    }
    print!("],\"edges\":[");
    for (i, e) in l.edges.iter().enumerate() {
        if i > 0 { print!(","); }
        print!("{{\"from\":\"{}\",\"to\":\"{}\"}}", esc(&e.from), esc(&e.to));
    }
    println!("]}}");
}

fn print_report(l: &Laid) {
    let (minx, miny, maxx, maxy) = bounds(l);
    let w = (maxx - minx).max(1.0);
    let h = (maxy - miny).max(1.0);
    let aspect = if h > w { h / w } else { w / h };
    let crossings = count_crossings(l);
    let overlaps = count_overlaps(l);
    println!("graph: {}", l.name);
    println!("  direction : {}{}", l.dir, if l.layered { " (layered)" } else { " (force-directed)" });
    println!("  nodes     : {}", l.cards.len());
    println!("  edges     : {}", l.edges.len());
    println!("  bounds    : {:.0} x {:.0}  (x {:.0}..{:.0}, y {:.0}..{:.0})", w, h, minx, maxx, miny, maxy);
    println!("  aspect    : {:.2}  ({})", aspect,
             if aspect <= 1.8 { "balanced" } else if aspect <= 4.0 { "wide" } else { "flat-and-long" });
    println!("  crossings : {}", crossings);
    println!("  overlaps  : {}", overlaps);
    if !l.boxes.is_empty() {
        let total_area: f32 = l.boxes.iter().map(|b| b.w * b.h).sum();
        println!("  groups    : {} (total box area {:.0})", l.boxes.len(), total_area);
        for b in &l.boxes {
            println!("    {:<20} {:.0} x {:.0}  ({} members, depth {})",
                b.title.chars().take(20).collect::<String>(), b.w, b.h, b.member_count, b.depth);
        }
    }
    println!();
    println!("  positions:");
    for (i, c) in l.cards.iter().enumerate() {
        println!("    {:<24} ({:>7.1}, {:>7.1})", l.labels[i].chars().take(24).collect::<String>(), c.x, c.y);
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() || args.iter().any(|a| a == "--help" || a == "-h") {
        eprintln!("layout_tool — evaluate web2qml graph layout on any mermaid file");
        eprintln!();
        eprintln!("USAGE:");
        eprintln!("  layout_tool <file.mmd | file.md> [--json]");
        eprintln!();
        eprintln!("Runs the real mermaid parser + layered layout and reports node");
        eprintln!("positions plus legibility metrics (aspect, crossings, overlaps,");
        eprintln!("bounds). --json emits positions+edges for scripts/layout_analyze.py.");
        exit(if args.is_empty() { 1 } else { 0 });
    }
    let json = args.iter().any(|a| a == "--json");
    let files: Vec<&String> = args.iter().filter(|a| !a.starts_with("--")).collect();
    if files.is_empty() { eprintln!("error: no input file"); exit(1); }

    if json { print!("["); }
    for (gi, path) in files.iter().enumerate() {
        let raw = match std::fs::read_to_string(path.as_str()) {
            Ok(s) => s,
            Err(e) => { eprintln!("error: cannot read {}: {}", path, e); exit(1); }
        };
        let src = extract_mermaid(path, &raw);
        let laid = run_layout(path, &src);
        if json {
            if gi > 0 { print!(","); }
            print_json(&laid);
        } else {
            if gi > 0 { println!(); }
            print_report(&laid);
        }
    }
    if json { println!("]"); }
}
