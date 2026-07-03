//! v0.1.679: per-block markdown render for the WYSIWYG note editor's
//! "inactive" lines. The editor blend shows the caret's line as raw source
//! and every OTHER line RENDERED. This is the renderer for those lines: one
//! block → styled inline HTML (headings sized, bold/italic/code styled,
//! ZBracket [[links]] as anchors). Focused + self-contained (no full-doc
//! state) so it's safe to call per-line; PURE + testable.
//!
//! ZBracket resolution is delegated to zbrackets::rewrite (already tested) so
//! the rendered links match the read-only render exactly. Everything else
//! (heading level, inline emphasis/code) is here.

/// v0.1.692 (feat 2): justified-rows collage layout (Flickr/Google-Photos
/// style). Given each image's aspect ratio (w/h), a container width, and a
/// target row height, pack images into rows; when a row's natural width
/// exceeds the container, scale that row's height down so it fills the width
/// exactly. Returns rows of tile (width,height) px. PURE + testable; the QML
/// lays out real Image elements from this plan.
#[derive(Debug, Clone, PartialEq)]
pub struct CollageTile { pub w: f32, pub h: f32 }
#[derive(Debug, Clone, PartialEq)]
pub struct CollageRow { pub tiles: Vec<CollageTile>, pub height: f32 }

pub fn collage_layout(aspects: &[f32], container_w: f32, target_row_h: f32,
                      gap: f32) -> Vec<CollageRow> {
    let mut rows: Vec<CollageRow> = Vec::new();
    if aspects.is_empty() || container_w <= 0.0 || target_row_h <= 0.0 {
        return rows;
    }
    let mut cur: Vec<f32> = Vec::new();
    let mut idx = 0;
    while idx < aspects.len() {
        cur.push(aspects[idx].max(0.05));
        idx += 1;
        let gaps = gap * (cur.len() as f32 - 1.0);
        let natural_w: f32 = cur.iter().map(|a| a * target_row_h).sum::<f32>() + gaps;
        let last = idx == aspects.len();
        if natural_w >= container_w || last {
            let avail = (container_w - gaps).max(1.0);
            let aspect_sum: f32 = cur.iter().sum();
            let row_h = if last && natural_w < container_w {
                target_row_h
            } else {
                (avail / aspect_sum).max(1.0)
            };
            let tiles = cur.iter()
                .map(|a| CollageTile { w: a * row_h, h: row_h })
                .collect();
            rows.push(CollageRow { tiles, height: row_h });
            cur.clear();
        }
    }
    rows
}

/// Wire format for QML: rows separated by ';', tiles by ',', each "w x h"
/// rounded. e.g. "120x90,80x90;200x70".
pub fn collage_to_wire(rows: &[CollageRow]) -> String {
    rows.iter().map(|r| {
        r.tiles.iter()
            .map(|t| format!("{}x{}", t.w.round() as i32, t.h.round() as i32))
            .collect::<Vec<_>>().join(",")
    }).collect::<Vec<_>>().join(";")
}

/// v0.1.691 (editor feat 3): smart image sizing heuristic. Given the doc's
/// total text length, the number of images, and an optional (w,h) for the
/// image in question, decide how big to render it. Rules the user asked for:
/// - text-heavy doc (lots of prose per image) → MEDIUM.
/// - a single image that is alone + high-resolution → LARGE.
/// - many images → COLLAGE (caller lays them out in a grid).
/// PURE + testable; the QML maps the size class to actual pixel widths.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum ImageSize {
    /// small thumbnail (e.g. in a dense grid / text-heavy)
    Thumb,
    Medium,
    Large,
    /// part of a dynamic collage (many images)
    Collage,
}

pub fn image_size(total_text_chars: usize, image_count: usize,
                  dims: Option<(u32, u32)>) -> ImageSize {
    // many images in one doc → collage them
    if image_count >= 4 {
        return ImageSize::Collage;
    }
    // a lone image: size by resolution
    if image_count <= 1 {
        let hi_res = dims.map(|(w, h)| w >= 1000 || h >= 1000).unwrap_or(false);
        // "alone" also means the doc isn't dominated by text
        if hi_res && total_text_chars < 1200 {
            return ImageSize::Large;
        }
        if total_text_chars < 400 {
            return ImageSize::Large;
        }
        return ImageSize::Medium;
    }
    // 2-3 images: text density decides thumb vs medium
    // text-heavy → smaller so prose stays readable
    let chars_per_image = total_text_chars / image_count.max(1);
    if chars_per_image > 800 {
        ImageSize::Thumb
    } else {
        ImageSize::Medium
    }
}

/// Max render width (px) for a size class — the QML caps <img> width here.
pub fn image_size_px(size: ImageSize) -> u32 {
    match size {
        ImageSize::Thumb => 180,
        ImageSize::Medium => 380,
        ImageSize::Large => 760,
        ImageSize::Collage => 240,
    }
}

/// v0.1.690: emit syntax-highlight HTML for a markdown line that preserves
/// EVERY character of the raw text (markers included), wrapping tokens in
/// colored <span>s. Used by the editor's active-block highlight overlay: a
/// transparent TextEdit sits on top for editing, this colored Text sits
/// behind, perfectly aligned (same monospace font/size/wrap). Because no
/// characters are added or removed (only span tags), the two align char-for-
/// char. Colors are theme-ish hex passed by the caller.
pub fn highlight_markdown_html(text: &str, colors: &HlColors) -> String {
    let spans = crate::syntax_hl::highlight_markdown(text);
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len() + spans.len() * 24);
    let mut pos = 0usize;
    for sp in &spans {
        // plain gap before this span
        if sp.start > pos {
            push_escaped(&text[pos..sp.start.min(text.len())], &mut out);
        }
        let end = (sp.start + sp.len).min(bytes.len());
        if sp.start >= end { continue; }
        let frag = &text[sp.start..end];
        let color = color_for(sp.kind, colors);
        if let Some(c) = color {
            out.push_str("<span style=\"color:");
            out.push_str(c);
            out.push_str("\">");
            push_escaped(frag, &mut out);
            out.push_str("</span>");
        } else {
            push_escaped(frag, &mut out);
        }
        pos = end;
    }
    if pos < text.len() {
        push_escaped(&text[pos..], &mut out);
    }
    out
}

/// Colors for the highlight overlay (hex strings incl. '#').
#[derive(Debug, Clone)]
pub struct HlColors {
    pub heading: String,
    pub emphasis: String,
    pub code: String,
    pub link: String,
    pub punct: String,
}
impl Default for HlColors {
    fn default() -> Self {
        HlColors {
            heading: "#7db4ff".into(), emphasis: "#e8c06a".into(),
            code: "#9ad19a".into(), link: "#7ddc7d".into(),
            punct: "#888888".into(),
        }
    }
}

fn color_for(k: crate::syntax_hl::TokenKind, c: &HlColors) -> Option<&str> {
    use crate::syntax_hl::TokenKind::*;
    match k {
        Heading => Some(&c.heading),
        Emphasis => Some(&c.emphasis),
        CodeSpan => Some(&c.code),
        Link => Some(&c.link),
        Punct => Some(&c.punct),
        _ => None,
    }
}

fn push_escaped(s: &str, out: &mut String) {
    for ch in s.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            _ => out.push(ch),
        }
    }
}

/// v0.1.690: render a whole multi-line BLOCK (paired with note_blend::Block).
/// `kind` is the wire string from note_blend ("p"/"h"/"code"/"li"/"blank").
/// This is what the editor's live-render blend uses for each INACTIVE block:
/// - "code": fenced block → monospace, fences stripped, lines preserved.
/// - "li":   list → one rendered bullet per item line.
/// - "h":    heading (single line).
/// - "p":    paragraph → inline-styled, soft lines joined with a space.
/// - "blank": empty.
pub fn render_full_block(raw: &str, kind: &str, current_path: &str, paths: &[String]) -> String {
    render_full_block_sized(raw, kind, current_path, paths, 0)
}

/// render_full_block with a max image width (px) threaded into inline image
/// rendering (smart sizing). 0 = no cap.
pub fn render_full_block_sized(raw: &str, kind: &str, current_path: &str,
                               paths: &[String], img_w: u32) -> String {
    match kind {
        "blank" => String::new(),
        "code" => {
            // strip opening/closing fence lines, keep inner verbatim (escaped)
            let lines: Vec<&str> = raw.split('\n').collect();
            // v0.1.690: a ```mermaid fence can't run in RichText — show a
            // labeled diagram box (the read-only view renders it for real via
            // the mermaid JS path; here in the editor we mark it).
            let is_mermaid = lines.first()
                .map(|l| l.trim_start().trim_start_matches('`').trim_start_matches('~')
                    .trim().eq_ignore_ascii_case("mermaid"))
                .unwrap_or(false);
            let inner: Vec<&str> = lines.iter()
                .filter(|l| {
                    let t = l.trim_start();
                    !(t.starts_with("```") || t.starts_with("~~~"))
                })
                .copied().collect();
            let escaped = inner.join("\n")
                .replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;");
            if is_mermaid {
                format!("<div style=\"color:#7db4ff\">▦ mermaid diagram</div>\
                    <pre style=\"font-family:monospace\">{}</pre>", escaped)
            } else {
                format!("<pre style=\"font-family:monospace\">{}</pre>", escaped)
            }
        }
        "li" => {
            raw.split('\n')
                .filter(|l| !l.trim().is_empty())
                .map(|l| {
                    // strip the marker, render the item inline as a bullet
                    let t = l.trim_start();
                    let body = strip_list_marker(t);
                    format!("• {}", inline_sized(body, current_path, paths, img_w))
                })
                .collect::<Vec<_>>().join("<br>")
        }
        "h" => render_block(raw, current_path, paths),
        // paragraph (default): soft-wrap lines join with a space, inline-styled
        _ => {
            let joined = raw.split('\n').map(|l| l.trim()).collect::<Vec<_>>().join(" ");
            inline_sized(&joined, current_path, paths, img_w)
        }
    }
}

/// Strip a leading list marker ("- ", "* ", "+ ", "1. ", "2) ") from a line.
fn strip_list_marker(line: &str) -> &str {
    for m in &["- ", "* ", "+ "] {
        if let Some(rest) = line.strip_prefix(m) { return rest; }
    }
    // ordered: digits then ". " / ") "
    let digits = line.chars().take_while(|c| c.is_ascii_digit()).count();
    if digits > 0 {
        let after = &line[digits..];
        if let Some(rest) = after.strip_prefix(". ").or_else(|| after.strip_prefix(") ")) {
            return rest;
        }
    }
    line
}

/// Render one markdown block (a single logical line) to display HTML for the
/// editor's rendered view. `current_path` + `paths` feed ZBracket link
/// resolution (same as render_note_zbrackets). A heading line (`#`..) is
/// wrapped in an <h*>; otherwise inline styling is applied.
pub fn render_block(line: &str, current_path: &str, paths: &[String]) -> String {
    let trimmed = line.trim_start();
    // heading?
    if let Some((level, rest)) = heading(trimmed) {
        let inner = inline(rest, current_path, paths);
        return format!("<h{0}>{1}</h{0}>", level.min(6), inner);
    }
    // blank line → small spacer (keeps vertical rhythm in the blend)
    if trimmed.is_empty() {
        return String::new();
    }
    // bullet?
    if let Some(rest) = trimmed.strip_prefix("- ").or_else(|| trimmed.strip_prefix("* ")) {
        return format!("• {}", inline(rest, current_path, paths));
    }
    inline(line, current_path, paths)
}

/// Detect a heading: returns (level, text_after_hashes).
fn heading(s: &str) -> Option<(usize, &str)> {
    let hashes = s.chars().take_while(|&c| c == '#').count();
    if hashes == 0 || hashes > 6 { return None; }
    let rest = &s[hashes..];
    // must be followed by a space (so "#tag" isn't a heading)
    if let Some(after) = rest.strip_prefix(' ') {
        Some((hashes, after.trim_end()))
    } else {
        None
    }
}

/// Inline styling: ZBrackets first (via the shared resolver), then bold /
/// italic / inline-code on the result. Order matters: ZBracket rewrite emits
/// `<a>` tags, and we must not re-style inside those — so we style only the
/// text OUTSIDE existing tags.
pub fn inline(text: &str, current_path: &str, paths: &[String]) -> String {
    inline_sized(text, current_path, paths, 0)
}

/// inline() with a max image width (px) for smart image sizing (0 = no cap).
pub fn inline_sized(text: &str, current_path: &str, paths: &[String], img_w: u32) -> String {
    // v0.1.690/691: render image links as <img> first (Qt RichText shows
    // file:// images inline), capped at img_w for smart sizing.
    let text = render_image_links_sized(text, current_path, img_w);
    let index = crate::zbrackets::build_index(paths);
    let with_links = crate::zbrackets::rewrite(&text, current_path, &index);
    style_outside_tags(&with_links)
}

/// v0.1.690: replace markdown image links `![alt](src)` with `<img>` tags,
/// resolving a relative `src` to a file:// URL under the current note's
/// directory. Leaves http(s)/file/data srcs as-is. Self-contained (no
/// manpages dep) so note_render stays standalone-testable.
pub fn render_image_links(text: &str, current_path: &str) -> String {
    render_image_links_sized(text, current_path, 0)
}

/// Like render_image_links but caps each <img> at `max_w` px (0 = no cap).
/// The editor passes a width from image_size_px (smart sizing).
pub fn render_image_links_sized(text: &str, current_path: &str, max_w: u32) -> String {
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < text.len() {
        // look for "!["
        if bytes[i] == b'!' && i + 1 < bytes.len() && bytes[i + 1] == b'[' {
            if let Some((alt_end, src_start, src_end, next)) = parse_image_at(text, i) {
                let alt = &text[i + 2..alt_end];
                let src = &text[src_start..src_end];
                let resolved = resolve_img_src(src, current_path);
                let wattr = if max_w > 0 { format!(" width=\"{}\"", max_w) } else { String::new() };
                out.push_str(&format!("<img src=\"{}\" alt=\"{}\"{}>",
                    attr_escape(&resolved), attr_escape(alt), wattr));
                i = next;
                continue;
            }
        }
        // copy this char (utf8-safe)
        let ch_len = utf8_len(bytes[i]);
        out.push_str(&text[i..(i + ch_len).min(text.len())]);
        i += ch_len;
    }
    out
}

fn utf8_len(b: u8) -> usize {
    if b < 0x80 { 1 } else if b >> 5 == 0b110 { 2 }
    else if b >> 4 == 0b1110 { 3 } else if b >> 3 == 0b11110 { 4 } else { 1 }
}

/// Parse an image link starting at `at` (which points at '!'). Returns
/// (alt_end, src_start, src_end, next_index) if it matches `![alt](src)`.
fn parse_image_at(text: &str, at: usize) -> Option<(usize, usize, usize, usize)> {
    let b = text.as_bytes();
    let alt_start = at + 2; // past "!["
    let mut j = alt_start;
    while j < b.len() && b[j] != b']' { j += 1; }
    if j >= b.len() { return None; }
    let alt_end = j;
    // expect "](":
    if j + 1 >= b.len() || b[j + 1] != b'(' { return None; }
    let src_start = j + 2;
    let mut k = src_start;
    while k < b.len() && b[k] != b')' { k += 1; }
    if k >= b.len() { return None; }
    Some((alt_end, src_start, k, k + 1))
}

fn resolve_img_src(src: &str, current_path: &str) -> String {
    let s = src.trim();
    if s.starts_with("http://") || s.starts_with("https://")
        || s.starts_with("file://") || s.starts_with("data:") {
        return s.to_string();
    }
    if s.starts_with('/') {
        return format!("file://{}", s);
    }
    // relative → join with current note's directory
    let dir = match current_path.rsplit_once('/') {
        Some((d, _)) => d,
        None => "",
    };
    if dir.is_empty() {
        format!("file://{}", s)
    } else {
        format!("file://{}/{}", dir, s)
    }
}

fn attr_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('"', "&quot;").replace('<', "&lt;").replace('>', "&gt;")
}

/// Apply bold/italic/code styling to text, skipping any region inside an
/// existing HTML tag pair `<a ...>...</a>` (so we don't mangle link markup).
fn style_outside_tags(html: &str) -> String {
    let mut out = String::with_capacity(html.len() + 16);
    let bytes = html.as_bytes();
    let mut i = 0;
    while i < html.len() {
        if bytes[i] == b'<' {
            // copy the whole tag (and if it's an <a>, copy through </a>) verbatim
            if let Some(end) = html[i..].find('>') {
                let tag = &html[i..i + end + 1];
                out.push_str(tag);
                i += end + 1;
                // if this opened an anchor, copy through its close verbatim
                if tag.starts_with("<a ") || tag == "<a>" {
                    if let Some(close) = html[i..].find("</a>") {
                        out.push_str(&html[i..i + close + 4]);
                        i += close + 4;
                    }
                }
                continue;
            }
        }
        // not in a tag: accumulate a run of plain text up to the next '<',
        // style it, emit it.
        let next_tag = html[i..].find('<').map(|p| i + p).unwrap_or(html.len());
        let span = &html[i..next_tag];
        out.push_str(&style_emphasis(span));
        i = next_tag;
    }
    out
}

/// Bold (`**x**`/`__x__`), italic (`*x*`/`_x_`), inline code (`` `x` ``).
/// Single-pass, left-to-right, non-nesting (sufficient for editor preview).
fn style_emphasis(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let b = s.as_bytes();
    let mut i = 0;
    while i < s.len() {
        // inline code first (verbatim inside)
        if b[i] == b'`' {
            if let Some(c) = s[i + 1..].find('`') {
                out.push_str("<code>");
                out.push_str(&s[i + 1..i + 1 + c]);
                out.push_str("</code>");
                i += c + 2;
                continue;
            }
        }
        // bold: ** or __  (byte compare — never slice across a char
        // boundary; `i` walks raw bytes, so `&s[i..i+2]` panicked when
        // i+2 landed inside a multibyte char like '—').
        if i + 1 < s.len() && b[i] == b[i + 1] && (b[i] == b'*' || b[i] == b'_') {
            let marker = if b[i] == b'*' { "**" } else { "__" };
            if let Some(c) = s[i + 2..].find(marker) {
                out.push_str("<b>");
                out.push_str(&style_emphasis(&s[i + 2..i + 2 + c]));
                out.push_str("</b>");
                i += c + 4;
                continue;
            }
        }
        // italic: * or _ (single)
        if b[i] == b'*' || b[i] == b'_' {
            let marker = b[i];
            if let Some(c) = s[i + 1..].find(marker as char) {
                out.push_str("<i>");
                out.push_str(&s[i + 1..i + 1 + c]);
                out.push_str("</i>");
                i += c + 2;
                continue;
            }
        }
        // default: copy one char
        let ch = s[i..].chars().next().unwrap();
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collage_fills_row_width() {
        // 3 square images (aspect 1.0), container 600, target 200, gap 0
        let rows = collage_layout(&[1.0, 1.0, 1.0], 600.0, 200.0, 0.0);
        // natural width at 200 = 600 = container → one row, scaled to fit
        assert_eq!(rows.len(), 1);
        let w: f32 = rows[0].tiles.iter().map(|t| t.w).sum();
        assert!((w - 600.0).abs() < 1.0, "row should fill width, got {}", w);
    }

    #[test]
    fn collage_wraps_to_multiple_rows() {
        // 6 squares, container 400, target 200 → 2 per row (2*200=400)
        let rows = collage_layout(&[1.0; 6], 400.0, 200.0, 0.0);
        assert!(rows.len() >= 3, "6 squares at 2/row → ~3 rows, got {}", rows.len());
        for r in &rows {
            let w: f32 = r.tiles.iter().map(|t| t.w).sum();
            // each full row fills the container (trailing row may be shorter)
            assert!(w <= 400.5);
        }
    }

    #[test]
    fn collage_respects_aspect() {
        // a wide pano (3.0) + a square (1.0), container 800, target 200
        let rows = collage_layout(&[3.0, 1.0], 800.0, 200.0, 0.0);
        assert_eq!(rows.len(), 1);
        // pano tile is wider than the square tile
        assert!(rows[0].tiles[0].w > rows[0].tiles[1].w);
        // same row height
        assert!((rows[0].tiles[0].h - rows[0].tiles[1].h).abs() < 0.01);
    }

    #[test]
    fn collage_empty_and_wire() {
        assert!(collage_layout(&[], 600.0, 200.0, 4.0).is_empty());
        let rows = collage_layout(&[1.0, 1.0], 400.0, 200.0, 0.0);
        let w = collage_to_wire(&rows);
        assert!(w.contains('x') && w.contains(','));
    }

    #[test]
    fn image_size_many_images_collage() {
        assert_eq!(image_size(2000, 6, None), ImageSize::Collage);
        assert_eq!(image_size(100, 4, Some((2000, 2000))), ImageSize::Collage);
    }

    #[test]
    fn image_size_lone_hires_large() {
        // single high-res image, little text → large
        assert_eq!(image_size(200, 1, Some((1920, 1080))), ImageSize::Large);
        // single image, almost no text → large regardless of res
        assert_eq!(image_size(50, 1, Some((400, 300))), ImageSize::Large);
    }

    #[test]
    fn image_size_text_heavy_medium_or_smaller() {
        // single image in a long article → medium (not large)
        assert_eq!(image_size(5000, 1, Some((1920, 1080))), ImageSize::Medium);
        // a few images in a very text-heavy doc → thumbs
        assert_eq!(image_size(5000, 3, None), ImageSize::Thumb);
        // a few images, light text → medium
        assert_eq!(image_size(600, 3, None), ImageSize::Medium);
    }

    #[test]
    fn image_size_px_mapping() {
        assert!(image_size_px(ImageSize::Large) > image_size_px(ImageSize::Medium));
        assert!(image_size_px(ImageSize::Medium) > image_size_px(ImageSize::Thumb));
    }

    #[test]
    fn image_link_relative_resolves_under_note_dir() {
        let out = render_image_links("see ![cat](imgs/cat.png) here", "/notes/a/note.md");
        assert!(out.contains("<img src=\"file:///notes/a/imgs/cat.png\""), "{}", out);
        assert!(out.contains("alt=\"cat\""));
    }

    #[test]
    fn image_link_absolute_and_url_preserved() {
        let a = render_image_links("![x](/abs/p.png)", "/n/note.md");
        assert!(a.contains("file:///abs/p.png"));
        let u = render_image_links("![x](https://e.com/p.png)", "/n/note.md");
        assert!(u.contains("https://e.com/p.png") && !u.contains("file://https"));
    }

    #[test]
    fn inline_renders_image() {
        let out = inline("![logo](logo.svg)", "/n/note.md", &[]);
        assert!(out.contains("<img") && out.contains("logo.svg"));
    }

    #[test]
    fn highlight_html_preserves_all_chars() {
        let src = "# Heading **bold** `code`";
        let html = highlight_markdown_html(src, &HlColors::default());
        // stripping tags must give back the original text exactly
        let stripped = strip_tags(&html);
        assert_eq!(stripped, src, "overlay must preserve every char for alignment");
        assert!(html.contains("<span"), "some tokens colored");
    }

    #[test]
    fn highlight_html_escapes() {
        let html = highlight_markdown_html("a < b & c", &HlColors::default());
        assert!(html.contains("&lt;") && html.contains("&amp;"));
        assert_eq!(strip_tags(&html), "a < b & c");
    }

    #[test]
    fn mermaid_block_labeled() {
        let raw = "```mermaid\ngraph TD\nA-->B\n```";
        let out = render_full_block(raw, "code", "/n/cur.md", &[]);
        assert!(out.contains("mermaid diagram"));
        assert!(out.contains("graph TD") && out.contains("A--&gt;B"));
    }

    // tiny tag stripper for tests (not for production)
    fn strip_tags(html: &str) -> String {
        let mut out = String::new();
        let mut in_tag = false;
        for ch in html.chars() {
            match ch {
                '<' => in_tag = true,
                '>' => in_tag = false,
                _ if !in_tag => out.push(ch),
                _ => {}
            }
        }
        out.replace("&lt;", "<").replace("&gt;", ">").replace("&amp;", "&")
    }

    #[test]
    fn full_block_code_strips_fences() {
        let raw = "```rust\nlet x = 1;\nlet y = 2;\n```";
        let out = render_full_block(raw, "code", "/n/cur.md", &[]);
        assert!(out.contains("<pre"));
        assert!(out.contains("let x = 1;") && out.contains("let y = 2;"));
        assert!(!out.contains("```"), "fences stripped: {}", out);
    }

    #[test]
    fn full_block_code_escapes_html() {
        let raw = "```\nif a < b && c > d\n```";
        let out = render_full_block(raw, "code", "/n/cur.md", &[]);
        assert!(out.contains("&lt;") && out.contains("&gt;") && out.contains("&amp;"));
    }

    #[test]
    fn full_block_list_renders_bullets() {
        let raw = "- one\n- **two**\n3. three";
        let out = render_full_block(raw, "li", "/n/cur.md", &[]);
        assert_eq!(out.matches('•').count(), 3);
        assert!(out.contains("<b>two</b>"), "inline styling inside items: {}", out);
        assert!(out.contains("three"));
    }

    #[test]
    fn full_block_paragraph_joins_soft_lines() {
        let raw = "line one\nline two";
        let out = render_full_block(raw, "p", "/n/cur.md", &[]);
        assert_eq!(out, "line one line two");
    }

    #[test]
    fn full_block_heading() {
        assert_eq!(render_full_block("## Head", "h", "/n/cur.md", &[]), "<h2>Head</h2>");
    }

    #[test]
    fn full_block_blank_empty() {
        assert_eq!(render_full_block("", "blank", "/n/cur.md", &[]), "");
    }

    fn r(line: &str) -> String { render_block(line, "/n/cur.md", &[]) }

    #[test]
    fn headings() {
        assert_eq!(r("# Title"), "<h1>Title</h1>");
        assert_eq!(r("### Sub"), "<h3>Sub</h3>");
        // #tag (no space) is not a heading
        assert_eq!(r("#notag"), "#notag");
    }

    #[test]
    fn bold_and_italic() {
        assert_eq!(style_emphasis("a **b** c"), "a <b>b</b> c");
        assert_eq!(style_emphasis("a *b* c"), "a <i>b</i> c");
        assert_eq!(style_emphasis("a __b__ c"), "a <b>b</b> c");
    }

    #[test]
    fn inline_code() {
        assert_eq!(style_emphasis("run `cargo test` now"),
                   "run <code>cargo test</code> now");
    }

    #[test]
    fn bullet() {
        assert_eq!(r("- item one"), "• item one");
    }

    #[test]
    fn bold_inside_heading() {
        assert_eq!(r("## A **bold** head"), "<h2>A <b>bold</b> head</h2>");
    }

    #[test]
    fn zbracket_link_renders_and_is_not_re_styled() {
        // a [[link]] resolves to an <a>; emphasis styling must not touch
        // inside the anchor markup.
        let paths = vec!["/n/Plan.md".to_string()];
        let out = render_block("see [[Plan]] now", "/n/cur.md", &paths);
        assert!(out.contains("<a "), "zbracket became an anchor: {}", out);
        assert!(out.contains("Plan"));
        // styling outside still works
        let out2 = render_block("**hi** [[Plan]]", "/n/cur.md", &paths);
        assert!(out2.contains("<b>hi</b>"));
    }

    #[test]
    fn plain_text_passes_through() {
        assert_eq!(r("just some words"), "just some words");
    }

    #[test]
    fn blank_line_is_empty() {
        assert_eq!(r("   "), "");
    }

    #[test]
    fn no_panic_on_unclosed_markers() {
        // unterminated markers must not panic and just pass through
        let _ = style_emphasis("a **b");
        let _ = style_emphasis("a `b");
        let _ = style_emphasis("*");
    }

    #[test]
    fn no_panic_on_multibyte_chars() {
        // Regression: `i` walks raw bytes, so a 2-byte marker probe
        // (`&s[i..i+2]`) used to split a multibyte char (e.g. '—',
        // 3 bytes) and panic on a char-boundary error. These must all
        // pass through cleanly.
        assert_eq!(style_emphasis("word—word"), "word—word");
        assert_eq!(style_emphasis("a — b — c"), "a — b — c");
        // multibyte right where a bold probe happens, then real bold
        assert_eq!(style_emphasis("é**x**"), "é<b>x</b>");
        // emoji (4-byte) and accents shouldn't trip it either
        let _ = style_emphasis("café — déjà vu 🚀 _italic_ done");
        assert_eq!(style_emphasis("naïve __bold__"), "naïve <b>bold</b>");
    }
}
