// src/mermaid.rs
//
// v0.1.326 (Mermaid+/cards arc 2.7.0): Mermaid flowchart parser
// (subset).
//
// What this parses:
//   - graph TD / graph LR / graph TB / graph BT / graph RL
//   - flowchart (alias of graph)
//   - %% comments (line and end-of-line)
//   - Node forms: A, A[Label], A(Round), A((Circle)), A{Diamond},
//     A[/Trapezoid/], A[\InvTrap\], A>Asym]
//   - Edges:
//       A --> B      arrow
//       A --- B      line
//       A ==> B      thick
//       A -.-> B     dotted
//       A -->|lbl| B labeled
//       A -- lbl --> B  alternate label form
//   - Chained edges: A --> B --> C
//   - Implicit node declaration: nodes appear in edges before
//     any explicit shape declaration
//
// What this does NOT parse (future ships):
//   - subgraph X ... end  (groups)
//   - classDef / class statements  (styling)
//   - linkStyle / style                (styling)
//   - click X cb "label" / interaction
//   - Quoted node IDs with special chars: "my node"
//   - Line continuations (\n at end of line)
//   - Sequence diagrams, classes, ER, etc.
//
// Architecture:
//   - Tokenizer (split lines, strip comments)
//   - Per-line recognizer for: header, edge, node-decl
//   - Build `MermaidGraph { direction, nodes, edges }`
//   - Errors collected per-line; parser is permissive — unknown
//     lines are skipped rather than fatal. The cards canvas will
//     show whatever the parser CAN extract.
//
// Why hand-rolled rather than a parser combinator:
//   - Mermaid's grammar is small enough that recursive descent
//     stays under 400 lines and is easier to debug.
//   - No new dep; aligns with the project's lean-deps stance.
//   - Permissive error model fits hand-written tokenizers better
//     than declarative grammars.

#![allow(dead_code)]

/// Layout direction declared in the Mermaid header.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    /// Top-down (graph TD / graph TB).
    TopDown,
    /// Bottom-up (graph BT).
    BottomTop,
    /// Left-right (graph LR).
    LeftRight,
    /// Right-left (graph RL).
    RightLeft,
}

impl Default for Direction {
    fn default() -> Self { Direction::TopDown }
}

/// Visual shape declared via brackets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NodeShape {
    /// Default — node with no explicit shape decl (just an id).
    Default,
    /// `A[Label]` — rectangle.
    Rectangle,
    /// `A(Label)` — rounded rectangle.
    Rounded,
    /// `A((Label))` — circle.
    Circle,
    /// `A{Label}` — diamond.
    Diamond,
    /// `A[/Label/]` — parallelogram (right).
    Parallelogram,
    /// `A[\Label\]` — parallelogram (left).
    ParallelogramAlt,
    /// `A>Label]` — asymmetric (flag-style).
    Asymmetric,
}

/// Edge line style (the dashes).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EdgeStyle {
    /// Standard `-->` arrow.
    Arrow,
    /// Plain `---` line (no head).
    Line,
    /// Thick `==>` arrow.
    Thick,
    /// Dotted `-.->` arrow.
    Dotted,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MermaidNode {
    /// Identifier as it appears in the source (case-sensitive).
    pub id: String,
    /// Visible label — defaults to id if no `[...]` form used.
    pub label: String,
    pub shape: NodeShape,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MermaidEdge {
    pub from: String,
    pub to: String,
    pub style: EdgeStyle,
    /// Optional edge label (`-->|text|` or `-- text -->`).
    pub label: Option<String>,
}

/// Parsed graph. `nodes` is deduplicated by id; later
/// declarations override earlier ones (so an explicit shape
/// after an implicit-via-edge declaration wins).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MermaidGraph {
    pub direction: Direction,
    pub nodes: Vec<MermaidNode>,
    pub edges: Vec<MermaidEdge>,
    /// v0.1.739: subgraph groups (id, title, member node ids). The renderer
    /// can draw a labeled box around members; the layout can keep members
    /// together. Empty for graphs with no `subgraph` blocks.
    pub groups: Vec<SubgraphInfo>,
    /// Lines that didn't match any rule. Captured for diagnostic
    /// reporting rather than discarded.
    pub unrecognized: Vec<String>,
}

/// A `subgraph ID["Title"] ... end` group.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SubgraphInfo {
    pub id: String,
    pub title: String,
    pub members: Vec<String>,
}

impl MermaidGraph {
    /// Look up a node by id. Linear scan; fine at expected scale
    /// (most diagrams < 100 nodes).
    pub fn find_node(&self, id: &str) -> Option<&MermaidNode> {
        self.nodes.iter().find(|n| n.id == id)
    }

    /// Ensure a node with the given id exists, inserting a
    /// Default-shaped one if missing. Returns whether a new
    /// entry was added.
    fn ensure_node(&mut self, id: &str) -> bool {
        if self.find_node(id).is_some() { return false; }
        self.nodes.push(MermaidNode {
            id: id.to_string(),
            label: id.to_string(),
            shape: NodeShape::Default,
        });
        true
    }
}

/// Parse a Mermaid source string. Always returns a graph
/// (possibly empty). Lines that don't match any rule are
/// collected in `graph.unrecognized` for diagnostics; they do
/// not abort the parse.
pub fn parse(source: &str) -> MermaidGraph {
    // v0.1.444: mindmap diagrams use INDENTATION hierarchy, not
    // `A --> B` edges. Detect the `mindmap` header and route to
    // the dedicated indentation parser — the flowchart parser
    // below would mis-extract the `NAME["label"]` lines as bare
    // nodes with no hierarchy (the cause of the scattered,
    // quote-littered boxes the user saw).
    for raw_line in source.lines() {
        let l = strip_comment(raw_line).trim();
        if l.is_empty() { continue; }
        if l.eq_ignore_ascii_case("mindmap") {
            return parse_mindmap(source);
        }
        break; // first non-empty line wasn't `mindmap` → flowchart
    }
    let mut g = MermaidGraph::default();
    let mut saw_header = false;
    // v0.1.739: subgraph nesting. `subgraph ID["Title"]` opens a group and
    // `end` closes it; nodes declared inside belong to the innermost open
    // group. The declaration line itself is NOT a node (the dogfood diagram
    // showed bogus "subgra…"/"Write Registers"/"end" cards because these were
    // parsed as nodes). We record membership so a later ship can draw group
    // boxes; for now it just prevents the spurious cards.
    let mut group_stack: Vec<String> = Vec::new();
    for raw_line in source.lines() {
        let line = strip_comment(raw_line).trim();
        if line.is_empty() { continue; }
        // First non-comment, non-empty line must be the header.
        if !saw_header {
            if let Some(dir) = parse_header(line) {
                g.direction = dir;
                saw_header = true;
                continue;
            }
            // No header? Default to TopDown and fall through —
            // user might have skipped the declaration.
            saw_header = true;
        }
        // v0.1.739: subgraph open / close — handled before node/edge parsing
        // so the declaration never becomes a card.
        if let Some(rest) = line.strip_prefix("subgraph") {
            // forms: `subgraph ID["Title"]`, `subgraph ID[Title]`,
            // `subgraph Title`, or bare `subgraph`.
            let rest = rest.trim();
            let (gid, gtitle) = parse_subgraph_decl(rest);
            g.groups.push(SubgraphInfo { id: gid.clone(), title: gtitle, members: Vec::new() });
            group_stack.push(gid);
            continue;
        }
        if line.eq_ignore_ascii_case("end") {
            group_stack.pop();
            continue;
        }
        // Try to recognize as an edge first (more specific
        // pattern). Edge tokens are distinctive (`-->`, `---`,
        // etc); node declarations are bare.
        let nodes_before = g.nodes.len();
        let edges_before = g.edges.len();
        if try_parse_edge_line(line, &mut g) {
            assign_new_nodes_to_group(&mut g, nodes_before, &group_stack);
            // v0.1.759: edge endpoints declared inside a subgraph are members
            // even when the node pre-existed (the dogfood MIPS bug: only the
            // last, newly-created member of each subgraph was captured).
            assign_edge_endpoints_to_group(&mut g, edges_before, &group_stack);
            continue;
        }
        // Then try a standalone node declaration like:
        //   A[Label]
        //   B(Round)
        //   C
        if try_parse_node_decl(line, &mut g) {
            assign_new_nodes_to_group(&mut g, nodes_before, &group_stack);
            // a node DECLARED inside a subgraph is a member even if it was
            // already referenced earlier (so not "new").
            assign_decl_node_to_group(line, &mut g, &group_stack);
            continue;
        }
        // Doesn't match — record for diagnostics.
        g.unrecognized.push(line.to_string());
    }
    g
}

/// Parse the bit after `subgraph`: `ID["Title"]` / `ID[Title]` / `Title` /
/// (empty). Returns (id, title). If no bracket, the whole thing is both.
fn parse_subgraph_decl(rest: &str) -> (String, String) {
    if rest.is_empty() {
        return ("sg".to_string(), String::new());
    }
    if let Some(bopen) = rest.find('[') {
        let id = rest[..bopen].trim().to_string();
        let inner = &rest[bopen + 1..];
        let title = inner.rfind(']').map(|e| unquote(inner[..e].trim())).unwrap_or_default();
        let id = if id.is_empty() { title.clone() } else { id };
        return (id, title);
    }
    // bare title, no explicit id
    (unquote(rest), unquote(rest))
}

/// v0.1.759: attach both endpoints of edges added since `before` to the
/// innermost open group. Unlike assign_new_nodes_to_group this captures
/// PRE-EXISTING nodes too — an edge written inside a subgraph block means both
/// its endpoints belong to that subgraph, even if they were created earlier.
fn assign_edge_endpoints_to_group(g: &mut MermaidGraph, before: usize, stack: &[String]) {
    let Some(group_id) = stack.last() else { return };
    let endpoints: Vec<String> = g.edges[before..].iter()
        .flat_map(|e| [e.from.clone(), e.to.clone()])
        .collect();
    if let Some(grp) = g.groups.iter_mut().find(|gr| &gr.id == group_id) {
        for id in endpoints {
            if !grp.members.contains(&id) { grp.members.push(id); }
        }
    }
}

/// v0.1.759: attach the node id DECLARED on `line` (e.g. `5a["Read register 1"]`)
/// to the innermost open group, even when the node already existed (so it wasn't
/// "new"). Mirrors how try_parse_node_decl extracts the leading id.
fn assign_decl_node_to_group(line: &str, g: &mut MermaidGraph, stack: &[String]) {
    let Some(group_id) = stack.last() else { return };
    // id is the leading token up to the first shape bracket / quote / space.
    let t = line.trim();
    let end = t.find(|c| "[(.{<\" \t".contains(c)).unwrap_or(t.len());
    let id = t[..end].trim().to_string();
    if id.is_empty() { return; }
    if let Some(grp) = g.groups.iter_mut().find(|gr| &gr.id == group_id) {
        if !grp.members.contains(&id) { grp.members.push(id); }
    }
}

/// Attach any nodes added since `before` to the innermost open group.
fn assign_new_nodes_to_group(g: &mut MermaidGraph, before: usize, stack: &[String]) {
    let Some(group_id) = stack.last() else { return };
    let new_ids: Vec<String> = g.nodes[before..].iter().map(|n| n.id.clone()).collect();
    if let Some(grp) = g.groups.iter_mut().find(|gr| &gr.id == group_id) {
        for id in new_ids {
            if !grp.members.contains(&id) { grp.members.push(id); }
        }
    }
}

/// v0.1.444: parse a Mermaid `mindmap` diagram. Unlike flowcharts,
/// mindmaps encode hierarchy by INDENTATION: each node's parent is
/// the nearest preceding line with a smaller indent. We synthesize
/// parent->child edges so the existing force-directed layout +
/// cards renderer can display it.
///
/// Node forms recognized (after the `mindmap` header):
///   root((Label))   -- root, Circle shape
///   NAME["Label"]   -- explicit id + bracketed/quoted label
///   NAME[Label]     -- explicit id + bracketed label
///   Label           -- bare text (id synthesized)
///
/// Literal `\n` (backslash-n) inside labels — common in these
/// hand-authored mindmaps — is converted to a space so the card
/// shows "FILM 2 DOMINION" instead of "FILM 2\nDOMINION".
fn parse_mindmap(source: &str) -> MermaidGraph {
    let mut g = MermaidGraph::default();
    // Stack of (indent, node_id) tracking current ancestry.
    let mut stack: Vec<(usize, String)> = Vec::new();
    let mut auto_id = 0usize;
    let mut saw_header = false;

    for raw_line in source.lines() {
        // Indent = leading-whitespace width (relative ordering is
        // all that matters).
        let indent = raw_line.len() - raw_line.trim_start().len();
        let line = strip_comment(raw_line).trim();
        if line.is_empty() { continue; }
        if !saw_header {
            if line.eq_ignore_ascii_case("mindmap") {
                saw_header = true;
                continue;
            }
            saw_header = true; // tolerate a missing header
        }

        let (id, raw_label, shape) = parse_mindmap_node(line, &mut auto_id);
        let label = normalize_label(&raw_label);

        // Pop to the parent: nearest entry with STRICTLY smaller
        // indent than this line.
        while let Some(&(top_indent, _)) = stack.last() {
            if top_indent >= indent { stack.pop(); } else { break; }
        }
        if let Some((_, parent_id)) = stack.last() {
            g.edges.push(MermaidEdge {
                from: parent_id.clone(),
                to: id.clone(),
                style: EdgeStyle::Line,
                label: None,
            });
        }
        if let Some(existing) = g.nodes.iter_mut().find(|n| n.id == id) {
            existing.label = label;
            existing.shape = shape;
        } else {
            g.nodes.push(MermaidNode { id: id.clone(), label, shape });
        }
        stack.push((indent, id));
    }
    g
}

/// Parse one mindmap line into (id, label, shape). Handles
/// `root((L))`, `NAME["L"]`, `NAME[L]`, and bare `L`.
fn parse_mindmap_node(line: &str, auto_id: &mut usize) -> (String, String, NodeShape) {
    if let Some(rest) = line.strip_prefix("root((") {
        let label = rest.strip_suffix("))").unwrap_or(rest);
        return ("root".to_string(), unquote(label), NodeShape::Circle);
    }
    if let Some((id, idlen)) = read_identifier(line) {
        let rest = line[idlen..].trim_start();
        if let Some(inner) = rest.strip_prefix('[') {
            if let Some(end) = inner.rfind(']') {
                let label = unquote(inner[..end].trim());
                return (id, label, NodeShape::Default);
            }
        }
        if rest.is_empty() {
            return (id.clone(), id, NodeShape::Default);
        }
    }
    *auto_id += 1;
    (format!("mm_{}", auto_id), unquote(line), NodeShape::Default)
}

/// Strip a single layer of surrounding double-quotes if present.
fn unquote(s: &str) -> String {
    let t = s.trim();
    let inner = if t.len() >= 2 && t.starts_with('"') && t.ends_with('"') {
        &t[1..t.len() - 1]
    } else {
        t
    };
    // v0.1.741: collapse literal escape sequences authors write inside labels
    // (`4\nconst`, `Upper\tALU`) to spaces — they were showing as raw `\n` text
    // in cards. Real newlines/tabs are likewise normalized to spaces (cards are
    // single-flow text; the wire can't carry \n anyway).
    inner.replace("\\n", " ")
         .replace("\\t", " ")
         .replace('\n', " ")
         .replace('\t', " ")
         .split_whitespace()
         .collect::<Vec<_>>()
         .join(" ")
}

/// Convert literal `\n` to spaces and collapse whitespace runs so
/// a multi-line mindmap label renders as one clean card line.
fn normalize_label(s: &str) -> String {
    let spaced = s.replace("\\n", " ");
    spaced.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Strip `%%` comments — both line-leading and end-of-line. The
/// `%%` token must be at a word boundary, otherwise `100%%` in a
/// label would be misparsed. Mermaid's actual grammar is also
/// permissive here; we keep it simple.
fn strip_comment(line: &str) -> &str {
    if let Some(idx) = line.find("%%") {
        &line[..idx]
    } else {
        line
    }
}

fn parse_header(line: &str) -> Option<Direction> {
    let lc = line.to_lowercase();
    let lc = lc.trim();
    // Accept "graph TD", "graph LR", "flowchart TD" etc.
    let parts: Vec<&str> = lc.split_whitespace().collect();
    if parts.len() < 2 { return None; }
    if parts[0] != "graph" && parts[0] != "flowchart" { return None; }
    match parts[1] {
        "td" | "tb" => Some(Direction::TopDown),
        "bt" => Some(Direction::BottomTop),
        "lr" => Some(Direction::LeftRight),
        "rl" => Some(Direction::RightLeft),
        _ => None,
    }
}

/// Try to parse the line as a standalone node declaration.
/// Returns true on success (graph mutated), false otherwise.
fn try_parse_node_decl(line: &str, g: &mut MermaidGraph) -> bool {
    if let Some((id, label, shape)) = take_node_with_shape(line) {
        // The remaining-after-node part must be empty for a
        // standalone declaration. Otherwise it's something else.
        let rest = line.split_at(id.len()).1.trim_start();
        let after_shape = strip_shape_braces(rest, shape);
        if after_shape.trim().is_empty() {
            // Either insert new or update existing.
            if let Some(existing) = g.nodes.iter_mut().find(|n| n.id == id) {
                existing.label = label;
                existing.shape = shape;
            } else {
                g.nodes.push(MermaidNode { id, label, shape });
            }
            return true;
        }
    }
    // Bare id (no shape) — only counts as a decl if line is
    // exactly an identifier (and we'd rather see it via an edge).
    let trimmed = line.trim();
    if is_bare_identifier(trimmed) {
        g.ensure_node(trimmed);
        return true;
    }
    false
}

/// Recognize bare identifier — alphanumeric + underscore + hyphen. Mermaid
/// permits ids that START WITH A DIGIT (e.g. `1s`, `5a`, `13c` — common in the
/// MIPS-datapath style diagrams), so accept alphanumeric-or-underscore first,
/// not just alphabetic. (v0.1.738: digit-first ids were silently dropped, so
/// most of a numbered flowchart never parsed.)
fn is_bare_identifier(s: &str) -> bool {
    if s.is_empty() { return false; }
    let mut chars = s.chars();
    let first = chars.next().unwrap();
    if !(first.is_ascii_alphanumeric() || first == '_') { return false; }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// Read an identifier from the start of `s`. Returns (id, len).
fn read_identifier(s: &str) -> Option<(String, usize)> {
    let bytes = s.as_bytes();
    if bytes.is_empty() { return None; }
    let first = bytes[0];
    if !(first.is_ascii_alphanumeric() || first == b'_') { return None; }
    let mut end = 1;
    while end < bytes.len() {
        let b = bytes[end];
        if b.is_ascii_alphanumeric() || b == b'_' || b == b'-' {
            end += 1;
        } else {
            break;
        }
    }
    Some((s[..end].to_string(), end))
}

/// Try to read `<id><shape>` from the start of the line.
/// Returns (id, label, shape) on success. Doesn't consume more
/// than the node — caller checks the rest.
fn take_node_with_shape(line: &str) -> Option<(String, String, NodeShape)> {
    let (id, idlen) = read_identifier(line)?;
    let rest = &line[idlen..];
    let rest_trim = rest.trim_start();
    // Shape detection by leading delimiters.
    if let Some(stripped) = rest_trim.strip_prefix("[/") {
        if let Some(end) = stripped.find("/]") {
            let label = unquote(stripped[..end].trim());
            return Some((id, label, NodeShape::Parallelogram));
        }
    }
    if let Some(stripped) = rest_trim.strip_prefix("[\\") {
        if let Some(end) = stripped.find("\\]") {
            let label = unquote(stripped[..end].trim());
            return Some((id, label, NodeShape::ParallelogramAlt));
        }
    }
    if let Some(stripped) = rest_trim.strip_prefix("((") {
        if let Some(end) = stripped.find("))") {
            let label = unquote(stripped[..end].trim());
            return Some((id, label, NodeShape::Circle));
        }
    }
    if let Some(stripped) = rest_trim.strip_prefix('(') {
        if let Some(end) = stripped.find(')') {
            let label = unquote(stripped[..end].trim());
            return Some((id, label, NodeShape::Rounded));
        }
    }
    if let Some(stripped) = rest_trim.strip_prefix('[') {
        if let Some(end) = stripped.find(']') {
            let label = unquote(stripped[..end].trim());
            return Some((id, label, NodeShape::Rectangle));
        }
    }
    // v0.1.741: {{ }} hexagon (mermaid control-signal shape) — must be tried
    // BEFORE the single `{`, else `{{"RegDst"}}` parses as Diamond with a
    // leading stray `{` in the label (the dogfood `{` artifacts).
    if let Some(stripped) = rest_trim.strip_prefix("{{") {
        if let Some(end) = stripped.find("}}") {
            let label = unquote(stripped[..end].trim());
            return Some((id, label, NodeShape::Diamond));
        }
    }
    if let Some(stripped) = rest_trim.strip_prefix('{') {
        if let Some(end) = stripped.find('}') {
            let label = unquote(stripped[..end].trim());
            return Some((id, label, NodeShape::Diamond));
        }
    }
    if let Some(stripped) = rest_trim.strip_prefix('>') {
        if let Some(end) = stripped.find(']') {
            let label = unquote(stripped[..end].trim());
            return Some((id, label, NodeShape::Asymmetric));
        }
    }
    // No shape — default node.
    Some((id, "".to_string(), NodeShape::Default))
}

/// After parsing a node with shape, return the byte length of
/// the consumed shape so callers can advance past it.
fn shape_consumed_len(rest_trim: &str, shape: NodeShape) -> usize {
    match shape {
        NodeShape::Default => 0,
        NodeShape::Rectangle => rest_trim.find(']').map(|i| i + 1).unwrap_or(0),
        NodeShape::Rounded => rest_trim.find(')').map(|i| i + 1).unwrap_or(0),
        NodeShape::Circle => rest_trim.find("))").map(|i| i + 2).unwrap_or(0),
        NodeShape::Diamond => rest_trim.find('}').map(|i| i + 1).unwrap_or(0),
        NodeShape::Parallelogram => rest_trim.find("/]").map(|i| i + 2).unwrap_or(0),
        NodeShape::ParallelogramAlt => rest_trim.find("\\]").map(|i| i + 2).unwrap_or(0),
        NodeShape::Asymmetric => rest_trim.find(']').map(|i| i + 1).unwrap_or(0),
    }
}

/// Helper for `try_parse_node_decl`: given the full remainder
/// after the id, return what's left after the shape brackets.
fn strip_shape_braces(rest: &str, shape: NodeShape) -> String {
    let rt = rest.trim_start();
    let consumed = shape_consumed_len(rt, shape);
    if consumed == 0 { return rt.to_string(); }
    rt[consumed..].to_string()
}

/// Try to parse the line as an edge expression (possibly with
/// inline node declarations and chains). Returns true on
/// success.
///
/// Strategy: walk left-to-right, consuming `<node><edge><node>...`
/// patterns. Each adjacent pair produces a MermaidEdge.
fn try_parse_edge_line(line: &str, g: &mut MermaidGraph) -> bool {
    let mut cursor = line.trim_start();
    let mut prev_id: Option<String> = None;
    let mut pending_edge: Option<(EdgeStyle, Option<String>)> = None;
    let mut any_progress = false;

    while !cursor.is_empty() {
        // 1. Try to read a node (with optional shape).
        if let Some((id, label, shape)) = take_node_with_shape(cursor) {
            // Advance cursor past the consumed node.
            let after_id = &cursor[id.len()..];
            let after_id_trim = after_id.trim_start();
            let consumed = shape_consumed_len(after_id_trim, shape);
            let total = cursor.len() - after_id.len()
                + (after_id.len() - after_id_trim.len()) + consumed;
            cursor = &cursor[total..];
            // Register node.
            if shape != NodeShape::Default {
                if let Some(existing) = g.nodes.iter_mut().find(|n| n.id == id) {
                    existing.label = label;
                    existing.shape = shape;
                } else {
                    g.nodes.push(MermaidNode { id: id.clone(), label, shape });
                }
            } else {
                g.ensure_node(&id);
            }
            // Pair with the pending edge.
            if let Some(prev) = &prev_id {
                if let Some((style, lbl)) = pending_edge.take() {
                    g.edges.push(MermaidEdge {
                        from: prev.clone(),
                        to: id.clone(),
                        style,
                        label: lbl,
                    });
                    any_progress = true;
                }
            }
            prev_id = Some(id);
            cursor = cursor.trim_start();
            continue;
        }
        // 2. Try to read an edge operator.
        if let Some((style, label, consumed)) = take_edge_op(cursor) {
            pending_edge = Some((style, label));
            cursor = cursor[consumed..].trim_start();
            continue;
        }
        // 3. No match — abort.
        break;
    }
    any_progress
}

/// Try to consume an edge operator from the cursor.
/// Returns (style, optional_label, bytes_consumed).
///
/// Recognized forms:
///   `-->`            Arrow, no label
///   `---`            Line, no label
///   `==>`            Thick, no label
///   `-.->`           Dotted, no label
///   `-->|text|`      Arrow with label (pipe-form)
///   `==>|text|`      Thick with label
///   `-.->|text|`     Dotted with label
///   `-- text -->`    Arrow with label (dash-form)
///   `== text ==>`    Thick with label
/// Clean a dash/thick-form edge label: trim, drop any trailing dashes left by
/// a 3+-dash arrow (`-- text --->` leaves "text -" after the inner `-->`
/// match), and strip a single pair of surrounding double-quotes. (v0.1.738)
fn clean_edge_label(raw: &str) -> String {
    let mut t = raw.trim().trim_end_matches('-').trim().to_string();
    if t.len() >= 2 && t.starts_with('"') && t.ends_with('"') {
        t = t[1..t.len() - 1].to_string();
    }
    t.trim().to_string()
}

fn take_edge_op(s: &str) -> Option<(EdgeStyle, Option<String>, usize)> {
    // Pipe-form labels are checked AFTER the base op succeeds.
    // Dash-form labels are recognized by `-- text -->` shape and
    // consume the trailing arrow.
    //
    // Detect base operator first.

    // 1. Try dash-form labeled: `-- LABEL -->`, `== LABEL ==>`
    if let Some(rest) = s.strip_prefix("--") {
        // Distinguish "--->" / "--->|" (no label) vs "-- label -->"
        let r = rest.trim_start();
        // No-label edge case first:
        if rest.starts_with('-') || rest.starts_with('>') {
            // Fall through to base-op detection below.
        } else if !r.starts_with("-->") && !r.starts_with("--") {
            // Looks like `-- ` followed by content; try to find the
            // closing `-->` or `---`.
            if let Some(end_arrow) = r.find("-->") {
                let label = clean_edge_label(&r[..end_arrow]);
                let consumed = 2 + (rest.len() - r.len()) + end_arrow + 3;
                if !label.is_empty() {
                    return Some((EdgeStyle::Arrow, Some(label), consumed));
                }
            }
            if let Some(end_line) = r.find("---") {
                // Could be the END of the line operator.
                let label = clean_edge_label(&r[..end_line]);
                let consumed = 2 + (rest.len() - r.len()) + end_line + 3;
                if !label.is_empty() {
                    return Some((EdgeStyle::Line, Some(label), consumed));
                }
            }
        }
    }
    if let Some(rest) = s.strip_prefix("==") {
        let r = rest.trim_start();
        if !r.starts_with("==>") && !r.starts_with("==") {
            if let Some(end_arrow) = r.find("==>") {
                let label = r[..end_arrow].trim().to_string();
                let consumed = 2 + (rest.len() - r.len()) + end_arrow + 3;
                if !label.is_empty() {
                    return Some((EdgeStyle::Thick, Some(label), consumed));
                }
            }
        }
    }

    // 2. Base operators (longest-first to avoid prefix collisions).
    let bases: &[(&str, EdgeStyle)] = &[
        ("-.->", EdgeStyle::Dotted),
        ("-->", EdgeStyle::Arrow),
        ("==>", EdgeStyle::Thick),
        ("---", EdgeStyle::Line),
    ];
    for &(tok, style) in bases {
        if let Some(after) = s.strip_prefix(tok) {
            let consumed_base = tok.len();
            // Check for pipe-form label: `-->|LABEL|`
            if let Some(after_pipe) = after.strip_prefix('|') {
                if let Some(end) = after_pipe.find('|') {
                    let label = after_pipe[..end].trim().to_string();
                    let consumed_label = 1 + end + 1;
                    if !label.is_empty() {
                        return Some((style, Some(label),
                                     consumed_base + consumed_label));
                    }
                }
            }
            return Some((style, None, consumed_base));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_input_returns_empty_graph() {
        let g = parse("");
        assert_eq!(g.nodes.len(), 0);
        assert_eq!(g.edges.len(), 0);
        assert_eq!(g.direction, Direction::TopDown);
    }

    #[test]
    fn parses_graph_td_header() {
        let g = parse("graph TD\nA --> B");
        assert_eq!(g.direction, Direction::TopDown);
    }

    #[test]
    fn parses_graph_lr_header() {
        let g = parse("graph LR\nA --> B");
        assert_eq!(g.direction, Direction::LeftRight);
    }

    #[test]
    fn parses_graph_rl_header() {
        let g = parse("graph RL\nA --> B");
        assert_eq!(g.direction, Direction::RightLeft);
    }

    #[test]
    fn parses_graph_bt_header() {
        let g = parse("graph BT\nA --> B");
        assert_eq!(g.direction, Direction::BottomTop);
    }

    #[test]
    fn flowchart_alias_works() {
        let g = parse("flowchart LR\nA --> B");
        assert_eq!(g.direction, Direction::LeftRight);
    }

    #[test]
    fn missing_header_defaults_topdown() {
        let g = parse("A --> B");
        assert_eq!(g.direction, Direction::TopDown);
        assert_eq!(g.edges.len(), 1);
    }

    #[test]
    fn simple_arrow_edge() {
        let g = parse("graph TD\nA --> B");
        assert_eq!(g.nodes.len(), 2);
        assert_eq!(g.edges.len(), 1);
        assert_eq!(g.edges[0].from, "A");
        assert_eq!(g.edges[0].to, "B");
        assert_eq!(g.edges[0].style, EdgeStyle::Arrow);
        assert_eq!(g.edges[0].label, None);
    }

    #[test]
    fn line_edge() {
        let g = parse("graph TD\nA --- B");
        assert_eq!(g.edges[0].style, EdgeStyle::Line);
    }

    #[test]
    fn thick_edge() {
        let g = parse("graph TD\nA ==> B");
        assert_eq!(g.edges[0].style, EdgeStyle::Thick);
    }

    #[test]
    fn dotted_edge() {
        let g = parse("graph TD\nA -.-> B");
        assert_eq!(g.edges[0].style, EdgeStyle::Dotted);
    }

    #[test]
    fn pipe_label_form() {
        let g = parse("graph TD\nA -->|yes| B");
        assert_eq!(g.edges[0].label, Some("yes".into()));
        assert_eq!(g.edges[0].style, EdgeStyle::Arrow);
    }

    #[test]
    fn dash_label_form() {
        let g = parse("graph TD\nA -- yes --> B");
        assert_eq!(g.edges[0].label, Some("yes".into()));
        assert_eq!(g.edges[0].style, EdgeStyle::Arrow);
    }

    #[test]
    fn rectangle_node_shape() {
        let g = parse("graph TD\nA[Start] --> B");
        let n = g.find_node("A").unwrap();
        assert_eq!(n.shape, NodeShape::Rectangle);
        assert_eq!(n.label, "Start");
    }

    #[test]
    fn rounded_node_shape() {
        let g = parse("graph TD\nA(Round) --> B");
        let n = g.find_node("A").unwrap();
        assert_eq!(n.shape, NodeShape::Rounded);
        assert_eq!(n.label, "Round");
    }

    #[test]
    fn circle_node_shape() {
        let g = parse("graph TD\nA((Circ)) --> B");
        let n = g.find_node("A").unwrap();
        assert_eq!(n.shape, NodeShape::Circle);
        assert_eq!(n.label, "Circ");
    }

    #[test]
    fn diamond_node_shape() {
        let g = parse("graph TD\nA{Choice} --> B");
        let n = g.find_node("A").unwrap();
        assert_eq!(n.shape, NodeShape::Diamond);
        assert_eq!(n.label, "Choice");
    }

    #[test]
    fn parallelogram_node_shape() {
        let g = parse("graph TD\nA[/Trap/] --> B");
        let n = g.find_node("A").unwrap();
        assert_eq!(n.shape, NodeShape::Parallelogram);
        assert_eq!(n.label, "Trap");
    }

    #[test]
    fn asymmetric_node_shape() {
        let g = parse("graph TD\nA>Asym] --> B");
        let n = g.find_node("A").unwrap();
        assert_eq!(n.shape, NodeShape::Asymmetric);
        assert_eq!(n.label, "Asym");
    }

    #[test]
    fn chained_edges() {
        let g = parse("graph TD\nA --> B --> C");
        assert_eq!(g.nodes.len(), 3);
        assert_eq!(g.edges.len(), 2);
        assert_eq!(g.edges[0].from, "A");
        assert_eq!(g.edges[0].to, "B");
        assert_eq!(g.edges[1].from, "B");
        assert_eq!(g.edges[1].to, "C");
    }

    #[test]
    fn implicit_node_decls() {
        // Nodes that only appear via edges should be created.
        let g = parse("graph TD\nA --> B");
        assert!(g.find_node("A").is_some());
        assert!(g.find_node("B").is_some());
        assert_eq!(g.find_node("A").unwrap().shape, NodeShape::Default);
    }

    #[test]
    fn explicit_overrides_implicit() {
        let g = parse("graph TD\nA --> B\nA[Hello]");
        let a = g.find_node("A").unwrap();
        assert_eq!(a.shape, NodeShape::Rectangle);
        assert_eq!(a.label, "Hello");
    }

    #[test]
    fn comment_lines_skipped() {
        let g = parse("graph TD\n%% this is a comment\nA --> B");
        assert_eq!(g.edges.len(), 1);
    }

    #[test]
    fn trailing_comment_stripped() {
        let g = parse("graph TD\nA --> B %% inline note");
        assert_eq!(g.edges.len(), 1);
        assert_eq!(g.edges[0].from, "A");
        assert_eq!(g.edges[0].to, "B");
    }

    #[test]
    fn unrecognized_line_captured() {
        let g = parse("graph TD\nA --> B\n@@@invalid@@@");
        assert_eq!(g.edges.len(), 1);
        assert_eq!(g.unrecognized.len(), 1);
        assert!(g.unrecognized[0].contains("invalid"));
    }

    #[test]
    fn multiline_diagram() {
        let src = r#"
graph TD
    Start[Start] --> Q1{Decide?}
    Q1 -->|yes| End[End]
    Q1 -->|no| Loop
    Loop --> Q1
"#;
        let g = parse(src);
        assert_eq!(g.direction, Direction::TopDown);
        // 4 unique nodes: Start, Q1, End, Loop
        assert_eq!(g.nodes.len(), 4);
        // 4 edges total
        assert_eq!(g.edges.len(), 4);
        assert_eq!(g.find_node("Q1").unwrap().shape, NodeShape::Diamond);
        assert_eq!(g.find_node("Start").unwrap().shape, NodeShape::Rectangle);
        // Check labeled edges
        let yes_edge = g.edges.iter().find(|e|
            e.from == "Q1" && e.to == "End").unwrap();
        assert_eq!(yes_edge.label, Some("yes".into()));
    }

    #[test]
    fn node_labels_with_spaces() {
        let g = parse("graph TD\nA[Hello World] --> B");
        assert_eq!(g.find_node("A").unwrap().label, "Hello World");
    }

    #[test]
    fn underscore_and_hyphen_in_ids() {
        let g = parse("graph TD\nA_1 --> b-2");
        assert!(g.find_node("A_1").is_some());
        assert!(g.find_node("b-2").is_some());
    }

    #[test]
    fn empty_label_in_brackets() {
        let g = parse("graph TD\nA[] --> B");
        let a = g.find_node("A").unwrap();
        assert_eq!(a.shape, NodeShape::Rectangle);
        assert_eq!(a.label, "");
    }

    #[test]
    fn duplicate_edge_kept_separately() {
        // Mermaid spec allows duplicate edges; we keep them.
        let g = parse("graph TD\nA --> B\nA --> B");
        assert_eq!(g.edges.len(), 2);
    }

    #[test]
    fn bare_node_id_alone() {
        let g = parse("graph TD\nLoneNode");
        assert!(g.find_node("LoneNode").is_some());
        assert_eq!(g.edges.len(), 0);
    }

    // v0.1.444: mindmap parsing.
    #[test]
    fn mindmap_builds_hierarchy() {
        let src = "mindmap\n    \
                   root((FILM 2\\nDOMINION))\n        \
                   PREMISE[\"CORE PREMISE\"]\n            \
                   SETUP[\"Where We Pick Up\"]\n            \
                   TONE[\"Intelligence\"]\n        \
                   BLACKSTAR[\"THE REAL WAR\"]";
        let g = parse(src);
        // root + 4 children.
        assert!(g.find_node("root").is_some());
        assert!(g.find_node("PREMISE").is_some());
        assert!(g.find_node("SETUP").is_some());
        // Parent -> child edges by indent.
        assert!(g.edges.iter().any(|e| e.from == "root" && e.to == "PREMISE"));
        assert!(g.edges.iter().any(|e| e.from == "PREMISE" && e.to == "SETUP"));
        assert!(g.edges.iter().any(|e| e.from == "PREMISE" && e.to == "TONE"));
        assert!(g.edges.iter().any(|e| e.from == "root" && e.to == "BLACKSTAR"));
        // SETUP is NOT a child of root (it's nested under PREMISE).
        assert!(!g.edges.iter().any(|e| e.from == "root" && e.to == "SETUP"));
    }

    #[test]
    fn mindmap_normalizes_literal_newline() {
        let g = parse("mindmap\n    root((FILM 2\\nDOMINION\\nDarker))");
        let root = g.find_node("root").unwrap();
        // Literal backslash-n became spaces; quotes/brackets gone.
        assert_eq!(root.label, "FILM 2 DOMINION Darker");
        assert!(!root.label.contains("\\n"));
        assert_eq!(root.shape, NodeShape::Circle);
    }

    #[test]
    fn mindmap_strips_bracket_quotes() {
        let g = parse("mindmap\n    root((R))\n        NODE[\"Quoted Label\"]");
        let n = g.find_node("NODE").unwrap();
        assert_eq!(n.label, "Quoted Label");
    }

    #[test]
    fn mindmap_dedents_correctly() {
        // A -> B -> C, then D back at A's child level.
        let src = "mindmap\n  root((R))\n    A[a]\n      B[b]\n    D[d]";
        let g = parse(src);
        assert!(g.edges.iter().any(|e| e.from == "A" && e.to == "B"));
        assert!(g.edges.iter().any(|e| e.from == "root" && e.to == "D"));
        // D is a sibling of A, not a child of B.
        assert!(!g.edges.iter().any(|e| e.from == "B" && e.to == "D"));
    }

    // v0.1.738 (dogfood MIPS datapath): digit-first ids + labeled edges.
    #[test]
    fn digit_first_ids_parse() {
        let g = parse("flowchart TD\n1s --> tmp5\n1s[\"PC\"]\n13c --> 14s");
        assert!(g.find_node("1s").is_some(), "1s should parse");
        assert!(g.find_node("13c").is_some(), "13c should parse");
        assert!(g.find_node("14s").is_some(), "14s should parse");
        assert_eq!(g.find_node("1s").unwrap().label, "PC");
        assert!(g.edges.iter().any(|e| e.from == "1s" && e.to == "tmp5"));
        assert!(g.edges.iter().any(|e| e.from == "13c" && e.to == "14s"));
    }

    #[test]
    fn dash_form_labeled_edge_strips_quotes() {
        let g = parse("flowchart TD\ntmp5 -- \"Read Address\" --> 3s");
        let e = g.edges.iter().find(|e| e.from == "tmp5" && e.to == "3s")
            .expect("edge should parse");
        assert_eq!(e.label.as_deref(), Some("Read Address"));
    }

    #[test]
    fn triple_dash_labeled_edge_no_trailing_dash() {
        // `---> ` (3 dashes) must not leave a trailing '-' in the label.
        let g = parse("flowchart TD\ntmp6 -- \"Instruction [25:21]\" ---> 5a");
        let e = g.edges.iter().find(|e| e.from == "tmp6" && e.to == "5a")
            .expect("edge should parse");
        assert_eq!(e.label.as_deref(), Some("Instruction [25:21]"));
    }

    // v0.1.739: subgraph blocks must not become bogus nodes; members tracked.
    #[test]
    fn subgraph_decl_not_a_node() {
        let src = "flowchart TD\nsubgraph 5s[\"Write Registers\"]\n5a[\"Read register 1\"]\n5b[\"Read register 2\"]\nend\nx --> 5a";
        let g = parse(src);
        // neither the subgraph id nor the keyword/title is a node
        assert!(g.find_node("subgraph").is_none());
        assert!(g.find_node("Write Registers").is_none());
        // the real members ARE nodes
        assert!(g.find_node("5a").is_some());
        assert!(g.find_node("5b").is_some());
        // group recorded with title + members
        assert_eq!(g.groups.len(), 1);
        assert_eq!(g.groups[0].title, "Write Registers");
        assert!(g.groups[0].members.contains(&"5a".to_string()));
        assert!(g.groups[0].members.contains(&"5b".to_string()));
    }

    /// v0.1.759: members that already exist (referenced by an edge BEFORE the
    /// subgraph block) must still be captured — the MIPS dogfood bug where only
    /// the last, newly-created member of each subgraph was recorded.
    #[test]
    fn subgraph_captures_preexisting_members() {
        // 5a..5c are first referenced by edges, THEN declared in the subgraph.
        let src = "flowchart TD\nx --> 5a\ny --> 5b\nz --> 5c\n\
                   subgraph 5s[\"Write Registers\"]\n5a\n5b\n5c\nend";
        let g = parse(src);
        assert_eq!(g.groups.len(), 1);
        let m = &g.groups[0].members;
        assert!(m.contains(&"5a".to_string()), "5a missing: {:?}", m);
        assert!(m.contains(&"5b".to_string()), "5b missing: {:?}", m);
        assert!(m.contains(&"5c".to_string()), "5c missing: {:?}", m);
        assert_eq!(m.len(), 3, "expected all 3 members, got {:?}", m);
    }

    #[test]
    fn bare_end_keyword_not_a_node() {
        let g = parse("flowchart TD\nsubgraph G\na\nend");
        assert!(g.find_node("end").is_none(), "`end` must not be a node");
        assert!(g.find_node("a").is_some());
    }

    // v0.1.741: {{ }} hexagon labels must not keep a stray brace.
    #[test]
    fn double_brace_hexagon_no_stray_brace() {
        let g = parse("flowchart TD\nRegDst{{\"RegDst\"}} --- 4s");
        let n = g.find_node("RegDst").expect("RegDst node");
        assert_eq!(n.label, "RegDst", "no stray brace/quote: got {:?}", n.label);
    }

    #[test]
    fn escape_sequences_collapse_to_space() {
        let g = parse("flowchart TD\ntmp1{{\"4\\nconst\"}} --> x");
        let n = g.find_node("tmp1").expect("tmp1 node");
        assert!(!n.label.contains('\\'), "no literal backslash: {:?}", n.label);
        assert!(!n.label.contains('{'), "no stray brace: {:?}", n.label);
        assert_eq!(n.label, "4 const");
    }
}
