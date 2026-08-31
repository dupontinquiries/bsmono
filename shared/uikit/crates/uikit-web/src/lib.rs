//! web — an efficient HTML → egui renderer for voiceterm.
//!
//! **The changed assumption.** The old path rendered web content into QML, which *retains* an
//! element tree in its scene graph. egui is immediate-mode: re-parsing HTML every frame would
//! be wasteful. So the model inverts — we parse the HTML **once** into a flat, render-ready
//! **display list** ([`RichDoc`]: blocks of styled inline runs), and each frame we just walk
//! that list into egui widgets. Layout/measurement is egui's job per frame, but tokenizing and
//! tree-flattening (the expensive part) happen only when the content changes.
//!
//! The parser is a tolerant subset — block tags (p, h1–h6, li, pre/code, blockquote, hr) and
//! inline tags (b/strong, i/em, code, a) with HTML entities — enough for command output, docs,
//! and HTML mail. Unknown tags degrade to their text content rather than breaking.

pub mod doc;

/// Inline text styling (flattened from nested inline tags).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct InlineStyle {
    pub bold: bool,
    pub italic: bool,
    pub code: bool,
    pub link: Option<String>,
}

/// A run of text sharing one style.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Inline {
    pub text: String,
    pub style: InlineStyle,
}

/// The kind of block (drives block-level layout).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlockKind {
    Paragraph,
    Heading(u8),
    ListItem,
    Code,
    Quote,
    Rule,
    /// A table, kept as rows rather than flattened into prose.
    ///
    /// Flattening is what the parser did with them until now, and a data
    /// table run together into one paragraph is not degraded — it is
    /// unreadable, and tables are where the facts on a page usually are.
    ///
    /// The first row is the header when the source marked it with `th`;
    /// `has_header` says which, because a table whose first data row is
    /// drawn as a heading misreports its own contents.
    Table { has_header: bool },
}

/// One table cell: inline runs, like everything else.
pub type Cell = Vec<Inline>;

/// The most rows rendered from one table.
///
/// Ported with its reason from the QML generator, where the comment records
/// what happened without it: a table is ONE node against the node budget
/// but emits every row, so a multi-thousand-row table blew past the cap and
/// produced a million lines of QML. Here it would be a display list that
/// takes a second to walk every frame. The overflow is COUNTED and shown,
/// not silently dropped — "showing 200 of 4,812" is information; a table
/// that quietly stops is a wrong answer.
pub const MAX_TABLE_ROWS: usize = 200;

/// A block: a kind plus its inline runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Block {
    pub kind: BlockKind,
    pub inlines: Vec<Inline>,
    /// Rows, for `BlockKind::Table`. Empty for every other kind — a table
    /// is the one block whose content is two-dimensional, and giving every
    /// block a `rows` field is cheaper than a second block type that half
    /// the code has to know about.
    pub rows: Vec<Vec<Cell>>,
    /// How many rows the source had, when that is more than were kept.
    pub rows_dropped: usize,
}

impl Block {
    /// A block of inline runs — every kind but a table.
    pub fn new(kind: BlockKind, inlines: Vec<Inline>) -> Block {
        Block { kind, inlines, rows: Vec::new(), rows_dropped: 0 }
    }
}

/// The render-ready display list. Built once from HTML; rendered many times.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RichDoc {
    pub blocks: Vec<Block>,
    /// Blocks the budget refused. Counted rather than dropped silently: a
    /// page that stops half way with no explanation reads as a broken
    /// parser, and the number is the thing that tells a reader to open the
    /// original.
    pub blocks_dropped: usize,
    /// Elements the caller's filter removed.
    pub filtered: usize,
}

/// One element, as a filter sees it.
///
/// The four attributes every ad and tracking heuristic in this fleet reads.
/// Passed as a struct rather than four arguments so adding a fifth does not
/// change every caller's signature.
#[derive(Debug, Clone, Copy)]
pub struct Element<'a> {
    pub tag: &'a str,
    pub id: Option<&'a str>,
    pub class: Option<&'a str>,
    pub src: Option<&'a str>,
}

/// The most blocks a document may produce.
///
/// The QML generator caps at 500 nodes, and its header records what
/// happens without a cap: one enormous page and the output is unusable.
/// Here the cost lands differently — the display list is walked EVERY
/// FRAME, so an unbounded one is a page that gets slower the longer it is,
/// forever, rather than a slow load.
pub const MAX_BLOCKS: usize = 2_000;

/// How to parse.
///
/// The element filter is a CALLBACK rather than a mode enum, because the
/// policy does not belong here. voiceterm's `ad_filter` knows about
/// tracking pixels, consent walls and sticky newsletter bars; that is
/// application knowledge and it changes on a different schedule from an
/// HTML parser. A shared widget kit reaching into an app for it would
/// invert the dependency; an enum here would mean the kit re-implementing
/// the list.
pub struct ParseOptions<'a> {
    pub max_blocks: usize,
    /// Return true to drop the element AND its subtree.
    pub skip_element: Option<&'a dyn Fn(Element<'_>) -> bool>,
}

impl Default for ParseOptions<'_> {
    fn default() -> Self {
        ParseOptions { max_blocks: MAX_BLOCKS, skip_element: None }
    }
}

impl RichDoc {
    /// Plain-text projection (handy for tests / a11y / search).
    pub fn text(&self) -> String {
        self.blocks
            .iter()
            .map(|b| b.inlines.iter().map(|i| i.text.as_str()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }
}

// ── tokenizer ────────────────────────────────────────────────────────────────

enum Token {
    Open { name: String, attrs: Vec<(String, String)> },
    Close(String),
    Text(String),
}

fn decode_entities(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        let after = &rest[amp..];
        if let Some(semi) = after.find(';') {
            let ent = &after[1..semi];
            let decoded = match ent {
                "amp" => Some('&'),
                "lt" => Some('<'),
                "gt" => Some('>'),
                "quot" => Some('"'),
                "apos" | "#39" => Some('\''),
                "nbsp" => Some(' '),
                _ if ent.starts_with('#') => ent[1..].parse::<u32>().ok().and_then(char::from_u32),
                _ => None,
            };
            match decoded {
                Some(c) => {
                    out.push(c);
                    rest = &after[semi + 1..];
                    continue;
                }
                None => {
                    out.push('&');
                    rest = &after[1..];
                    continue;
                }
            }
        } else {
            out.push('&');
            rest = &after[1..];
        }
    }
    out.push_str(rest);
    out
}

fn tokenize(html: &str) -> Vec<Token> {
    let mut toks = Vec::new();
    let bytes = html.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'<' {
            // find the end of the tag
            if let Some(end) = html[i..].find('>') {
                let raw = &html[i + 1..i + end];
                i += end + 1;
                let raw = raw.trim();
                if raw.starts_with('!') || raw.starts_with('?') {
                    continue; // comments / doctype / PIs
                }
                if let Some(name) = raw.strip_prefix('/') {
                    toks.push(Token::Close(name.trim().to_lowercase()));
                } else {
                    let self_close = raw.ends_with('/');
                    let body = raw.trim_end_matches('/').trim();
                    let mut parts = body.splitn(2, char::is_whitespace);
                    let name = parts.next().unwrap_or("").to_lowercase();
                    let attrs = parts.next().map(parse_attrs).unwrap_or_default();
                    toks.push(Token::Open { name: name.clone(), attrs });
                    if self_close {
                        toks.push(Token::Close(name));
                    }
                }
            } else {
                break; // unterminated tag
            }
        } else {
            let next = html[i..].find('<').map(|p| i + p).unwrap_or(bytes.len());
            let text = decode_entities(&html[i..next]);
            if !text.is_empty() {
                toks.push(Token::Text(text));
            }
            i = next;
        }
    }
    toks
}

fn parse_attrs(s: &str) -> Vec<(String, String)> {
    let mut attrs = Vec::new();
    let chars: Vec<char> = s.chars().collect();
    let n = chars.len();
    let mut i = 0;
    while i < n {
        while i < n && chars[i].is_whitespace() {
            i += 1;
        }
        let ks = i;
        while i < n && chars[i] != '=' && !chars[i].is_whitespace() {
            i += 1;
        }
        let key: String = chars[ks..i].iter().collect::<String>().to_lowercase();
        if key.is_empty() {
            break;
        }
        while i < n && chars[i].is_whitespace() {
            i += 1;
        }
        let mut val = String::new();
        if i < n && chars[i] == '=' {
            i += 1;
            while i < n && chars[i].is_whitespace() {
                i += 1;
            }
            if i < n && (chars[i] == '"' || chars[i] == '\'') {
                let q = chars[i];
                i += 1;
                let vs = i;
                while i < n && chars[i] != q {
                    i += 1;
                }
                val = chars[vs..i].iter().collect();
                if i < n {
                    i += 1;
                }
            } else {
                let vs = i;
                while i < n && !chars[i].is_whitespace() {
                    i += 1;
                }
                val = chars[vs..i].iter().collect();
            }
        }
        attrs.push((key, decode_entities(&val)));
    }
    attrs
}

// ── build the display list ───────────────────────────────────────────────────

/// Tags with no closing tag, which therefore have no subtree.
///
/// Load-bearing for the filter: opening a skip depth on an `<img>` would
/// swallow every element after it, because the close that would have ended
/// it never comes.
fn is_void_tag(tag: &str) -> bool {
    matches!(tag, "img" | "br" | "hr" | "input" | "meta" | "link" | "source"
                | "embed" | "area" | "base" | "col" | "track" | "wbr")
}

fn is_block(tag: &str) -> Option<BlockKind> {
    match tag {
        "p" | "div" => Some(BlockKind::Paragraph),
        "h1" => Some(BlockKind::Heading(1)),
        "h2" => Some(BlockKind::Heading(2)),
        "h3" => Some(BlockKind::Heading(3)),
        "h4" => Some(BlockKind::Heading(4)),
        "h5" => Some(BlockKind::Heading(5)),
        "h6" => Some(BlockKind::Heading(6)),
        "li" => Some(BlockKind::ListItem),
        "pre" => Some(BlockKind::Code),
        "blockquote" => Some(BlockKind::Quote),
        _ => None,
    }
}

/// Parse HTML into a flat [`RichDoc`] display list. Do this once, on content change.
/// Tags whose CONTENTS are not content.
///
/// Shared with `doc::parse_page` deliberately. The two parsers here are
/// different algorithms over different models — a flat block list and a tree —
/// and that is fine, but they are not allowed to disagree about what counts as
/// prose. `parse_html` used to have no notion of this at all and rendered
/// `var x = 1;` and `p{color:red}` straight into the document; the browser view
/// dropped them and the manual reader did not, which reads as a broken page
/// rather than as a bug and so goes unreported.
pub(crate) fn is_noise_tag(tag: &str) -> bool {
    matches!(tag, "script" | "style" | "head" | "noscript" | "svg")
}

/// Parse with the defaults: the standard block budget and no filter.
pub fn parse_html(html: &str) -> RichDoc {
    parse_html_with(html, &ParseOptions::default())
}

pub fn parse_html_with(html: &str, opts: &ParseOptions<'_>) -> RichDoc {
    let toks = tokenize(html);
    let mut dropped = 0usize;
    let mut filtered = 0usize;
    /// Depth of the filtered element we are inside, like `noise_depth`: a
    /// filter that dropped the open tag and not the subtree would leave the
    /// advertisement's text in the document with its wrapper gone, which is
    /// worse than not filtering.
    let mut skip_depth: usize = 0;
    let mut blocks: Vec<Block> = Vec::new();
    /// While inside a noise element, text tokens are discarded. Depth-counted
    /// rather than a flag, because `<svg><style>…</style></svg>` would
    /// otherwise switch prose back on at the inner close.
    let mut noise_depth: usize = 0;
    let mut cur: Option<Block> = None;
    let mut style = InlineStyle::default();
    let mut style_stack: Vec<InlineStyle> = Vec::new();
    /// The table being built, if any. A stack would let nested tables work
    /// and they are rare enough — and pathological enough as a layout
    /// device — that the inner one flattening into the outer cell is the
    /// better failure than the code to handle it.
    let mut table: Option<Block> = None;
    let mut row: Option<Vec<Cell>> = None;
    let mut cell: Option<Cell> = None;

    let cap = opts.max_blocks;
    let flush = |cur: &mut Option<Block>, blocks: &mut Vec<Block>, dropped: &mut usize| {
        if let Some(b) = cur.take() {
            if !b.inlines.iter().all(|i| i.text.trim().is_empty()) || b.kind == BlockKind::Rule {
                if blocks.len() < cap { blocks.push(b); } else { *dropped += 1; }
            }
        }
    };

    for tok in toks {
        // Handled before anything else: a noise element's own attributes and
        // children must not reach the block builder either.
        match &tok {
            Token::Open { name, .. } if is_noise_tag(name) => { noise_depth += 1; continue; }
            Token::Close(name) if is_noise_tag(name) => {
                noise_depth = noise_depth.saturating_sub(1);
                continue;
            }
            _ if noise_depth > 0 => continue,
            _ => {}
        }
        // THE CALLER'S FILTER, and the subtree with it. Counted by tag
        // depth: a filter that dropped the open tag alone would leave the
        // advertisement's text in the document with its wrapper gone.
        if let Some(skip) = opts.skip_element {
            match &tok {
                Token::Open { name, attrs } => {
                    let attr = |k: &str| attrs.iter()
                        .find(|(a, _)| a == k).map(|(_, v)| v.as_str());
                    if skip_depth > 0 {
                        // Already inside a filtered element: only the depth
                        // matters, and a self-closing tag inside it must not
                        // push one that never closes.
                        if !is_void_tag(name) { skip_depth += 1; }
                        continue;
                    }
                    if skip(Element { tag: name, id: attr("id"), class: attr("class"),
                                      src: attr("src") }) {
                        filtered += 1;
                        // A void element has no subtree to skip — an
                        // `<img>` filter that opened a depth would swallow
                        // the rest of the page.
                        if !is_void_tag(name) { skip_depth = 1; }
                        continue;
                    }
                }
                Token::Close(name) if skip_depth > 0 => {
                    if !is_void_tag(name) { skip_depth = skip_depth.saturating_sub(1); }
                    continue;
                }
                _ if skip_depth > 0 => continue,
                _ => {}
            }
        }
        match tok {
            Token::Open { name, attrs } => {
                if name == "table" {
                    flush(&mut cur, &mut blocks, &mut dropped);
                    table = Some(Block::new(BlockKind::Table { has_header: false }, vec![]));
                    continue;
                }
                if table.is_some() {
                    match name.as_str() {
                        "tr" => { row = Some(Vec::new()); continue; }
                        "td" | "th" => {
                            cell = Some(Vec::new());
                            // A `th` anywhere marks the table as headed.
                            // Checked per cell rather than per row because
                            // real tables put a `th` in the first COLUMN
                            // too, and only the first row's matters.
                            if name == "th" && table.as_ref().is_some_and(|t| t.rows.is_empty()) {
                                if let Some(t) = table.as_mut() {
                                    t.kind = BlockKind::Table { has_header: true };
                                }
                            }
                            continue;
                        }
                        // Inline styling inside a cell still applies; block
                        // tags inside one do not start a new block, or a
                        // `<td><p>x</p></td>` would tear the table apart.
                        _ if is_block(&name).is_some() => continue,
                        _ => {}
                    }
                }
                if name == "hr" {
                    flush(&mut cur, &mut blocks, &mut dropped);
                    blocks.push(Block::new(BlockKind::Rule, vec![]));
                } else if let Some(kind) = is_block(&name) {
                    flush(&mut cur, &mut blocks, &mut dropped);
                    cur = Some(Block::new(kind, vec![]));
                } else {
                    // inline styling
                    style_stack.push(style.clone());
                    match name.as_str() {
                        "b" | "strong" => style.bold = true,
                        "i" | "em" => style.italic = true,
                        "code" => style.code = true,
                        "a" => style.link = attrs.iter().find(|(k, _)| k == "href").map(|(_, v)| v.clone()),
                        "br" => {
                            if let Some(b) = cur.as_mut() {
                                b.inlines.push(Inline { text: "\n".into(), style: style.clone() });
                            }
                            style_stack.pop();
                        }
                        _ => {} // unknown inline: keep current style, render its text
                    }
                }
            }
            Token::Close(name) => {
                if table.is_some() {
                    match name.as_str() {
                        "td" | "th" => {
                            if let (Some(c), Some(r)) = (cell.take(), row.as_mut()) {
                                r.push(c);
                            }
                            continue;
                        }
                        "tr" => {
                            if let (Some(r), Some(t)) = (row.take(), table.as_mut()) {
                                if r.is_empty() {
                                    continue;
                                }
                                if t.rows.len() < MAX_TABLE_ROWS {
                                    t.rows.push(r);
                                } else {
                                    // Counted, not dropped silently — see
                                    // MAX_TABLE_ROWS. A table that just
                                    // stops is a wrong answer.
                                    t.rows_dropped += 1;
                                }
                            }
                            continue;
                        }
                        "table" => {
                            if let Some(t) = table.take() {
                                if !t.rows.is_empty() { blocks.push(t); }
                            }
                            row = None;
                            cell = None;
                            continue;
                        }
                        _ if is_block(&name).is_some() => continue,
                        _ => {}
                    }
                }
                if is_block(&name).is_some() || name == "hr" {
                    flush(&mut cur, &mut blocks, &mut dropped);
                } else if matches!(name.as_str(), "b" | "strong" | "i" | "em" | "code" | "a") {
                    if let Some(prev) = style_stack.pop() {
                        style = prev;
                    }
                }
            }
            Token::Text(text) => {
                // Inside a cell, text belongs to the cell. Text inside a
                // table but OUTSIDE a cell — whitespace between tags,
                // usually — is dropped rather than becoming a paragraph
                // wedged between the rows.
                if table.is_some() {
                    if let Some(c) = cell.as_mut() {
                        let t = collapse_ws(&text);
                        if !t.is_empty() {
                            c.push(Inline { text: t, style: style.clone() });
                        }
                    }
                    continue;
                }
                if cur.is_none() {
                    cur = Some(Block::new(BlockKind::Paragraph, vec![]));
                }
                let preserve = style.code || cur.as_ref().map(|b| b.kind == BlockKind::Code).unwrap_or(false);
                if let Some(b) = cur.as_mut() {
                    let text = if preserve { text } else { collapse_ws(&text) };
                    if !text.is_empty() {
                        b.inlines.push(Inline { text, style: style.clone() });
                    }
                }
            }
        }
    }
    flush(&mut cur, &mut blocks, &mut dropped);
    // An unclosed <table> still yields its rows. Malformed HTML is the
    // normal case, and losing a whole table to a missing </table> would be
    // the parser being right about the input and useless about the page.
    if let Some(t) = table.take() {
        if !t.rows.is_empty() { blocks.push(t); }
    }
    RichDoc { blocks, blocks_dropped: dropped, filtered }
}

fn collapse_ws(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_ws = false;
    for ch in s.chars() {
        if ch.is_whitespace() {
            if !prev_ws {
                out.push(' ');
            }
            prev_ws = true;
        } else {
            out.push(ch);
            prev_ws = false;
        }
    }
    out
}

// ── egui render (thin; walks the prebuilt list each frame) ───────────────────

/// Render a prebuilt [`RichDoc`] into egui. Cheap — no parsing here.
/// Draw the document. Returns the href of a link the user followed.
///
/// It used to return `()`, and links were drawn in the accent colour and
/// were not clickable — which is the worst version: they look like links,
/// so a person taps one and nothing happens. For a module whose job is to
/// replace the web handler, following a link is not a feature, it is the
/// thing a page is for.
pub fn render(ui: &mut egui::Ui, doc: &RichDoc,
              colors: &uikit_egui::PaletteColors) -> Option<String> {
    let mut followed = None;
    for block in &doc.blocks {
        if let BlockKind::Table { has_header } = block.kind {
            if let Some(h) = table_ui(ui, block, has_header, colors) {
                followed = Some(h);
            }
            continue;
        }
        match &block.kind {
            BlockKind::Rule => {
                ui.separator();
                continue;
            }
            BlockKind::Heading(level) => {
                let size = match level {
                    1 => 22.0,
                    2 => 19.0,
                    3 => 16.0,
                    _ => 14.0,
                };
                ui.horizontal_wrapped(|ui| {
                    for run in &block.inlines {
                        ui.label(egui::RichText::new(&run.text).size(size).strong().color(colors.text));
                    }
                });
                continue;
            }
            _ => {}
        }
        let quote = block.kind == BlockKind::Quote;
        let bullet = block.kind == BlockKind::ListItem;
        ui.horizontal_wrapped(|ui| {
            if bullet {
                ui.label(egui::RichText::new("• ").color(colors.text_weak));
            }
            if quote {
                ui.label(egui::RichText::new("▏").color(colors.accent_soft));
            }
            for run in &block.inlines {
                if let Some(h) = inline_ui(ui, run, quote, colors) {
                    followed = Some(h);
                }
            }
        });
    }
    if doc.blocks_dropped > 0 {
        // A page that stops half way with no explanation reads as a broken
        // parser. Saying how much is missing is what tells a reader to open
        // the original instead of scrolling for something that is not there.
        ui.label(egui::RichText::new(format!(
            "{} more block(s) not shown — the page is longer than this view renders",
            doc.blocks_dropped)).small().color(colors.text_weak));
    }
    followed
}

/// One styled run. A link becomes a real `ui.link`, which brings the hover
/// cursor and the underline with it — a coloured label has neither, and
/// both are how a person knows something is clickable before they try.
fn inline_ui(ui: &mut egui::Ui, run: &Inline, quote: bool,
             colors: &uikit_egui::PaletteColors) -> Option<String> {
    let mut rt = egui::RichText::new(&run.text);
    if run.style.bold { rt = rt.strong(); }
    if run.style.italic { rt = rt.italics(); }
    if run.style.code { rt = rt.monospace().background_color(colors.bg_raised); }
    match &run.style.link {
        Some(href) => {
            let r = ui.link(rt.color(colors.accent));
            // The destination on hover. A link whose text is "click here"
            // is the common case, and the only way to know where it goes
            // before committing is to be told.
            let r = if href.trim().is_empty() { r } else { r.on_hover_text(href) };
            if r.clicked() { return Some(href.clone()); }
            None
        }
        None => {
            ui.label(rt.color(if quote { colors.text_weak } else { colors.text }));
            None
        }
    }
}

/// Draw a table as a grid.
///
/// `egui::Grid` rather than a hand-laid set of columns: it measures the
/// widest cell per column and aligns every row to it, which is the whole
/// reason a table is a table. Laying columns out by hand means picking
/// widths before knowing the content.
fn table_ui(ui: &mut egui::Ui, block: &Block, has_header: bool,
            colors: &uikit_egui::PaletteColors) -> Option<String> {
    let mut followed = None;
    let id = ui.id().with(("web_table", block.rows.len(), block.rows_dropped));
    egui::Grid::new(id).striped(true).show(ui, |ui| {
        for (i, row) in block.rows.iter().enumerate() {
            let header = has_header && i == 0;
            for cell in row {
                ui.horizontal_wrapped(|ui| {
                    for run in cell {
                        if header {
                            // Bolded here rather than in the model: whether
                            // a header LOOKS like one is the renderer's
                            // business, and baking it into the runs would
                            // make the plain-text projection say it too.
                            let mut b = run.clone();
                            b.style.bold = true;
                            if let Some(h) = inline_ui(ui, &b, false, colors) {
                                followed = Some(h);
                            }
                        } else if let Some(h) = inline_ui(ui, run, false, colors) {
                            followed = Some(h);
                        }
                    }
                });
            }
            ui.end_row();
        }
    });
    if block.rows_dropped > 0 {
        // Said, not hidden. A table that silently stops at 200 rows is a
        // wrong answer; one that says how much it is not showing is a
        // limitation the reader can work around.
        ui.label(egui::RichText::new(format!(
            "showing {} of {} rows", block.rows.len(),
            block.rows.len() + block.rows_dropped)).small().color(colors.text_weak));
    }
    followed
}

#[cfg(test)]
mod filter_tests {
    use super::*;

    /// A stand-in for the app's `ad_filter`: the shape of the real one
    /// (tag, id, class, src) without its list.
    fn skip_ads(e: Element<'_>) -> bool {
        e.class.is_some_and(|c| c.contains("ad-"))
            || e.id.is_some_and(|i| i == "banner")
            || e.src.is_some_and(|s| s.contains("tracker"))
    }

    fn with_filter(html: &str) -> RichDoc {
        parse_html_with(html, &ParseOptions {
            skip_element: Some(&skip_ads), ..Default::default() })
    }

    #[test]
    fn a_filtered_element_takes_its_subtree_with_it() {
        // Dropping the wrapper alone leaves the advertisement's text in the
        // document with nothing around it — worse than not filtering,
        // because it reads as content.
        let d = with_filter(
            "<p>real</p><div class=\"ad-slot\"><p>BUY NOW</p><p>more ad</p></div><p>also real</p>");
        assert_eq!(d.text().contains("BUY NOW"), false, "{:?}", d.text());
        assert!(d.text().contains("real") && d.text().contains("also real"));
        assert_eq!(d.filtered, 1);
    }

    #[test]
    fn a_void_element_filter_does_not_swallow_the_rest_of_the_page() {
        // The bug this guards: an `<img>` has no close tag, so opening a
        // skip depth on one never ends and everything after it vanishes.
        let d = with_filter("<p>before</p><img src=\"https://x/tracker.gif\"><p>after</p>");
        assert!(d.text().contains("before"), "{:?}", d.text());
        assert!(d.text().contains("after"), "the page ended at the tracking pixel: {:?}",
                d.text());
        assert_eq!(d.filtered, 1);
    }

    #[test]
    fn nested_filtered_elements_close_at_the_right_depth() {
        // `<div class="ad-"><div class="ad-">…</div></div>` — the inner
        // close must not end the skip early and let the outer's tail back
        // in.
        let d = with_filter(
            "<div class=\"ad-a\"><div class=\"ad-b\">inner</div>outer tail</div><p>kept</p>");
        assert!(!d.text().contains("inner"), "{:?}", d.text());
        assert!(!d.text().contains("outer tail"), "the inner close ended the skip: {:?}",
                d.text());
        assert!(d.text().contains("kept"));
    }

    #[test]
    fn no_filter_means_no_filtering_and_no_cost() {
        let d = parse_html("<div class=\"ad-slot\"><p>BUY NOW</p></div>");
        assert!(d.text().contains("BUY NOW"));
        assert_eq!(d.filtered, 0);
    }

    #[test]
    fn the_block_budget_is_bounded_and_says_what_it_dropped() {
        // The display list is walked EVERY FRAME, so an unbounded one is a
        // page that stays slow rather than one that loads slowly.
        let html: String = (0..50).map(|i| format!("<p>para {i}</p>")).collect();
        let d = parse_html_with(&html, &ParseOptions { max_blocks: 20, ..Default::default() });
        assert_eq!(d.blocks.len(), 20);
        assert_eq!(d.blocks_dropped, 30);
        // And the default is generous enough that ordinary documents are
        // untouched, or the cap becomes the common case.
        let d2 = parse_html(&html);
        assert_eq!(d2.blocks_dropped, 0);
    }
}

#[cfg(test)]
mod table_tests {
    use super::*;

    fn cells(b: &Block) -> Vec<Vec<String>> {
        b.rows.iter().map(|r| r.iter()
            .map(|c| c.iter().map(|i| i.text.as_str()).collect::<String>())
            .collect()).collect()
    }

    #[test]
    fn a_table_keeps_its_shape_instead_of_running_together() {
        // Before this, a table flattened into one paragraph — not degraded,
        // unreadable, and tables are where the facts on a page usually are.
        let doc = parse_html(
            "<table><tr><th>City</th><th>Pop</th></tr>             <tr><td>Oslo</td><td>700k</td></tr>             <tr><td>Bergen</td><td>290k</td></tr></table>");
        assert_eq!(doc.blocks.len(), 1);
        assert_eq!(doc.blocks[0].kind, BlockKind::Table { has_header: true });
        assert_eq!(cells(&doc.blocks[0]), vec![
            vec!["City".to_string(), "Pop".to_string()],
            vec!["Oslo".to_string(), "700k".to_string()],
            vec!["Bergen".to_string(), "290k".to_string()],
        ]);
    }

    #[test]
    fn a_table_with_no_th_is_not_given_a_header_row() {
        // Drawing the first data row as a heading misreports the table's
        // own contents.
        let doc = parse_html("<table><tr><td>a</td><td>b</td></tr></table>");
        assert_eq!(doc.blocks[0].kind, BlockKind::Table { has_header: false });
    }

    #[test]
    fn a_th_in_the_first_column_does_not_make_every_row_a_header() {
        // Real tables mark the row label with `th`. Only the first row's
        // matters.
        let doc = parse_html(
            "<table><tr><td>a</td></tr><tr><th>label</th><td>v</td></tr></table>");
        assert_eq!(doc.blocks[0].kind, BlockKind::Table { has_header: false });
    }

    #[test]
    fn a_huge_table_is_capped_and_says_how_much_it_is_not_showing() {
        // The QML generator's comment records what happens without the cap:
        // one node, every row emitted, a million lines of output. Here it
        // would be a display list walked every frame.
        let mut html = String::from("<table>");
        for i in 0..(MAX_TABLE_ROWS + 37) {
            html.push_str(&format!("<tr><td>{i}</td></tr>"));
        }
        html.push_str("</table>");
        let doc = parse_html(&html);
        let t = &doc.blocks[0];
        assert_eq!(t.rows.len(), MAX_TABLE_ROWS);
        assert_eq!(t.rows_dropped, 37, "the overflow was dropped without a count");
    }

    #[test]
    fn a_block_tag_inside_a_cell_does_not_tear_the_table_apart() {
        // `<td><p>x</p></td>` is ordinary markup. Letting the `p` start a
        // new block ends the table at the first cell.
        let doc = parse_html(
            "<table><tr><td><p>one</p></td><td><div>two</div></td></tr></table>");
        assert_eq!(doc.blocks.len(), 1, "{:?}", doc.blocks.iter()
                   .map(|b| &b.kind).collect::<Vec<_>>());
        assert_eq!(cells(&doc.blocks[0]), vec![vec!["one".to_string(), "two".to_string()]]);
    }

    #[test]
    fn an_unclosed_table_still_yields_its_rows() {
        // Malformed HTML is the normal case. Losing a whole table to a
        // missing </table> is the parser being right about the input and
        // useless about the page.
        let doc = parse_html("<table><tr><td>a</td></tr>");
        assert_eq!(cells(&doc.blocks[0]), vec![vec!["a".to_string()]]);
    }

    #[test]
    fn styling_and_links_survive_inside_a_cell() {
        let doc = parse_html(
            "<table><tr><td><b>bold</b></td><td><a href=\"/x\">go</a></td></tr></table>");
        let row = &doc.blocks[0].rows[0];
        assert!(row[0][0].style.bold);
        assert_eq!(row[1][0].style.link.as_deref(), Some("/x"));
    }

    #[test]
    fn text_between_the_tags_does_not_become_a_paragraph_in_the_table() {
        // Whitespace and stray text between rows would otherwise be wedged
        // between them as prose.
        let doc = parse_html("<table>\n  <tr>\n    <td>a</td>\n  </tr>\n</table>");
        assert_eq!(doc.blocks.len(), 1, "{:?}", doc.blocks.iter()
                   .map(|b| &b.kind).collect::<Vec<_>>());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heading_and_paragraph() {
        let doc = parse_html("<h1>Title</h1><p>hello world</p>");
        assert_eq!(doc.blocks.len(), 2);
        assert_eq!(doc.blocks[0].kind, BlockKind::Heading(1));
        assert_eq!(doc.blocks[0].inlines[0].text, "Title");
        assert_eq!(doc.blocks[1].kind, BlockKind::Paragraph);
        assert_eq!(doc.blocks[1].inlines[0].text, "hello world");
    }

    #[test]
    fn inline_styles_flatten() {
        let doc = parse_html("<p>a <b>bold</b> and <i>italic</i> c</p>");
        let runs = &doc.blocks[0].inlines;
        assert!(runs.iter().any(|r| r.text == "bold" && r.style.bold && !r.style.italic));
        assert!(runs.iter().any(|r| r.text == "italic" && r.style.italic));
        // the plain runs are not styled
        assert!(runs.iter().any(|r| r.text.contains('a') && !r.style.bold));
    }

    #[test]
    fn nested_styles_compose() {
        let doc = parse_html("<p><b><i>x</i></b></p>");
        let r = &doc.blocks[0].inlines[0];
        assert!(r.style.bold && r.style.italic);
    }

    #[test]
    fn links_carry_href() {
        let doc = parse_html(r#"<p><a href="https://x.test">click</a></p>"#);
        let r = &doc.blocks[0].inlines[0];
        assert_eq!(r.text, "click");
        assert_eq!(r.style.link.as_deref(), Some("https://x.test"));
    }

    #[test]
    fn list_items_and_rule() {
        let doc = parse_html("<ul><li>one</li><li>two</li></ul><hr>");
        let kinds: Vec<_> = doc.blocks.iter().map(|b| b.kind.clone()).collect();
        assert_eq!(kinds, vec![BlockKind::ListItem, BlockKind::ListItem, BlockKind::Rule]);
    }

    #[test]
    fn entities_decode() {
        let doc = parse_html("<p>a &amp; b &lt;tag&gt; &#39;q&#39;</p>");
        assert_eq!(doc.blocks[0].inlines[0].text, "a & b <tag> 'q'");
    }

    #[test]
    fn pre_preserves_whitespace_others_collapse() {
        let collapsed = parse_html("<p>a   b\n\tc</p>");
        assert_eq!(collapsed.blocks[0].inlines[0].text, "a b c");
        let pre = parse_html("<pre>a   b</pre>");
        assert_eq!(pre.blocks[0].inlines[0].text, "a   b");
    }

    #[test]
    fn unknown_tags_degrade_to_text() {
        let doc = parse_html("<section><span>kept</span></section>");
        assert!(doc.text().contains("kept"));
    }
}

#[cfg(test)]
mod noise_parity_tests {
    use super::*;

    /// The two parsers in this crate must agree about what is not content.
    ///
    /// `parse_html` (flat blocks, used by the manual reader and the mail view)
    /// and `doc::parse_page` (a tree, used by the browser) are deliberately
    /// different algorithms over different models. They are NOT allowed to
    /// disagree about which tags are noise: the same page rendered in two views
    /// showing JavaScript source in one of them is the kind of inconsistency
    /// nobody reports because it looks like a broken page rather than a bug.
    #[test]
    fn neither_parser_renders_script_or_style_as_prose() {
        let html = "<p>before</p><script>var x = 1;</script>\
                    <style>p{color:red}</style><p>after</p>";
        let flat = parse_html(html);
        let text: String = flat.blocks.iter()
            .flat_map(|b| b.inlines.iter().map(|r| r.text.clone()))
            .collect::<Vec<_>>().join(" ");
        assert!(!text.contains("var x"), "parse_html leaked script source: {text:?}");
        assert!(!text.contains("color:red"), "parse_html leaked CSS: {text:?}");
        assert!(text.contains("before") && text.contains("after"));

        let tree = crate::doc::parse_page(html).root.text();
        assert!(!tree.contains("var x"));
        assert!(!tree.contains("color:red"));
    }
}
