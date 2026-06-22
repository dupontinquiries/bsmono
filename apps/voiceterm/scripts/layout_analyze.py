#!/usr/bin/env python3
"""v0.1.740: layout-quality analyzer. Reads node positions + edges produced by
layout_metrics.sh, computes legibility metrics for each graph, and derives
concrete lessons about how to improve the layout (vs a mature renderer like
mermaid/dagre). Pure analysis — no live code touched.

Metrics chosen to approximate what makes a layered diagram READABLE:
  aspect ratio, edge crossings, edge length stats, backward edges, layer
  occupancy balance, node overlaps, source clustering, label-fit, edge-angle
  spread, disconnected nodes, layer count vs node count.
"""
import json, sys, math
from collections import defaultdict

CHAR_W = 5.5  # must match card_density

def seg_intersect(p1, p2, p3, p4):
    """Do segments p1p2 and p3p4 properly cross?"""
    def ccw(a, b, c):
        return (c[1]-a[1])*(b[0]-a[0]) - (b[1]-a[1])*(c[0]-a[0])
    d1, d2 = ccw(p3, p4, p1), ccw(p3, p4, p2)
    d3, d4 = ccw(p1, p2, p3), ccw(p1, p2, p4)
    if ((d1 > 0) != (d2 > 0)) and ((d3 > 0) != (d4 > 0)):
        return True
    return False

def analyze(g):
    nodes = {n["id"]: n for n in g["nodes"]}
    edges = [(e["from"], e["to"]) for e in g["edges"]
             if e["from"] in nodes and e["to"] in nodes]
    n = len(g["nodes"])
    horiz = g["dir"] in ("LeftRight", "RightLeft")
    m = {}

    # bounding box + aspect ratio
    xs = [nd["x"] for nd in g["nodes"]]; ys = [nd["y"] for nd in g["nodes"]]
    hws = [nd["hw"] for nd in g["nodes"]]; hhs = [nd["hh"] for nd in g["nodes"]]
    if not xs:
        return {"name": g["name"], "empty": True}
    w = (max(x+h for x, h in zip(xs, hws)) - min(x-h for x, h in zip(xs, hws)))
    h = (max(y+hh for y, hh in zip(ys, hhs)) - min(y-hh for y, hh in zip(ys, hhs)))
    m["aspect"] = round(w / h, 2) if h > 0 else 0
    m["bbox"] = (round(w), round(h))

    # edge geometry
    def center(i): return (nodes[i]["x"], nodes[i]["y"])
    lengths = [math.dist(center(a), center(b)) for a, b in edges]
    m["edge_count"] = len(edges)
    m["edge_len_mean"] = round(sum(lengths)/len(lengths), 1) if lengths else 0
    m["edge_len_max"] = round(max(lengths), 1) if lengths else 0
    m["edge_len_cv"] = round((statistics_std(lengths)/m["edge_len_mean"]), 2) if lengths and m["edge_len_mean"] else 0

    # backward edges (against flow): in TD, child y should be > parent y
    back = 0
    for a, b in edges:
        ca, cb = center(a), center(b)
        if g["dir"] == "TopDown" and cb[1] < ca[1] - 1: back += 1
        elif g["dir"] == "BottomTop" and cb[1] > ca[1] + 1: back += 1
        elif g["dir"] == "LeftRight" and cb[0] < ca[0] - 1: back += 1
        elif g["dir"] == "RightLeft" and cb[0] > ca[0] + 1: back += 1
    m["backward_edges"] = back

    # edge crossings
    crossings = 0
    for i in range(len(edges)):
        for j in range(i+1, len(edges)):
            a, b = edges[i]; c, d = edges[j]
            if len({a, b, c, d}) < 4: continue  # share an endpoint
            if seg_intersect(center(a), center(b), center(c), center(d)):
                crossings += 1
    m["crossings"] = crossings

    # layer occupancy (group by flow-axis coordinate)
    layer_key = (lambda nd: round(nd["x"])) if horiz else (lambda nd: round(nd["y"]))
    layers = defaultdict(list)
    for nd in g["nodes"]: layers[layer_key(nd)].append(nd)
    occ = sorted(len(v) for v in layers.values())
    m["layer_count"] = len(layers)
    m["layer_occ_max"] = occ[-1] if occ else 0
    m["layer_occ_mean"] = round(sum(occ)/len(occ), 1) if occ else 0
    m["layer_imbalance"] = round(occ[-1]/(sum(occ)/len(occ)), 2) if occ and sum(occ) else 0

    # source clustering: fraction of nodes with no incoming edge sitting in the
    # first layer
    indeg = defaultdict(int)
    for a, b in edges: indeg[b] += 1
    sources = [nd["id"] for nd in g["nodes"] if indeg[nd["id"]] == 0]
    m["source_nodes"] = len(sources)
    first_layer_key = min(layers.keys()) if not g["dir"] in ("BottomTop", "RightLeft") else max(layers.keys())
    first_layer_ids = {nd["id"] for nd in layers.get(first_layer_key, [])}
    m["sources_in_first_layer"] = sum(1 for s in sources if s in first_layer_ids)
    m["source_frac"] = round(len(sources)/n, 2) if n else 0

    # node overlaps (boxes intersecting)
    overlaps = 0
    nl = g["nodes"]
    for i in range(len(nl)):
        for j in range(i+1, len(nl)):
            a, b = nl[i], nl[j]
            if (abs(a["x"]-b["x"]) < a["hw"]+b["hw"]) and (abs(a["y"]-b["y"]) < a["hh"]+b["hh"]):
                overlaps += 1
    m["node_overlaps"] = overlaps

    # label fit: how many labels exceed their box width (cut off)
    cut = 0
    for nd in g["nodes"]:
        need = len(nd["label"]) * CHAR_W + 16
        if need > nd["hw"]*2 + 1: cut += 1
    m["labels_cut_off"] = cut
    m["label_cut_frac"] = round(cut/n, 2) if n else 0

    # disconnected nodes
    connected = set()
    for a, b in edges: connected.add(a); connected.add(b)
    m["disconnected_nodes"] = sum(1 for nd in g["nodes"] if nd["id"] not in connected)

    # edge angle spread: stdev of edge angles (clean layered = consistent)
    angles = []
    for a, b in edges:
        ca, cb = center(a), center(b)
        angles.append(math.degrees(math.atan2(cb[1]-ca[1], cb[0]-ca[0])))
    m["edge_angle_std"] = round(statistics_std(angles), 1) if len(angles) > 1 else 0

    m["groups"] = g["groups"]
    m["layered"] = g["layered"]
    m["nodes"] = n
    return {"name": g["name"], "dir": g["dir"], **m}

def statistics_std(xs):
    if len(xs) < 2: return 0.0
    mean = sum(xs)/len(xs)
    return math.sqrt(sum((x-mean)**2 for x in xs)/len(xs))

def derive_lessons(results):
    """Cross-graph rules → concrete, actionable lessons."""
    L = []
    def add(sev, text): L.append((sev, text))

    aspects = [r["aspect"] for r in results if not r.get("empty")]
    flat = [r for r in results if not r.get("empty") and (r["aspect"] > 2.5 or r["aspect"] < 0.4)]
    if flat:
        names = ", ".join(r["name"] for r in flat)
        add("HIGH", f"Aspect ratio is extreme on {len(flat)}/{len(results)} graphs ({names}) — "
            f"layouts read 'flat and long'. Target ~1.0-1.8. Cause: layer_gap/sibling_gap "
            f"ratio + single-chain graphs stretch one axis. Fix: cap the flow-axis extent or "
            f"scale gaps to graph shape; for long chains, allow wrapping a chain across columns.")

    cut = [r for r in results if not r.get("empty") and r["label_cut_frac"] > 0.3]
    if cut:
        worst = max(cut, key=lambda r: r["label_cut_frac"])
        add("HIGH", f"{len(cut)} graphs have >30% labels cut off (worst {worst['name']} at "
            f"{int(worst['label_cut_frac']*100)}%). Compact density boxes are too narrow for "
            f"hardware labels. Fix: default DIRECTIONAL flowcharts to Expanded density, or raise "
            f"the compact min-width floor to fit short technical labels (≈12-16 chars).")

    imbal = [r for r in results if not r.get("empty") and r["layer_imbalance"] > 2.0]
    if imbal:
        worst = max(imbal, key=lambda r: r["layer_imbalance"])
        add("HIGH", f"Layer occupancy is lopsided on {len(imbal)} graphs (worst {worst['name']}: "
            f"max layer has {worst['layer_occ_max']} nodes vs mean {worst['layer_occ_mean']}). "
            f"Source/control nodes pile into one layer. Fix: place in-edge-less nodes adjacent to "
            f"their single successor instead of all at rank 0.")

    srcheavy = [r for r in results if not r.get("empty") and r["source_frac"] > 0.25]
    if srcheavy:
        names = ", ".join(r["name"] for r in srcheavy)
        add("MED", f"High source-node fraction on {names} (>25% have no incoming edge). Many are "
            f"control signals joined by undirected '---'. Fix: treat '---' (Line) edges as "
            f"placement hints so signals dock beside their consumer, not at the top rank.")

    crossy = [r for r in results if not r.get("empty") and r["crossings"] > max(2, r["edge_count"]*0.15)]
    if crossy:
        worst = max(crossy, key=lambda r: r["crossings"])
        add("MED", f"{len(crossy)} graphs exceed the crossing budget (worst {worst['name']}: "
            f"{worst['crossings']} crossings on {worst['edge_count']} edges). The median-heuristic "
            f"ordering helps but isn't converging. Fix: more sweeps, or weight by edge count, or "
            f"add a transpose/adjacent-swap pass after the median sweeps.")

    backy = [r for r in results if not r.get("empty") and r["backward_edges"] > 0]
    if backy:
        names = ", ".join(f"{r['name']}({r['backward_edges']})" for r in backy)
        add("MED", f"Backward edges present ({names}) — feedback loops (CPU stall/forward, power "
            f"feedback) point against flow and read as errors. Fix: detect cycles (we already find "
            f"back-edges for layering) and render them as visually distinct curved 'return' edges.")

    ov = [r for r in results if not r.get("empty") and r["node_overlaps"] > 0]
    if ov:
        add("HIGH", f"{len(ov)} graphs still have node overlaps ({', '.join(r['name'] for r in ov)}). "
            f"Within-layer packing isn't accounting for full box extents after centering. Fix: "
            f"verify sibling_gap uses both cards' half-extents; re-check the centering shift math.")
    else:
        add("INFO", "No node overlaps in any corpus graph — within-layer packing is collision-free.")

    disc = [r for r in results if not r.get("empty") and r["disconnected_nodes"] > 0]
    if disc:
        names = ", ".join(f"{r['name']}({r['disconnected_nodes']})" for r in disc)
        add("MED", f"Disconnected nodes float with no edges ({names}) — usually parse misses "
            f"(breakout/wire nodes) or label-only decls. Fix: drop truly isolated synthetic nodes, "
            f"or grid them in a footer instead of scattering.")

    grp = [r for r in results if not r.get("empty") and r["groups"] > 0]
    if grp:
        names = ", ".join(f"{r['name']}({r['groups']})" for r in grp)
        add("HIGH", f"Subgraph groups parsed but NOT drawn ({names}). The reference's legibility "
            f"comes largely from labeled group boxes (Data Memory, Write Registers). Fix: render a "
            f"titled container rect behind member cards AND bias the layout to keep members "
            f"contiguous within their layers.")

    angly = [r for r in results if not r.get("empty") and r["edge_angle_std"] > 50]
    if angly:
        worst = max(angly, key=lambda r: r["edge_angle_std"])
        add("LOW", f"Edge angles vary widely (worst {worst['name']}: σ={worst['edge_angle_std']}°). "
            f"A clean layered look has mostly parallel edges. Fix: orthogonal edge routing "
            f"(L-shaped) or align nodes to their parent's cross-axis position when single-child.")

    longe = [r for r in results if not r.get("empty") and r["edge_len_cv"] > 0.6]
    if longe:
        add("LOW", f"{len(longe)} graphs have high edge-length variance (CV>0.6) — some edges span "
            f"many layers. Fix: insert virtual 'dummy' nodes on long edges (standard Sugiyama step "
            f"we skip) so edges step through intermediate layers and stay short + routable.")

    # always-on structural observations
    nonlayered = [r for r in results if not r.get("empty") and not r["layered"]]
    if nonlayered:
        add("INFO", f"{len(nonlayered)} graphs fell back to force-directed ({', '.join(r['name'] for r in nonlayered)}) "
            f"— either edgeless or non-directional. Confirm that's intended.")

    add("LOW", "Add dummy nodes for multi-layer edges (skipped Sugiyama phase) — biggest single "
        "lever for both crossing reduction AND short routable edges.")
    add("LOW", "Consider per-graph gap scaling: small graphs (full_adder) want tighter gaps; large "
        "ones (mips) want more — a fixed 90px layer_gap over-stretches small graphs.")
    return L

def main():
    data = json.load(open(sys.argv[1]))
    results = [analyze(g) for g in data]
    print("\n" + "="*72)
    print("LAYOUT METRICS  (per graph)")
    print("="*72)
    cols = ["name","nodes","aspect","crossings","backward_edges","layer_imbalance",
            "node_overlaps","label_cut_frac","disconnected_nodes","edge_angle_std","groups"]
    print("  " + " | ".join(f"{c[:9]:>9}" for c in cols))
    for r in results:
        if r.get("empty"): continue
        print("  " + " | ".join(f"{str(r.get(c,'')):>9}" for c in cols))
    print("\n" + "="*72)
    print("LESSONS LEARNED")
    print("="*72)
    lessons = derive_lessons(results)
    order = {"HIGH":0,"MED":1,"LOW":2,"INFO":3}
    lessons.sort(key=lambda x: order.get(x[0],9))
    for i,(sev,text) in enumerate(lessons,1):
        print(f"\n{i:2}. [{sev}] {text}")
    print(f"\n  → {len(lessons)} lessons across {len([r for r in results if not r.get('empty')])} graphs.")

if __name__ == "__main__":
    main()
