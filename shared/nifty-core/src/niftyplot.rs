// src/niftyplot.rs
//
// v0.1.343 (niftyplot arc): parser + AST for the niftyplot
// diagram format. This ship implements the GRAPH KIND only.
// gantt/hardware/sequence/state are spec'd but stubbed.
//
// The parser is permissive per the spec's design principle:
// unknown directives become warnings, not errors. The renderer
// continues even when constraints conflict — it picks a
// largest-working-subset interpretation and emits a debug
// footer the user can copy.
//
// Format reference: /home/claude/docs/niftyplot-spec.md
//
// This module deliberately does NOT implement:
//   - A real constraint solver (deferred to v0.1.344). For now
//     geometric constraints like `isosceles(A,B,C)` are recorded
//     and surfaced via warnings if they conflict, but the
//     renderer falls back to the existing force-directed
//     layout in graph_layout.rs.
//   - Plane Z-axis rendering (also v0.1.344). Planes are
//     parsed and tracked as groups for now.
//   - The four non-graph kinds (gantt, hardware, sequence,
//     state). Their parsers return a kind-not-yet-implemented
//     warning.

#![allow(dead_code)]

use std::collections::HashMap;

/// What kind of diagram this niftyplot block represents.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Graph,
    Gantt,
    Hardware,
    Sequence,
    State,
    /// Default when `%kind:` is missing — assume Graph but warn.
    Unspecified,
}

impl Kind {
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_lowercase().as_str() {
            "graph" => Some(Kind::Graph),
            "gantt" => Some(Kind::Gantt),
            "hardware" => Some(Kind::Hardware),
            "sequence" => Some(Kind::Sequence),
            "state" => Some(Kind::State),
            _ => None,
        }
    }
}

/// Node shape hints.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shape {
    Rect,
    Circle,
    Diamond,
    Cylinder,
    Cloud,
    Hexagon,
    Parallelogram,
    Note,
}

impl Shape {
    fn parse(s: &str) -> Option<Self> {
        match s.trim().to_lowercase().as_str() {
            "rect" | "rectangle" => Some(Shape::Rect),
            "circle" | "round" => Some(Shape::Circle),
            "diamond" => Some(Shape::Diamond),
            "cylinder" | "db" | "database" => Some(Shape::Cylinder),
            "cloud" => Some(Shape::Cloud),
            "hexagon" | "hex" => Some(Shape::Hexagon),
            "parallelogram" => Some(Shape::Parallelogram),
            "note" => Some(Shape::Note),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Shape::Rect => "rect",
            Shape::Circle => "circle",
            Shape::Diamond => "diamond",
            Shape::Cylinder => "cylinder",
            Shape::Cloud => "cloud",
            Shape::Hexagon => "hexagon",
            Shape::Parallelogram => "parallelogram",
            Shape::Note => "note",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Node {
    pub id: String,
    pub label: Option<String>,
    pub shape: Shape,
    /// Plane this node belongs to, if any. "" = default plane.
    pub plane: String,
    /// v0.1.691: optional image to render inside the node (path/url). Set via
    /// `A image("photo.png"): "label"` in niftyplot source.
    pub image: Option<String>,
}

impl Node {
    fn new(id: &str) -> Self {
        Self {
            id: id.to_string(),
            label: None,
            shape: Shape::Rect,
            plane: String::new(),
            image: None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EdgeStyle {
    Solid,
    Dashed,
    Heavy,
    Bidirectional,
}

#[derive(Clone, Debug)]
pub struct Edge {
    pub from: String,
    pub to: String,
    pub style: EdgeStyle,
    pub label: Option<String>,
}

#[derive(Clone, Debug)]
pub struct Plane {
    pub name: String,
    /// Node IDs that live in this plane. Filled during parse;
    /// nodes inside `plane X { … }` blocks land here.
    pub members: Vec<String>,
}

/// A geometric or spatial constraint declared in the source.
/// Recorded literally; resolution happens later (in the layout
/// stage, not in the parser).
#[derive(Clone, Debug)]
pub enum Constraint {
    /// `pinned at (x, y)` — Tier 0. Hard pixel position.
    Pin { node: String, x: f32, y: f32 },
    /// `A above B by 60`, `right of C by 80`, etc. — Tier 2.
    Relative {
        node: String,
        direction: Direction,
        anchor: String,
        distance: f32,
    },
    /// Node geometry like `isosceles(A,B,C)`, `circle(...)`. Tier 3.
    NodeGeom { kind: NodeGeomKind, nodes: Vec<String> },
    /// Plane geometry like `parallel(A,B,C)`, `intersect(...)`. Tier 4.
    PlaneGeom { kind: PlaneGeomKind, planes: Vec<String> },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    Above, Below, Left, Right,
    AboveLeft, AboveRight, BelowLeft, BelowRight,
}

impl Direction {
    fn parse(s: &str) -> Option<Self> {
        match s.trim().to_lowercase().as_str() {
            "above" => Some(Direction::Above),
            "below" => Some(Direction::Below),
            "left" => Some(Direction::Left),
            "right" => Some(Direction::Right),
            "above-left" | "above and to the left" => Some(Direction::AboveLeft),
            "above-right" | "above and to the right" => Some(Direction::AboveRight),
            "below-left" | "below and to the left" => Some(Direction::BelowLeft),
            "below-right" | "below and to the right" => Some(Direction::BelowRight),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NodeGeomKind {
    Isosceles, Rhombus, Circle, Line, Grid, Arc,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlaneGeomKind {
    Parallel, Intersect, Coplanar, Between, Behind, Nested,
    Facing, Radial, Perpendicular, Stack, Siblings, Venn,
}

/// A warning surfaced to the user. Renders in the debug footer
/// under the diagram with a copy-to-clipboard button.
#[derive(Clone, Debug)]
pub struct Warning {
    pub line: usize,
    pub severity: Severity,
    pub message: String,
    /// The original source text that triggered the warning, so
    /// the user can grep for it.
    pub source: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Severity {
    Info, Warning, Error,
}

impl Severity {
    pub fn glyph(&self) -> &'static str {
        match self {
            Severity::Info => "ℹ",
            Severity::Warning => "⚠",
            Severity::Error => "✗",
        }
    }
}

/// The parsed contents of a niftyplot block (graph kind).
/// Other kinds will get their own structs as they're implemented.
#[derive(Clone, Debug, Default)]
pub struct GraphDoc {
    pub kind: Option<Kind>,
    pub title: Option<String>,
    pub tilt: Option<f32>,
    pub zoom: Option<f32>,
    pub theme: Option<String>,
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
    pub planes: Vec<Plane>,
    pub constraints: Vec<Constraint>,
    pub warnings: Vec<Warning>,
    /// Fast lookup by id.
    pub node_idx: HashMap<String, usize>,
}

impl GraphDoc {
    pub fn new() -> Self { Self::default() }

    fn warn(&mut self, line: usize, sev: Severity, msg: impl Into<String>, src: &str) {
        self.warnings.push(Warning {
            line,
            severity: sev,
            message: msg.into(),
            source: src.to_string(),
        });
    }

    /// Get or create a node by id. Used by edge parsing —
    /// niftyplot lets you reference a node implicitly via an
    /// edge declaration (the node doesn't need a separate
    /// declaration line).
    fn ensure_node(&mut self, id: &str, current_plane: &str) -> usize {
        if let Some(&idx) = self.node_idx.get(id) {
            return idx;
        }
        let mut n = Node::new(id);
        n.plane = current_plane.to_string();
        let idx = self.nodes.len();
        self.nodes.push(n);
        self.node_idx.insert(id.to_string(), idx);
        idx
    }
}

/// Parse a niftyplot fence body. Strips the leading/trailing
/// ``` markers if present; otherwise interprets the input as
/// the body content directly.
pub fn parse(source: &str) -> GraphDoc {
    let body = extract_body(source);
    parse_body(&body)
}

fn extract_body(s: &str) -> String {
    // If the source starts with a fence, strip the first and
    // last lines (the fence markers).
    let trimmed = s.trim();
    if trimmed.starts_with("```") {
        let lines: Vec<&str> = trimmed.lines().collect();
        if lines.len() >= 2 {
            // Drop first and last (the fences). The closing
            // fence might not exist if the input is malformed;
            // in that case keep everything after the first
            // fence line.
            let last = lines.len() - 1;
            let body_lines = if lines[last].trim().starts_with("```") {
                &lines[1..last]
            } else {
                &lines[1..]
            };
            return body_lines.join("\n");
        }
    }
    s.to_string()
}

fn parse_body(body: &str) -> GraphDoc {
    let mut doc = GraphDoc::new();
    let mut current_plane = String::new();
    let mut brace_stack: Vec<String> = Vec::new();  // tracks plane/group names

    for (line_idx, raw_line) in body.lines().enumerate() {
        let line_no = line_idx + 1;
        // Strip trailing `#` comments.
        let line = match raw_line.split_once('#') {
            Some((before, _)) => before,
            None => raw_line,
        };
        let line = line.trim();
        if line.is_empty() { continue; }

        // Brace handling for plane / group blocks.
        if line == "}" {
            if let Some(_popped) = brace_stack.pop() {
                current_plane = brace_stack.last().cloned().unwrap_or_default();
            } else {
                doc.warn(line_no, Severity::Warning,
                    "unmatched closing brace",
                    line);
            }
            continue;
        }

        // %kind: directive.
        if let Some(rest) = line.strip_prefix("%kind:") {
            match Kind::parse(rest.trim()) {
                Some(k) => doc.kind = Some(k),
                None => doc.warn(line_no, Severity::Warning,
                    format!("unknown kind '{}'; expected one of: graph, gantt, hardware, sequence, state",
                        rest.trim()),
                    line),
            }
            continue;
        }
        // %title:
        if let Some(rest) = line.strip_prefix("%title:") {
            doc.title = Some(strip_quotes(rest.trim()));
            continue;
        }
        // %tilt:
        if let Some(rest) = line.strip_prefix("%tilt:") {
            let v = rest.trim().trim_end_matches('°').trim();
            if let Ok(angle) = v.parse::<f32>() {
                if angle.is_finite() && (0.0..=75.0).contains(&angle) {
                    doc.tilt = Some(angle);
                } else {
                    doc.warn(line_no, Severity::Warning,
                        format!("tilt {}° outside 0-75°; clamping", angle),
                        line);
                    doc.tilt = Some(angle.clamp(0.0, 75.0));
                }
            } else {
                doc.warn(line_no, Severity::Warning,
                    format!("unparseable tilt value '{}'", v), line);
            }
            continue;
        }
        // %zoom:
        if let Some(rest) = line.strip_prefix("%zoom:") {
            if let Ok(z) = rest.trim().parse::<f32>() {
                if z.is_finite() && z > 0.0 { doc.zoom = Some(z); }
                else { doc.warn(line_no, Severity::Warning,
                    "zoom must be positive", line); }
            }
            continue;
        }
        // %theme:
        if let Some(rest) = line.strip_prefix("%theme:") {
            doc.theme = Some(strip_quotes(rest.trim()));
            continue;
        }
        // Unknown %directive
        if line.starts_with('%') {
            // Skip the %layout sentinel — it's a section marker,
            // not a directive with effect.
            if line.starts_with("%layout") {
                continue;
            }
            doc.warn(line_no, Severity::Info,
                format!("unrecognized directive: {}", first_word(line)),
                line);
            continue;
        }

        // plane <name> { ... }
        if let Some(rest) = line.strip_prefix("plane ") {
            let name = rest.trim_end_matches('{').trim();
            let name = strip_quotes(name);
            if !line.ends_with('{') {
                doc.warn(line_no, Severity::Warning,
                    "plane declaration must end with '{'",
                    line);
                continue;
            }
            if !doc.planes.iter().any(|p| p.name == name) {
                doc.planes.push(Plane { name: name.clone(), members: Vec::new() });
            }
            current_plane = name.clone();
            brace_stack.push(name);
            continue;
        }

        // Edge declaration: contains an arrow.
        // Forms supported: A → B, A -> B, A => B, A -.-> B, A ↔ B,
        //                   A -- "label" → B, chain A → B → C
        if let Some(parsed) = try_parse_edges(line) {
            let (chain, style, label) = parsed;
            for win in chain.windows(2) {
                let from = win[0].clone();
                let to = win[1].clone();
                doc.ensure_node(&from, &current_plane);
                doc.ensure_node(&to, &current_plane);
                doc.edges.push(Edge {
                    from, to, style, label: label.clone(),
                });
            }
            continue;
        }

        // Constraint: starts with a known geometry keyword.
        if let Some(c) = try_parse_constraint(line, line_no, &mut doc) {
            doc.constraints.push(c);
            continue;
        }

        // Otherwise it's a node declaration. Forms:
        //   A
        //   A : "label"
        //   A as circle : "label"
        //   A as diamond
        if let Some(node) = try_parse_node(line) {
            let idx = doc.ensure_node(&node.id, &current_plane);
            // Apply optional label / shape from declaration.
            if let Some(lbl) = node.label {
                doc.nodes[idx].label = Some(lbl);
            }
            if node.shape != Shape::Rect {
                doc.nodes[idx].shape = node.shape;
            }
            if node.image.is_some() {
                doc.nodes[idx].image = node.image.clone();
            }
            if !current_plane.is_empty() {
                doc.nodes[idx].plane = current_plane.clone();
                if let Some(plane) = doc.planes.iter_mut().find(|p| p.name == current_plane) {
                    if !plane.members.contains(&doc.nodes[idx].id) {
                        plane.members.push(doc.nodes[idx].id.clone());
                    }
                }
            }
            continue;
        }

        // Default: unrecognized line.
        doc.warn(line_no, Severity::Info,
            format!("unrecognized line"),
            line);
    }

    // Unclosed braces at end.
    while let Some(name) = brace_stack.pop() {
        doc.warn(0, Severity::Warning,
            format!("plane/group '{}' was not closed", name),
            "");
    }

    // Default kind = graph (with warning) if none specified.
    if doc.kind.is_none() {
        doc.warn(0, Severity::Info,
            "no %kind: declared; defaulting to graph",
            "");
        doc.kind = Some(Kind::Graph);
    }

    doc
}

fn strip_quotes(s: &str) -> String {
    let t = s.trim();
    if (t.starts_with('"') && t.ends_with('"') && t.len() >= 2)
        || (t.starts_with('\'') && t.ends_with('\'') && t.len() >= 2)
    {
        t[1..t.len()-1].to_string()
    } else {
        t.to_string()
    }
}

fn first_word(s: &str) -> String {
    s.split_whitespace().next().unwrap_or("").to_string()
}

/// Parse `A → B`, `A → B → C`, `A -- "label" → B`. Returns
/// (chain, style, label) on success.
fn try_parse_edges(line: &str) -> Option<(Vec<String>, EdgeStyle, Option<String>)> {
    // Look for any arrow token.
    let arrow_tokens = ["→", "->", "=>", "↔", "<->", "-.->", "-.-", "<=>"];
    if !arrow_tokens.iter().any(|t| line.contains(t)) {
        return None;
    }
    // Style detection. Order matters (longer first).
    let style = if line.contains("=>") || line.contains("<=>") {
        EdgeStyle::Heavy
    } else if line.contains("-.->") || line.contains("-.-") {
        EdgeStyle::Dashed
    } else if line.contains("↔") || line.contains("<->") {
        EdgeStyle::Bidirectional
    } else {
        EdgeStyle::Solid
    };
    // Label extraction. Look for `"..."` between identifiers.
    let label = if let Some(start) = line.find('"') {
        if let Some(end_rel) = line[start+1..].find('"') {
            Some(line[start+1..start+1+end_rel].to_string())
        } else { None }
    } else { None };
    // Strip label segment so the chain split doesn't see it.
    let work = if let Some(start) = line.find('"') {
        if let Some(end_rel) = line[start+1..].find('"') {
            let end = start + 1 + end_rel + 1;
            let before = &line[..start];
            let after = &line[end..];
            format!("{} {}", before, after)
        } else { line.to_string() }
    } else { line.to_string() };

    // Replace all arrow variants with a sentinel so a single
    // split gives us the chain.
    let mut normalized = work;
    for tok in &arrow_tokens {
        normalized = normalized.replace(tok, "\x1F");
    }
    // Also strip dash sequences left over from `-- "label" `
    // patterns (where the label was extracted but the `--`
    // separator remains).
    normalized = normalized.replace("--", " ");

    let chain: Vec<String> = normalized
        .split('\x1F')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();

    if chain.len() < 2 { return None; }
    Some((chain, style, label))
}

/// Parse a node declaration like `A`, `A: "label"`, `A as circle: "label"`.
fn try_parse_node(line: &str) -> Option<Node> {
    // The line must start with an identifier (alphanumeric +
    // underscore, no whitespace).
    let first_ws = line.find(|c: char| c.is_whitespace() || c == ':');
    let id = match first_ws {
        Some(p) => &line[..p],
        None => line,
    };
    if id.is_empty() || !is_valid_id(id) { return None; }

    let mut node = Node::new(id);
    let rest = &line[id.len()..].trim_start();

    if rest.is_empty() { return Some(node); }

    // `as <shape>` (optional)
    let after_shape = if let Some(after_as) = rest.strip_prefix("as ") {
        let after_as = after_as.trim_start();
        let shape_end = after_as.find(|c: char| c.is_whitespace() || c == ':')
            .unwrap_or(after_as.len());
        let shape_word = &after_as[..shape_end];
        if let Some(s) = Shape::parse(shape_word) {
            node.shape = s;
        } else {
            // Unknown shape — keep as Rect, label will likely
            // include this token by accident. Conservative: don't
            // consume.
            return Some(node);
        }
        after_as[shape_end..].trim_start().to_string()
    } else {
        rest.to_string()
    };

    // v0.1.691: optional `image("path")` attribute (before the label colon).
    // e.g. `A as circle image("photo.png"): "Alice"`.
    let after_image = if let Some(rest_img) = after_shape.strip_prefix("image(") {
        if let Some(close) = rest_img.find(')') {
            let raw = &rest_img[..close];
            node.image = Some(strip_quotes(raw.trim()));
            rest_img[close + 1..].trim_start().to_string()
        } else {
            after_shape.clone()
        }
    } else {
        after_shape.clone()
    };

    // `: "label"` (optional)
    if let Some(after_colon) = after_image.strip_prefix(':') {
        let lbl = strip_quotes(after_colon.trim());
        if !lbl.is_empty() { node.label = Some(lbl); }
    }
    Some(node)
}

fn is_valid_id(s: &str) -> bool {
    !s.is_empty()
        && s.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '-')
}

/// Parse a single constraint line. Returns the constraint on
/// success, or None if the line isn't a constraint (caller falls
/// through to node parsing).
fn try_parse_constraint(line: &str, line_no: usize, doc: &mut GraphDoc) -> Option<Constraint> {
    // Try node geometries first (recognized by `<keyword>(...)`)
    for (kw, kind) in &[
        ("isosceles", NodeGeomKind::Isosceles),
        ("rhombus", NodeGeomKind::Rhombus),
        ("circle", NodeGeomKind::Circle),
        ("line", NodeGeomKind::Line),
        ("grid", NodeGeomKind::Grid),
        ("arc", NodeGeomKind::Arc),
    ] {
        if let Some(args) = match_call(line, kw) {
            let nodes = parse_id_list(&args);
            if nodes.len() < 2 {
                doc.warn(line_no, Severity::Warning,
                    format!("{} needs at least 2 nodes; got {}", kw, nodes.len()),
                    line);
                return None;
            }
            return Some(Constraint::NodeGeom { kind: *kind, nodes });
        }
    }

    // Plane geometries.
    for (kw, kind) in &[
        ("parallel", PlaneGeomKind::Parallel),
        ("intersect", PlaneGeomKind::Intersect),
        ("coplanar", PlaneGeomKind::Coplanar),
        ("between", PlaneGeomKind::Between),
        ("behind", PlaneGeomKind::Behind),
        ("nested", PlaneGeomKind::Nested),
        ("facing", PlaneGeomKind::Facing),
        ("radial", PlaneGeomKind::Radial),
        ("perpendicular", PlaneGeomKind::Perpendicular),
        ("stack", PlaneGeomKind::Stack),
        ("siblings", PlaneGeomKind::Siblings),
        ("venn", PlaneGeomKind::Venn),
    ] {
        if let Some(args) = match_call(line, kw) {
            let planes = parse_id_list(&args);
            if planes.is_empty() {
                doc.warn(line_no, Severity::Warning,
                    format!("{} called with no arguments", kw),
                    line);
                return None;
            }
            return Some(Constraint::PlaneGeom { kind: *kind, planes });
        }
    }

    // pin <node> at (x, y)
    if let Some(rest) = line.strip_prefix("pin ") {
        if let Some(node_end) = rest.find(" at ") {
            let node = rest[..node_end].trim().to_string();
            let coord = rest[node_end+4..].trim();
            if let Some((x, y)) = parse_xy(coord) {
                return Some(Constraint::Pin { node, x, y });
            } else {
                doc.warn(line_no, Severity::Warning,
                    "pin expected `<node> at (x, y)` form", line);
            }
        }
    }

    // <node> above|below|left|right <of> <anchor> by <dist>
    if let Some(c) = try_parse_relative(line) {
        return Some(c);
    }

    None
}

fn match_call(line: &str, kw: &str) -> Option<String> {
    // Word boundary: kw followed by `(`, possibly with whitespace
    // between.
    let lower = line.to_lowercase();
    let kw_lower = kw.to_lowercase();
    let idx = lower.find(&kw_lower)?;
    // Ensure kw is at start (allow leading whitespace).
    let before = &lower[..idx];
    if !before.trim().is_empty() { return None; }
    let after = &line[idx + kw.len()..];
    let after = after.trim_start();
    if !after.starts_with('(') { return None; }
    // Find matching ')'.
    let close = after.find(')')?;
    Some(after[1..close].to_string())
}

fn parse_id_list(args: &str) -> Vec<String> {
    args.split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty() && is_valid_id(s))
        .collect()
}

fn parse_xy(coord: &str) -> Option<(f32, f32)> {
    let inner = coord.trim_start_matches('(').trim_end_matches(')');
    let parts: Vec<&str> = inner.split(',').map(|s| s.trim()).collect();
    if parts.len() != 2 { return None; }
    let x: f32 = parts[0].parse().ok()?;
    let y: f32 = parts[1].parse().ok()?;
    if !x.is_finite() || !y.is_finite() { return None; }
    Some((x, y))
}

fn try_parse_relative(line: &str) -> Option<Constraint> {
    // Form: `<node> <direction> of <anchor> by <dist>`
    // Form: `<node>: <direction> of <anchor> by <dist>` (inside %layout)
    // Direction tokens: above, below, left, right.
    let line = line.trim_start_matches(|c: char| c.is_whitespace());

    // Strip optional `<node>:` prefix.
    let (node, rest) = if let Some(colon) = line.find(':') {
        let head = line[..colon].trim();
        if is_valid_id(head) {
            (head.to_string(), line[colon+1..].trim().to_string())
        } else {
            return None;
        }
    } else {
        // form: `<node> above <anchor> by <N>`
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 3 { return None; }
        if !is_valid_id(parts[0]) { return None; }
        (parts[0].to_string(), parts[1..].join(" "))
    };

    // Now parse "<direction> [of] <anchor> [by <dist>]" or
    // "<cardinal>" alone (top-left, center, etc.)
    let words: Vec<&str> = rest.split_whitespace().collect();
    if words.is_empty() { return None; }

    let direction = Direction::parse(words[0])?;
    // Expect "of <anchor>" or "<anchor>" next.
    let mut idx = 1;
    if idx < words.len() && words[idx].eq_ignore_ascii_case("of") {
        idx += 1;
    }
    if idx >= words.len() { return None; }
    let anchor = words[idx].to_string();
    if !is_valid_id(&anchor) { return None; }
    idx += 1;

    // Optional "by <dist>".
    let mut distance = 60.0;
    if idx < words.len() && words[idx].eq_ignore_ascii_case("by") {
        idx += 1;
        if idx < words.len() {
            if let Ok(d) = words[idx].trim_end_matches("px").parse::<f32>() {
                if d.is_finite() { distance = d; }
            }
        }
    }
    Some(Constraint::Relative { node, direction, anchor, distance })
}

// ── Suggestive helper ──

/// Context-aware suggestion chips for the niftyplot editor.
/// Given a source buffer and a caret byte offset, return up to
/// `max` chip labels appropriate for that location.
pub fn suggest(source: &str, caret_byte: usize, max: usize) -> Vec<String> {
    let caret_byte = caret_byte.min(source.len());
    // Walk back from caret to start-of-line; that's our "current line".
    let line_start = source[..caret_byte]
        .rfind('\n')
        .map(|p| p + 1)
        .unwrap_or(0);
    let current_line_before_caret = &source[line_start..caret_byte];

    // Parse the source up to (but not including) the current
    // line to learn what nodes/planes exist.
    let history = &source[..line_start];
    let doc = parse_body(history);

    let trimmed = current_line_before_caret.trim_start();

    // 1. Completely empty buffer or empty line: top-level suggestions.
    if trimmed.is_empty() {
        // v0.1.466: check whether the user has LITERALLY typed a
        // `%kind:` directive — NOT doc.kind, because parse_body
        // defaults doc.kind to Some(Graph) (with a warning) so a
        // renderable graph always has a kind. That default made the
        // `if doc.kind.is_none()` check below dead — it was never
        // true after a full parse, so the `%kind:` suggestion never
        // appeared for an empty buffer (the bug behind
        // suggest_kind_for_empty_buffer). Scan the history text for
        // an actual `%kind:` line instead.
        let has_kind_directive = history.lines()
            .any(|l| l.trim_start().starts_with("%kind:"));
        if !has_kind_directive {
            return vec!["%kind: graph".into(), "%title: \"...\"".into()];
        }
        let mut out = vec![
            "plane <name> {".into(),
            "%layout".into(),
        ];
        // Suggest pairs from existing nodes.
        if doc.nodes.len() >= 2 {
            let a = &doc.nodes[0].id;
            let b = &doc.nodes[1].id;
            out.push(format!("{} → {}", a, b));
        }
        if doc.nodes.len() >= 3 {
            let ids: Vec<&str> = doc.nodes.iter().take(3).map(|n| n.id.as_str()).collect();
            out.push(format!("isosceles({}, {}, {})", ids[0], ids[1], ids[2]));
        }
        return out.into_iter().take(max).collect();
    }

    // 2. Just typed %kind:
    if trimmed.starts_with("%kind:") && trimmed.len() <= 6 + 1 {
        return vec!["graph", "gantt", "hardware", "sequence", "state"]
            .into_iter().take(max).map(|s| s.to_string()).collect();
    }

    // 3. Just typed an arrow — suggest existing nodes.
    let arrow_endings = ["→ ", "-> ", "=> ", "↔ "];
    if arrow_endings.iter().any(|a| trimmed.ends_with(a)) {
        return doc.nodes.iter().rev()
            .take(max)
            .map(|n| n.id.clone())
            .collect();
    }

    // Constraint keywords (hoisted above branch 4 so that branch
    // can defer to the constraint branch when the partial token is
    // a prefix of one of these — see below).
    let kws = [
        "isosceles", "rhombus", "circle", "line", "grid", "arc",
        "parallel", "intersect", "coplanar", "between", "behind",
        "nested", "facing", "radial", "perpendicular", "stack",
        "siblings", "venn", "pin", "plane",
    ];

    // 4. Just typed a node id (line is `<id>` with no trailing space).
    // v0.1.466: but DON'T claim it if the partial is a strict prefix
    // of a constraint keyword — e.g. "iso" is a valid id AND the
    // start of "isosceles". The bare-id branch used to swallow these
    // (returning "→ "/"as circle"…) so the constraint completion in
    // branch 5 was unreachable for any keyword-prefix (the bug behind
    // suggest_constraint_completion). Defer to branch 5 in that case.
    // A COMPLETE keyword (e.g. exactly "circle") still falls through
    // to 5 too, which offers the callable form `circle(...)`.
    let is_constraint_prefix = kws.iter()
        .any(|k| k.starts_with(trimmed) && trimmed.len() < k.len());
    if is_valid_id(trimmed) && !trimmed.contains(' ') && !is_constraint_prefix {
        let mut out = vec![
            "→ ".to_string(),
            ": \"label\"".to_string(),
            "as circle".to_string(),
            "as diamond".to_string(),
        ];
        if doc.nodes.len() >= 1 {
            let last = &doc.nodes[doc.nodes.len()-1].id;
            out.push(format!("→ {}", last));
        }
        return out.into_iter().take(max).collect();
    }

    // 5. Typing a constraint keyword — fuzzy match.
    let matches: Vec<&&str> = kws.iter()
        .filter(|k| k.starts_with(trimmed) && trimmed.len() < k.len())
        .collect();
    if !matches.is_empty() {
        return matches.into_iter()
            .take(max)
            .map(|k| {
                // Append `(...)` for callable constraints, `<name>`
                // for plane.
                if *k == "plane" {
                    format!("plane <name> {{")
                } else if *k == "pin" {
                    "pin <node> at (x, y)".to_string()
                } else {
                    format!("{}({}, ...)",
                        k,
                        doc.nodes.first().map(|n| n.id.as_str()).unwrap_or("A"))
                }
            })
            .collect();
    }

    // 6. Otherwise: generic suggestions based on parsed state.
    let mut out = Vec::new();
    if doc.nodes.len() >= 2 {
        let last = &doc.nodes[doc.nodes.len()-1];
        let prev = &doc.nodes[doc.nodes.len()-2];
        out.push(format!("{} → {}", last.id, prev.id));
    }
    out.into_iter().take(max).collect()
}

// ── Scoped subgraph detection ──

/// Given a source buffer and a caret byte offset, return the
/// name of the innermost unclosed `plane` (or `group`) block the
/// caret is inside, or None if at top level.
///
/// Used by the live-preview scoping feature: when the caret is
/// inside a `plane X { ... }`, the preview shows only X (full
/// detail) with the rest dimmed.
pub fn scope_at(source: &str, caret_byte: usize) -> Option<String> {
    let caret_byte = caret_byte.min(source.len());
    let prefix = &source[..caret_byte];
    let mut stack: Vec<String> = Vec::new();

    for line in prefix.lines() {
        let line = match line.split_once('#') {
            Some((before, _)) => before,
            None => line,
        };
        let trimmed = line.trim();
        if trimmed == "}" {
            stack.pop();
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("plane ") {
            if line.ends_with('{') {
                let name = strip_quotes(rest.trim_end_matches('{').trim());
                stack.push(name);
            }
        } else if let Some(rest) = trimmed.strip_prefix("group ") {
            if line.ends_with('{') {
                let name = strip_quotes(rest.trim_end_matches('{').trim());
                stack.push(name);
            }
        }
    }
    stack.pop()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(src: &str) -> GraphDoc {
        parse_body(src)
    }

    #[test]
    fn node_image_attribute_v691() {
        // image() before the label colon
        let d = p("A image(\"photo.png\"): \"Alice\"");
        let n = d.nodes.iter().find(|n| n.id == "A").unwrap();
        assert_eq!(n.image.as_deref(), Some("photo.png"));
        assert_eq!(n.label.as_deref(), Some("Alice"));
        // with a shape too
        let d2 = p("B as circle image(\"b.jpg\"): \"Bob\"");
        let n2 = d2.nodes.iter().find(|n| n.id == "B").unwrap();
        assert_eq!(n2.image.as_deref(), Some("b.jpg"));
        assert_eq!(n2.shape, Shape::Circle);
        // no image → None
        let d3 = p("C: \"Carol\"");
        let n3 = d3.nodes.iter().find(|n| n.id == "C").unwrap();
        assert!(n3.image.is_none());
    }

    // ── Kind detection ──

    #[test]
    fn parse_kind_directive() {
        let doc = p("%kind: graph\n");
        assert_eq!(doc.kind, Some(Kind::Graph));
        assert!(doc.warnings.iter().all(|w| w.severity != Severity::Warning));
    }

    #[test]
    fn unknown_kind_warns() {
        let doc = p("%kind: nonsense\n");
        // Defaults to graph + warns.
        assert_eq!(doc.kind, Some(Kind::Graph));
        assert!(doc.warnings.iter().any(|w| w.message.contains("unknown kind")));
    }

    #[test]
    fn missing_kind_defaults_to_graph_with_info() {
        let doc = p("A → B\n");
        assert_eq!(doc.kind, Some(Kind::Graph));
        assert!(doc.warnings.iter().any(|w|
            w.severity == Severity::Info && w.message.contains("defaulting to graph")));
    }

    // ── Nodes ──

    #[test]
    fn parse_bare_node() {
        let doc = p("Alice\n");
        assert_eq!(doc.nodes.len(), 1);
        assert_eq!(doc.nodes[0].id, "Alice");
        assert_eq!(doc.nodes[0].shape, Shape::Rect);
    }

    #[test]
    fn parse_node_with_label() {
        let doc = p("Alice: \"The protagonist\"\n");
        assert_eq!(doc.nodes.len(), 1);
        assert_eq!(doc.nodes[0].label.as_deref(), Some("The protagonist"));
    }

    #[test]
    fn parse_node_with_shape_and_label() {
        let doc = p("Alice as circle: \"Hero\"\n");
        assert_eq!(doc.nodes.len(), 1);
        assert_eq!(doc.nodes[0].shape, Shape::Circle);
        assert_eq!(doc.nodes[0].label.as_deref(), Some("Hero"));
    }

    // ── Edges ──

    #[test]
    fn parse_simple_edge() {
        let doc = p("A → B\n");
        assert_eq!(doc.edges.len(), 1);
        assert_eq!(doc.edges[0].from, "A");
        assert_eq!(doc.edges[0].to, "B");
        assert_eq!(doc.edges[0].style, EdgeStyle::Solid);
        assert_eq!(doc.nodes.len(), 2);
    }

    #[test]
    fn parse_chained_edges() {
        let doc = p("A → B → C → D\n");
        assert_eq!(doc.edges.len(), 3);
        assert_eq!(doc.edges[0].from, "A");
        assert_eq!(doc.edges[2].to, "D");
        assert_eq!(doc.nodes.len(), 4);
    }

    #[test]
    fn parse_ascii_arrow() {
        let doc = p("A -> B\n");
        assert_eq!(doc.edges.len(), 1);
    }

    #[test]
    fn parse_dashed_edge() {
        let doc = p("A -.-> B\n");
        assert_eq!(doc.edges[0].style, EdgeStyle::Dashed);
    }

    #[test]
    fn parse_heavy_edge() {
        let doc = p("A => B\n");
        assert_eq!(doc.edges[0].style, EdgeStyle::Heavy);
    }

    // ── Planes ──

    #[test]
    fn parse_plane_block() {
        let doc = p("plane services {\nA\nB\n}\n");
        assert_eq!(doc.planes.len(), 1);
        assert_eq!(doc.planes[0].name, "services");
        assert_eq!(doc.planes[0].members, vec!["A".to_string(), "B".to_string()]);
        for n in &doc.nodes {
            assert_eq!(n.plane, "services");
        }
    }

    #[test]
    fn unclosed_plane_warns() {
        let doc = p("plane services {\nA\n");
        assert!(doc.warnings.iter().any(|w|
            w.message.contains("services") && w.message.contains("not closed")));
    }

    // ── Constraints ──

    #[test]
    fn parse_isosceles() {
        let doc = p("isosceles(A, B, C)\n");
        assert_eq!(doc.constraints.len(), 1);
        match &doc.constraints[0] {
            Constraint::NodeGeom { kind, nodes } => {
                assert_eq!(*kind, NodeGeomKind::Isosceles);
                assert_eq!(nodes, &vec!["A".to_string(), "B".to_string(), "C".to_string()]);
            }
            _ => panic!("expected NodeGeom"),
        }
    }

    #[test]
    fn parse_parallel_planes() {
        let doc = p("parallel(top, middle, bottom)\n");
        assert_eq!(doc.constraints.len(), 1);
        match &doc.constraints[0] {
            Constraint::PlaneGeom { kind, planes } => {
                assert_eq!(*kind, PlaneGeomKind::Parallel);
                assert_eq!(planes.len(), 3);
            }
            _ => panic!("expected PlaneGeom"),
        }
    }

    #[test]
    fn parse_pin() {
        let doc = p("pin A at (120, 80)\n");
        assert_eq!(doc.constraints.len(), 1);
        match &doc.constraints[0] {
            Constraint::Pin { node, x, y } => {
                assert_eq!(node, "A");
                assert!((x - 120.0).abs() < 0.01);
                assert!((y - 80.0).abs() < 0.01);
            }
            _ => panic!("expected Pin"),
        }
    }

    #[test]
    fn parse_relative() {
        let doc = p("A above B by 80\n");
        assert_eq!(doc.constraints.len(), 1);
        match &doc.constraints[0] {
            Constraint::Relative { node, direction, anchor, distance } => {
                assert_eq!(node, "A");
                assert_eq!(*direction, Direction::Above);
                assert_eq!(anchor, "B");
                assert!((distance - 80.0).abs() < 0.01);
            }
            _ => panic!("expected Relative"),
        }
    }

    // ── Warnings ──

    #[test]
    fn unknown_directive_warns_but_continues() {
        let doc = p("%kind: graph\n%made_up_directive: 42\nA → B\n");
        assert_eq!(doc.edges.len(), 1, "edge after unknown directive should still parse");
        assert!(doc.warnings.iter().any(|w|
            w.message.contains("unrecognized directive")
            && w.source.contains("made_up_directive")));
    }

    #[test]
    fn tilt_out_of_range_clamps_and_warns() {
        let doc = p("%kind: graph\n%tilt: 200°\n");
        assert_eq!(doc.tilt, Some(75.0));
        assert!(doc.warnings.iter().any(|w| w.message.contains("clamping")));
    }

    #[test]
    fn unmatched_close_brace_warns() {
        let doc = p("A\n}\nB\n");
        assert_eq!(doc.nodes.len(), 2);
        assert!(doc.warnings.iter().any(|w| w.message.contains("unmatched closing brace")));
    }

    // ── Edge cases ──

    #[test]
    fn empty_body_is_valid() {
        let doc = p("");
        assert_eq!(doc.nodes.len(), 0);
        assert_eq!(doc.edges.len(), 0);
        // Still gets the "defaulting to graph" info.
        assert!(doc.warnings.iter().any(|w| w.message.contains("defaulting")));
    }

    #[test]
    fn comments_are_stripped() {
        let doc = p("A → B   # this is the main edge\n");
        assert_eq!(doc.edges.len(), 1);
        assert_eq!(doc.edges[0].from, "A");
    }

    #[test]
    fn implicit_node_via_edge() {
        // No declaration line for either node, just the edge.
        let doc = p("Foo → Bar\n");
        assert_eq!(doc.nodes.len(), 2);
        assert!(doc.nodes.iter().any(|n| n.id == "Foo"));
        assert!(doc.nodes.iter().any(|n| n.id == "Bar"));
    }

    // ── Fence extraction ──

    #[test]
    fn extract_strips_fence_markers() {
        let body = extract_body("```niftyplot\nA → B\n```");
        assert!(body.contains("A → B"));
        assert!(!body.contains("```"));
    }

    #[test]
    fn extract_passes_through_naked_body() {
        let body = extract_body("A → B");
        assert_eq!(body, "A → B");
    }

    // ── Suggestion helper ──

    #[test]
    fn suggest_kind_for_empty_buffer() {
        let s = suggest("", 0, 5);
        assert!(s.iter().any(|c| c.contains("%kind")));
    }

    #[test]
    fn suggest_kinds_after_kind_colon() {
        let buf = "%kind: ";
        let s = suggest(buf, buf.len(), 5);
        assert!(s.contains(&"graph".to_string()));
        assert!(s.contains(&"gantt".to_string()));
    }

    #[test]
    fn suggest_existing_nodes_after_arrow() {
        let buf = "%kind: graph\nAlice\nBob\nCharlie\nDavid → ";
        let s = suggest(buf, buf.len(), 5);
        // Should include recently-typed nodes.
        assert!(s.iter().any(|c| c == "Charlie" || c == "Bob"));
    }

    #[test]
    fn suggest_constraint_completion() {
        let buf = "%kind: graph\nA\nB\nC\niso";
        let s = suggest(buf, buf.len(), 5);
        assert!(s.iter().any(|c| c.starts_with("isosceles(")));
    }

    // ── Scope detection ──

    #[test]
    fn scope_at_top_level_is_none() {
        let buf = "A → B\n";
        assert!(scope_at(buf, buf.len()).is_none());
    }

    #[test]
    fn scope_at_inside_plane() {
        let buf = "plane services {\nA\n";
        let scope = scope_at(buf, buf.len());
        assert_eq!(scope, Some("services".to_string()));
    }

    #[test]
    fn scope_at_after_close_is_none() {
        let buf = "plane services {\nA\n}\n";
        let scope = scope_at(buf, buf.len());
        assert!(scope.is_none(), "expected None after close, got {:?}", scope);
    }

    #[test]
    fn scope_at_nested_returns_innermost() {
        let buf = "plane outer {\nplane inner {\nA\n";
        let scope = scope_at(buf, buf.len());
        assert_eq!(scope, Some("inner".to_string()));
    }
}
