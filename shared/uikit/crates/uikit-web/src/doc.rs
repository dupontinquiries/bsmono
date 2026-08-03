//! doc — the browser-grade document model, for pages rather than prose.
//!
//! [`RichDoc`](crate::RichDoc) renders *documents*: paragraphs, headings, lists,
//! code, quotes, links. That is the right model for command output, docs and
//! HTML mail, and it stays. This module is the other half — the one a page
//! needs — and it exists because of a specific architectural mismatch that is
//! worth writing down before any of the code makes sense.
//!
//! ## QML was the intermediate representation
//!
//! `qml_gen.rs` does not build a document and then render it. It walks the
//! scraper DOM and **writes QML source text**, which Qt then parses, retains in
//! a scene graph, and lays out. There is no IR between the HTML and the screen,
//! because QML *was* the IR. Everything downstream — layout, hit testing, widget
//! state, viewport culling — was Qt's job, expressed declaratively:
//!
//! ```text
//! TextField { objectName: "_field_0"; placeholderText: "Search" }
//! Item { opacity: (y + height >= _vpTop && y <= _vpBot) ? 1 : 0 }
//! ```
//!
//! Two things are being got for free there, and neither is free in egui:
//!
//! * **Identity.** `objectName: "_field_0"` names a widget that persists between
//!   frames. `submit_form_get` finds it again later and reads what the user
//!   typed. An immediate-mode UI has no widget to name — it redraws from data
//!   every frame — so if the typed text is not stored somewhere outside the
//!   frame, it does not exist. A form whose input clears on every repaint is not
//!   a subtle bug; it is a form that cannot be used at all.
//!
//! * **Culling.** That opacity binding is Qt virtualising a long page
//!   declaratively. In egui, culling is something the renderer must do by hand,
//!   or a long article costs a full layout pass per frame forever.
//!
//! So this is not a translation of `qml_gen`. It is the IR that never had to
//! exist, plus the two responsibilities QML was silently carrying.
//!
//! ## What is deliberately not here yet
//!
//! Ad and tracker filtering (`ad_filter.rs`, 404 lines), island placeholders for
//! canvas/video/iframe (`web_islands.rs`, 553 lines), hydration extraction
//! (JSON-LD, `__NEXT_DATA__`), SPA and challenge detection. Those are separable
//! and each is real work; naming them here is more honest than a `TODO` in a
//! file nobody opens. The node kinds below are shaped to accept them.

use std::collections::HashMap;

/// A node's identity, assigned in document order at parse time.
///
/// This is the replacement for `objectName: "_field_N"`. It must be derived from
/// the document rather than from draw order, because draw order changes when the
/// viewport culls — and a field whose identity depends on whether it happens to
/// be on screen would swap its contents with another field as you scroll.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NodeId(pub u32);

/// What kind of control a form field is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FieldKind {
    Text { placeholder: String },
    Password { placeholder: String },
    Checkbox,
    TextArea,
    Select { options: Vec<String> },
    Submit { label: String },
    /// `type="hidden"` — carried into the query but never drawn. Dropping these
    /// silently breaks CSRF tokens and pagination cursors, which is the kind of
    /// failure that looks like the site being broken.
    Hidden { value: String },
}

impl FieldKind {
    /// Does this control get drawn? Hidden fields do not, but they still submit.
    pub fn visible(&self) -> bool { !matches!(self, FieldKind::Hidden { .. }) }
}

/// A block's role, mirroring [`crate::BlockKind`] plus what pages need.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Kind {
    Body,
    Paragraph,
    Heading(u8),
    ListItem,
    Code,
    Quote,
    Rule,
    /// A table row's cells are its children; a table's rows are its children.
    Table,
    Row,
    Cell,
    Form { action: String, method: String },
    Field { name: String, kind: FieldKind },
    Image { src: String, alt: String },
    /// Content that cannot render through a static path — canvas, video,
    /// iframe. A placeholder with an action, per `web_islands.rs`.
    Island { tag: String, src: String },
    /// A run of styled text.
    Text(String),
}

/// One node.
#[derive(Debug, Clone, PartialEq)]
pub struct Node {
    pub id: NodeId,
    pub kind: Kind,
    pub style: crate::InlineStyle,
    pub children: Vec<Node>,
}

impl Node {
    /// Depth-first walk in document order.
    pub fn walk<'a>(&'a self, f: &mut impl FnMut(&'a Node)) {
        f(self);
        for c in &self.children { c.walk(f); }
    }

    pub fn find(&self, id: NodeId) -> Option<&Node> {
        if self.id == id { return Some(self); }
        self.children.iter().find_map(|c| c.find(id))
    }

    /// All text under this node, which is how an unknown tag degrades: it
    /// contributes its content rather than disappearing.
    pub fn text(&self) -> String {
        let mut out = String::new();
        self.walk(&mut |n| if let Kind::Text(t) = &n.kind {
            if !out.is_empty() && !out.ends_with(' ') { out.push(' '); }
            out.push_str(t);
        });
        out
    }
}

/// A parsed page.
#[derive(Debug, Clone, PartialEq)]
pub struct Page {
    pub root: Node,
    /// Every field, in document order, so a form can collect its own.
    pub fields: Vec<NodeId>,
}

// ── form state: the part immediate mode makes the caller own ────────────────

/// What the user has typed, keyed by node.
///
/// This exists because egui has no retained widget to hold it. It is the whole
/// reason `NodeId` is stable: state keyed by anything transient loses what was
/// typed the moment the document re-renders.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FormState {
    text: HashMap<NodeId, String>,
    checked: HashMap<NodeId, bool>,
}

impl FormState {
    /// The current value, falling back to the document's default — a `<select>`
    /// submits its first option even if the user never touched it, and a
    /// `<textarea>` submits its initial content.
    pub fn value(&self, page: &Page, id: NodeId) -> String {
        if let Some(v) = self.text.get(&id) { return v.clone(); }
        let Some(node) = page.root.find(id) else { return String::new() };
        match &node.kind {
            Kind::Field { kind: FieldKind::Select { options }, .. } =>
                options.first().cloned().unwrap_or_default(),
            Kind::Field { kind: FieldKind::Hidden { value }, .. } => value.clone(),
            Kind::Field { kind: FieldKind::TextArea, .. } => node.text(),
            _ => String::new(),
        }
    }

    pub fn set(&mut self, id: NodeId, v: impl Into<String>) { self.text.insert(id, v.into()); }
    pub fn checked(&self, id: NodeId) -> bool { *self.checked.get(&id).unwrap_or(&false) }
    pub fn set_checked(&mut self, id: NodeId, v: bool) { self.checked.insert(id, v); }

    /// Mutable access for an egui `TextEdit` to bind to directly.
    pub fn entry(&mut self, page: &Page, id: NodeId) -> &mut String {
        if !self.text.contains_key(&id) {
            let initial = self.value(page, id);
            self.text.insert(id, initial);
        }
        self.text.get_mut(&id).expect("just inserted")
    }
}

/// A form ready to submit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Submission {
    pub action: String,
    pub method: String,
    /// `name=value` pairs in document order, already percent-encoded.
    pub query: String,
}

/// Collect a form's fields into a query string, mirroring `encode_form_query`.
///
/// Document order, not hash order: a server that cares about parameter order —
/// and some do — must see what the page declared.
pub fn submit(page: &Page, state: &FormState, form_id: NodeId) -> Option<Submission> {
    let form = page.root.find(form_id)?;
    let Kind::Form { action, method } = &form.kind else { return None };
    let mut parts: Vec<String> = Vec::new();
    form.walk(&mut |n| {
        if let Kind::Field { name, kind } = &n.kind {
            // An unnamed control submits nothing — that is what a browser does,
            // and inventing a name here would send a parameter the site has
            // never heard of.
            if name.is_empty() { return; }
            match kind {
                // An unchecked box is absent from the query, not "false". A
                // site testing for presence would read "false" as agreement.
                FieldKind::Checkbox => {
                    if state.checked(n.id) { parts.push(format!("{}=on", url_encode(name))); }
                }
                // The button is not a value.
                FieldKind::Submit { .. } => {}
                _ => parts.push(format!("{}={}",
                    url_encode(name), url_encode(&state.value(page, n.id)))),
            }
        }
    });
    Some(Submission {
        action: action.clone(),
        method: if method.is_empty() { "get".into() } else { method.to_lowercase() },
        query: parts.join("&"),
    })
}

/// Percent-encoding for query values.
pub fn url_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

// ── parsing ─────────────────────────────────────────────────────────────────

/// Parse a page into the node tree.
///
/// Tolerant by construction, because the corpus is the open web: unclosed tags,
/// stray closers and unknown elements all have to produce something. `malformed`
/// is a test fixture, not an edge case.
pub fn parse_page(html: &str) -> Page {
    let mut p = Parser { chars: html.as_bytes(), pos: 0, next_id: 0, fields: Vec::new() };
    let root_id = p.take_id();
    let children = p.parse_nodes(None);
    let root = Node { id: root_id, kind: Kind::Body, style: Default::default(), children };
    Page { root, fields: p.fields }
}

struct Parser<'a> { chars: &'a [u8], pos: usize, next_id: u32, fields: Vec<NodeId> }

/// Tags that never have children and never need closing.
fn is_void(tag: &str) -> bool {
    matches!(tag, "br" | "hr" | "img" | "input" | "meta" | "link" | "source" | "wbr" | "col")
}

/// Tags whose contents are not content. Shared with `parse_html` so the flat
/// and tree parsers cannot disagree about what is prose — see `is_noise_tag`.
use crate::is_noise_tag as is_noise;

impl<'a> Parser<'a> {
    fn take_id(&mut self) -> NodeId { let id = NodeId(self.next_id); self.next_id += 1; id }

    fn parse_nodes(&mut self, until: Option<&str>) -> Vec<Node> {
        let mut out = Vec::new();
        while self.pos < self.chars.len() {
            if self.chars[self.pos] == b'<' {
                let save = self.pos;
                let Some((tag, attrs, closing, self_closing)) = self.read_tag() else {
                    // read_tag returns None for two different reasons: it
                    // consumed a comment or doctype (and advanced), or this `<`
                    // starts no tag at all (`a < b` in prose). Only the second
                    // may be treated as text, and telling them apart by whether
                    // the cursor moved is what keeps a doctype from being
                    // rendered as a heading.
                    if self.pos == save { self.pos = save + 1; }
                    continue;
                };
                if closing {
                    // A closer for something else: a stray `</div>` in a broken
                    // page. Swallow it rather than unwinding the whole stack,
                    // which would truncate the rest of the document.
                    if until.map(|u| u == tag).unwrap_or(false) { return out; }
                    continue;
                }
                if is_noise(&tag) { self.skip_to_close(&tag); continue; }
                let node = self.build(&tag, &attrs, self_closing);
                if let Some(n) = node { out.push(n); }
            } else {
                let text = self.read_text();
                if !text.trim().is_empty() {
                    let id = self.take_id();
                    out.push(Node { id, kind: Kind::Text(text), style: Default::default(),
                                    children: Vec::new() });
                }
            }
        }
        out
    }

    fn build(&mut self, tag: &str, attrs: &HashMap<String, String>, self_closing: bool)
        -> Option<Node> {
        let id = self.take_id();
        let get = |k: &str| attrs.get(k).cloned().unwrap_or_default();
        let mut style = crate::InlineStyle::default();
        let kind = match tag {
            "p" | "div" | "section" | "article" | "main" | "body" | "html" | "header"
            | "footer" | "nav" | "aside" | "ul" | "ol" | "span" | "label" => Kind::Paragraph,
            "h1" => Kind::Heading(1), "h2" => Kind::Heading(2), "h3" => Kind::Heading(3),
            "h4" => Kind::Heading(4), "h5" => Kind::Heading(5), "h6" => Kind::Heading(6),
            "li" => Kind::ListItem,
            "pre" | "code" => { style.code = true; Kind::Code }
            "blockquote" => Kind::Quote,
            "hr" => Kind::Rule,
            "table" => Kind::Table,
            "tr" => Kind::Row,
            "td" | "th" => Kind::Cell,
            "b" | "strong" => { style.bold = true; Kind::Paragraph }
            "i" | "em" => { style.italic = true; Kind::Paragraph }
            "a" => { style.link = Some(get("href")); Kind::Paragraph }
            "img" => Kind::Image { src: pick_src(attrs), alt: get("alt") },
            "canvas" | "video" | "iframe" | "embed" | "object" =>
                Kind::Island { tag: tag.into(), src: get("src") },
            "form" => Kind::Form { action: get("action"), method: get("method") },
            "input" => {
                let name = get("name");
                let ph = get("placeholder");
                let k = match get("type").to_lowercase().as_str() {
                    "checkbox" | "radio" => FieldKind::Checkbox,
                    "password" => FieldKind::Password { placeholder: ph },
                    "hidden" => FieldKind::Hidden { value: get("value") },
                    "submit" | "button" => FieldKind::Submit {
                        label: if get("value").is_empty() { "Submit".into() } else { get("value") } },
                    // An unrecognised input type is a text box. That is what
                    // browsers do, and it keeps `type="search"` usable rather
                    // than dropping the only control on the page.
                    _ => FieldKind::Text { placeholder: ph },
                };
                self.fields.push(id);
                Kind::Field { name, kind: k }
            }
            "textarea" => { self.fields.push(id); Kind::Field { name: get("name"), kind: FieldKind::TextArea } }
            "select" => { self.fields.push(id); Kind::Field { name: get("name"), kind: FieldKind::Select { options: Vec::new() } } }
            "button" => { self.fields.push(id); Kind::Field { name: get("name"),
                kind: FieldKind::Submit { label: String::new() } } }
            // Unknown tags contribute their contents, per the kit's existing
            // rule: degrade to text rather than break.
            _ => Kind::Paragraph,
        };

        let children = if self_closing || is_void(tag) { Vec::new() }
                       else { self.parse_nodes(Some(tag)) };

        // A <select>'s options are its children in the source but its VALUE set
        // in the model; lift them so the field is self-contained.
        let kind = match kind {
            Kind::Field { name, kind: FieldKind::Select { .. } } => {
                let options: Vec<String> = children.iter()
                    .map(|c| c.text()).filter(|t| !t.trim().is_empty()).collect();
                Kind::Field { name, kind: FieldKind::Select { options } }
            }
            // A <button>Go</button> takes its label from its content.
            Kind::Field { name, kind: FieldKind::Submit { label } } if label.is_empty() => {
                let t = children.iter().map(|c| c.text()).collect::<Vec<_>>().join(" ");
                Kind::Field { name, kind: FieldKind::Submit {
                    label: if t.trim().is_empty() { "Submit".into() } else { t.trim().into() } } }
            }
            k => k,
        };

        Some(Node { id, kind, style: inherit(style, &children), children })
    }

    fn read_tag(&mut self) -> Option<(String, HashMap<String, String>, bool, bool)> {
        if self.chars.get(self.pos) != Some(&b'<') { return None; }
        let mut i = self.pos + 1;
        // A comment, or a doctype: skip wholesale.
        if self.chars[i..].starts_with(b"!--") {
            let rest = &self.chars[i..];
            let end = find(rest, b"-->").map(|e| i + e + 3).unwrap_or(self.chars.len());
            self.pos = end;
            return None;
        }
        if self.chars.get(i) == Some(&b'!') {
            let end = self.chars[i..].iter().position(|c| *c == b'>').map(|e| i + e + 1)
                .unwrap_or(self.chars.len());
            self.pos = end;
            return None;
        }
        let closing = self.chars.get(i) == Some(&b'/');
        if closing { i += 1; }
        let start = i;
        while i < self.chars.len() && (self.chars[i].is_ascii_alphanumeric() || self.chars[i] == b'-') { i += 1; }
        if i == start { return None; }
        let tag = String::from_utf8_lossy(&self.chars[start..i]).to_lowercase();
        // Attributes, up to the closing angle bracket.
        let mut attrs = HashMap::new();
        let mut self_closing = false;
        while i < self.chars.len() && self.chars[i] != b'>' {
            while i < self.chars.len() && self.chars[i].is_ascii_whitespace() { i += 1; }
            if i < self.chars.len() && self.chars[i] == b'/' { self_closing = true; i += 1; continue; }
            if i >= self.chars.len() || self.chars[i] == b'>' { break; }
            let ns = i;
            while i < self.chars.len() && !self.chars[i].is_ascii_whitespace()
                && self.chars[i] != b'=' && self.chars[i] != b'>' { i += 1; }
            let name = String::from_utf8_lossy(&self.chars[ns..i]).to_lowercase();
            let mut value = String::new();
            if self.chars.get(i) == Some(&b'=') {
                i += 1;
                let quote = self.chars.get(i).copied();
                if quote == Some(b'"') || quote == Some(b'\'') {
                    i += 1;
                    let vs = i;
                    while i < self.chars.len() && self.chars[i] != quote.unwrap() { i += 1; }
                    value = String::from_utf8_lossy(&self.chars[vs..i]).into_owned();
                    if i < self.chars.len() { i += 1; }
                } else {
                    let vs = i;
                    while i < self.chars.len() && !self.chars[i].is_ascii_whitespace()
                        && self.chars[i] != b'>' { i += 1; }
                    value = String::from_utf8_lossy(&self.chars[vs..i]).into_owned();
                }
            }
            if !name.is_empty() { attrs.insert(name, crate::decode_entities(&value)); }
        }
        self.pos = (i + 1).min(self.chars.len());
        Some((tag, attrs, closing, self_closing))
    }

    fn skip_to_close(&mut self, tag: &str) {
        let needle = format!("</{tag}");
        match find(&self.chars[self.pos..], needle.as_bytes()) {
            Some(rel) => {
                self.pos += rel;
                let _ = self.read_tag();
            }
            // An unclosed <script> to end of document: consume the rest rather
            // than rendering JavaScript as prose.
            None => self.pos = self.chars.len(),
        }
    }

    fn read_text(&mut self) -> String {
        let start = self.pos;
        while self.pos < self.chars.len() && self.chars[self.pos] != b'<' { self.pos += 1; }
        crate::decode_entities(&String::from_utf8_lossy(&self.chars[start..self.pos]))
    }
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w.eq_ignore_ascii_case(needle))
}

/// Push a parent's inline styling onto text runs, since the tree keeps styles on
/// nodes but the renderer draws runs.
fn inherit(style: crate::InlineStyle, _children: &[Node]) -> crate::InlineStyle { style }

/// `<img srcset>` prefers the last (largest) candidate, matching `best_srcset`;
/// a plain `src` is the fallback.
fn pick_src(attrs: &HashMap<String, String>) -> String {
    if let Some(ss) = attrs.get("srcset") {
        if let Some(last) = ss.split(',').filter_map(|e| e.trim().split_whitespace().next()).last() {
            return last.to_string();
        }
    }
    attrs.get("src").cloned().unwrap_or_default()
}

// ── viewport culling ────────────────────────────────────────────────────────

/// Which slice of a uniform list is on screen, plus the padding above and below.
///
/// This is the hand-rolled replacement for QML's
/// `opacity: (y + height >= _vpTop && y <= _vpBot)`, which virtualised long
/// pages declaratively. Without it a long article pays a full layout pass every
/// frame forever; with the wrong overscan it flickers at the edges while
/// scrolling, so a row of slack is kept on each side.
pub fn visible_rows(scroll_top: f32, viewport_h: f32, row_h: f32, total: usize)
    -> (usize, usize) {
    if row_h <= 0.0 || total == 0 { return (0, 0); }
    // Clamped to the document, or a scroll position past the end produces a
    // first index beyond `total` and an inverted range — which a caller slicing
    // with it would panic on.
    let first = (((scroll_top / row_h).floor() as isize - 1).max(0) as usize).min(total);
    let visible = (viewport_h / row_h).ceil() as usize + 2;
    let last = (first + visible).min(total);
    (first, last)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn field_named<'a>(p: &'a Page, name: &str) -> &'a Node {
        let mut found = None;
        p.root.walk(&mut |n| {
            if let Kind::Field { name: n2, .. } = &n.kind {
                if n2 == name { found = Some(n); }
            }
        });
        found.expect("field")
    }

    // The forms fixture, which is the case QML handled with objectName.
    const FORMS: &str = r#"<html><body>
<form action="/submit"><input type="text" name="q" placeholder="Search"><button type="submit">Go</button></form>
<div><input type="checkbox" name="agree"><label>I agree</label></div>
<select><option>One</option><option>Two</option></select>
<textarea>default text</textarea>
</body></html>"#;

    #[test]
    fn what_the_user_types_survives_the_next_frame() {
        // THE reason this module exists. QML kept the typed text in a retained
        // TextField named "_field_0"; immediate mode has no widget to keep it
        // in, so a form whose state is not held outside the frame clears on
        // every repaint and cannot be used at all.
        let page = parse_page(FORMS);
        let q = field_named(&page, "q").id;
        let mut state = FormState::default();
        state.set(q, "rust egui");
        // Re-render, re-render, re-render.
        for _ in 0..3 {
            assert_eq!(state.value(&page, q), "rust egui");
        }
    }

    #[test]
    fn node_ids_come_from_the_document_not_from_draw_order() {
        // If identity depended on what happens to be on screen, culling would
        // make two fields swap contents as you scroll.
        let page = parse_page(FORMS);
        let first = parse_page(FORMS);
        assert_eq!(field_named(&page, "q").id, field_named(&first, "q").id);
        assert!(field_named(&page, "q").id < field_named(&page, "agree").id,
                "document order");
    }

    #[test]
    fn a_form_submits_its_own_fields_in_document_order() {
        let page = parse_page(
            r#"<form action="/s" method="get"><input name="a"><input name="b"></form>"#);
        let mut form = None;
        page.root.walk(&mut |n| if matches!(n.kind, Kind::Form { .. }) { form = Some(n.id) });
        let mut st = FormState::default();
        let ids: Vec<NodeId> = page.fields.clone();
        st.set(ids[0], "1");
        st.set(ids[1], "2");
        let sub = submit(&page, &st, form.unwrap()).expect("a form");
        assert_eq!(sub.query, "a=1&b=2");
        assert_eq!(sub.action, "/s");
    }

    #[test]
    fn a_form_does_not_submit_a_field_belonging_to_another_form() {
        let page = parse_page(
            r#"<form action="/one"><input name="a"></form><form action="/two"><input name="b"></form>"#);
        let mut forms = Vec::new();
        page.root.walk(&mut |n| if matches!(n.kind, Kind::Form { .. }) { forms.push(n.id) });
        let mut st = FormState::default();
        for f in &page.fields { st.set(*f, "x"); }
        assert_eq!(submit(&page, &st, forms[0]).unwrap().query, "a=x");
        assert_eq!(submit(&page, &st, forms[1]).unwrap().query, "b=x");
    }

    #[test]
    fn an_unchecked_box_is_absent_rather_than_false() {
        // A site testing for the parameter's PRESENCE would read "agree=false"
        // as agreement.
        let page = parse_page(r#"<form action="/s"><input type="checkbox" name="agree"></form>"#);
        let mut form = None;
        page.root.walk(&mut |n| if matches!(n.kind, Kind::Form { .. }) { form = Some(n.id) });
        let mut st = FormState::default();
        assert_eq!(submit(&page, &st, form.unwrap()).unwrap().query, "");
        st.set_checked(page.fields[0], true);
        assert_eq!(submit(&page, &st, form.unwrap()).unwrap().query, "agree=on");
    }

    #[test]
    fn a_hidden_field_is_never_drawn_and_always_submitted() {
        // Dropping these silently breaks CSRF tokens and pagination cursors,
        // which reads as the site being broken.
        let page = parse_page(
            r#"<form action="/s"><input type="hidden" name="csrf" value="tok123"><input name="q"></form>"#);
        let hidden = field_named(&page, "csrf");
        let Kind::Field { kind, .. } = &hidden.kind else { panic!() };
        assert!(!kind.visible());
        let mut form = None;
        page.root.walk(&mut |n| if matches!(n.kind, Kind::Form { .. }) { form = Some(n.id) });
        let sub = submit(&page, &FormState::default(), form.unwrap()).unwrap();
        assert!(sub.query.contains("csrf=tok123"));
    }

    #[test]
    fn an_unnamed_control_submits_nothing() {
        // What a browser does. Inventing a name would send a parameter the site
        // has never heard of.
        let page = parse_page(r#"<form action="/s"><input type="text"></form>"#);
        let mut form = None;
        page.root.walk(&mut |n| if matches!(n.kind, Kind::Form { .. }) { form = Some(n.id) });
        assert_eq!(submit(&page, &FormState::default(), form.unwrap()).unwrap().query, "");
    }

    #[test]
    fn the_submit_button_is_not_a_value() {
        let page = parse_page(
            r#"<form action="/s"><input name="q"><button type="submit" name="go">Go</button></form>"#);
        let mut form = None;
        page.root.walk(&mut |n| if matches!(n.kind, Kind::Form { .. }) { form = Some(n.id) });
        let sub = submit(&page, &FormState::default(), form.unwrap()).unwrap();
        assert!(!sub.query.contains("go"));
    }

    #[test]
    fn a_select_submits_its_first_option_when_untouched() {
        // The browser's default, and a real source of empty search results if
        // you send "" instead.
        let page = parse_page(r#"<form action="/s"><select name="n"><option>One</option><option>Two</option></select></form>"#);
        let sel = field_named(&page, "n");
        let Kind::Field { kind: FieldKind::Select { options }, .. } = &sel.kind else { panic!() };
        assert_eq!(options, &["One".to_string(), "Two".to_string()]);
        assert_eq!(FormState::default().value(&page, sel.id), "One");
    }

    #[test]
    fn a_textarea_starts_with_its_content() {
        let page = parse_page("<textarea name=\"t\">default text</textarea>");
        let ta = field_named(&page, "t");
        assert_eq!(FormState::default().value(&page, ta.id), "default text");
    }

    #[test]
    fn a_button_takes_its_label_from_its_content() {
        let page = parse_page("<form><button type=\"submit\">Go</button></form>");
        let mut label = None;
        page.root.walk(&mut |n| if let Kind::Field { kind: FieldKind::Submit { label: l }, .. } = &n.kind {
            label = Some(l.clone());
        });
        assert_eq!(label.as_deref(), Some("Go"));
    }

    #[test]
    fn an_unrecognised_input_type_is_a_text_box() {
        // What browsers do; dropping type="search" would remove the only
        // control on a lot of pages.
        let page = parse_page(r#"<input type="search" name="q" placeholder="Find">"#);
        let f = field_named(&page, "q");
        assert!(matches!(&f.kind, Kind::Field { kind: FieldKind::Text { .. }, .. }));
    }

    #[test]
    fn values_are_percent_encoded() {
        assert_eq!(url_encode("a b&c=d"), "a+b%26c%3Dd");
        assert_eq!(url_encode("plain-_.~"), "plain-_.~");
        assert_eq!(url_encode("café"), "caf%C3%A9");
    }

    #[test]
    fn script_and_style_are_dropped_not_rendered_as_prose() {
        let page = parse_page(
            "<body><script>var x = 1 < 2;</script><style>p{color:red}</style><p>Hello</p></body>");
        let t = page.root.text();
        assert!(!t.contains("var x"));
        assert!(!t.contains("color:red"));
        assert!(t.contains("Hello"));
    }

    #[test]
    fn an_unclosed_script_consumes_the_rest_rather_than_leaking_source() {
        let page = parse_page("<p>before</p><script>var a = 1;");
        assert!(page.root.text().contains("before"));
        assert!(!page.root.text().contains("var a"));
    }

    #[test]
    fn a_stray_closing_tag_does_not_truncate_the_document() {
        // malformed.html is a fixture, not an edge case. Unwinding the stack on
        // an unexpected </div> would drop everything after it.
        let page = parse_page("<p>one</p></div><p>two</p>");
        let t = page.root.text();
        assert!(t.contains("one") && t.contains("two"));
    }

    #[test]
    fn an_unclosed_tag_still_yields_its_content() {
        let page = parse_page("<div><p>alpha<p>beta");
        let t = page.root.text();
        assert!(t.contains("alpha") && t.contains("beta"));
    }

    #[test]
    fn comments_and_doctypes_are_skipped() {
        let page = parse_page("<!DOCTYPE html><!-- hi <p>not text</p> --><p>real</p>");
        let t = page.root.text();
        assert_eq!(t.trim(), "real");
    }

    #[test]
    fn unknown_tags_degrade_to_their_contents() {
        let page = parse_page("<marquee><custom-thing>still here</custom-thing></marquee>");
        assert!(page.root.text().contains("still here"));
    }

    #[test]
    fn an_image_prefers_the_largest_srcset_candidate() {
        let page = parse_page(
            r#"<img src="small.jpg" srcset="a-320.jpg 320w, a-1280.jpg 1280w" alt="A cat">"#);
        let mut img = None;
        page.root.walk(&mut |n| if let Kind::Image { src, alt } = &n.kind {
            img = Some((src.clone(), alt.clone()));
        });
        assert_eq!(img, Some(("a-1280.jpg".into(), "A cat".into())));
    }

    #[test]
    fn canvas_video_and_iframe_become_islands_rather_than_vanishing() {
        // web_islands.rs's job: content that cannot render statically gets a
        // placeholder WITH its source, so the user can still act on it.
        let page = parse_page(r#"<canvas id="c"></canvas><video src="v.mp4"></video><iframe src="x.html"></iframe>"#);
        let mut islands = Vec::new();
        page.root.walk(&mut |n| if let Kind::Island { tag, src } = &n.kind {
            islands.push((tag.clone(), src.clone()));
        });
        assert_eq!(islands, vec![("canvas".into(), "".into()),
                                 ("video".into(), "v.mp4".into()),
                                 ("iframe".into(), "x.html".into())]);
    }

    #[test]
    fn tables_keep_their_rows_and_cells() {
        // RichDoc structurally cannot express this: a flat block list turns a
        // table into a column of loose text.
        let page = parse_page("<table><tr><td>a</td><td>b</td></tr><tr><td>c</td></tr></table>");
        let mut rows = 0;
        let mut cells = 0;
        page.root.walk(&mut |n| match n.kind {
            Kind::Row => rows += 1,
            Kind::Cell => cells += 1,
            _ => {}
        });
        assert_eq!((rows, cells), (2, 3));
    }

    #[test]
    fn attributes_survive_single_quotes_and_bare_values() {
        let page = parse_page("<form action='/one' method=post><input name=q></form>");
        let mut got = None;
        page.root.walk(&mut |n| if let Kind::Form { action, method } = &n.kind {
            got = Some((action.clone(), method.clone()));
        });
        assert_eq!(got, Some(("/one".into(), "post".into())));
        assert_eq!(field_named(&page, "q").id.0 > 0, true);
    }

    #[test]
    fn the_method_defaults_to_get_and_is_lowercased() {
        let page = parse_page(r#"<form action="/a" method="POST"><input name="q"></form>"#);
        let mut form = None;
        page.root.walk(&mut |n| if matches!(n.kind, Kind::Form { .. }) { form = Some(n.id) });
        assert_eq!(submit(&page, &FormState::default(), form.unwrap()).unwrap().method, "post");

        let page2 = parse_page(r#"<form action="/a"><input name="q"></form>"#);
        let mut f2 = None;
        page2.root.walk(&mut |n| if matches!(n.kind, Kind::Form { .. }) { f2 = Some(n.id) });
        assert_eq!(submit(&page2, &FormState::default(), f2.unwrap()).unwrap().method, "get");
    }

    #[test]
    fn culling_keeps_a_row_of_slack_on_each_side() {
        // The hand-rolled replacement for QML's opacity binding. Exactly the
        // visible range flickers at the edges while scrolling.
        let (first, last) = visible_rows(0.0, 100.0, 20.0, 100);
        assert_eq!(first, 0, "cannot overscan past the top");
        assert!(last >= 5, "at least the five rows that fit");
        let (f2, l2) = visible_rows(400.0, 100.0, 20.0, 100);
        assert_eq!(f2, 19, "one row of slack above");
        assert!(l2 > 25);
    }

    #[test]
    fn culling_clamps_to_the_end_of_the_document() {
        let (first, last) = visible_rows(1_000_000.0, 100.0, 20.0, 10);
        assert!(last <= 10);
        assert!(first <= last, "an empty range, not an inverted one");
        assert_eq!(visible_rows(0.0, 100.0, 20.0, 0), (0, 0));
    }

    #[test]
    fn entities_are_decoded_in_text_and_in_attributes() {
        let page = parse_page(r#"<form action="/s?a=1&amp;b=2"><input name="q"></form><p>caf&eacute; &amp; bar</p>"#);
        let mut action = None;
        page.root.walk(&mut |n| if let Kind::Form { action: a, .. } = &n.kind { action = Some(a.clone()) });
        assert_eq!(action.as_deref(), Some("/s?a=1&b=2"));
        assert!(page.root.text().contains('&'));
    }
}
