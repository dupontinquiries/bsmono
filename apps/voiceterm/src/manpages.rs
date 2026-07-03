//! Manpage system: named markdown documents looked up by name.
//!
//! Substrate A of the multi-user onboarding arc (filed in v0.1.202
//! as ship 2.6.1). The onboarding doc, voice command reference,
//! and any other guided content lives here.
//!
//! ## Architecture
//!
//! Docs are bundled at compile time via `include_str!`. To add a
//! new manpage:
//!   1. Drop a `.md` file in `data/manpages/`
//!   2. Add an `m.insert(...)` line in `registry()` below
//!
//! The registry returns the raw markdown. The URL-scheme handler
//! in main.rs routes `web2qml://man/<name>` requests here and
//! passes the result through to the QML preview.
//!
//! ## Two rendering paths
//!
//! **Native (v0.1.215+, the live path):** `to_qml(md, sty)` emits
//! QML directly. No HTML round-trip; no scraper parse. Cached as
//! `CachedPageKind::NativeQml` so re-renders skip parsing entirely.
//! This is what users see when they navigate to a manpage.
//!
//! **HTML (`to_html`, `render`):** legacy path that converts to
//! HTML markup. No longer wired into the URL handler. Kept for
//! tests and any future "export manpage as HTML" use case.
//!
//! Webpages continue through the existing HTML pipeline
//! (scraper → qml_gen) because we don't own real-world HTML
//! and need scraper to handle the mess. Manpages are clean
//! markdown we own, so the round-trip is pure overhead — hence
//! the native path.
//!
//! ## Markdown subset
//!
//! Deliberately small for v0.1.203. Supports:
//!   - Headings (`#` through `######`)
//!   - Paragraphs (blank-line separated)
//!   - Unordered (`-`) and ordered (`1.`) lists
//!   - Inline links (`[text](url)`)
//!   - Inline code (`` `code` ``) and code blocks (```` ``` ````)
//!   - Bold (`**text**`)
//!   - Tables (v0.1.214) — pipe-delimited with a separator row
//!
//! Not yet supported (filed for follow-up):
//!   - Blockquotes
//!   - Emphasis with `*` or `_`
//!   - Images
//!   - Nested lists
//!   - Strikethrough
//!
//! For richer content, swap the parser for pulldown-cmark; the
//! `to_qml` and `to_html` interfaces are stable.

use std::collections::HashMap;
use std::sync::OnceLock;

/// Look up a manpage by name. Returns the raw markdown source or
/// None if no such page is registered.
pub fn get(name: &str) -> Option<&'static str> {
    registry().get(name).copied()
}

/// List all registered manpage names (sorted, for stable display).
#[allow(dead_code)]  // useful for index page generation later
pub fn list_names() -> Vec<&'static str> {
    let mut names: Vec<&str> = registry().keys().copied().collect();
    names.sort();
    names
}

/// Render the manpage at `name` as HTML. Returns None if no such
/// page is registered. Convenience wrapper around `get()` + `to_html()`.
///
/// v0.1.215: no longer the primary path — manpages now render via
/// `to_qml` directly (native pathway, no HTML round-trip). This
/// function and `to_html` are kept for the test suite and for any
/// future "export manpage as HTML" use case (e.g. printing,
/// sending to a static-site generator).
#[allow(dead_code)]
pub fn render(name: &str) -> Option<String> {
    get(name).map(to_html)
}

/// The compile-time registry. Built once via OnceLock, then frozen.
fn registry() -> &'static HashMap<&'static str, &'static str> {
    static REG: OnceLock<HashMap<&'static str, &'static str>> = OnceLock::new();
    REG.get_or_init(|| {
        let mut m = HashMap::new();
        // Adding a new page: drop the .md file in data/manpages/
        // and add the include_str! line here.
        m.insert("welcome", include_str!("../data/manpages/welcome.md"));
        m.insert("manpages", include_str!("../data/manpages/manpages.md"));
        // v0.1.212 (ship 2.6.5): onboarding flow content. The
        // registration dialog walks the user through reading the
        // "paragraph" section aloud while we capture their voice.
        m.insert("onboarding", include_str!("../data/manpages/onboarding.md"));
        // v0.1.568 (graph-onboarding arc): the welcome page is now the
        // graph entry node and links (via wikilinks) to these topic
        // nodes. `voice-setup` is the slug `[[Voice Setup]]` resolves
        // to; it aliases the existing onboarding content (whose H1 is
        // already "Voice Setup") so the welcome→voice link lands on the
        // real registration flow rather than a dead node.
        m.insert("voice-setup", include_str!("../data/manpages/onboarding.md"));
        m.insert("getting-around", include_str!("../data/manpages/getting-around.md"));
        m.insert("capabilities", include_str!("../data/manpages/capabilities.md"));
        m
    })
}

/// Convert a small markdown subset to HTML. See module docs for
/// the supported syntax.
///
/// The output is wrapped in `<html><body>...</body></html>` so it
/// can flow through the existing scraper-based render pipeline
/// without special-casing.
/// v0.1.674: resolve ONE image src against the .md file's directory →
/// `file://` absolute. Absolute/special srcs (http(s), file, data, /abs,
/// web2qml://, empty) pass through unchanged. Pure + testable.
pub fn resolve_one_img_src(src: &str, source_file: &str) -> String {
    let low = src.to_lowercase();
    if src.is_empty()
        || low.starts_with("http://") || low.starts_with("https://")
        || low.starts_with("file://") || low.starts_with("data:")
        || src.starts_with('/') || low.starts_with("web2qml://") {
        return src.to_string();
    }
    match source_file.rsplit_once('/') {
        Some((dir, _)) => format!("file://{}/{}", dir, src),
        None => src.to_string(),
    }
}

/// v0.1.674: resolve relative `<img src="...">` paths in rendered html
/// against the markdown file's directory, into `file://` absolute URLs.
/// Without this, a note like `![](SR Latch.assets/x.png)` renders a broken
/// image (the renderer can't find the relative path) — the camera-placeholder
/// bug. `source_file` is the .md file's filesystem path. Pure + testable.
pub fn resolve_img_srcs(html: &str, source_file: &str) -> String {
    let mut out = String::with_capacity(html.len() + 64);
    let needle = "<img src=\"";
    let mut rest = html;
    while let Some(pos) = rest.find(needle) {
        out.push_str(&rest[..pos + needle.len()]);
        rest = &rest[pos + needle.len()..];
        let end = match rest.find('"') { Some(e) => e, None => break };
        out.push_str(&resolve_one_img_src(&rest[..end], source_file));
        out.push('"');
        rest = &rest[end + 1..];
    }
    out.push_str(rest);
    out
}

/// v0.1.683: is `line` an indented-code-block line? CommonMark: a line
/// indented 4+ spaces (or a tab) at block level is code. We use this to
/// render the big 4-space-indented code blocks (common in pasted ChatGPT
/// exports) as actual code instead of mangled paragraphs — the report
/// "inline code blocks don't render / content goes off the page on the
/// left." Pure + testable. The caller only treats a run as code when it
/// begins at a block boundary (after a blank line), per CommonMark, so list
/// continuations aren't misclassified.
pub fn is_indented_code_line(line: &str) -> bool {
    if line.starts_with('\t') { return true; }
    // count leading spaces (up to 4)
    let leading = line.chars().take_while(|&c| c == ' ').count();
    leading >= 4 && line.trim().len() > 0
}

/// Strip one level of code indentation (4 spaces or one tab) from a line.
pub fn strip_code_indent(line: &str) -> &str {
    if let Some(rest) = line.strip_prefix('\t') { return rest; }
    if line.starts_with("    ") { return &line[4..]; }
    line.trim_start_matches(' ')
}

pub fn to_html(md: &str) -> String {
    let mut out = String::with_capacity(md.len() + 256);
    out.push_str("<html><body>");

    // Parse line-by-line with small lookahead for paragraphs and
    // lists. Each pass through the loop handles ONE block element.
    let lines: Vec<&str> = md.lines().collect();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        let trimmed = line.trim_start();

        // Blank line: paragraph separator, skip.
        if trimmed.is_empty() {
            i += 1;
            continue;
        }

        // Fenced code block.
        if trimmed.starts_with("```") {
            // v0.1.330 (cards arc): detect `mermaid` language tag
            // on the opening fence so the renderer can emit a
            // "view as diagram" affordance. The fence opening
            // line looks like:
            //   ```mermaid
            //   ```mermaid title=Foo
            // Anything else (including bare ```) falls through
            // to the existing code-block path.
            let fence_lang = trimmed
                .trim_start_matches('`')
                .trim()
                .split_whitespace()
                .next()
                .unwrap_or("")
                .to_lowercase();
            i += 1;
            let mut code = String::new();
            while i < lines.len() && !lines[i].trim_start().starts_with("```") {
                code.push_str(lines[i]);
                code.push('\n');
                i += 1;
            }
            // Consume closing fence if present. If we ran off the
            // end without finding one, don't bump past lines.len()
            // (outer loop check would still terminate, but defensive).
            if i < lines.len() {
                i += 1;
            }
            if fence_lang == "mermaid" {
                // Emit the source plus an "Open as diagram" link
                // using the custom `web2qml://mermaid?src=...`
                // scheme. The QML link handler decodes and
                // invokes `cardsCanvas.openMermaid(src)`.
                //
                // Source is URL-encoded inline so newlines and
                // special chars round-trip cleanly through the
                // anchor href. Base64 would be more robust but
                // would require base64 decoding QML-side; the
                // percent-encode path is dependency-free both
                // ways (the QML link handler uses
                // decodeURIComponent).
                out.push_str("<pre><code>");
                escape_html_into(&code, &mut out);
                out.push_str("</code></pre>");
                out.push_str("<a href=\"web2qml://mermaid?src=");
                percent_encode_into(&code, &mut out);
                out.push_str("\">View as diagram</a>");
            } else {
                out.push_str("<pre><code>");
                escape_html_into(&code, &mut out);
                out.push_str("</code></pre>");
            }
            continue;
        }

        // Heading: `#` through `######` followed by space.
        if let Some(level) = heading_level(trimmed) {
            // Slice off the `#`s and any following space(s).
            let text = trimmed[level..].trim_start();
            out.push_str(&format!("<h{}>", level));
            render_inline(text, &mut out);
            out.push_str(&format!("</h{}>", level));
            i += 1;
            continue;
        }

        // Unordered list: lines starting with `- `.
        if trimmed.starts_with("- ") {
            out.push_str("<ul>");
            while i < lines.len() && lines[i].trim_start().starts_with("- ") {
                let item = lines[i].trim_start()[2..].trim_start();
                let mut full_item = String::from(item);
                // Continuation lines: indented non-bullet lines
                // belong to the previous item.
                let mut j = i + 1;
                while j < lines.len() {
                    let next = lines[j];
                    let next_trim = next.trim_start();
                    if next_trim.is_empty() { break; }
                    if next_trim.starts_with("- ") || heading_level(next_trim).is_some() { break; }
                    // v0.1.222: block image ends the item.
                    if detect_block_image(next).is_some() { break; }
                    // Treat as continuation; append with a space.
                    full_item.push(' ');
                    full_item.push_str(next_trim);
                    j += 1;
                }
                i = j;
                out.push_str("<li>");
                render_inline(&full_item, &mut out);
                out.push_str("</li>");
            }
            out.push_str("</ul>");
            continue;
        }

        // Ordered list: lines starting with `<digit>. `.
        if is_ordered_list_item(trimmed) {
            out.push_str("<ol>");
            while i < lines.len() && is_ordered_list_item(lines[i].trim_start()) {
                // Strip the `<digits>. ` prefix.
                let after = strip_ordered_prefix(lines[i].trim_start());
                let mut full_item = String::from(after);
                let mut j = i + 1;
                while j < lines.len() {
                    let next = lines[j];
                    let next_trim = next.trim_start();
                    if next_trim.is_empty() { break; }
                    if is_ordered_list_item(next_trim)
                        || next_trim.starts_with("- ")
                        || heading_level(next_trim).is_some()
                    {
                        break;
                    }
                    // v0.1.222: block image ends the item.
                    if detect_block_image(next).is_some() { break; }
                    full_item.push(' ');
                    full_item.push_str(next_trim);
                    j += 1;
                }
                i = j;
                out.push_str("<li>");
                render_inline(&full_item, &mut out);
                out.push_str("</li>");
            }
            out.push_str("</ol>");
            continue;
        }

        // v0.1.214 (ship: markdown tables in manpage path).
        // Pipe-leading line followed by a separator row → table.
        // Both `| a | b |` and `a | b` rows are valid (the table_ops
        // parser handles missing edge pipes), but the SEPARATOR row
        // requires explicit recognition. We trigger on a current
        // line that contains `|` AND a next non-empty line that
        // looks like a separator (only -, :, whitespace, |).
        //
        // The block ends at the first non-table line: blank,
        // heading, list item, code fence, or anything without a
        // pipe. We gather the whole block as a single string, hand
        // it to table_ops::parse_markdown_table for validation, and
        // emit the resulting TableData as HTML <table>. If the
        // parser rejects it (malformed), fall through to the
        // paragraph fallback below — same fate as before.
        if trimmed.contains('|') && i + 1 < lines.len() {
            let next_trim = lines[i + 1].trim();
            if looks_like_table_separator(next_trim) {
                // Gather contiguous table lines.
                let mut block = String::new();
                let mut j = i;
                while j < lines.len() {
                    let l = lines[j];
                    let lt = l.trim();
                    if lt.is_empty() { break; }
                    if heading_level(lt).is_some() { break; }
                    if lt.starts_with("- ") || is_ordered_list_item(lt) { break; }
                    if lt.starts_with("```") { break; }
                    // Non-pipe line after the separator ends the
                    // block too. (The separator itself contains
                    // pipes, so this only triggers after rows.)
                    if !lt.contains('|') { break; }
                    block.push_str(l);
                    block.push('\n');
                    j += 1;
                }
                // Run through the canonical parser. If it returns
                // a table, emit HTML; otherwise fall through.
                if let Some(data) = crate::table_ops::parse_markdown_table(&block) {
                    emit_table_html(&data, &mut out);
                    i = j;
                    continue;
                }
                // Parse rejected — fall through to paragraph (the
                // pipe-containing lines will be swept into a para
                // and rendered literally). Don't bump i; the
                // paragraph block below will pick them up.
            }
        }

        // v0.1.222: block-level image. Standalone `![alt](src)`
        // line becomes a real <img> tag at block level (so HTML
        // rendering of the exported page would show the image,
        // not inline-wrap it inside a <p>).
        if let Some((alt, src)) = detect_block_image(line) {
            out.push_str("<img src=\"");
            escape_attr_into(src, &mut out);
            out.push_str("\" alt=\"");
            escape_attr_into(alt, &mut out);
            out.push_str("\">");
            i += 1;
            continue;
        }

        // Default: paragraph. Collect contiguous non-blank lines.
        let mut para = String::from(line.trim_end());
        let mut j = i + 1;
        while j < lines.len() {
            let next = lines[j];
            let next_trim = next.trim_start();
            if next_trim.is_empty() { break; }
            if heading_level(next_trim).is_some() { break; }
            if next_trim.starts_with("- ") || is_ordered_list_item(next_trim) { break; }
            if next_trim.starts_with("```") { break; }
            // v0.1.222: block image also ends a paragraph.
            if detect_block_image(next).is_some() { break; }
            para.push(' ');
            para.push_str(next.trim());
            j += 1;
        }
        i = j;
        out.push_str("<p>");
        render_inline(&para, &mut out);
        out.push_str("</p>");
    }

    out.push_str("</body></html>");
    out
}

/// v0.1.215: native QML pathway. Renders manpage markdown DIRECTLY
/// to QML, skipping the HTML round-trip + scraper parse + qml_gen
/// rewalk. The result goes straight to push_preview.
///
/// Architecture rationale (the "leave the old stuff for webpages"
/// design): webpages need scraper to handle messy real-world HTML
/// — that's irreducible. Manpages are content we own, written in
/// a constrained markdown subset, so the round-trip is pure
/// overhead. Cutting it out:
///   - Skips the parse cost (scraper builds a full DOM tree)
///   - Skips qml_gen's per-element classification logic
///   - Skips JS analysis, web-island detection, challenge
///     detection, layout-cache lookup — all webpage concerns
///   - Lets us preserve inline markup INSIDE table cells (via
///     QML rich-text), where the webpage path strips it
///   - Makes manpage list items get clickable links (the webpage
///     path flattens them via `.text().collect()`)
///
/// Element mapping:
///   - Heading → Text with bold + bigger font
///   - Paragraph (prose) → wrap_for_paragraph_selection so the
///     user can tap to select a paragraph and read it aloud.
///     Links inside paragraph text are FLATTENED — same as the
///     webpage path. Long-form prose is the selection target;
///     a link inside running text doesn't compete.
///   - List item → Text with bullet/number prefix + Text.RichText
///     + onLinkActivated → backend.nav. Lists are navigation;
///     clickable links matter.
///   - Code block → Text with monospace font, indented
///   - Table → emit_adaptive_table (same as webpage path; the
///     adaptive table is already a native QML component)
///
/// Counters: reset_paragraph_counter and reset_table_counter are
/// called once at the top so the emitted ids don't collide with
/// a future webpage navigation's counters. Same hygiene as
/// generate_safe_qml does for HTML pages.
pub fn to_qml(md: &str, sty: &crate::qml_gen::RenderStyle) -> String {
    to_qml_with_source(md, sty, None)
}

/// v0.1.342 (sidecar transforms arc): variant that accepts the
/// source file path. When provided, mermaid fence links carry
/// `&source_file=<encoded>` so the cards canvas can load + save
/// the sidecar (`<source>.transforms`) when the user pins nodes.
///
/// Pass `None` for content not backed by a real file (built-in
/// manpages, corpus listings, audit log, etc.) — those still
/// emit mermaid links but without the source_file param. The
/// canvas treats absent source_file as "use the global
/// card_pins.txt" (legacy behavior).
pub fn to_qml_with_source(
    md: &str,
    sty: &crate::qml_gen::RenderStyle,
    source_file: Option<&str>,
) -> String {
    crate::qml_gen::reset_paragraph_counter();
    crate::qml_gen::reset_table_counter();

    let mut out = String::with_capacity(md.len() * 4 + 256);
    // v0.1.342: track which mermaid block we're emitting (in
    // document order). The block_id (e.g. "mermaid_0") gets
    // appended to the URL so the sidecar can distinguish pins
    // for different diagrams in the same file.
    let mut mermaid_block_idx: usize = 0;
    let _ = &mut mermaid_block_idx;  // silence unused warning if path doesn't hit
    // v0.1.700: running h1-h3 index for anchor objectNames (mdhdg:<n>),
    // aligned with the doc_outline order so the codemap can scroll exactly.
    let mut md_heading_idx: usize = 0;
    // No outer Column: push_preview wraps the snippet in
    // `Column { id: _root ... }` which already handles vertical
    // stacking. Adding another Column here would just nest
    // pointlessly. Same convention as qml_gen::generate_safe_qml
    // — emit children directly, let the host Column lay them out.

    let lines: Vec<&str> = md.lines().collect();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        let trimmed = line.trim_start();
        if trimmed.is_empty() {
            i += 1;
            continue;
        }

        // v0.1.683: indented code block (4+ spaces / tab) at a block
        // boundary. Collect the contiguous indented run and emit as a code
        // block — this is what was rendering as mangled, off-page paragraphs
        // for big 4-space-indented exports. Block boundary = first line, or
        // the previous line was blank. (Guard: don't swallow list-item
        // continuations — a preceding bullet line means we're inside a list,
        // handled below.)
        let at_block_boundary = i == 0 || lines[i - 1].trim().is_empty();
        let prev_was_list = i > 0 && {
            let pt = lines[i - 1].trim_start();
            pt.starts_with("- ") || pt.starts_with("* ") || is_ordered_list_item(pt)
        };
        if at_block_boundary && !prev_was_list && is_indented_code_line(line) {
            let mut code = String::new();
            while i < lines.len()
                && (lines[i].trim().is_empty() || is_indented_code_line(lines[i])) {
                // stop at a blank line that is followed by a non-code line
                if lines[i].trim().is_empty() {
                    let next_is_code = i + 1 < lines.len() && is_indented_code_line(lines[i + 1]);
                    if !next_is_code { break; }
                    code.push('\n');
                    i += 1;
                    continue;
                }
                code.push_str(strip_code_indent(lines[i]));
                code.push('\n');
                i += 1;
            }
            let mut block_qml = String::with_capacity(code.len() + 128);
            emit_code_block_qml(&code, sty, &mut block_qml);
            let plain_text = code.trim_end_matches('\n').to_string();
            out.push_str(&crate::qml_gen::maybe_wrap_extended(
                block_qml, &plain_text, sty, None, crate::qml_gen::ContentType::Code,
            ));
            continue;
        }

        // Fenced code block.
        if trimmed.starts_with("```") {
            // v0.1.332: detect language tag (parallel to to_html's
            // v0.1.330 detection). When the fence is tagged
            // `mermaid`, we still emit the code block (so the
            // source stays readable) but ALSO emit a Text element
            // with a tappable link that opens the cards canvas in
            // mermaid mode. Same end behavior as the markdown→HTML
            // path: tap → `backend.nav("web2qml://mermaid?src=...")`
            // → Backend intercepts → emits mermaid_source_clicked
            // → QML opens canvas.
            let fence_lang = trimmed
                .trim_start_matches('`')
                .trim()
                .split_whitespace()
                .next()
                .unwrap_or("")
                .to_lowercase();
            i += 1;
            let mut code = String::new();
            while i < lines.len() && !lines[i].trim_start().starts_with("```") {
                code.push_str(lines[i]);
                code.push('\n');
                i += 1;
            }
            if i < lines.len() { i += 1; }
            let mut block_qml = String::with_capacity(code.len() + 128);
            emit_code_block_qml(&code, sty, &mut block_qml);
            let plain_text = code.trim_end_matches('\n').to_string();
            out.push_str(&crate::qml_gen::maybe_wrap_extended(
                block_qml,
                &plain_text,
                sty,
                None,
                crate::qml_gen::ContentType::Code,
            ));
            // v0.1.332: if this was a mermaid fence, append a
            // "View as diagram" link Text element below the code
            // block.
            //
            // v0.1.342: extended URL with `&source_file=<path>`
            // (when provided) + `&block=mermaid_<N>` so the
            // cards canvas can locate the right sidecar entries
            // for pinning. Multiple mermaid blocks in the same
            // file get distinct block IDs based on document
            // order.
            if fence_lang == "mermaid" {
                let mut url = String::with_capacity(code.len() * 2 + 128);
                url.push_str("web2qml://mermaid?src=");
                percent_encode_into(&code, &mut url);
                url.push_str("&block=mermaid_");
                url.push_str(&mermaid_block_idx.to_string());
                if let Some(sf) = source_file {
                    url.push_str("&source_file=");
                    percent_encode_into(sf, &mut url);
                }
                mermaid_block_idx += 1;
                out.push_str("Text {\nwidth: parent.width\n");
                out.push_str(&format!("color: \"{}\"\n", sty.link_color));
                out.push_str(&format!("font.pixelSize: {}\n", sty.font_size));
                out.push_str(&format!("font.family: \"{}\"\n", sty.font_family));
                out.push_str("textFormat: Text.RichText\n");
                out.push_str(&format!("linkColor: \"{}\"\n", sty.link_color));
                out.push_str("onLinkActivated: function(link) { if (backend) backend.link_nav(link) }\n");
                out.push_str("text: \"<a href=\\\"");
                for c in url.chars() {
                    match c {
                        '\\' => out.push_str("\\\\"),
                        '"' => out.push_str("\\\""),
                        _ => out.push(c),
                    }
                }
                out.push_str("\\\">View as diagram</a>\"\n");
                out.push_str("}\n");
            }
            // v0.1.343: niftyplot fence — emits a parallel
            // "View as niftyplot" link. Same URL plumbing as
            // mermaid but a different scheme so the nav
            // intercept routes through the niftyplot parser
            // rather than the mermaid one.
            if fence_lang == "niftyplot" {
                let mut url = String::with_capacity(code.len() * 2 + 128);
                url.push_str("web2qml://niftyplot?src=");
                percent_encode_into(&code, &mut url);
                url.push_str("&block=niftyplot_");
                url.push_str(&mermaid_block_idx.to_string());
                if let Some(sf) = source_file {
                    url.push_str("&source_file=");
                    percent_encode_into(sf, &mut url);
                }
                mermaid_block_idx += 1;
                out.push_str("Text {\nwidth: parent.width\n");
                out.push_str(&format!("color: \"{}\"\n", sty.link_color));
                out.push_str(&format!("font.pixelSize: {}\n", sty.font_size));
                out.push_str(&format!("font.family: \"{}\"\n", sty.font_family));
                out.push_str("textFormat: Text.RichText\n");
                out.push_str(&format!("linkColor: \"{}\"\n", sty.link_color));
                out.push_str("onLinkActivated: function(link) { if (backend) backend.link_nav(link) }\n");
                out.push_str("text: \"<a href=\\\"");
                for c in url.chars() {
                    match c {
                        '\\' => out.push_str("\\\\"),
                        '"' => out.push_str("\\\""),
                        _ => out.push(c),
                    }
                }
                out.push_str("\\\">View as niftyplot</a>\"\n");
                out.push_str("}\n");
            }
            continue;
        }

        // v0.1.337: horizontal rule. A standalone line of 3+
        // matching `-`, `_`, or `*` chars (with optional spaces)
        // becomes a thematic break. This branch handles the
        // "lone HR" case — a `---` that immediately follows a
        // paragraph is handled at the paragraph site as a
        // setext heading instead (CommonMark's disambiguation
        // rule).
        //
        // Why this matters for BLACKSTAR.md: the file uses
        // `------` between sections as separators. Without this
        // branch they rendered as standalone lines of dashes
        // (technically OK but visually noisy).
        if is_horizontal_rule(trimmed) {
            // Visual: a 1px-tall Rectangle, themed border color,
            // small vertical breathing room. Same convention
            // most CommonMark renderers use.
            out.push_str("Rectangle {\nwidth: parent.width - 16\n");
            out.push_str("x: 8\n");
            out.push_str("height: 1\n");
            out.push_str(&format!("color: \"{}\"\n", sty.text_color));
            out.push_str("opacity: 0.25\n");
            out.push_str("}\n");
            i += 1;
            continue;
        }

        // Heading.
        if let Some(level) = heading_level(trimmed) {
            let text = trimmed[level..].trim_start();
            // v0.1.240: heading selection for corpus. h1/h2/h3
            // get wrapped; h4-h6 stay unwrapped (inline labels).
            //
            // v0.1.241: routed through qml_gen::maybe_wrap_extended
            // which handles the bounds check + pid + wrap. h4-h6
            // are filtered out before — they don't even try.
            //
            // v0.1.306: tag as ContentType::Heading (not Prose)
            // so the reading overlay uses the slower heading
            // multiplier (0.80 vs 0.95 prose).
            let mut inner = String::with_capacity(text.len() + 256);
            emit_heading_qml(text, level, sty, &mut inner);
            // v0.1.700: tag h1-h3 with an anchor objectName ("mdhdg:<n>") so
            // the doc-view codemap can scroll to the heading's REAL y (exact
            // jump, not proportional). n is a running heading index, aligned
            // with current_doc_outline's order. Inject right after the opening
            // "Text {\n" of the emitted element.
            if matches!(level, 1 | 2 | 3) {
                let anchored = inner.replacen("Text {\n",
                    &format!("Text {{\nobjectName: \"mdhdg:{}\"\n", md_heading_idx), 1);
                inner = anchored;
                md_heading_idx += 1;
            }
            if matches!(level, 1 | 2 | 3) {
                let plain_text = strip_inline_markup(text);
                out.push_str(&crate::qml_gen::maybe_wrap_extended(inner, &plain_text, sty, None, crate::qml_gen::ContentType::Heading));
            } else {
                out.push_str(&inner);
            }
            i += 1;
            continue;
        }

        // v0.1.336: blockquote handler. A run of consecutive
        // `> `-prefixed lines (with bare `>` lines acting as
        // intra-blockquote paragraph separators) becomes one
        // visually-styled quotation block. Without this handler,
        // the blockquote falls through to the paragraph
        // accumulator below, which mashes everything into one
        // line with literal `> ` chars left in — BLACKSTAR.md's
        // Optimus dialogue rendered as:
        //   "> "In the vastness..." > > "Worlds are born..." > > ..."
        // which is technically readable but visually broken.
        //
        // CommonMark blockquote semantics:
        //   - Each `> ` line contributes its tail (after the
        //     `> `) to the blockquote's content.
        //   - A bare `>` line is a paragraph break WITHIN the
        //     blockquote.
        //   - A blank line (zero `>` chars) terminates the
        //     blockquote.
        //
        // Visual treatment: left-edge accent strip (4px wide,
        // theme link color) + italic text + indented body.
        if trimmed == ">" || trimmed.starts_with("> ") {
            let mut inner_lines: Vec<String> = Vec::new();
            while i < lines.len() {
                let l_trim = lines[i].trim_start();
                if l_trim == ">" {
                    inner_lines.push(String::new());
                    i += 1;
                } else if let Some(rest) = l_trim.strip_prefix("> ") {
                    inner_lines.push(rest.trim_end().to_string());
                    i += 1;
                } else if l_trim.starts_with('>') {
                    // `>foo` without a space — CommonMark allows
                    // this as a blockquote line; some markdown
                    // editors emit it. Treat the rest as the
                    // line body.
                    inner_lines.push(l_trim[1..].trim_end().to_string());
                    i += 1;
                } else {
                    break;
                }
            }
            // Group consecutive non-empty inner_lines into
            // paragraphs separated by empty entries (the bare
            // `>` markers).
            let mut paragraphs: Vec<String> = Vec::new();
            let mut current = String::new();
            for line in &inner_lines {
                if line.is_empty() {
                    if !current.is_empty() {
                        paragraphs.push(std::mem::take(&mut current));
                    }
                } else {
                    if !current.is_empty() { current.push(' '); }
                    current.push_str(line);
                }
            }
            if !current.is_empty() { paragraphs.push(current); }
            if !paragraphs.is_empty() {
                // Unique id per blockquote so `height: _bq_body_N.height + 8`
                // refs the right inner Column when a document has
                // multiple blockquotes. Reuses paragraph counter
                // for uniqueness — never collides with other
                // generated IDs.
                let bqid = crate::qml_gen::next_paragraph_id();
                let acc = &sty.link_color;
                out.push_str(&format!("Item {{\nwidth: parent.width\nheight: _bq_body_{}.height + 8\n", bqid));
                out.push_str(&format!(
                    "Rectangle {{\n\
                        width: 4\n\
                        height: parent.height\n\
                        color: \"{}\"\n\
                        radius: 2\n\
                    }}\n", acc));
                out.push_str(&format!("Column {{\nid: _bq_body_{}\nwidth: parent.width - 18\n", bqid));
                out.push_str("x: 14\n");
                out.push_str("y: 4\n");
                out.push_str("spacing: 6\n");
                for p in &paragraphs {
                    let mut rich = String::with_capacity(p.len() + 32);
                    inline_to_richtext(p, &mut rich);
                    out.push_str("Text {\nwidth: parent.width\n");
                    out.push_str(&format!("color: \"{}\"\n", sty.text_color));
                    out.push_str(&format!("font.pixelSize: {}\n", sty.font_size));
                    out.push_str(&format!("font.family: \"{}\"\n", sty.font_family));
                    out.push_str("font.italic: true\n");
                    out.push_str("wrapMode: Text.WrapAtWordBoundaryOrAnywhere\n");
                    out.push_str("textFormat: Text.RichText\n");
                    out.push_str(&format!("linkColor: \"{}\"\n", sty.link_color));
                    out.push_str("onLinkActivated: function(link) { if (backend) backend.link_nav(link) }\n");
                    out.push_str("text: \"");
                    crate::qml_gen::write_esc_keep_html(&mut out, &rich);
                    out.push_str("\"\n}\n");
                }
                out.push_str("}\n");  // close Column _bq_body
                out.push_str("}\n");  // close outer Item
            }
            continue;
        }

        // Unordered list.
        if trimmed.starts_with("- ") {
            // v0.1.240: build list QML in a temp buffer so we can
            // optionally wrap the whole list for paragraph
            // selection (corpus). Items collected for plain-text
            // backend payload too, joined with " · " separator.
            //
            // v0.1.241: wrap routed through
            // qml_gen::maybe_wrap_extended (single helper for the
            // bounds check + pid + wrap).
            let mut list_qml = String::with_capacity(256);
            list_qml.push_str("Column {\nwidth: parent.width\nspacing: 3\n");
            let mut item_plains: Vec<String> = Vec::new();
            while i < lines.len() && lines[i].trim_start().starts_with("- ") {
                let item = lines[i].trim_start()[2..].trim_start();
                let mut full_item = String::from(item);
                let mut j = i + 1;
                while j < lines.len() {
                    let next = lines[j];
                    let next_trim = next.trim_start();
                    if next_trim.is_empty() { break; }
                    if next_trim.starts_with("- ") || heading_level(next_trim).is_some() { break; }
                    // v0.1.222: block image ends the item — same
                    // semantics as headings/new bullets. An image
                    // belongs to the document, not the list item.
                    if detect_block_image(next).is_some() { break; }
                    // v0.1.336: blockquote also ends the item.
                    if next_trim == ">" || next_trim.starts_with('>') { break; }
                    full_item.push(' ');
                    full_item.push_str(next_trim);
                    j += 1;
                }
                i = j;
                item_plains.push(strip_inline_markup(&full_item));
                emit_list_item_qml("\u{2022} ", &full_item, sty, &mut list_qml);
            }
            list_qml.push_str("}\n");
            let combined = item_plains.join(" \u{00B7} ");
            out.push_str(&crate::qml_gen::maybe_wrap_extended(list_qml, &combined, sty, None, crate::qml_gen::ContentType::List));
            out.push('\n');
            continue;
        }

        // Ordered list.
        if is_ordered_list_item(trimmed) {
            // v0.1.240: same selection-wrap pattern as unordered.
            let mut list_qml = String::with_capacity(256);
            list_qml.push_str("Column {\nwidth: parent.width\nspacing: 3\n");
            let mut item_plains: Vec<String> = Vec::new();
            let mut n: u32 = 1;
            while i < lines.len() && is_ordered_list_item(lines[i].trim_start()) {
                let after = strip_ordered_prefix(lines[i].trim_start());
                let mut full_item = String::from(after);
                let mut j = i + 1;
                while j < lines.len() {
                    let next = lines[j];
                    let next_trim = next.trim_start();
                    if next_trim.is_empty() { break; }
                    if is_ordered_list_item(next_trim)
                        || next_trim.starts_with("- ")
                        || heading_level(next_trim).is_some()
                    {
                        break;
                    }
                    // v0.1.222: block image ends the item.
                    if detect_block_image(next).is_some() { break; }
                    // v0.1.336: blockquote ends the item.
                    if next_trim == ">" || next_trim.starts_with('>') { break; }
                    full_item.push(' ');
                    full_item.push_str(next_trim);
                    j += 1;
                }
                i = j;
                item_plains.push(strip_inline_markup(&full_item));
                let prefix = format!("{}. ", n);
                emit_list_item_qml(&prefix, &full_item, sty, &mut list_qml);
                n += 1;
            }
            list_qml.push_str("}\n");
            let combined = item_plains.join(" \u{00B7} ");
            out.push_str(&crate::qml_gen::maybe_wrap_extended(list_qml, &combined, sty, None, crate::qml_gen::ContentType::List));
            out.push('\n');
            continue;
        }

        // Markdown table. Detection mirrors to_html: pipe-line
        // followed by separator row, gathered until next non-
        // pipe block boundary.
        if trimmed.contains('|') && i + 1 < lines.len() {
            let next_trim = lines[i + 1].trim();
            if looks_like_table_separator(next_trim) {
                let mut block = String::new();
                let mut j = i;
                while j < lines.len() {
                    let l = lines[j];
                    let lt = l.trim();
                    if lt.is_empty() { break; }
                    if heading_level(lt).is_some() { break; }
                    if lt.starts_with("- ") || is_ordered_list_item(lt) { break; }
                    if lt.starts_with("```") { break; }
                    if !lt.contains('|') { break; }
                    block.push_str(l);
                    block.push('\n');
                    j += 1;
                }
                if let Some(data) = crate::table_ops::parse_markdown_table(&block) {
                    // v0.1.220: pre-convert each cell's markdown
                    // inline markup to Qt rich-text HTML, then
                    // ask emit_adaptive_table to render with
                    // rich_cells=true. This is what lets bold,
                    // inline code, links, and wikilinks INSIDE
                    // table cells survive into Qt — previously
                    // they were stripped by qml_gen's write_esc
                    // (which removes HTML tags).
                    let rich_data = crate::table_ops::TableData {
                        headers: data.headers.iter()
                            .map(|h| {
                                let mut r = String::with_capacity(h.len() + 16);
                                inline_to_richtext(h, &mut r);
                                r
                            })
                            .collect(),
                        rows: data.rows.iter()
                            .map(|row| row.iter()
                                .map(|cell| {
                                    let mut r = String::with_capacity(cell.len() + 16);
                                    inline_to_richtext(cell, &mut r);
                                    r
                                })
                                .collect()
                            )
                            .collect(),
                    };
                    let table_qml = crate::qml_gen::emit_adaptive_table(&rich_data, sty, true);
                    // v0.1.240: table selection for corpus.
                    // Plain-text payload built from PRE-RICH
                    // data (cells with inline markup stripped).
                    // Cells joined with spaces, rows separated
                    // with " ⁄ " (FRACTION SLASH) — same
                    // convention as the qml_gen table-selection
                    // wrap (v0.1.236).
                    //
                    // v0.1.241: routed through
                    // qml_gen::maybe_wrap_extended for the
                    // bounds check + pid + wrap.
                    let mut parts: Vec<String> = Vec::new();
                    if !data.headers.is_empty() {
                        let hp: Vec<String> = data.headers.iter()
                            .map(|h| strip_inline_markup(h)).collect();
                        parts.push(hp.join(" "));
                    }
                    for row in &data.rows {
                        let rp: Vec<String> = row.iter()
                            .map(|c| strip_inline_markup(c)).collect();
                        parts.push(rp.join(" "));
                    }
                    let combined = parts.join(" \u{2044} ");
                    out.push_str(&crate::qml_gen::maybe_wrap_extended(table_qml, &combined, sty, None, crate::qml_gen::ContentType::Table));
                    out.push('\n');
                    i = j;
                    continue;
                }
                // Parse rejected — fall through to paragraph.
            }
        }

        // v0.1.222: block-level image. A line that is purely
        // `![alt](src)` becomes a bordered card with the alt
        // text + src path, rather than being swept into a
        // paragraph and rendered as a tiny inline placeholder.
        // Image-heavy docs (course notes, slide decks rendered
        // as markdown) read much better this way.
        if let Some((alt, src)) = detect_block_image(line) {
            // v0.1.674: resolve a relative image src against the note's
            // directory so `![](X.assets/y.png)` loads (was the camera-
            // placeholder bug). Absolute/special srcs pass through.
            let resolved = match source_file {
                Some(sf) => resolve_one_img_src(src, sf),
                None => src.to_string(),
            };
            emit_block_image_qml(alt, &resolved, sty, &mut out);
            out.push('\n');
            i += 1;
            continue;
        }

        // v0.1.337: setext-style heading detection.
        // BEFORE entering paragraph accumulation, check if the
        // NEXT line is a setext underline. If so, the current
        // line is actually a heading, not a paragraph. We emit
        // it as such and skip past the underline.
        //
        // Examples this handles:
        //   "Title One\n========="  → h1 "Title One"
        //   "Subtitle\n-----------" → h2 "Subtitle"
        //
        // Pure single-line paragraphs only: setext applies to
        // ONE preceding line. (CommonMark allows lazy
        // continuation of a paragraph that becomes setext, but
        // that's rare in practice and forking the accumulator
        // for it isn't worth the complexity.)
        if i + 1 < lines.len() {
            if let Some(setext_level) = is_setext_underline(
                lines[i + 1].trim_start())
            {
                let text = line.trim();
                if !text.is_empty() {
                    let mut inner = String::with_capacity(text.len() + 256);
                    emit_heading_qml(text, setext_level, sty, &mut inner);
                    if matches!(setext_level, 1 | 2 | 3) {
                        let plain_text = strip_inline_markup(text);
                        out.push_str(&crate::qml_gen::maybe_wrap_extended(
                            inner, &plain_text, sty, None,
                            crate::qml_gen::ContentType::Heading));
                    } else {
                        out.push_str(&inner);
                    }
                    i += 2;  // past the text and the underline
                    continue;
                }
            }
        }

        // Paragraph (prose). Selection-wrapped.
        let mut para = String::from(line.trim_end());
        let mut j = i + 1;
        while j < lines.len() {
            let next = lines[j];
            let next_trim = next.trim_start();
            if next_trim.is_empty() { break; }
            if heading_level(next_trim).is_some() { break; }
            if next_trim.starts_with("- ") || is_ordered_list_item(next_trim) { break; }
            if next_trim.starts_with("```") { break; }
            // v0.1.222: block image also ends a paragraph.
            if detect_block_image(next).is_some() { break; }
            // v0.1.336: blockquote start also ends a paragraph
            // — the blockquote branch above handles its own
            // accumulation; we must not sweep `> ...` lines into
            // a preceding non-quote paragraph.
            if next_trim == ">" || next_trim.starts_with('>') { break; }
            // v0.1.337: setext underline ends the paragraph too.
            // Without this, "para line 1\npara line 2\n=====" would
            // sweep all three into a paragraph and miss the heading
            // promotion. With it, we end at line 2 — but the OUTER
            // setext-detection above only catches single-line
            // setext, so this still produces a paragraph. That's
            // a known limitation; the doc-style usage (h1/h2 above
            // body) is what we cover.
            if is_setext_underline(next_trim).is_some() { break; }
            para.push(' ');
            para.push_str(next.trim());
            j += 1;
        }
        i = j;
        emit_paragraph_qml(&para, sty, &mut out);
    }

    out
}

/// v0.1.215: emit a heading as a QML Text element.
/// Level maps to font-size: h1 = 1.6x base, h2 = 1.4x, h3 = 1.25x,
/// h4 = 1.15x, h5 = 1.05x, h6 = base. All bold.
///
/// Inline markup goes through inline_to_richtext so bold/code/
/// links inside the heading render correctly. textFormat:
/// Text.RichText handles the markup, and onLinkActivated → nav
/// makes links inside headings clickable (Qt's built-in
/// behavior when textFormat is RichText).
fn emit_heading_qml(text: &str, level: usize, sty: &crate::qml_gen::RenderStyle, out: &mut String) {
    let scale = match level {
        1 => 1.6, 2 => 1.4, 3 => 1.25, 4 => 1.15, 5 => 1.05, _ => 1.0,
    };
    let font_size = (sty.font_size as f32 * scale).round() as i32;
    let mut rich = String::with_capacity(text.len() + 16);
    inline_to_richtext(text, &mut rich);
    out.push_str("Text {\nwidth: parent.width\n");
    out.push_str(&format!("color: \"{}\"\n", sty.text_color));
    out.push_str(&format!("font.pixelSize: {}\n", font_size));
    out.push_str("font.bold: true\n");
    out.push_str("wrapMode: Text.WrapAtWordBoundaryOrAnywhere\n");
    out.push_str("textFormat: Text.RichText\n");
    out.push_str(&format!("linkColor: \"{}\"\n", sty.link_color));
    out.push_str("onLinkActivated: function(link) { if (backend) backend.link_nav(link) }\n");
    out.push_str("text: \"");
    // v0.1.220: use HTML-preserving escape so <b>/<a>/<i> tags
    // survive into the QML string. write_esc_pub strips them.
    crate::qml_gen::write_esc_keep_html(out, &rich);
    out.push_str("\"\n}\n");
}

/// v0.1.215: emit a paragraph (prose) wrapped for selection.
/// Inline markup in the paragraph is stripped to plain text —
/// matching the webpage path's behavior, where flattening is
/// done so the selection wrapper's MouseArea doesn't compete
/// with link MouseAreas inside the paragraph.
///
/// The text_content (plain) is also threaded to wrap_for_paragraph_selection
/// so it can be forwarded to backend.paragraph_selected for
/// reading aloud / TTS / future ML uses.
fn emit_paragraph_qml(para: &str, sty: &crate::qml_gen::RenderStyle, out: &mut String) {
    let plain = strip_inline_markup(para);
    // Inner text emission mirrors qml_gen's sty.text_pre/text_suf
    // shape so the visual matches webpage paragraphs.
    let mut inner = String::with_capacity(plain.len() + 64);
    inner.push_str(&sty.text_pre);
    crate::qml_gen::write_esc_pub(&mut inner, &plain);
    inner.push_str(&sty.text_suf);

    // v0.1.299: respect the same word-count bounds that
    // qml_gen.rs uses for webpage <p>. Without this gate,
    // a 1-word manpage paragraph became selectable while
    // an equivalent webpage paragraph wouldn't (the
    // paragraph_word_bounds filter). The inconsistency
    // also broke phoneme-db coverage: the generator
    // filters by WC_MIN=3 and didn't include any of the
    // tiny manpage paragraphs that the runtime made
    // selectable. Use paragraph_word_bounds (10-50) for
    // consistency with webpage prose.
    //
    // When out-of-bounds: emit a plain Text (no selection
    // wrapper). Reader still sees the text, just can't
    // tap-select it for read-aloud.
    let wc = crate::qml_gen::approx_word_count_pub(&plain);
    let (wc_min, wc_max) = crate::qml_gen::paragraph_word_bounds();
    if wc < wc_min || wc > wc_max {
        // Out of bounds: emit a plain text block without
        // selection wrapper. Matches webpage behavior.
        out.push_str("Text {\nwidth: parent.width\n");
        out.push_str(&format!("color: \"{}\"\n", sty.text_color));
        out.push_str(&format!("font.pixelSize: {}\n", sty.font_size));
        out.push_str(&format!("font.family: \"{}\"\n", sty.font_family));
        out.push_str("wrapMode: Text.WrapAtWordBoundaryOrAnywhere\n");
        out.push_str("text: \"");
        crate::qml_gen::write_esc_pub(out, &plain);
        out.push_str("\"\n}\n");
        return;
    }

    let pid = crate::qml_gen::next_paragraph_id();
    out.push_str(&crate::qml_gen::wrap_for_paragraph_selection(
        &inner, pid, &plain, sty, crate::qml_gen::ContentType::Prose,
    ));
    out.push('\n');
}

/// v0.1.215: emit a list item as a Text with bullet/number
/// prefix. Uses Text.RichText so inline links work.
fn emit_list_item_qml(prefix: &str, body: &str, sty: &crate::qml_gen::RenderStyle, out: &mut String) {
    let mut rich = String::with_capacity(body.len() + prefix.len() + 16);
    // Prefix is plain (just "• " or "1. "); body gets inline-rich.
    push_plain_into_richtext(prefix, &mut rich);
    inline_to_richtext(body, &mut rich);
    out.push_str("Text {\nwidth: parent.width - 16\nx: 16\n");
    out.push_str(&format!("color: \"{}\"\n", sty.text_color));
    out.push_str(&format!("font.pixelSize: {}\n", sty.font_size));
    out.push_str(&format!("font.family: \"{}\"\n", sty.font_family));
    out.push_str("wrapMode: Text.WrapAtWordBoundaryOrAnywhere\n");
    out.push_str("textFormat: Text.RichText\n");
    out.push_str(&format!("linkColor: \"{}\"\n", sty.link_color));
    out.push_str("onLinkActivated: function(link) { if (backend) backend.link_nav(link) }\n");
    out.push_str("text: \"");
    // v0.1.220: use HTML-preserving escape (write_esc_pub strips
    // tags, which we don't want here).
    crate::qml_gen::write_esc_keep_html(out, &rich);
    out.push_str("\"\n}\n");
}

/// v0.1.215: emit a fenced code block. Monospace, indented, with
/// a subtle background.
///
/// v0.1.223: derive a UNIQUE id for the inner Text from
/// `next_paragraph_id()`. The previous fixed `id: _code` caused
/// a QML duplicate-id parse error whenever a manpage had two or
/// more code blocks. Latent since v0.1.215 (bundled manpages
/// had at most one code block); fixed before any manpage with
/// multiple code fences triggered it. Same pattern as
/// `emit_block_image_qml` from v0.1.222.
///
/// v0.1.244: code blocks ARE now selectable (PA1 finish ship).
/// The caller wraps the emitted block with
/// `qml_gen::maybe_wrap_extended` + `ContentType::Code` so the
/// reading-overlay's code multiplier (0.70) applies. The earlier
/// "code blocks are NOT selectable" comment was from v0.1.215
/// before paragraph selection existed for non-prose elements.
fn emit_code_block_qml(code: &str, sty: &crate::qml_gen::RenderStyle, out: &mut String) {
    let trimmed = code.trim_end_matches('\n');
    let cid = crate::qml_gen::next_paragraph_id();
    let inner_id = format!("_code_{}", cid);
    out.push_str("Rectangle {\nwidth: parent.width\n");
    out.push_str(&format!("color: \"{}\"\n", sty.input_bg));
    out.push_str(&format!("border.color: \"{}\"\n", sty.border_color));
    out.push_str("border.width: 1\nradius: 4\n");
    // Height auto-sizes to text content + padding. Outer references
    // the unique inner id so two code blocks on a page resolve
    // independently.
    out.push_str(&format!("height: {}.implicitHeight + 12\n", inner_id));
    out.push_str(&format!("Text {{\nid: {}\n", inner_id));
    out.push_str("anchors.left: parent.left\nanchors.right: parent.right\nanchors.top: parent.top\n");
    out.push_str("anchors.margins: 6\n");
    out.push_str(&format!("color: \"{}\"\n", sty.text_color));
    out.push_str(&format!("font.pixelSize: {}\n", sty.font_size));
    out.push_str("font.family: \"monospace\"\n");
    out.push_str("wrapMode: Text.WrapAtWordBoundaryOrAnywhere\n");
    out.push_str("text: \"");
    crate::qml_gen::write_esc_pub(out, trimmed);
    out.push_str("\"\n}\n}\n");
}

/// v0.1.222: emit a block-level image. Two modes depending on
/// whether the `src` looks loadable:
///
/// **Loadable** (absolute URL: `http://`, `https://`, `file://`,
/// `data:`): emit a real QML `Image` element with the camera
/// emoji + alt text underneath. Image fetches asynchronously
/// and renders inline.
///
/// **Not loadable** (relative path, no scheme, empty): fall
/// back to a placeholder card showing 📷 + alt + src in
/// monospace. Same as the pre-v0.1.233 behavior. We can't
/// resolve relative paths in manpages because the manpage
/// content is bundled via `include_str!` — no on-disk
/// directory to anchor against.
///
/// v0.1.233 (was: "our new image custom thing is not drawing
/// images"): the v0.1.222 design was placeholder-only because
/// of the relative-path concern, but for absolute URLs the
/// resolution problem doesn't apply. The card was misleading
/// for URLs that WOULD have rendered. Now: try rendering when
/// possible; fall back gracefully when not.
///
/// Anchoring strategy: outer Rectangle's height references a
/// NAMED inner Column via a unique id from
/// `next_paragraph_id()` — same id-collision discipline as
/// `emit_code_block_qml`.
fn emit_block_image_qml(alt: &str, src: &str, sty: &crate::qml_gen::RenderStyle, out: &mut String) {
    let nid = crate::qml_gen::next_paragraph_id();
    let col_id = format!("_imgCol_{}", nid);

    // Detect loadable schemes. We deliberately accept only the
    // ones we trust QML's Image element to handle:
    //   - http/https: network fetch (rate-limited by ureq normally
    //     but for QML Image it's Qt's own network stack — fine)
    //   - file: local file (user-uploaded markdown referencing a
    //     local image, common for course notes / lab reports)
    //   - data: inline base64 (rare in markdown but cheap to
    //     support)
    //
    // Anything else (relative paths, exotic schemes like `mailto:`,
    // empty string) falls through to the placeholder.
    let src_trim = src.trim();
    let loadable = src_trim.starts_with("http://")
        || src_trim.starts_with("https://")
        || src_trim.starts_with("file://")
        || src_trim.starts_with("data:image/");

    out.push_str("Rectangle {\nwidth: parent.width\n");
    out.push_str(&format!("color: \"{}\"\n", sty.input_bg));
    out.push_str(&format!("border.color: \"{}\"\n", sty.border_color));
    out.push_str("border.width: 1\nradius: 4\n");
    out.push_str(&format!("height: {}.height + 16\n", col_id));
    out.push_str(&format!("Column {{\nid: {}\n", col_id));
    out.push_str("anchors.left: parent.left\nanchors.right: parent.right\nanchors.top: parent.top\nanchors.margins: 8\nspacing: 4\n");

    if loadable {
        // Real Image. Constrained to parent.width, preserve
        // aspect ratio, async fetch + cache.
        out.push_str("Image {\nwidth: parent.width\n");
        out.push_str("fillMode: Image.PreserveAspectFit\n");
        out.push_str("asynchronous: true\ncache: true\n");
        // Bind source. write_esc_pub for QML string safety.
        out.push_str("source: \"");
        crate::qml_gen::write_esc_pub(out, src_trim);
        out.push_str("\"\n}\n");
        // Caption beneath: 📷 + alt text in muted style. Only if
        // alt is non-empty.
        if !alt.trim().is_empty() {
            out.push_str("Text {\nwidth: parent.width\n");
            out.push_str("text: \"📷 ");
            crate::qml_gen::write_esc_pub(out, alt);
            out.push_str("\"\n");
            out.push_str(&format!("color: \"{}\"\n", sty.text_color));
            out.push_str("opacity: 0.7\n");
            // 2px smaller than body for caption distinction.
            let cap_size = (sty.font_size - 2).max(9);
            out.push_str(&format!("font.pixelSize: {}\n", cap_size));
            out.push_str("font.italic: true\n");
            out.push_str("wrapMode: Text.WrapAtWordBoundaryOrAnywhere\n");
            out.push_str("}\n");
        }
    } else {
        // Placeholder card path (pre-v0.1.233 behavior).
        // Camera icon + alt text on top.
        out.push_str("Text {\nwidth: parent.width\n");
        out.push_str("text: \"📷 ");
        crate::qml_gen::write_esc_pub(out, alt);
        out.push_str("\"\n");
        out.push_str(&format!("color: \"{}\"\n", sty.text_color));
        out.push_str(&format!("font.pixelSize: {}\n", sty.font_size));
        out.push_str("wrapMode: Text.WrapAtWordBoundaryOrAnywhere\n");
        out.push_str("}\n");
        // Src path in muted monospace, smaller. Skipped if empty.
        if !src.trim().is_empty() {
            out.push_str("Text {\nwidth: parent.width\ntext: \"");
            crate::qml_gen::write_esc_pub(out, src);
            out.push_str("\"\n");
            out.push_str(&format!("color: \"{}\"\n", sty.text_color));
            out.push_str("opacity: 0.6\n");
            out.push_str("font.family: \"monospace\"\n");
            let small = (sty.font_size - 2).max(9);
            out.push_str(&format!("font.pixelSize: {}\n", small));
            out.push_str("wrapMode: Text.WrapAnywhere\n");
            out.push_str("}\n");
        }
    }
    out.push_str("}\n"); // end Column
    out.push_str("}\n"); // end Rectangle
}

/// v0.1.215: convert inline markdown (bold, code, links) to QML
/// rich-text suitable for a Text element with
/// `textFormat: Text.RichText`.
///
/// Rich text grammar Qt accepts:
///   - `<b>...</b>` for bold
///   - `<i>...</i>` for italic
///   - `<code>...</code>` for monospace (we use `<span style="font-family:monospace">`
///     because vanilla Qt rich-text doesn't honor `<code>` strongly)
///   - `<a href="...">...</a>` for links
///
/// The output goes into a QML string literal (the Text's `text:`
/// value), so QML-special characters (`\"` etc.) are escaped by
/// write_esc_pub at the emission site. Here we only emit valid
/// rich-text HTML.
fn inline_to_richtext(s: &str, out: &mut String) {
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];

        // v0.1.220: backslash escape `\X`.
        if b == b'\\' {
            if let Some((escaped, after)) = detect_escape(bytes, i) {
                push_byte_into_richtext(escaped, out);
                i = after;
                continue;
            }
        }

        // v0.1.220: image `![alt](src)`. Qt's rich-text supports
        // `<img src="...">` but we can't trust arbitrary paths to
        // resolve (a manpage might reference `L09.assets/foo.png`
        // which doesn't exist in the bundled registry). Emit a
        // placeholder text span instead — "📷 alt" — so the user
        // sees where the image was supposed to go without a
        // broken-image icon.
        //
        // A future ship could resolve local file:// paths or
        // image:// URLs; for now the placeholder is honest about
        // the limitation.
        if b == b'!' {
            if let Some((alt, _src, after)) = detect_image(s, i) {
                // 📷 (U+1F4F7, CAMERA) as a literal UTF-8 char.
                // write_esc_keep_html passes it through; Qt's
                // rich-text parser renders it inline.
                out.push_str("📷 ");
                push_plain_into_richtext(alt, out);
                i = after;
                continue;
            }
        }

        if b == b'`' {
            // Inline code: walk to the closing backtick.
            let start = i + 1;
            let mut end = start;
            while end < bytes.len() && bytes[end] != b'`' { end += 1; }
            if end < bytes.len() {
                let code = &s[start..end];
                out.push_str("<span style=\"font-family:monospace\">");
                push_plain_into_richtext(code, out);
                out.push_str("</span>");
                i = end + 1;
                continue;
            }
            // Unclosed backtick: emit literal.
        }
        // Math passthrough (v0.1.891): `$…$` inline and `$$…$$` display math
        // rendered as a distinguishable monospace span — a cheap pre-render
        // until real typesetting (KaTeX) lands, so the user can at least SEE
        // that something is math. The `$` delimiters are kept INSIDE the span
        // so math reads as math rather than as inline code. Strict delimiter
        // rules (see detect_math) avoid mangling currency like "$5 to $10".
        if b == b'$' {
            if let Some((span, after)) = detect_math(s, bytes, i) {
                out.push_str("<span style=\"font-family:monospace\">");
                push_plain_into_richtext(span, out); // span includes the $…$
                out.push_str("</span>");
                i = after;
                continue;
            }
            // Not a math span (e.g. currency) — fall through to literal.
        }
        // v0.1.402: strikethrough `~~text~~` (GFM). Mirrors
        // the bold detector — `~` delimiter, count=2. Qt's
        // RichText renders `<s>` as struck-through text.
        // Placed before the emphasis branches because `~` is
        // a distinct delimiter that never collides with
        // `*`/`_`; ordering relative to them doesn't matter,
        // but keeping it adjacent to the other run-delimited
        // detectors is tidy. Single `~` is NOT strikethrough
        // (GFM requires the double form) — a lone `~` falls
        // through to literal output.
        if b == b'~' && i + 1 < bytes.len() && bytes[i + 1] == b'~' {
            if let Some((body_start, body_end, after)) = detect_emphasis(bytes, i, b'~', 2) {
                let body = &s[body_start..body_end];
                out.push_str("<s>");
                inline_to_richtext(body, out);
                out.push_str("</s>");
                i = after;
                continue;
            }
        }
        // v0.1.286: Bold+italic: ***...*** or ___...___.
        // Try count=3 FIRST so we don't get a half-match from
        // the bold (count=2) detector. detect_emphasis's
        // "next byte must not be same delim" check correctly
        // rejects ambiguous 4-open cases.
        //
        // CommonMark says ***foo*** renders as bold+italic
        // (both <strong> and <em>). We emit <b><i>foo</i></b>
        // which is what Qt's RichText renderer pairs with.
        if (b == b'*' || b == b'_')
            && i + 2 < bytes.len()
            && bytes[i + 1] == b
            && bytes[i + 2] == b
        {
            if let Some((body_start, body_end, after)) = detect_emphasis(bytes, i, b, 3) {
                let body = &s[body_start..body_end];
                out.push_str("<b><i>");
                inline_to_richtext(body, out);
                out.push_str("</i></b>");
                i = after;
                continue;
            }
        }
        // Bold: **...** or __...__ (v0.1.221 adds underscore).
        if (b == b'*' || b == b'_') && i + 1 < bytes.len() && bytes[i + 1] == b {
            if let Some((body_start, body_end, after)) = detect_emphasis(bytes, i, b, 2) {
                let body = &s[body_start..body_end];
                out.push_str("<b>");
                inline_to_richtext(body, out);
                out.push_str("</b>");
                i = after;
                continue;
            }
        }
        // Italic: *...* or _..._ (v0.1.221 adds underscore).
        if b == b'*' || b == b'_' {
            if let Some((body_start, body_end, after)) = detect_emphasis(bytes, i, b, 1) {
                out.push_str("<i>");
                inline_to_richtext(&s[body_start..body_end], out);
                out.push_str("</i>");
                i = after;
                continue;
            }
        }
        // v0.1.220: wikilink `[[Target]]` / `[[Target|Display]]`.
        if b == b'[' && i + 1 < bytes.len() && bytes[i + 1] == b'[' {
            if let Some((slug, display, after)) = detect_wikilink(s, i) {
                out.push_str("<a href=\"web2qml://man/");
                push_plain_into_richtext(&slug, out);
                out.push_str("\">");
                inline_to_richtext(display, out);
                out.push_str("</a>");
                i = after;
                continue;
            }
        }
        if b == b'[' {
            // Link: [text](url)
            let text_start = i + 1;
            let mut text_end = text_start;
            while text_end < bytes.len() && bytes[text_end] != b']' { text_end += 1; }
            if text_end < bytes.len() && text_end + 1 < bytes.len() && bytes[text_end + 1] == b'(' {
                let url_start = text_end + 2;
                let mut url_end = url_start;
                while url_end < bytes.len() && bytes[url_end] != b')' { url_end += 1; }
                if url_end < bytes.len() {
                    let text = &s[text_start..text_end];
                    let url = &s[url_start..url_end];
                    out.push_str("<a href=\"");
                    push_plain_into_richtext(url, out);
                    out.push_str("\">");
                    inline_to_richtext(text, out);
                    out.push_str("</a>");
                    i = url_end + 1;
                    continue;
                }
            }
        }
        // Plain character: push with HTML-escape.
        //
        // v0.1.331: detect multi-byte UTF-8 lead bytes and
        // consume the full codepoint as one char. The earlier
        // version did `push_byte_into_richtext(b, out)` for
        // every byte, casting each continuation byte to a
        // Latin-1 char — that produced mojibake for every
        // non-ASCII codepoint (emoji, curly quotes, em-dashes,
        // ellipsis). Qt's RichText parser then choked on the
        // garbled output. See v0.1.331 changelog for the full
        // bug trace.
        if b < 0x80 {
            push_byte_into_richtext(b, out);
            i += 1;
        } else {
            // Multi-byte: figure out the length from the lead
            // byte's bit pattern (RFC 3629).
            let len = if b < 0xC0 {
                // Stray continuation byte (shouldn't happen on
                // valid UTF-8 input, but be defensive — skip).
                1
            } else if b < 0xE0 { 2 }
            else if b < 0xF0 { 3 }
            else { 4 };
            let end = (i + len).min(bytes.len());
            // Convert this slice back to &str safely. If the
            // slice happens to be invalid (truncated source),
            // skip the lead byte and continue — same as the
            // defensive case above.
            if let Ok(piece) = std::str::from_utf8(&bytes[i..end]) {
                for c in piece.chars() {
                    push_char_into_richtext(c, out);
                }
                i = end;
            } else {
                i += 1;
            }
        }
    }
}

/// Detect a math span starting at `bytes[i] == b'$'`. Returns the span text
/// INCLUDING the `$`/`$$` delimiters (so it renders as visibly-math mono) plus
/// the index just past the closing delimiter. Display math `$$…$$` is matched
/// first (the doubled delimiter is unambiguous, never currency). Inline `$…$`
/// uses strict open/close rules so currency ("$5 to $10", "save $3") isn't
/// treated as math:
///   - the opening `$` must be followed by a non-space, non-digit char
///     (rejects "$ x$" and the "$5" currency lead-in),
///   - the closing `$` must be preceded by a non-space char and not be
///     followed by a digit (rejects the "$10" half of "$5 to $10"),
///   - the span may not cross a newline.
/// Slices land on the ASCII `$` positions, so multibyte content is safe.
fn detect_math<'a>(s: &'a str, bytes: &[u8], i: usize) -> Option<(&'a str, usize)> {
    // Display math: $$ … $$ (matched first; unambiguous).
    if bytes.get(i + 1) == Some(&b'$') {
        let content_start = i + 2;
        let mut j = content_start;
        while j + 1 < bytes.len() {
            if bytes[j] == b'$' && bytes[j + 1] == b'$' {
                if j > content_start {
                    return Some((&s[i..j + 2], j + 2));
                }
                return None; // empty $$$$
            }
            if bytes[j] == b'\n' {
                return None;
            }
            j += 1;
        }
        return None;
    }
    // Inline math: $ … $ (strict, currency-safe).
    let open_next = *bytes.get(i + 1)?;
    if open_next == b' ' || open_next == b'\t' || open_next.is_ascii_digit() {
        return None;
    }
    let mut j = i + 1;
    while j < bytes.len() {
        match bytes[j] {
            b'\n' => return None, // single-line only
            b'$' => {
                let prev = bytes[j - 1];
                let next_is_digit = bytes.get(j + 1).map_or(false, |c| c.is_ascii_digit());
                if prev != b' ' && prev != b'\t' && !next_is_digit {
                    return Some((&s[i..j + 1], j + 1));
                }
                return None; // a `$` that fails the close rules → not math
            }
            _ => j += 1,
        }
    }
    None
}

/// v0.1.215: push a plain-text string into a rich-text buffer with
/// proper HTML escaping (so `<`, `>`, `&`, `"` don't break the
/// resulting markup).
fn push_plain_into_richtext(s: &str, out: &mut String) {
    // v0.1.331: iterate CHARS not BYTES. The previous byte-wise
    // iteration corrupted multi-byte UTF-8 sequences — every
    // non-ASCII codepoint (emoji, curly quotes, em-dashes,
    // ellipsis, accented chars) became 2-4 garbage chars in
    // the U+0080..U+00FF range because `b as char` reinterprets
    // each UTF-8 continuation byte as a Latin-1 codepoint.
    //
    // Result: Qt's RichText parser saw mojibake AND raw C1
    // control chars (0x80..0x9F) embedded in heading/list/
    // table content. Heading-heavy documents with emoji
    // (TRANSFORMERS BLACKSTAR.md, etc.) rendered blank or
    // truncated because Qt's renderer aborted on the bad
    // input. ASCII-only documents were unaffected.
    //
    // Fix: walk by `char` and escape only the HTML metachars,
    // pushing everything else as-is. UTF-8 round-trips intact.
    for c in s.chars() {
        push_char_into_richtext(c, out);
    }
}

fn push_byte_into_richtext(b: u8, out: &mut String) {
    // v0.1.331: kept for the inline_to_richtext escape-handler
    // path, which works at byte level (detect_escape returns
    // the escaped byte). Caller MUST guarantee `b` is ASCII
    // (the escape table only produces ASCII bytes). For any
    // non-ASCII path, use push_char_into_richtext instead.
    debug_assert!(b < 0x80, "push_byte_into_richtext given non-ASCII byte 0x{:02x}", b);
    match b {
        b'<' => out.push_str("&lt;"),
        b'>' => out.push_str("&gt;"),
        b'&' => out.push_str("&amp;"),
        b'"' => out.push_str("&quot;"),
        _ => out.push(b as char),
    }
}

/// v0.1.331: char-level RichText escape. Pushes one Unicode
/// codepoint into `out`, escaping HTML metacharacters and
/// passing everything else through. Use this for any user
/// content that may contain non-ASCII text.
fn push_char_into_richtext(c: char, out: &mut String) {
    match c {
        '<' => out.push_str("&lt;"),
        '>' => out.push_str("&gt;"),
        '&' => out.push_str("&amp;"),
        '"' => out.push_str("&quot;"),
        // Pass everything else through. Qt RichText is UTF-8
        // aware; emoji, curly quotes, em-dashes, etc. all
        // render correctly when delivered as proper UTF-8.
        _ => out.push(c),
    }
}

/// v0.1.215: strip inline markdown from a string, returning plain
/// text. Used for paragraph bodies that go through the selection
/// wrapper (which can't show rich text; the inner is a plain
/// Text element).
fn strip_inline_markup(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        // v0.1.220: backslash escape — emit literal next char.
        if b == b'\\' {
            if let Some((escaped, after)) = detect_escape(bytes, i) {
                out.push(escaped as char);
                i = after;
                continue;
            }
        }
        // v0.1.220: image — emit "[image: alt]" so TTS reading
        // has a verbal marker for where the image was.
        if b == b'!' {
            if let Some((alt, _src, after)) = detect_image(s, i) {
                out.push_str("[image");
                if !alt.is_empty() {
                    out.push_str(": ");
                    out.push_str(&strip_inline_markup(alt));
                }
                out.push(']');
                i = after;
                continue;
            }
        }
        if b == b'`' {
            // Skip the backticks; include the content.
            let start = i + 1;
            let mut end = start;
            while end < bytes.len() && bytes[end] != b'`' { end += 1; }
            if end < bytes.len() {
                out.push_str(&s[start..end]);
                i = end + 1;
                continue;
            }
        }
        // Math passthrough (v0.1.891): strip the `$`/`$$` delimiters and keep
        // the inner expression as plain text for read-aloud / titles ("$x^2$"
        // → "x^2"). NOT recursed — math content (e.g. `a_b`) must not be
        // re-parsed as markdown emphasis.
        if b == b'$' {
            if let Some((span, after)) = detect_math(s, bytes, i) {
                out.push_str(span.trim_matches('$'));
                i = after;
                continue;
            }
        }
        // v0.1.402: strikethrough `~~text~~` — strip the
        // delimiters, keep the body (struck-through text is
        // still read aloud / shown in titles as plain text).
        if b == b'~' && i + 1 < bytes.len() && bytes[i + 1] == b'~' {
            if let Some((body_start, body_end, after)) = detect_emphasis(bytes, i, b'~', 2) {
                out.push_str(&strip_inline_markup(&s[body_start..body_end]));
                i = after;
                continue;
            }
        }
        // v0.1.286: ***bold+italic*** — strip all 3 delim runs
        // and the body's nested markup recursively.
        if (b == b'*' || b == b'_')
            && i + 2 < bytes.len()
            && bytes[i + 1] == b
            && bytes[i + 2] == b
        {
            if let Some((body_start, body_end, after)) = detect_emphasis(bytes, i, b, 3) {
                out.push_str(&strip_inline_markup(&s[body_start..body_end]));
                i = after;
                continue;
            }
        }
        // Bold: **...** or __...__ (v0.1.221 adds underscore).
        if (b == b'*' || b == b'_') && i + 1 < bytes.len() && bytes[i + 1] == b {
            if let Some((body_start, body_end, after)) = detect_emphasis(bytes, i, b, 2) {
                out.push_str(&strip_inline_markup(&s[body_start..body_end]));
                i = after;
                continue;
            }
        }
        // Italic: *...* or _..._ (v0.1.221 adds underscore).
        if b == b'*' || b == b'_' {
            if let Some((body_start, body_end, after)) = detect_emphasis(bytes, i, b, 1) {
                out.push_str(&strip_inline_markup(&s[body_start..body_end]));
                i = after;
                continue;
            }
        }
        // v0.1.220: wikilink — emit the display text.
        if b == b'[' && i + 1 < bytes.len() && bytes[i + 1] == b'[' {
            if let Some((_slug, display, after)) = detect_wikilink(s, i) {
                out.push_str(&strip_inline_markup(display));
                i = after;
                continue;
            }
        }
        if b == b'[' {
            let text_start = i + 1;
            let mut text_end = text_start;
            while text_end < bytes.len() && bytes[text_end] != b']' { text_end += 1; }
            if text_end < bytes.len() && text_end + 1 < bytes.len() && bytes[text_end + 1] == b'(' {
                let mut url_end = text_end + 2;
                while url_end < bytes.len() && bytes[url_end] != b')' { url_end += 1; }
                if url_end < bytes.len() {
                    // Keep the link TEXT (drop the URL).
                    out.push_str(&s[text_start..text_end]);
                    i = url_end + 1;
                    continue;
                }
            }
        }
        // v0.1.331: walk multi-byte UTF-8 as a unit. Same bug
        // as inline_to_richtext — cast `b as char` corrupts
        // non-ASCII codepoints. Fix: detect lead byte length
        // and emit the proper char.
        if b < 0x80 {
            out.push(b as char);
            i += 1;
        } else {
            let len = if b < 0xC0 { 1 }
                else if b < 0xE0 { 2 }
                else if b < 0xF0 { 3 }
                else { 4 };
            let end = (i + len).min(bytes.len());
            if let Ok(piece) = std::str::from_utf8(&bytes[i..end]) {
                out.push_str(piece);
                i = end;
            } else {
                i += 1;
            }
        }
    }
    out
}

// v0.1.220: helpers for inline-parser additions (italic, wikilink,
// backslash escape, image). All three parser flavors (render_inline
// for HTML, inline_to_richtext for QML rich-text, strip_inline_markup
// for plain text) use these so the detection rules stay in one place.

/// v0.1.221: generalized emphasis detector. `delim` is the
/// delimiter byte (`b'*'` or `b'_'`) and `count` is the run
/// length (1 = italic, 2 = bold).
///
/// Returns `Some((body_start, body_end, after))` where
/// `body_start..body_end` is the emphasized content (excluding
/// the delimiter runs) and `after` is the byte position just
/// past the closing run.
///
/// Common rules (apply to both delimiters):
///   - At least `count` delimiter bytes at `start`
///   - For italic (`count=1`): reject when next byte is the same
///     delimiter (would be bold's open)
///   - For bold (`count=2`): reject when next byte is also the
///     same delimiter (would be `***` triple — currently not
///     handled cleanly)
///   - Body first char must not be whitespace (list-marker /
///     spurious match guard)
///   - Body must not cross a newline
///   - Closing run must not be preceded by whitespace
///   - Match the EXACT count: not 1 from a 2-run, not 2 from a
///     3-run
///
/// Underscore-only rules (intraword guard, since `foo_bar_baz`
/// in identifiers shouldn't become emphasized "bar"):
///   - Char BEFORE `start` (if any) must NOT be ascii-alnum
///   - Char AFTER the closing run (if any) must NOT be ascii-alnum
///
/// Star has no intraword guard — `a*b*c` does match (rare in
/// practice).
fn detect_emphasis(bytes: &[u8], start: usize, delim: u8, count: usize) -> Option<(usize, usize, usize)> {
    // Confirm `count` delimiter bytes at `start`.
    if start + count > bytes.len() { return None; }
    for k in 0..count {
        if bytes[start + k] != delim { return None; }
    }
    // For italic (count=1): the next byte after the open MUST NOT
    // be the same delim (that would be bold's open).
    // For bold (count=2): the next byte after `**` must NOT be
    // another `*` (that's triple, ambiguous).
    let body_start = start + count;
    if body_start < bytes.len() && bytes[body_start] == delim {
        return None;
    }
    if body_start >= bytes.len() { return None; }
    // Reject body starting with whitespace.
    if bytes[body_start] == b' ' || bytes[body_start] == b'\t' { return None; }
    // Underscore intraword guard: opening must not be inside a word.
    if delim == b'_' && start > 0 {
        let prev = bytes[start - 1];
        if prev.is_ascii_alphanumeric() { return None; }
    }
    // Walk to find the closing run.
    let mut end = body_start;
    while end < bytes.len() {
        if bytes[end] == delim {
            // Count consecutive delims here.
            let mut run = 1;
            while end + run < bytes.len() && bytes[end + run] == delim {
                run += 1;
            }
            // Must match exactly `count`. If shorter, skip past
            // this run and keep searching. If longer, this also
            // isn't our close — skip it (a longer run might be
            // someone else's open).
            if run != count {
                end += run;
                continue;
            }
            // Reject closing run preceded by whitespace.
            if end > body_start
                && (bytes[end - 1] == b' ' || bytes[end - 1] == b'\t')
            {
                end += run;
                continue;
            }
            // Underscore intraword guard: char after close must
            // not be ascii-alnum.
            if delim == b'_' {
                let after_idx = end + count;
                if after_idx < bytes.len() && bytes[after_idx].is_ascii_alphanumeric() {
                    end += run;
                    continue;
                }
            }
            return Some((body_start, end, end + count));
        }
        // Newline ends emphasis candidacy.
        if bytes[end] == b'\n' { return None; }
        end += 1;
    }
    None
}

/// v0.1.220 detect_italic — kept as a thin wrapper for the call
/// sites that already use it. v0.1.221 added `_` support via
/// `detect_emphasis`; call sites now invoke detect_emphasis
/// directly with the delimiter they care about.
///
/// Kept #[allow(dead_code)] in case anything outside this module
/// imports it.
#[allow(dead_code)]
fn detect_italic(bytes: &[u8], start: usize) -> Option<(usize, usize, usize)> {
    detect_emphasis(bytes, start, b'*', 1)
}

/// Detect a `[[wikilink]]` or `[[Target|Display]]` at `start` (the
/// first `[`). Returns `Some((target, display, after))` where
/// `target` is the slug (lowercased, normalized) and `display` is
/// the rendered text (may equal target).
fn detect_wikilink(s: &str, start: usize) -> Option<(String, &str, usize)> {
    let bytes = s.as_bytes();
    if start + 1 >= bytes.len() { return None; }
    if bytes[start] != b'[' || bytes[start + 1] != b'[' { return None; }
    // Find closing `]]`.
    let body_start = start + 2;
    let mut i = body_start;
    while i + 1 < bytes.len() {
        if bytes[i] == b']' && bytes[i + 1] == b']' {
            let body = &s[body_start..i];
            // Split on `|` for the alias form.
            let (target_raw, display) = match body.find('|') {
                Some(pipe) => (&body[..pipe], &body[pipe + 1..]),
                None => (body, body),
            };
            let target_slug = slugify(target_raw);
            return Some((target_slug, display, i + 2));
        }
        // Newline ends wikilink candidacy.
        if bytes[i] == b'\n' { return None; }
        i += 1;
    }
    None
}

/// Convert a wikilink target to a URL-safe manpage slug.
/// Lowercase, ASCII spaces → `-`, drop chars outside [a-z0-9_-].
/// E.g. `ECSE-2660` → `ecse-2660`, `My Notes` → `my-notes`.
fn slugify(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        let lc = c.to_ascii_lowercase();
        if lc.is_ascii_alphanumeric() || lc == '-' || lc == '_' {
            out.push(lc);
        } else if c.is_whitespace() {
            out.push('-');
        }
        // Drop anything else.
    }
    out
}

/// Detect a backslash escape at `start` (the `\` byte). Returns
/// `Some((escaped_char, after))` if the next byte is an ASCII
/// punctuation char.
///
/// Pattern: `\<punct>` → emit the punct literally, skip the
/// backslash. Lets users write `\#` for a literal `#` (otherwise
/// would start a heading at line start), `\*` for literal star,
/// `\[`, `\\` (literal backslash), etc.
///
/// We only escape ASCII punctuation — backslash before a regular
/// letter is left alone (consistent with CommonMark: `\a` → `\a`).
fn detect_escape(bytes: &[u8], start: usize) -> Option<(u8, usize)> {
    if bytes[start] != b'\\' { return None; }
    if start + 1 >= bytes.len() { return None; }
    let next = bytes[start + 1];
    // ASCII punctuation set per CommonMark: !"#$%&'()*+,-./:;<=>?@[\]^_`{|}~
    let is_punct = (b'!'..=b'/').contains(&next)
        || (b':'..=b'@').contains(&next)
        || (b'['..=b'`').contains(&next)
        || (b'{'..=b'~').contains(&next);
    if is_punct { Some((next, start + 2)) } else { None }
}

/// Detect a markdown image at `start` (the `!` byte). Returns
/// `Some((alt, src, after))` if the syntax `![alt](src)` matches.
///
/// Note: an image is just a link prefixed by `!`. We reuse the
/// existing link-detection pattern (close-bracket, open-paren,
/// close-paren) shifted by 1.
fn detect_image(s: &str, start: usize) -> Option<(&str, &str, usize)> {
    let bytes = s.as_bytes();
    if bytes[start] != b'!' { return None; }
    if start + 1 >= bytes.len() || bytes[start + 1] != b'[' { return None; }
    // From `[` onwards, same shape as a link.
    let alt_start = start + 2;
    let mut alt_end = alt_start;
    while alt_end < bytes.len() && bytes[alt_end] != b']' {
        if bytes[alt_end] == b'\n' { return None; }
        alt_end += 1;
    }
    if alt_end >= bytes.len() || alt_end + 1 >= bytes.len() {
        return None;
    }
    if bytes[alt_end + 1] != b'(' { return None; }
    let src_start = alt_end + 2;
    let mut src_end = src_start;
    while src_end < bytes.len() && bytes[src_end] != b')' {
        if bytes[src_end] == b'\n' { return None; }
        src_end += 1;
    }
    if src_end >= bytes.len() { return None; }
    Some((&s[alt_start..alt_end], &s[src_start..src_end], src_end + 1))
}

/// v0.1.222: detect a line that is ENTIRELY an image — `![alt](src)`
/// with optional surrounding whitespace. Returns `Some((alt, src))`
/// when the trimmed line matches `![...](...)` with nothing else.
///
/// Block-level images render as a bordered card (in to_qml) rather
/// than an inline placeholder squished into a paragraph. Markdown
/// documents commonly put images on lines by themselves — see the
/// L09.md sample with `![image-foo](L09.assets/image-foo.png)` on
/// many standalone lines.
///
/// We only trigger on a line that's PURELY an image. A line with
/// text plus an image (e.g. "see ![diagram](x.png) above") still
/// flows through the paragraph parser, which emits the image as
/// an inline placeholder via inline_to_richtext.
fn detect_block_image(line: &str) -> Option<(&str, &str)> {
    let trimmed = line.trim();
    if trimmed.is_empty() { return None; }
    let result = detect_image(trimmed, 0)?;
    let (alt, src, after) = result;
    // Must consume the entire trimmed string.
    if after != trimmed.len() { return None; }
    Some((alt, src))
}

/// v0.1.215: generate a "page not found" stub in native QML.
/// Used by the URL handler when the requested manpage isn't in
/// the registry. Includes a link back to the index.
pub fn not_found_qml(name: &str, sty: &crate::qml_gen::RenderStyle) -> String {
    let mut out = String::with_capacity(512);
    // No outer Column — _root in push_preview handles stacking.
    // Heading
    out.push_str(&format!(
        "Text {{\nwidth: parent.width\ntext: \"Page not found\"\ncolor: \"{}\"\nfont.pixelSize: {}\nfont.bold: true\n}}\n",
        sty.text_color, (sty.font_size as f32 * 1.6).round() as i32
    ));
    // Body paragraph (not selection-wrapped — short error stub).
    out.push_str(&format!(
        "Text {{\nwidth: parent.width\ntext: \"No manpage named \\\"",
    ));
    crate::qml_gen::write_esc_pub(&mut out, name);
    out.push_str("\\\" is registered.\"\ncolor: \"");
    out.push_str(&sty.text_color);
    out.push_str(&format!(
        "\"\nfont.pixelSize: {}\nwrapMode: Text.WrapAtWordBoundaryOrAnywhere\n}}\n",
        sty.font_size
    ));
    // Index link
    out.push_str(&format!(
        "Text {{\nwidth: parent.width\ntext: \"See <a href=\\\"web2qml://man/manpages\\\">the index</a> for a list of available pages.\"\ncolor: \"{}\"\nlinkColor: \"{}\"\nfont.pixelSize: {}\ntextFormat: Text.RichText\nonLinkActivated: function(link) {{ if (backend) backend.link_nav(link) }}\nwrapMode: Text.WrapAtWordBoundaryOrAnywhere\n}}\n",
        sty.text_color, sty.link_color, sty.font_size
    ));
    out
}

/// Count leading `#`s if this is a heading (1-6 followed by space).
/// Returns None if not a heading line.
fn heading_level(s: &str) -> Option<usize> {
    let mut count = 0;
    for c in s.chars() {
        if c == '#' { count += 1; } else { break; }
    }
    if count == 0 || count > 6 { return None; }
    // Must be followed by space.
    let after = &s[count..];
    if after.starts_with(' ') {
        Some(count)
    } else {
        None
    }
}

/// v0.1.337: horizontal-rule detection.
///
/// CommonMark spec: a line consisting of 3+ matching `-`, `_`, or
/// `*` characters with optional spaces between them and optional
/// trailing whitespace is a thematic break.
///
/// Examples that match:
///   `---`        `----`        `- - -`        `***`
///   `___`        `-----------` (BLACKSTAR.md uses `------`)
///
/// Examples that don't:
///   `--`         (only 2 chars)
///   `-+-`        (mixed)
///   `-- foo`     (trailing non-marker text)
///
/// The function ignores leading whitespace. After the
/// 0-3-space indent rule (CommonMark says 4+ spaces makes it a
/// code block instead), we accept any leading whitespace because
/// our parser doesn't track indent for code-block detection.
fn is_horizontal_rule(s: &str) -> bool {
    let t = s.trim();
    if t.len() < 3 { return false; }
    // Determine candidate marker from first non-space char.
    let marker = t.chars().next().unwrap();
    if marker != '-' && marker != '_' && marker != '*' { return false; }
    let mut count = 0;
    for c in t.chars() {
        if c == marker { count += 1; }
        else if c.is_whitespace() { continue; }
        else { return false; }
    }
    count >= 3
}

/// v0.1.337: setext-style heading underline detection.
///
/// CommonMark setext headings: a line of text immediately
/// followed (no blank line) by a row of `=` (h1) or `-` (h2).
/// Examples:
///
///   Title One
///   =========
///
///   Subtitle
///   --------
///
/// This function only tests the UNDERLINE line. The paragraph
/// accumulator peeks at the next line and, if this returns Some,
/// emits the current text as a heading instead of a paragraph.
///
/// Returns:
///   Some(1) — line is all `=`s (with optional whitespace, len ≥ 1)
///   Some(2) — line is all `-`s (with optional whitespace, len ≥ 1)
///   None    — anything else
///
/// Note: ambiguity with HR. A line of `---` ALONE is an HR; a
/// line of `---` immediately after a text line is a setext h2.
/// The paragraph fallback resolves the ambiguity by checking
/// FIRST whether the following line is a setext underline and
/// only emitting a paragraph otherwise — so the standalone-HR
/// branch sees the dashes only when no text precedes.
fn is_setext_underline(s: &str) -> Option<usize> {
    let t = s.trim();
    if t.is_empty() { return None; }
    let marker = t.chars().next().unwrap();
    if marker != '=' && marker != '-' { return None; }
    for c in t.chars() {
        if c != marker && !c.is_whitespace() { return None; }
    }
    // CommonMark requires the underline match the entire text
    // line's length, but our rendering doesn't care; any non-
    // empty matching-char run promotes to a heading.
    Some(if marker == '=' { 1 } else { 2 })
}

/// True if the line starts like `N. ` where N is digits.
fn is_ordered_list_item(s: &str) -> bool {
    let digit_end = s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len());
    if digit_end == 0 { return false; }
    let rest = &s[digit_end..];
    rest.starts_with(". ")
}

/// Strip the `<digits>. ` prefix from an ordered list line.
fn strip_ordered_prefix(s: &str) -> &str {
    let digit_end = s.find(|c: char| !c.is_ascii_digit()).unwrap_or(0);
    s[digit_end..].strip_prefix(". ").unwrap_or(s).trim_start()
}

/// v0.1.214: check whether a line looks like a markdown table
/// separator row — cells of only dashes, colons, and whitespace,
/// at least one pipe present.
///
/// We deliberately mirror table_ops::is_separator_row's logic at
/// the LINE level (where they do per-cell) because we're checking
/// the raw line BEFORE the table_ops parser runs. The trigger is
/// cheap and false positives just mean we send a few extra lines
/// through the parser, which returns None on rejection.
///
/// Accepts: `|---|---|`, `|:---|---:|`, `| --- | :--- |`,
///          `---|---`, `:---:|:---:`
/// Rejects: `text | text` (has alphanumerics), `--no-pipe--`
///
/// v0.1.452: also accept en-dash `–` (U+2013) and em-dash `—`
/// (U+2014) as dash characters. Editors / word processors with
/// "smart dash" autocorrect silently turn `---` or `--` into
/// these, producing a separator that LOOKS valid but is rejected
/// by strict ASCII-hyphen matching — a likely cause of the
/// reported "valid but not formatted tables don't render". We
/// normalize them to hyphens so the table still renders. This
/// can't cause false positives: a line of only dashes/colons/
/// pipes/spaces is unambiguously a separator, not prose.
fn looks_like_table_separator(line: &str) -> bool {
    if !line.contains('|') { return false; }
    let has_dash = line.chars().any(|c| matches!(c, '-' | '\u{2013}' | '\u{2014}'));
    if !has_dash { return false; }
    line.chars().all(|c| matches!(c,
        '-' | '\u{2013}' | '\u{2014}' | ':' | '|' | ' ' | '\t'))
}

/// v0.1.214: emit a TableData as HTML `<table>` markup. The
/// generated HTML matches what the existing renderer expects
/// (qml_gen's `extract_table_data` + `emit_adaptive_table`).
///
/// Cell content is rendered through `render_inline` so bold,
/// inline code, and links inside cells survive. This matches
/// what scraper-parsed HTML tables get when an attribute or
/// children contain markup. Cells are HTML-escaped inside
/// render_inline's text branch.
fn emit_table_html(data: &crate::table_ops::TableData, out: &mut String) {
    out.push_str("<table>");
    if !data.headers.is_empty() {
        out.push_str("<thead><tr>");
        for h in &data.headers {
            out.push_str("<th>");
            render_inline(h, out);
            out.push_str("</th>");
        }
        out.push_str("</tr></thead>");
    }
    if !data.rows.is_empty() {
        out.push_str("<tbody>");
        for row in &data.rows {
            out.push_str("<tr>");
            for cell in row {
                out.push_str("<td>");
                render_inline(cell, out);
                out.push_str("</td>");
            }
            out.push_str("</tr>");
        }
        out.push_str("</tbody>");
    }
    out.push_str("</table>");
}

/// Render an inline span: bold, inline code, links, text.
/// Handles escapes in text portions.
fn render_inline(s: &str, out: &mut String) {
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];

        // v0.1.220: backslash escape. `\X` where X is ASCII punct
        // emits X literally. Lets users write `\#` for a literal
        // hash (otherwise would start a heading at line start),
        // `\*` for star, etc. Must come BEFORE other handlers so
        // an escape can suppress them.
        if b == b'\\' {
            if let Some((escaped, after)) = detect_escape(bytes, i) {
                push_escaped_char(escaped as char, out);
                i = after;
                continue;
            }
        }

        // v0.1.220: image `![alt](src)`. Emit as <img> so a future
        // HTML export round-trips correctly. The QML path renders
        // a placeholder; HTML rendering is for tests + export.
        if b == b'!' {
            if let Some((alt, src, after)) = detect_image(s, i) {
                out.push_str("<img src=\"");
                escape_attr_into(src, out);
                out.push_str("\" alt=\"");
                escape_attr_into(alt, out);
                out.push_str("\">");
                i = after;
                continue;
            }
        }

        // Inline code: `...`
        if b == b'`' {
            if let Some(end) = find_byte_after(bytes, i + 1, b'`') {
                out.push_str("<code>");
                escape_html_into(&s[i + 1..end], out);
                out.push_str("</code>");
                i = end + 1;
                continue;
            }
        }

        // Math passthrough (v0.1.891): mirror the QML richtext path so HTML
        // output (tests + export) shows math as distinguishable mono too.
        // `$…$` / `$$…$$` → <code> with the delimiters kept; same currency-safe
        // rules (detect_math).
        if b == b'$' {
            if let Some((span, after)) = detect_math(s, bytes, i) {
                out.push_str("<code>");
                escape_html_into(span, out);
                out.push_str("</code>");
                i = after;
                continue;
            }
        }

        // v0.1.402: strikethrough `~~text~~` (GFM) → <del>.
        // Mirrors the QML path's <s> emission; HTML uses the
        // semantic <del> tag.
        if b == b'~' && i + 1 < bytes.len() && bytes[i + 1] == b'~' {
            if let Some((body_start, body_end, after)) = detect_emphasis(bytes, i, b'~', 2) {
                out.push_str("<del>");
                render_inline(&s[body_start..body_end], out);
                out.push_str("</del>");
                i = after;
                continue;
            }
        }

        // v0.1.286: Bold+italic: ***...*** or ___...___ → <strong><em>.
        if (b == b'*' || b == b'_')
            && i + 2 < bytes.len()
            && bytes[i + 1] == b
            && bytes[i + 2] == b
        {
            if let Some((body_start, body_end, after)) = detect_emphasis(bytes, i, b, 3) {
                out.push_str("<strong><em>");
                render_inline(&s[body_start..body_end], out);
                out.push_str("</em></strong>");
                i = after;
                continue;
            }
        }

        // Bold: **...** or __...__ (v0.1.221 adds underscore).
        if (b == b'*' || b == b'_') && i + 1 < bytes.len() && bytes[i + 1] == b {
            if let Some((body_start, body_end, after)) = detect_emphasis(bytes, i, b, 2) {
                out.push_str("<strong>");
                render_inline(&s[body_start..body_end], out);
                out.push_str("</strong>");
                i = after;
                continue;
            }
        }

        // Italic: *...* or _..._ (v0.1.221 adds underscore).
        // Must check AFTER bold so `**`/`__` doesn't get caught.
        if b == b'*' || b == b'_' {
            if let Some((body_start, body_end, after)) = detect_emphasis(bytes, i, b, 1) {
                out.push_str("<em>");
                render_inline(&s[body_start..body_end], out);
                out.push_str("</em>");
                i = after;
                continue;
            }
        }

        // v0.1.220: wikilink `[[Target]]` / `[[Target|Display]]`.
        // Must check BEFORE the regular `[text](url)` link parser
        // because both start with `[`.
        if b == b'[' && i + 1 < bytes.len() && bytes[i + 1] == b'[' {
            if let Some((slug, display, after)) = detect_wikilink(s, i) {
                out.push_str("<a href=\"web2qml://man/");
                escape_attr_into(&slug, out);
                out.push_str("\">");
                render_inline(display, out);
                out.push_str("</a>");
                i = after;
                continue;
            }
        }

        // Link: [text](url)
        if b == b'[' {
            if let Some((text, url, after)) = parse_link(s, i) {
                out.push_str("<a href=\"");
                escape_attr_into(url, out);
                out.push_str("\">");
                render_inline(text, out);
                out.push_str("</a>");
                i = after;
                continue;
            }
        }

        // Plain text: emit one char with escaping.
        let ch = s[i..].chars().next().unwrap();
        push_escaped_char(ch, out);
        i += ch.len_utf8();
    }
}

/// Find the next occurrence of `target` in `bytes` starting at `from`.
fn find_byte_after(bytes: &[u8], from: usize, target: u8) -> Option<usize> {
    bytes[from..].iter().position(|&b| b == target).map(|p| from + p)
}

/// Find `**` after position `from`.
///
/// v0.1.221: superseded by detect_emphasis. Kept #[allow(dead_code)]
/// in case anything imports it.
#[allow(dead_code)]
fn find_double_star(bytes: &[u8], from: usize) -> Option<usize> {
    let mut i = from;
    while i + 1 < bytes.len() {
        if bytes[i] == b'*' && bytes[i + 1] == b'*' { return Some(i); }
        i += 1;
    }
    None
}

/// Parse a markdown link starting at `[` position `i`. Returns
/// (text, url, position_after) on success.
fn parse_link(s: &str, i: usize) -> Option<(&str, &str, usize)> {
    let bytes = s.as_bytes();
    // We're at '['. Find ']'.
    let close_bracket = find_byte_after(bytes, i + 1, b']')?;
    // Must be immediately followed by '('.
    if close_bracket + 1 >= bytes.len() || bytes[close_bracket + 1] != b'(' {
        return None;
    }
    let close_paren = find_byte_after(bytes, close_bracket + 2, b')')?;
    let text = &s[i + 1..close_bracket];
    let url = &s[close_bracket + 2..close_paren];
    Some((text, url, close_paren + 1))
}

/// HTML-escape one character into `out`.
fn push_escaped_char(c: char, out: &mut String) {
    match c {
        '<' => out.push_str("&lt;"),
        '>' => out.push_str("&gt;"),
        '&' => out.push_str("&amp;"),
        _ => out.push(c),
    }
}

/// HTML-escape a whole string into `out`.
fn escape_html_into(s: &str, out: &mut String) {
    for c in s.chars() {
        push_escaped_char(c, out);
    }
}

/// Escape attribute value (URL): `&` and `"` need quoting.
fn escape_attr_into(s: &str, out: &mut String) {
    for c in s.chars() {
        match c {
            '"' => out.push_str("&quot;"),
            '&' => out.push_str("&amp;"),
            _ => out.push(c),
        }
    }
}

/// v0.1.330 (cards arc): percent-encode bytes for embedding in
/// a URL query. RFC 3986 unreserved set: `A-Z a-z 0-9 - _ . ~`.
/// Everything else encodes as `%XX`. Multi-byte UTF-8 sequences
/// encode as multiple `%XX` triplets (one per byte) — same as
/// `encodeURIComponent` in JavaScript so the QML side can
/// decode cleanly.
fn percent_encode_into(s: &str, out: &mut String) {
    const HEX: &[u8] = b"0123456789ABCDEF";
    for byte in s.bytes() {
        let unreserved = byte.is_ascii_alphanumeric()
            || byte == b'-' || byte == b'_'
            || byte == b'.' || byte == b'~';
        if unreserved {
            out.push(byte as char);
        } else {
            out.push('%');
            out.push(HEX[(byte >> 4) as usize] as char);
            out.push(HEX[(byte & 0x0f) as usize] as char);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn indented_code_detection_v683() {
        assert!(is_indented_code_line("    let x = 5;"));
        assert!(is_indented_code_line("\tlet x = 5;"));
        assert!(!is_indented_code_line("  only two spaces"));
        assert!(!is_indented_code_line("no indent"));
        assert!(!is_indented_code_line("    ")); // blank-ish → not code
        assert_eq!(strip_code_indent("    let x = 5;"), "let x = 5;");
        assert_eq!(strip_code_indent("\tlet x = 5;"), "let x = 5;");
    }

    #[test]
    fn img_src_resolution_v674() {
        let sf = "/home/k/Documents/notes/rpi/SR Latch.md";
        // relative → file:// against the note's dir (the camera-placeholder fix)
        assert_eq!(resolve_one_img_src("SR Latch.assets/x.png", sf),
                   "file:///home/k/Documents/notes/rpi/SR Latch.assets/x.png");
        // absolute / special srcs pass through untouched
        assert_eq!(resolve_one_img_src("https://e.com/a.png", sf), "https://e.com/a.png");
        assert_eq!(resolve_one_img_src("file:///abs/a.png", sf), "file:///abs/a.png");
        assert_eq!(resolve_one_img_src("/abs/a.png", sf), "/abs/a.png");
        assert_eq!(resolve_one_img_src("data:image/png;base64,AAA", sf), "data:image/png;base64,AAA");
        // html-wide rewrite hits only relative srcs
        let html = "<img src=\"a/b.png\"> text <img src=\"https://x/c.png\">";
        let out = resolve_img_srcs(html, sf);
        assert!(out.contains("file:///home/k/Documents/notes/rpi/a/b.png"));
        assert!(out.contains("https://x/c.png"));
    }

    #[test]
    fn registry_has_welcome() {
        assert!(get("welcome").is_some());
    }

    #[test]
    fn registry_has_index() {
        assert!(get("manpages").is_some());
    }

    #[test]
    fn unknown_page_returns_none() {
        assert!(get("does-not-exist").is_none());
    }

    #[test]
    fn list_is_sorted() {
        let names = list_names();
        let mut sorted = names.clone();
        sorted.sort();
        assert_eq!(names, sorted);
    }

    #[test]
    fn paragraph_renders() {
        let html = to_html("hello world");
        assert!(html.contains("<p>hello world</p>"));
    }

    #[test]
    fn paragraph_continues_across_lines() {
        let html = to_html("hello\nworld");
        assert!(html.contains("<p>hello world</p>"));
    }

    #[test]
    fn blank_line_separates_paragraphs() {
        let html = to_html("one\n\ntwo");
        assert!(html.contains("<p>one</p>"));
        assert!(html.contains("<p>two</p>"));
    }

    #[test]
    fn h1_renders() {
        let html = to_html("# Title");
        assert!(html.contains("<h1>Title</h1>"));
    }

    #[test]
    fn h3_renders() {
        let html = to_html("### Sub");
        assert!(html.contains("<h3>Sub</h3>"));
    }

    #[test]
    fn ul_renders() {
        let html = to_html("- one\n- two");
        assert!(html.contains("<ul><li>one</li><li>two</li></ul>"));
    }

    #[test]
    fn ol_renders() {
        let html = to_html("1. one\n2. two");
        assert!(html.contains("<ol><li>one</li><li>two</li></ol>"));
    }

    #[test]
    fn link_renders() {
        let html = to_html("see [docs](web2qml://man/welcome) here");
        assert!(html.contains("<a href=\"web2qml://man/welcome\">docs</a>"));
    }

    #[test]
    fn inline_code_renders() {
        let html = to_html("the `foo` function");
        assert!(html.contains("<code>foo</code>"));
    }

    #[test]
    fn bold_renders() {
        let html = to_html("**important** note");
        assert!(html.contains("<strong>important</strong>"));
    }

    /// v0.1.286: triple-asterisk emits bold+italic (CommonMark).
    #[test]
    fn triple_asterisk_renders_bold_italic() {
        let html = to_html("this is ***very important*** here");
        // The exact tag nesting could be <strong><em>X</em></strong>
        // or <em><strong>X</strong></em>; both are valid. We
        // just assert both strong + em appear around the text.
        assert!(html.contains("<strong>") && html.contains("<em>"),
            "expected <strong> + <em> in: {}", html);
        assert!(html.contains("very important"),
            "expected the content in: {}", html);
    }

    /// v0.1.286: triple-underscore emits bold+italic (CommonMark).
    #[test]
    fn triple_underscore_renders_bold_italic() {
        let html = to_html("note ___very important___ here");
        assert!(html.contains("<strong>") && html.contains("<em>"),
            "expected <strong> + <em> in: {}", html);
        assert!(html.contains("very important"),
            "expected the content in: {}", html);
    }

    /// Triple-asterisk must not be misparsed as bold-only when
    /// the body itself has length 1. This regression test
    /// guards against the v0.1.286 detect_emphasis(count=3)
    /// being called BEFORE count=2 — the order matters.
    #[test]
    fn triple_asterisk_preferred_over_bold() {
        let html = to_html("a ***x*** b");
        // Must NOT just be <strong>*x*</strong> (which would
        // be bold containing literal asterisks).
        assert!(html.contains("<em>"),
            "expected <em> for triple-asterisk in: {}", html);
    }

    #[test]
    fn code_block_renders() {
        let html = to_html("```\nlet x = 1;\n```");
        assert!(html.contains("<pre><code>"));
        assert!(html.contains("let x = 1;"));
    }

    #[test]
    fn html_special_chars_escaped() {
        let html = to_html("a < b & c > d");
        assert!(html.contains("&lt;"));
        assert!(html.contains("&amp;"));
        assert!(html.contains("&gt;"));
    }

    #[test]
    fn welcome_doc_renders_clean() {
        // Make sure the actual onboarding doc converts without
        // panic. The output should at least have an h1 (the
        // doc starts with "# Welcome...").
        let html = render("welcome").unwrap();
        assert!(html.starts_with("<html><body>"));
        // v0.1.875: welcome.md's H1 was shortened from "Welcome to
        // web2qml" to just "Welcome"; assert the current heading.
        assert!(html.contains("<h1>Welcome</h1>"), "got: {}", html);
    }

    #[test]
    fn manpages_index_renders() {
        let html = render("manpages").unwrap();
        // Contains a link to welcome.
        assert!(html.contains("href=\"web2qml://man/welcome\""));
    }

    #[test]
    fn heading_without_space_not_heading() {
        // "#word" without space isn't a heading (could be a CSS id).
        // Verify it parses as a paragraph.
        let html = to_html("#word");
        assert!(html.contains("<p>"));
        // The # should be preserved (just escaped if needed).
        assert!(html.contains("#word"));
    }

    #[test]
    fn heading_level_clamps_at_6() {
        // 7 hashes = paragraph (h7 doesn't exist).
        let html = to_html("####### deep");
        assert!(!html.contains("<h7>"));
    }

    // v0.1.214: markdown table tests.

    #[test]
    fn table_emits_table_html() {
        let md = "| col1 | col2 |\n| ---- | ---- |\n| a | b |\n| c | d |\n";
        let html = to_html(md);
        assert!(html.contains("<table>"), "missing <table>: {}", html);
        assert!(html.contains("<thead>"), "missing <thead>: {}", html);
        assert!(html.contains("<th>col1</th>"), "missing col1 header: {}", html);
        assert!(html.contains("<th>col2</th>"));
        assert!(html.contains("<tbody>"));
        assert!(html.contains("<td>a</td>"));
        assert!(html.contains("<td>d</td>"));
        assert!(html.contains("</table>"));
    }

    #[test]
    fn table_without_edge_pipes() {
        // table_ops accepts rows without leading/trailing pipes.
        let md = "col1 | col2\n---- | ----\na | b\n";
        let html = to_html(md);
        assert!(html.contains("<table>"));
        assert!(html.contains("<th>col1</th>"));
        assert!(html.contains("<td>a</td>"));
    }

    #[test]
    fn table_with_alignment_separators() {
        let md = "| name | qty |\n| :--- | --: |\n| widget | 3 |\n";
        let html = to_html(md);
        assert!(html.contains("<table>"));
        assert!(html.contains("<th>name</th>"));
    }

    #[test]
    fn non_table_pipes_stay_paragraph() {
        // Sentence containing pipes shouldn't be misread as a table.
        let md = "use grep | head to filter output";
        let html = to_html(md);
        assert!(html.contains("<p>"), "expected paragraph: {}", html);
        assert!(!html.contains("<table>"), "false-positive table: {}", html);
    }

    #[test]
    fn pipe_line_without_separator_stays_paragraph() {
        // Just one pipe line, no separator row → paragraph.
        let md = "| oops | not a table |\nmore text";
        let html = to_html(md);
        assert!(!html.contains("<table>"));
    }

    #[test]
    fn table_followed_by_paragraph() {
        let md = "| a | b |\n| - | - |\n| 1 | 2 |\n\nafter the table";
        let html = to_html(md);
        assert!(html.contains("<table>"));
        assert!(html.contains("</table>"));
        assert!(html.contains("<p>after the table</p>"));
    }

    #[test]
    fn table_after_heading() {
        let md = "# Numbers\n\n| n |\n| - |\n| 1 |\n";
        let html = to_html(md);
        assert!(html.contains("<h1>Numbers</h1>"));
        assert!(html.contains("<table>"));
        assert!(html.contains("<td>1</td>"));
    }

    #[test]
    fn table_with_bold_in_cell() {
        let md = "| name | note |\n| - | - |\n| **emphasis** | regular |\n";
        let html = to_html(md);
        // The to_html table path (emit_table_html → render_inline) emits
        // <strong> for **bold**. The QML path (inline_to_richtext) emits <b>
        // instead — covered separately by table_with_bold_in_cell_emits_rich.
        assert!(html.contains("<strong>emphasis</strong>"));
    }

    #[test]
    fn table_with_link_in_cell() {
        let md = "| label | link |\n| - | - |\n| docs | [here](web2qml://man/welcome) |\n";
        let html = to_html(md);
        assert!(html.contains("href=\"web2qml://man/welcome\""));
    }

    // v0.1.402: strikethrough composes inside table cells.
    // emit_table_html runs each cell through render_inline,
    // which emits <del> for ~~text~~.
    #[test]
    fn table_with_strikethrough_in_cell() {
        let md = "| name | note |\n| - | - |\n| ~~old~~ | new |\n";
        let html = to_html(md);
        assert!(html.contains("<del>old</del>"),
            "strikethrough should render in cell: {}", html);
    }

    #[test]
    fn separator_detection_helper() {
        assert!(looks_like_table_separator("|---|---|"));
        assert!(looks_like_table_separator("| --- | --- |"));
        assert!(looks_like_table_separator(":---:|:---:"));
        assert!(looks_like_table_separator("---|---"));
        assert!(!looks_like_table_separator("text | text"));
        assert!(!looks_like_table_separator("no pipe here"));
        assert!(!looks_like_table_separator(""));
        // v0.1.452: smart-dash autocorrect artifacts accepted.
        assert!(looks_like_table_separator("\u{2013}|\u{2013}"),  // en-dash
            "en-dash separator accepted");
        assert!(looks_like_table_separator("\u{2014} | \u{2014}"), // em-dash
            "em-dash separator accepted");
        // But a colon-only (no dash of any kind) is still rejected.
        assert!(!looks_like_table_separator(":|:"),
            "colon-only separator still rejected");
    }

    // v0.1.452: a table whose separator got smart-dash-autocorrected
    // (en/em-dash instead of hyphen) still renders. This is a likely
    // cause of the reported "valid but unformatted tables don't render".
    #[test]
    fn table_with_emdash_separator_renders() {
        let md = "| Scene | Description |\n| \u{2014} | \u{2014} |\n| 1 | Opening |\n";
        let html = to_html(md);
        assert!(html.contains("<table>"), "em-dash table should render: {}", html);
        assert!(html.contains("Opening"), "row content present: {}", html);
    }

    // v0.1.215: native QML pathway tests. Validate that the
    // direct markdown → QML emission produces structurally-sound
    // output. These don't EXECUTE QML (no Qt in unit tests) —
    // they check that the text contains the expected element
    // names, attributes, and structure so we catch malformed
    // emission early.

    fn test_style() -> crate::qml_gen::RenderStyle {
        crate::qml_gen::RenderStyle::from_theme(
            false, 14, false, crate::ad_filter::FilterMode::Standard,
        )
    }

    #[test]
    fn to_qml_heading_emits_text_with_bold() {
        let sty = test_style();
        let qml = to_qml("# Hello", &sty);
        assert!(qml.contains("Text {"), "expected Text element: {}", qml);
        assert!(qml.contains("font.bold: true"), "heading should be bold");
        assert!(qml.contains("Hello"));
        // h1 → 1.6× base. Base 14 → 22.
        assert!(qml.contains("font.pixelSize: 22"),
            "expected h1 size 22: {}", qml);
    }

    #[test]
    fn to_qml_heading_levels_scale() {
        let sty = test_style();
        let q1 = to_qml("# h1", &sty);
        let q2 = to_qml("## h2", &sty);
        let q6 = to_qml("###### h6", &sty);
        // h1=22 (1.6×14), h2=20 (1.4×14), h6=14 (1.0×14)
        assert!(q1.contains("font.pixelSize: 22"));
        assert!(q2.contains("font.pixelSize: 20"));
        assert!(q6.contains("font.pixelSize: 14"));
    }

    #[test]
    fn to_qml_paragraph_wraps_for_selection() {
        let sty = test_style();
        // Selection-wrap only engages for paragraphs within
        // paragraph_word_bounds() (default 10-50 words) — matching the
        // webpage prose filter. Use a 16-word paragraph so the wrapper
        // (outer Item with _pid + MouseArea) is emitted.
        let para = "This is a normal paragraph that contains more than ten words so the selection wrapper engages.";
        let qml = to_qml(para, &sty);
        // wrap_for_paragraph_selection emits an outer Item with
        // a _pid property and a MouseArea.
        assert!(qml.contains("_pid"), "expected _pid: {}", qml);
        assert!(qml.contains("MouseArea"));
        assert!(qml.contains("the selection wrapper engages."));
    }

    #[test]
    fn to_qml_list_emits_richtext_with_link_handler() {
        let sty = test_style();
        let qml = to_qml("- [link](http://x.com) here", &sty);
        // Lists use RichText so links survive.
        assert!(qml.contains("textFormat: Text.RichText"),
            "expected RichText: {}", qml);
        assert!(qml.contains("onLinkActivated"));
        assert!(qml.contains("<a href="));
        assert!(qml.contains("http://x.com"));
    }

    #[test]
    fn to_qml_ordered_list_numbers_items() {
        let sty = test_style();
        let qml = to_qml("1. first\n2. second\n3. third", &sty);
        // Bullets / numbers are emitted as text prefix.
        assert!(qml.contains("1. "), "expected '1. ' prefix");
        assert!(qml.contains("2. "));
        assert!(qml.contains("3. "));
    }

    #[test]
    fn to_qml_table_uses_emit_adaptive_table() {
        let sty = test_style();
        let md = "| a | b |\n| - | - |\n| 1 | 2 |\n";
        let qml = to_qml(md, &sty);
        // emit_adaptive_table tags with `_tid: "t_<n>"`.
        assert!(qml.contains("_tid:"), "expected _tid from adaptive table: {}",
            &qml[..qml.len().min(500)]);
    }

    #[test]
    fn to_qml_table_has_outer_border() {
        // v0.1.235 polish: wide mode wraps the whole table in a
        // Rectangle with border for grouping.
        let sty = test_style();
        let md = "| a | b |\n| - | - |\n| 1 | 2 |\n";
        let qml = to_qml(md, &sty);
        // The wide-mode container is now `_wideCol` as Rectangle
        // with border + radius (was bare ColumnLayout).
        assert!(qml.contains("id: _wideCol"), "missing _wideCol: {}",
            &qml[..qml.len().min(800)]);
        assert!(qml.contains("border.width: 1"));
        assert!(qml.contains("radius: 4"));
    }

    #[test]
    fn to_qml_table_uses_zebra_on_odd_rows() {
        // v0.1.235 polish: data rows alternate between transparent
        // and zebra_bg backgrounds. Two-row tables should have
        // one transparent + one zebra-tinted.
        let sty = test_style();
        let md = "| a | b |\n| - | - |\n| 1 | 2 |\n| 3 | 4 |\n| 5 | 6 |\n";
        let qml = to_qml(md, &sty);
        // The zebra_bg value from from_theme(dark=false) is "#08000000".
        // Should appear in the QML (only for rows 1, 3, ... = odd-indexed).
        assert!(qml.contains("#08000000"),
            "expected zebra tint: {}", &qml[..qml.len().min(1200)]);
    }

    #[test]
    fn to_qml_table_data_row_has_padding() {
        // v0.1.235 polish: each row wrapper computes
        // `height: <inner>.height + 12` so cells have ~6px top
        // and bottom padding.
        let sty = test_style();
        let md = "| a |\n| - |\n| 1 |\n";
        let qml = to_qml(md, &sty);
        assert!(qml.contains(".height + 12"),
            "expected row padding: {}", &qml[..qml.len().min(1200)]);
    }

    #[test]
    fn to_qml_code_block_uses_monospace() {
        let sty = test_style();
        let qml = to_qml("```\nfoo bar\n```", &sty);
        assert!(qml.contains("font.family: \"monospace\""));
        assert!(qml.contains("foo bar"));
    }

    #[test]
    fn to_qml_not_found_includes_link_to_index() {
        let sty = test_style();
        let qml = not_found_qml("nope", &sty);
        assert!(qml.contains("Page not found"));
        assert!(qml.contains("nope"));
        assert!(qml.contains("web2qml://man/manpages"));
        assert!(qml.contains("onLinkActivated"));
    }

    #[test]
    fn to_qml_does_not_have_outer_column() {
        // _root in push_preview is the host Column. Adding
        // another at the top of to_qml would nest pointlessly.
        let sty = test_style();
        let qml = to_qml("# hi", &sty);
        // Should NOT start with "Column {"
        assert!(!qml.starts_with("Column {"),
            "to_qml should not emit an outer Column: {}", qml);
    }

    #[test]
    fn inline_to_richtext_bold() {
        let mut out = String::new();
        inline_to_richtext("plain **bold** plain", &mut out);
        assert!(out.contains("<b>bold</b>"), "got: {}", out);
    }

    // v0.1.402: strikethrough tests across all three inline
    // paths (richtext for QML, render_inline for HTML, and
    // strip_inline_markup for read-aloud/titles).

    #[test]
    fn inline_to_richtext_strikethrough() {
        let mut out = String::new();
        inline_to_richtext("plain ~~struck~~ plain", &mut out);
        assert!(out.contains("<s>struck</s>"), "got: {}", out);
    }

    #[test]
    fn inline_to_richtext_strikethrough_with_nested_bold() {
        let mut out = String::new();
        inline_to_richtext("~~struck **and bold**~~", &mut out);
        // Nested markup inside strikethrough should resolve.
        assert!(out.contains("<s>"), "got: {}", out);
        assert!(out.contains("<b>and bold</b>"), "got: {}", out);
    }

    // v0.1.891: math passthrough — `$…$` / `$$…$$` → mono span (delimiters kept).
    #[test]
    fn inline_math_renders_as_mono_with_delimiters() {
        let mut out = String::new();
        inline_to_richtext("the area $\\pi r^2$ here", &mut out);
        assert!(out.contains("<span style=\"font-family:monospace\">"), "got: {}", out);
        // delimiters kept inside the span so it reads as math, not code
        assert!(out.contains("$\\pi r^2$"), "math should keep its $…$: {}", out);
    }

    #[test]
    fn display_math_renders_as_mono() {
        let mut out = String::new();
        inline_to_richtext("see $$E = mc^2$$ above", &mut out);
        assert!(out.contains("<span style=\"font-family:monospace\">"), "got: {}", out);
        assert!(out.contains("$$E = mc^2$$"), "got: {}", out);
    }

    #[test]
    fn currency_is_not_treated_as_math() {
        // The whole point of the strict delimiter rules: prices must survive.
        for s in ["I paid $5 for it", "$5 to $10 range", "save $3 today",
                  "from $1,000 to $2,000"] {
            let mut out = String::new();
            inline_to_richtext(s, &mut out);
            assert!(!out.contains("font-family:monospace"),
                "currency wrongly matched as math in {s:?}: {out}");
        }
    }

    #[test]
    fn math_then_prose_and_two_spans() {
        let mut out = String::new();
        inline_to_richtext("$a$ and $b$ done", &mut out);
        // two separate math spans
        assert_eq!(out.matches("font-family:monospace").count(), 2, "got: {}", out);
        assert!(out.contains("$a$") && out.contains("$b$"), "got: {}", out);
    }

    #[test]
    fn math_edge_cases_dont_panic() {
        // unclosed, empty, space-led, and multibyte content must be safe
        for s in ["unclosed $x + 1", "empty $$$$ here", "$ x$ (space-led)",
                  "café $µ_0$ value", "$a$5 (digit after close)"] {
            let mut out = String::new();
            inline_to_richtext(s, &mut out); // must not panic
            let _ = out;
        }
        // the genuinely-valid multibyte one DOES render
        let mut out = String::new();
        inline_to_richtext("café $µ_0$ value", &mut out);
        assert!(out.contains("$µ_0$"), "multibyte math should render: {}", out);
    }

    #[test]
    fn render_inline_math_uses_code() {
        // HTML path: math → <code> with delimiters kept; currency untouched.
        let mut out = String::new();
        render_inline("area $\\pi r^2$ here", &mut out);
        assert!(out.contains("<code>$\\pi r^2$</code>"), "got: {}", out);
        let mut out2 = String::new();
        render_inline("I paid $5 and $10", &mut out2);
        assert!(!out2.contains("<code>"), "currency must not be <code>: {}", out2);
    }

    #[test]
    fn strip_inline_markup_drops_math_delimiters() {
        // read-aloud/titles: keep the expression, drop the $; don't re-parse
        // math content as markdown (a_b stays a_b, not <i>).
        assert_eq!(strip_inline_markup("the $x^2$ term"), "the x^2 term");
        assert_eq!(strip_inline_markup("display $$a_b$$ here"), "display a_b here");
        // currency survives intact
        assert_eq!(strip_inline_markup("paid $5 and $10"), "paid $5 and $10");
    }

    #[test]
    fn inline_to_richtext_single_tilde_is_literal() {
        // A lone ~ (not doubled) is NOT strikethrough.
        let mut out = String::new();
        inline_to_richtext("about ~5 minutes", &mut out);
        assert!(!out.contains("<s>"), "single ~ should not strike: {}", out);
        assert!(out.contains("~5"), "got: {}", out);
    }

    #[test]
    fn render_inline_strikethrough_uses_del() {
        let mut out = String::new();
        render_inline("plain ~~struck~~ plain", &mut out);
        assert!(out.contains("<del>struck</del>"), "got: {}", out);
    }

    #[test]
    fn strip_inline_markup_removes_strikethrough() {
        assert_eq!(strip_inline_markup("~~struck~~ stuff"), "struck stuff");
    }

    #[test]
    fn strip_inline_markup_single_tilde_preserved() {
        // Lone ~ stays literal in stripped output.
        assert_eq!(strip_inline_markup("about ~5 min"), "about ~5 min");
    }

    #[test]
    fn inline_to_richtext_link() {
        let mut out = String::new();
        inline_to_richtext("[label](https://example.com)", &mut out);
        assert!(out.contains("<a href=\"https://example.com\">label</a>"),
            "got: {}", out);
    }

    #[test]
    fn inline_to_richtext_escapes_html() {
        let mut out = String::new();
        inline_to_richtext("a < b & c > d", &mut out);
        assert!(out.contains("&lt;"));
        assert!(out.contains("&amp;"));
        assert!(out.contains("&gt;"));
    }

    #[test]
    fn strip_inline_markup_removes_bold() {
        assert_eq!(strip_inline_markup("**bold** stuff"), "bold stuff");
    }

    #[test]
    fn strip_inline_markup_keeps_link_text_drops_url() {
        assert_eq!(
            strip_inline_markup("see [docs](https://x.com) here"),
            "see docs here"
        );
    }

    #[test]
    fn strip_inline_markup_removes_inline_code() {
        assert_eq!(strip_inline_markup("call `foo()` next"), "call foo() next");
    }

    // v0.1.220: italic, wikilink, escape, image tests.

    #[test]
    fn italic_renders_em() {
        let html = to_html("*hello*");
        assert!(html.contains("<em>hello</em>"), "got: {}", html);
    }

    #[test]
    fn italic_not_bold() {
        // Bold should still win when `**`.
        let html = to_html("**bold** and *italic*");
        assert!(html.contains("<strong>bold</strong>"));
        assert!(html.contains("<em>italic</em>"));
    }

    #[test]
    fn italic_rejects_list_marker() {
        // `* foo` is a list marker, not italic. Should NOT get
        // <em> wrapping.
        let html = to_html("* foo bar");
        assert!(!html.contains("<em>"));
    }

    #[test]
    fn italic_rejects_space_close() {
        // `a *b * c` — the trailing `*` is preceded by space, so
        // it's not a closing delimiter. No italic match.
        let html = to_html("a *b * c");
        assert!(!html.contains("<em>"));
    }

    #[test]
    fn italic_to_qml_uses_i_tag() {
        // Paragraphs intentionally FLATTEN inline markup (see
        // emit_paragraph_qml — the selection wrapper's MouseArea must
        // not compete with inner link MouseAreas). Emphasis IS preserved
        // by inline_to_richtext, used for headings/list-items/blockquotes.
        // Test that conversion directly: italic `*x*` → <i>x</i>.
        let mut out = String::new();
        inline_to_richtext("*hello*", &mut out);
        assert!(out.contains("<i>hello</i>"), "got: {}", out);
    }

    #[test]
    fn wikilink_renders_link() {
        let html = to_html("see [[ECSE-2660]] notes");
        assert!(html.contains("href=\"web2qml://man/ecse-2660\""),
            "got: {}", html);
        assert!(html.contains(">ECSE-2660</a>"));
    }

    #[test]
    fn wikilink_with_alias() {
        let html = to_html("see [[Target|the display]]");
        assert!(html.contains("href=\"web2qml://man/target\""));
        assert!(html.contains(">the display</a>"));
    }

    #[test]
    fn wikilink_slug_normalizes_spaces() {
        let html = to_html("[[My Notes]]");
        assert!(html.contains("href=\"web2qml://man/my-notes\""), "got: {}", html);
    }

    #[test]
    fn wikilink_to_qml_links_with_onLinkActivated() {
        let sty = test_style();
        let qml = to_qml("- [[Foo]]", &sty);
        // The href sits inside a QML `text: "..."` string literal, so
        // its quotes are backslash-escaped (href=\"...\"). Assert on the
        // URL itself (present regardless of quote escaping) plus the
        // link handler, rather than the unescaped attribute form.
        assert!(qml.contains("web2qml://man/foo"), "got: {}", qml);
        assert!(qml.contains("onLinkActivated"));
    }

    #[test]
    fn escape_hash_emits_literal() {
        // `\#` should not be a heading; should emit literal `#`.
        let html = to_html("\\# bits");
        assert!(html.contains("<p># bits</p>"), "got: {}", html);
        assert!(!html.contains("<h1>"));
    }

    #[test]
    fn escape_star_prevents_bold() {
        // `\*not bold\*` should be plain text.
        let html = to_html("\\*not bold\\*");
        assert!(html.contains("*not bold*"));
        assert!(!html.contains("<strong>"));
        assert!(!html.contains("<em>"));
    }

    #[test]
    fn escape_backslash_emits_literal() {
        // `\\` should produce one literal backslash.
        let html = to_html("path C:\\\\foo");
        // Two backslashes → one literal. HTML doesn't escape
        // backslashes specially, so we expect "C:\foo" in output.
        assert!(html.contains("C:\\foo"), "got: {}", html);
    }

    #[test]
    fn escape_non_punct_left_alone() {
        // `\a` is not a valid escape, should pass through.
        let html = to_html("foo\\abar");
        assert!(html.contains("foo\\abar"));
    }

    #[test]
    fn image_renders_img_in_html() {
        let html = to_html("![cat](cat.png)");
        assert!(html.contains("<img"));
        assert!(html.contains("src=\"cat.png\""));
        assert!(html.contains("alt=\"cat\""));
    }

    #[test]
    fn image_renders_placeholder_in_qml() {
        let sty = test_style();
        let qml = to_qml("![my photo](x.png)", &sty);
        // Camera emoji placeholder + alt text. The literal char
        // 📷 should survive through write_esc_keep_html.
        assert!(qml.contains("my photo"), "got: {}", qml);
        assert!(qml.contains("📷"), "missing camera emoji: {}", qml);
    }

    #[test]
    fn image_in_strip_becomes_marker() {
        assert_eq!(strip_inline_markup("see ![the cat](c.png) here"),
            "see [image: the cat] here");
        assert_eq!(strip_inline_markup("![](x.png)"), "[image]");
    }

    #[test]
    fn table_with_bold_in_cell_emits_rich() {
        // The killer use case: bold inside a table cell that we
        // want to render with markup, not strip.
        let sty = test_style();
        let md = "| name | note |\n| - | - |\n| **alice** | regular |\n";
        let qml = to_qml(md, &sty);
        // Cell content goes through inline_to_richtext, producing
        // <b>alice</b>. emit_adaptive_table emits it with rich.
        assert!(qml.contains("<b>alice</b>"), "got: {}", &qml[..qml.len().min(800)]);
        assert!(qml.contains("textFormat: Text.RichText"));
    }

    #[test]
    fn table_with_wikilink_in_cell() {
        let sty = test_style();
        let md = "| topic | ref |\n| - | - |\n| math | [[Calc-101]] |\n";
        let qml = to_qml(md, &sty);
        assert!(qml.contains("href=\\\"web2qml://man/calc-101\\\""),
            "got: {}", &qml[..qml.len().min(800)]);
    }

    #[test]
    fn table_with_escaped_hash_in_cell() {
        // Real-world case from a course-notes table:
        // | header | next |
        // | ------ | ---- |
        // | \# bits |  6  |
        let sty = test_style();
        let md = "| h | n |\n| - | - |\n| \\# bits | 6 |\n";
        let qml = to_qml(md, &sty);
        // The cell should render as "# bits", not "\# bits".
        assert!(qml.contains("# bits"), "got: {}", &qml[..qml.len().min(800)]);
        assert!(!qml.contains("\\\\# bits"));
    }

    #[test]
    fn rich_text_survives_qml_escape() {
        // Regression guard for v0.1.220: write_esc strips HTML
        // tags, which would defeat rich-text. The new path uses
        // write_esc_keep_html. Verify a <b> tag survives.
        let sty = test_style();
        let qml = to_qml("# **bold heading**", &sty);
        assert!(qml.contains("<b>bold heading</b>"),
            "rich-text was stripped by escape: {}", qml);
    }

    // v0.1.221: underscore emphasis (CommonMark `_italic_` and
    // `__bold__`). Both delimiters work, with the intraword
    // restriction applied to `_` only (so `foo_bar_baz` stays
    // literal).

    #[test]
    fn underscore_italic_renders_em() {
        let html = to_html("_hello_");
        assert!(html.contains("<em>hello</em>"), "got: {}", html);
    }

    #[test]
    fn underscore_bold_renders_strong() {
        let html = to_html("__hello__");
        assert!(html.contains("<strong>hello</strong>"), "got: {}", html);
    }

    #[test]
    fn underscore_intraword_not_emphasized() {
        // foo_bar_baz is a common identifier shape — should NOT
        // become "foo<em>bar</em>baz". The intraword guard
        // rejects `_` openings preceded by alnum.
        let html = to_html("call foo_bar_baz here");
        assert!(!html.contains("<em>"), "got: {}", html);
        assert!(!html.contains("<strong>"), "got: {}", html);
        assert!(html.contains("foo_bar_baz"));
    }

    #[test]
    fn star_intraword_still_matches() {
        // `*` has no intraword guard (CommonMark allows). `a*b*c`
        // does emphasize "b". Documenting the asymmetric behavior.
        let html = to_html("a*b*c");
        assert!(html.contains("<em>b</em>"), "got: {}", html);
    }

    #[test]
    fn underscore_italic_to_qml() {
        // See italic_to_qml_uses_i_tag: emphasis lives in inline_to_richtext,
        // not in flattened paragraphs. `_x_` → <i>x</i>.
        let mut out = String::new();
        inline_to_richtext("_emph_", &mut out);
        assert!(out.contains("<i>emph</i>"), "got: {}", out);
    }

    #[test]
    fn underscore_bold_to_qml() {
        // `__x__` → <b>x</b> via inline_to_richtext.
        let mut out = String::new();
        inline_to_richtext("__strong__", &mut out);
        assert!(out.contains("<b>strong</b>"), "got: {}", out);
    }

    #[test]
    fn underscore_strip_keeps_text() {
        assert_eq!(strip_inline_markup("_quietly_ stripped"), "quietly stripped");
        assert_eq!(strip_inline_markup("__bold__ to plain"), "bold to plain");
    }

    #[test]
    fn underscore_at_start_works() {
        // Edge of the intraword guard: no preceding char, so
        // emphasis IS allowed.
        let html = to_html("_first_ word");
        assert!(html.contains("<em>first</em>"));
    }

    #[test]
    fn underscore_after_space_works() {
        // Preceded by space (non-alnum) → emphasis allowed.
        let html = to_html("note: _careful_ here");
        assert!(html.contains("<em>careful</em>"));
    }

    #[test]
    fn underscore_after_punctuation_works() {
        // Preceded by punctuation (non-alnum) → allowed.
        let html = to_html("(_parenthetical_)");
        assert!(html.contains("<em>parenthetical</em>"));
    }

    #[test]
    fn underscore_followed_by_alnum_not_close() {
        // Closing `_` followed by alnum → NOT a valid close
        // (intraword guard at closing too).
        let html = to_html("foo _bar_baz");
        assert!(!html.contains("<em>"), "got: {}", html);
    }

    #[test]
    fn mixed_underscore_and_star() {
        // `*outer _inner_ outer*` — italic with italic inside.
        // Inner emphasis uses different delimiter. Both should
        // render as <em>.
        let html = to_html("*outer _inner_ outer*");
        // The outer is italic; the inner is also italic.
        // Resulting structure: <em>outer <em>inner</em> outer</em>
        assert!(html.contains("<em>"));
        // Inner should be present.
        assert!(html.contains("inner"));
    }

    // v0.1.222: block-level image tests.

    #[test]
    fn detect_block_image_pure() {
        assert_eq!(detect_block_image("![cat](cat.png)"), Some(("cat", "cat.png")));
    }

    #[test]
    fn detect_block_image_with_whitespace() {
        // Surrounding whitespace is fine — block-level if trimmed
        // line is purely the image.
        assert_eq!(detect_block_image("   ![x](y.png)   "), Some(("x", "y.png")));
    }

    #[test]
    fn detect_block_image_rejects_mixed_content() {
        // Text + image on the same line → NOT block-level.
        // (Renders inline via paragraph's inline_to_richtext.)
        assert_eq!(detect_block_image("see ![diagram](x.png) above"), None);
        assert_eq!(detect_block_image("![x](y.png) trailing"), None);
        assert_eq!(detect_block_image("leading ![x](y.png)"), None);
    }

    #[test]
    fn detect_block_image_rejects_non_image() {
        assert_eq!(detect_block_image("not an image"), None);
        assert_eq!(detect_block_image("[link](url)"), None);  // link, not image
        assert_eq!(detect_block_image(""), None);
    }

    #[test]
    fn block_image_emits_img_tag_in_html() {
        // In to_html, a standalone image line becomes a top-level
        // <img>, NOT wrapped in <p>.
        let html = to_html("![photo](pic.jpg)");
        assert!(html.contains("<img src=\"pic.jpg\" alt=\"photo\">"),
            "got: {}", html);
        // Not wrapped in <p>.
        assert!(!html.contains("<p><img"));
    }

    #[test]
    fn block_image_emits_card_in_qml() {
        // v0.1.222 behavior preserved by v0.1.233 for relative paths:
        // the src `L09.assets/diagram.png` has no scheme, so we fall
        // back to the placeholder card (camera + alt + src in mono).
        let sty = test_style();
        let qml = to_qml("![my diagram](L09.assets/diagram.png)", &sty);
        assert!(qml.contains("Rectangle {"), "missing card: {}", qml);
        assert!(qml.contains("📷"), "missing icon");
        assert!(qml.contains("my diagram"), "missing alt");
        assert!(qml.contains("L09.assets/diagram.png"), "missing src");
        // v0.1.223: fixed escape — the QML output has plain
        // `font.family: "monospace"`, not `font.family: \"monospace\"`.
        assert!(qml.contains("font.family: \"monospace\""),
            "src should be monospace in placeholder mode");
        // v0.1.233: no real Image element emitted for relative paths
        // (the fallback path is placeholder-only).
        assert!(!qml.contains("Image {\nwidth: parent.width"),
            "should not emit Image for relative path");
    }

    #[test]
    fn block_image_https_url_emits_real_image() {
        // v0.1.233: absolute http(s):// URLs render as real QML
        // Image elements (asynchronous fetch, aspect-preserving).
        let sty = test_style();
        let qml = to_qml("![diagram](https://example.com/foo.png)", &sty);
        assert!(qml.contains("Image {"), "missing Image element: {}", qml);
        assert!(qml.contains("source: \"https://example.com/foo.png\""),
            "missing source binding");
        assert!(qml.contains("fillMode: Image.PreserveAspectFit"),
            "should preserve aspect");
        assert!(qml.contains("asynchronous: true"),
            "should be async");
        // The placeholder's `font.family: "monospace"` (used for the
        // src caption in placeholder mode) shouldn't appear here.
        assert!(!qml.contains("font.family: \"monospace\""),
            "real-image mode shouldn't have mono src caption");
        // Caption (alt text) IS emitted as italic muted Text.
        assert!(qml.contains("diagram"), "should include alt as caption");
        assert!(qml.contains("font.italic: true"),
            "caption should be italic");
    }

    #[test]
    fn block_image_file_scheme_emits_real_image() {
        // v0.1.233: file:// URLs also count as loadable.
        let sty = test_style();
        let qml = to_qml("![local](file:///tmp/test.png)", &sty);
        assert!(qml.contains("Image {"));
        assert!(qml.contains("source: \"file:///tmp/test.png\""));
    }

    #[test]
    fn block_image_data_url_emits_real_image() {
        // v0.1.233: data:image/ URLs (inline base64) also loadable.
        let sty = test_style();
        let qml = to_qml("![tiny](data:image/png;base64,iVBORw0KGgo=)", &sty);
        assert!(qml.contains("Image {"));
    }

    #[test]
    fn block_image_non_image_data_url_is_placeholder() {
        // v0.1.233: `data:` URLs that aren't image MIME types fall
        // back to placeholder (defensive — we don't want to feed
        // arbitrary data: schemes to QML Image).
        let sty = test_style();
        let qml = to_qml("![weird](data:text/plain,hello)", &sty);
        // Placeholder mode = has the monospace src caption.
        assert!(qml.contains("font.family: \"monospace\""),
            "should use placeholder for non-image data: URL");
        assert!(!qml.contains("Image {\nwidth: parent.width\nfillMode"),
            "should not emit Image for non-image data: URL");
    }

    #[test]
    fn block_image_https_empty_alt_no_caption() {
        // When alt is empty AND src is loadable, the caption Text
        // is skipped (just shows the image, no caption below).
        let sty = test_style();
        let qml = to_qml("![](https://example.com/foo.png)", &sty);
        assert!(qml.contains("Image {"));
        // No caption Text. Could test for the absence of the italic
        // caption marker.
        assert!(!qml.contains("font.italic: true"),
            "empty-alt loadable image should skip caption");
    }

    #[test]
    fn block_image_separates_from_paragraph() {
        // A paragraph followed by an image should NOT sweep the
        // image into the paragraph.
        let html = to_html("Some prose here.\n![cat](x.png)\nMore prose.");
        assert!(html.contains("<p>Some prose here.</p>"));
        assert!(html.contains("<img src=\"x.png\""));
        assert!(html.contains("<p>More prose.</p>"));
    }

    #[test]
    fn block_image_separates_from_list() {
        // An image after a list item should NOT be swept into
        // the item's text.
        let html = to_html("- first item\n![cat](x.png)");
        assert!(html.contains("<li>first item</li>"));
        assert!(html.contains("<img src=\"x.png\""));
        // The image is not inside the li.
        assert!(!html.contains("<li>first item <img"));
    }

    #[test]
    fn block_image_with_empty_src_handled() {
        // Edge case: `![alt]()` with empty src. Should still
        // emit a card (alt only).
        let sty = test_style();
        let qml = to_qml("![just alt]()", &sty);
        assert!(qml.contains("just alt"), "got: {}", qml);
    }

    #[test]
    fn inline_image_still_works() {
        // Backward-compat: image WITHIN a paragraph still flows
        // through the paragraph parser, which strips inline
        // markup for the selection wrapper. So inline images
        // appear as "[image: alt]" plain text — different from
        // block-level images (which get a full card).
        let sty = test_style();
        let qml = to_qml("see ![diagram](x.png) above", &sty);
        // No block-image card — the line isn't pure image.
        assert!(!qml.contains("📷"), "shouldn't render block card: {}", qml);
        // strip_inline_markup converts the image to a marker.
        assert!(qml.contains("[image: diagram]"), "got: {}", qml);
        assert!(qml.contains("see "));
        assert!(qml.contains(" above"));
    }

    // v0.1.223: code-block id uniqueness tests.

    #[test]
    fn code_block_inner_text_has_unique_id() {
        // Regression guard for the v0.1.215 latent bug: every
        // code block emitted `id: _code`. A page with 2+ code
        // blocks would trigger QML duplicate-id parse error.
        let sty = test_style();
        let qml = to_qml(
            "First block:\n\n```\nlet x = 1;\n```\n\nSecond block:\n\n```\nlet y = 2;\n```\n",
            &sty,
        );
        // The fixed pre-v0.1.223 bug would have emitted `_code`
        // twice. Now each block gets a unique `_code_<N>` id.
        assert!(!qml.contains("id: _code\n"), "naked _code id leaked: {}", qml);
        // Both code bodies should be present.
        assert!(qml.contains("let x = 1;"));
        assert!(qml.contains("let y = 2;"));
        // At least two distinct `_code_N` ids should appear (one
        // per block). We don't pin the exact numbers — they
        // depend on counter sharing with paragraphs.
        let mut code_ids = std::collections::HashSet::new();
        for line in qml.lines() {
            let line = line.trim();
            if let Some(rest) = line.strip_prefix("id: _code_") {
                code_ids.insert(rest.to_string());
            }
        }
        assert!(code_ids.len() >= 2,
            "expected ≥2 unique code ids, got {:?} in: {}", code_ids, qml);
    }

    #[test]
    fn code_block_outer_references_inner_id() {
        // Each Rectangle's `height: _code_<N>.implicitHeight` must
        // reference its OWN inner Text's id, not a different one.
        // Quick check: every `_code_<N>.implicitHeight` reference
        // has a matching `id: _code_<N>` somewhere.
        let sty = test_style();
        let qml = to_qml("```\na\n```\n\n```\nb\n```\n", &sty);
        for line in qml.lines() {
            let line = line.trim();
            if let Some(rest) = line.strip_prefix("height: _code_") {
                // rest is like "0.implicitHeight + 12"
                if let Some(num_end) = rest.find('.') {
                    let num = &rest[..num_end];
                    let expected_decl = format!("id: _code_{}", num);
                    assert!(qml.contains(&expected_decl),
                        "outer references _code_{} but no matching declaration", num);
                }
            }
        }
    }

    #[test]
    fn single_code_block_still_works() {
        // Make sure the v0.1.215 happy path still renders. A
        // single code block should produce a Rectangle + inner
        // Text with monospace font.
        let sty = test_style();
        let qml = to_qml("```\nhello world\n```", &sty);
        assert!(qml.contains("Rectangle {"), "missing Rectangle: {}", qml);
        assert!(qml.contains("font.family: \"monospace\""), "missing mono");
        assert!(qml.contains("hello world"), "missing code body");
    }

    // ─── v0.1.330 mermaid + percent_encode tests ───

    #[test]
    fn percent_encode_unreserved_passthrough() {
        let mut out = String::new();
        percent_encode_into("abc123-_.~", &mut out);
        assert_eq!(out, "abc123-_.~");
    }

    #[test]
    fn percent_encode_space() {
        let mut out = String::new();
        percent_encode_into("a b", &mut out);
        assert_eq!(out, "a%20b");
    }

    #[test]
    fn percent_encode_newline() {
        let mut out = String::new();
        percent_encode_into("line1\nline2", &mut out);
        assert_eq!(out, "line1%0Aline2");
    }

    #[test]
    fn percent_encode_reserved_chars() {
        let mut out = String::new();
        // Some chars that MUST encode: % & = ?
        percent_encode_into("a&b=c?d%e", &mut out);
        assert_eq!(out, "a%26b%3Dc%3Fd%25e");
    }

    #[test]
    fn percent_encode_utf8_multibyte() {
        let mut out = String::new();
        // "é" is 2 bytes in UTF-8: 0xC3 0xA9
        percent_encode_into("é", &mut out);
        assert_eq!(out, "%C3%A9");
    }

    #[test]
    fn html_render_mermaid_fence_emits_view_link() {
        // A markdown ```mermaid block should produce both the
        // code block AND the "View as diagram" anchor with the
        // source percent-encoded.
        let md = "```mermaid\nA --> B\n```";
        let html = to_html(md);
        assert!(html.contains("View as diagram"),
            "missing 'View as diagram' anchor: {}", html);
        assert!(html.contains("web2qml://mermaid?src="),
            "missing diagram URL scheme: {}", html);
        // Encoded source: "A --> B\n" → spaces and newlines
        // percent-encoded. "A" stays plain, " " → %20, "-" stays
        // plain, ">" → %3E, "\n" → %0A.
        assert!(html.contains("A%20--%3E%20B"),
            "source not percent-encoded: {}", html);
    }

    #[test]
    fn html_render_non_mermaid_fence_no_diagram_link() {
        // A plain (or differently-tagged) fence should NOT emit
        // the diagram link.
        let md = "```rust\nfn main() {}\n```";
        let html = to_html(md);
        assert!(!html.contains("View as diagram"),
            "should not have diagram link for rust fence: {}", html);
        let md_bare = "```\nplain code\n```";
        let html_bare = to_html(md_bare);
        assert!(!html_bare.contains("View as diagram"),
            "should not have diagram link for bare fence: {}", html_bare);
    }

    // ─── v0.1.331 UTF-8 regression tests ───
    //
    // The pre-v0.1.331 inline_to_richtext / strip_inline_markup /
    // push_plain_into_richtext functions walked bytes and cast
    // each byte to char, corrupting any multi-byte UTF-8
    // sequence. This made emoji-headed documents (and any with
    // curly quotes, em-dashes, accented chars, etc.) render
    // blank or truncated because Qt's RichText parser choked
    // on the resulting Latin-1 mojibake. These tests pin the
    // fix in place.

    #[test]
    fn inline_to_richtext_preserves_emoji() {
        let mut out = String::new();
        inline_to_richtext("🎬 Scene 1", &mut out);
        // The emoji is 4 bytes in UTF-8 (F0 9F 8E AC). It must
        // come through as a single char, not as 4 garbled chars.
        assert!(out.contains('🎬'), "emoji corrupted: {:?}", out);
        // No Latin-1 mojibake (the old bug produced char 0xF0
        // which is 'ð' — Latin small letter eth).
        assert!(!out.contains('ð'), "Latin-1 mojibake leaked: {:?}", out);
    }

    #[test]
    fn inline_to_richtext_preserves_curly_quotes() {
        let mut out = String::new();
        inline_to_richtext("\u{201C}Hello\u{201D}", &mut out);
        assert!(out.contains('\u{201C}'), "left quote corrupted: {:?}", out);
        assert!(out.contains('\u{201D}'), "right quote corrupted: {:?}", out);
    }

    #[test]
    fn inline_to_richtext_preserves_em_dash() {
        let mut out = String::new();
        inline_to_richtext("a — b", &mut out);
        assert!(out.contains('—'), "em-dash corrupted: {:?}", out);
    }

    #[test]
    fn inline_to_richtext_preserves_ellipsis() {
        let mut out = String::new();
        inline_to_richtext("wait…", &mut out);
        assert!(out.contains('…'), "ellipsis corrupted: {:?}", out);
    }

    #[test]
    fn inline_to_richtext_emoji_with_markdown_still_works() {
        // Bold inside emoji-heading text shouldn't break.
        let mut out = String::new();
        inline_to_richtext("🎬 **TITLE**", &mut out);
        assert!(out.contains('🎬'), "emoji lost: {:?}", out);
        assert!(out.contains("<b>"), "bold lost: {:?}", out);
        assert!(out.contains("TITLE"), "title body lost: {:?}", out);
    }

    #[test]
    fn strip_inline_markup_preserves_utf8() {
        let s = strip_inline_markup("🎬 **TRANSFORMERS** — “Blackstar”");
        // No mojibake.
        assert!(s.contains('🎬'), "emoji corrupted: {:?}", s);
        assert!(s.contains('—'), "em-dash corrupted: {:?}", s);
        assert!(s.contains('\u{201C}'), "left curly quote corrupted: {:?}", s);
        // Bold markdown stripped.
        assert!(!s.contains("**"), "markdown left in plain text: {:?}", s);
        assert!(s.contains("TRANSFORMERS"), "bold body lost: {:?}", s);
    }

    #[test]
    fn push_plain_into_richtext_preserves_utf8() {
        let mut out = String::new();
        push_plain_into_richtext("🎬 Hello — world “quoted”", &mut out);
        // Round-trips intact apart from HTML-escape of quote
        // chars (curly quotes don't HTML-escape, so they pass).
        // Verify via parse-and-rebuild rather than char-range
        // gymnastics: the output should contain each non-ASCII
        // codepoint from the input as a real char.
        for c in "🎬— \u{201C}\u{201D}".chars() {
            if c == ' ' { continue; }
            assert!(out.contains(c),
                "lost codepoint U+{:04X} ({}): {:?}", c as u32, c, out);
        }
        // No naked Latin-1 continuation-byte mojibake. The 🎬
        // emoji's first byte is 0xF0 → 'ð'; second 0x9F →
        // \u{009f} (APPLICATION PROGRAM COMMAND, a C1 control).
        // Output must not contain either.
        assert!(!out.contains('ð'), "Latin-1 mojibake leaked: {:?}", out);
        assert!(!out.contains('\u{009F}'), "C1 control leaked: {:?}", out);
    }

    /// Full integration test: render the BLACKSTAR-style header
    /// pattern (emoji heading + curly-quoted blockquote) and
    /// confirm the emoji + quotes survive.
    #[test]
    fn full_emoji_heading_renders_correctly() {
        let sty = test_style();
        let md = "# 🎬 TRANSFORMERS: BLACKSTAR\n\n## 🎙️ Voiceover\n\n> \u{201C}In the cosmos…\u{201D}";
        let qml = to_qml(md, &sty);
        // The emoji must survive as a real codepoint in the
        // output. Before the fix it became 4 Latin-1 chars
        // (mojibake).
        assert!(qml.contains('🎬'), "emoji lost in QML output");
        assert!(qml.contains('🎙'), "second emoji lost in QML output");
        // Curly quotes from the blockquote must survive too.
        // (Note: blockquote `>` prefix passes through as text
        // since manpages::to_qml has no blockquote handler.)
        assert!(qml.contains('\u{201C}'), "left curly quote lost");
        assert!(qml.contains('\u{201D}'), "right curly quote lost");
        // Should NOT contain known Latin-1 mojibake artifacts.
        // U+00F0 is 'ð' (LATIN SMALL LETTER ETH) — the lead byte
        // of the 🎬 emoji (0xF0) misinterpreted as Latin-1.
        assert!(!qml.contains('ð'), "Latin-1 mojibake present");
    }

    // ─── v0.1.332 to_qml mermaid-fence integration ───

    #[test]
    fn to_qml_mermaid_fence_emits_diagram_link() {
        // The to_qml path should now ALSO inject a "View as
        // diagram" Text element after a ```mermaid fence,
        // matching the to_html behavior added in v0.1.330.
        let sty = test_style();
        let md = "```mermaid\nA --> B\n```";
        let qml = to_qml(md, &sty);
        assert!(qml.contains("View as diagram"),
            "missing 'View as diagram' Text element: {}", qml);
        assert!(qml.contains("web2qml://mermaid?src="),
            "missing diagram URL: {}", qml);
        assert!(qml.contains("A%20--%3E%20B"),
            "source not percent-encoded: {}", qml);
        assert!(qml.contains("onLinkActivated"),
            "missing link activation handler: {}", qml);
    }

    #[test]
    fn to_qml_non_mermaid_fence_no_diagram_link() {
        let sty = test_style();
        let md = "```rust\nfn main() {}\n```";
        let qml = to_qml(md, &sty);
        assert!(!qml.contains("View as diagram"),
            "unexpected diagram link for rust fence: {}", qml);
        let md_bare = "```\nplain code\n```";
        let qml_bare = to_qml(md_bare, &sty);
        assert!(!qml_bare.contains("View as diagram"),
            "unexpected diagram link for bare fence: {}", qml_bare);
    }

    // ─── v0.1.336 blockquote handler ───

    #[test]
    fn to_qml_blockquote_single_line() {
        let sty = test_style();
        let md = "> Hello world";
        let qml = to_qml(md, &sty);
        // Should emit the italic styling marker.
        assert!(qml.contains("font.italic: true"),
            "expected italic styling: {}", qml);
        // The body should contain the prose WITHOUT the leading `> `.
        assert!(qml.contains("Hello world"), "missing body: {}", qml);
        // The naive paragraph fallback would have left the literal
        // "> " prefix in the output. Pin that it's gone.
        assert!(!qml.contains("text: \"> "), "literal '> ' leaked: {}", qml);
    }

    #[test]
    fn to_qml_blockquote_multi_line_one_paragraph() {
        let sty = test_style();
        let md = "> Line one\n> line two\n> line three";
        let qml = to_qml(md, &sty);
        // All three lines should join into one paragraph body
        // separated by spaces.
        assert!(qml.contains("Line one line two line three"),
            "lines not joined: {}", qml);
    }

    #[test]
    fn to_qml_blockquote_paragraph_break_with_bare_gt() {
        // Bare `>` line is a paragraph break WITHIN the blockquote.
        let sty = test_style();
        let md = "> First quote.\n>\n> Second quote.";
        let qml = to_qml(md, &sty);
        // Both quotes appear, but as separate Text elements (we
        // verify by checking they're NOT joined into one).
        assert!(qml.contains("First quote."), "first missing: {}", qml);
        assert!(qml.contains("Second quote."), "second missing: {}", qml);
        // The two should NOT be in the same Text body.
        assert!(!qml.contains("First quote. Second quote."),
            "paragraph break ignored: {}", qml);
    }

    #[test]
    fn to_qml_blockquote_ends_at_blank_line() {
        let sty = test_style();
        let md = "> Quoted.\n\nNot quoted.";
        let qml = to_qml(md, &sty);
        // The italic styling appears ONCE (for the quoted line).
        // "Not quoted." should not be in an italic Text.
        let italic_count = qml.matches("font.italic: true").count();
        assert!(italic_count >= 1, "no italic emission: {}", qml);
        // The follower paragraph should be present as a normal
        // (non-italic) emission.
        assert!(qml.contains("Not quoted."), "follower missing: {}", qml);
    }

    #[test]
    fn to_qml_blockquote_preserves_utf8() {
        // Combine v0.1.331 utf-8 fix + v0.1.336 blockquote.
        let sty = test_style();
        let md = "> \u{201C}In the cosmos…\u{201D} — Optimus";
        let qml = to_qml(md, &sty);
        assert!(qml.contains('\u{201C}'), "left curly quote lost");
        assert!(qml.contains('\u{201D}'), "right curly quote lost");
        assert!(qml.contains('—'), "em-dash lost");
        assert!(qml.contains('…'), "ellipsis lost");
        assert!(!qml.contains('ð'), "Latin-1 mojibake present: {}", qml);
    }

    #[test]
    fn to_qml_multiple_blockquotes_get_unique_ids() {
        // Two separate blockquotes (separated by a blank line)
        // must generate distinct inner-Column IDs so QML doesn't
        // hit a duplicate-id parse error.
        let sty = test_style();
        let md = "> First.\n\nMiddle prose.\n\n> Second.";
        let qml = to_qml(md, &sty);
        // Find all _bq_body_ id declarations.
        let ids: Vec<&str> = qml.lines()
            .filter_map(|l| {
                let t = l.trim();
                if let Some(rest) = t.strip_prefix("id: _bq_body_") {
                    Some(rest)
                } else { None }
            })
            .collect();
        assert_eq!(ids.len(), 2,
            "expected 2 blockquote IDs, got {}: {}", ids.len(), qml);
        assert_ne!(ids[0], ids[1], "IDs collide: both = {}", ids[0]);
    }

    // ─── v0.1.337 horizontal rule + setext heading ───

    #[test]
    fn is_hr_recognizes_dashes() {
        assert!(is_horizontal_rule("---"));
        assert!(is_horizontal_rule("----"));
        assert!(is_horizontal_rule("------"));   // BLACKSTAR.md style
        assert!(is_horizontal_rule("- - -"));    // spaced
        assert!(is_horizontal_rule("---  "));    // trailing space
        assert!(is_horizontal_rule("  ---"));    // leading space
    }

    #[test]
    fn is_hr_recognizes_underscores_and_asterisks() {
        assert!(is_horizontal_rule("___"));
        assert!(is_horizontal_rule("***"));
        assert!(is_horizontal_rule("________"));
        assert!(is_horizontal_rule("*****"));
    }

    #[test]
    fn is_hr_rejects_too_short() {
        assert!(!is_horizontal_rule("--"));
        assert!(!is_horizontal_rule("__"));
        assert!(!is_horizontal_rule("**"));
        assert!(!is_horizontal_rule("-"));
    }

    #[test]
    fn is_hr_rejects_mixed_markers() {
        assert!(!is_horizontal_rule("-*-"));
        assert!(!is_horizontal_rule("---_"));
    }

    #[test]
    fn is_hr_rejects_with_trailing_text() {
        assert!(!is_horizontal_rule("--- text"));
        assert!(!is_horizontal_rule("---x"));
        assert!(!is_horizontal_rule("--- 1"));
    }

    #[test]
    fn is_hr_rejects_empty_and_whitespace() {
        assert!(!is_horizontal_rule(""));
        assert!(!is_horizontal_rule("   "));
    }

    #[test]
    fn is_setext_underline_h1() {
        assert_eq!(is_setext_underline("="), Some(1));
        assert_eq!(is_setext_underline("==="), Some(1));
        assert_eq!(is_setext_underline("========"), Some(1));
        assert_eq!(is_setext_underline("  ===  "), Some(1));
    }

    #[test]
    fn is_setext_underline_h2() {
        assert_eq!(is_setext_underline("-"), Some(2));
        assert_eq!(is_setext_underline("---"), Some(2));
        assert_eq!(is_setext_underline("---------"), Some(2));
    }

    #[test]
    fn is_setext_underline_rejects_other_markers() {
        assert_eq!(is_setext_underline("***"), None);
        assert_eq!(is_setext_underline("___"), None);
        assert_eq!(is_setext_underline("=-=-"), None);
        assert_eq!(is_setext_underline("=x"), None);
    }

    #[test]
    fn to_qml_emits_hr_rectangle() {
        // A standalone ------ line should render as a thin
        // Rectangle, not as a paragraph of dashes.
        let sty = test_style();
        let md = "Above.\n\n------\n\nBelow.";
        let qml = to_qml(md, &sty);
        // The HR is a Rectangle of height 1. Three Rectangles
        // (or one with width-and-height-1) is the marker.
        assert!(qml.contains("height: 1"), "missing HR height: {}", qml);
        // The literal text "------" should NOT appear in any Text
        // element body. (It may appear elsewhere in commentary,
        // but not as text content.)
        assert!(!qml.contains("text: \"------"),
            "raw dashes leaked as paragraph: {}", qml);
        // The surrounding paragraphs should still be there.
        assert!(qml.contains("Above."), "above text missing");
        assert!(qml.contains("Below."), "below text missing");
    }

    #[test]
    fn to_qml_promotes_setext_h1_above_paragraph() {
        // "Title\n=====" should become an h1.
        let sty = test_style();
        let md = "My Title\n=========\n\nSome body.";
        let qml = to_qml(md, &sty);
        // h1 = font-size 1.6x base. Test style's base is documented
        // in test_style(); we just need the title to be styled as
        // a heading (font.bold: true is the heading marker).
        assert!(qml.contains("font.bold: true"),
            "title should be bold (heading): {}", qml);
        assert!(qml.contains("My Title"), "title body missing");
        // The underline shouldn't be rendered as a Text body.
        assert!(!qml.contains("text: \"========"),
            "underline rendered as text: {}", qml);
        assert!(qml.contains("Some body."), "body paragraph missing");
    }

    #[test]
    fn to_qml_promotes_setext_h2_above_paragraph() {
        // "Subtitle\n-----" should become an h2 (NOT an HR
        // because text immediately precedes the dashes).
        let sty = test_style();
        let md = "Subtitle\n--------\n\nBody.";
        let qml = to_qml(md, &sty);
        assert!(qml.contains("font.bold: true"),
            "subtitle should be bold (heading): {}", qml);
        assert!(qml.contains("Subtitle"), "subtitle text missing");
        // The dashes should NOT have become an HR Rectangle —
        // height: 1 markers only appear from HR branches, and
        // here the dashes promote the previous line instead.
        // Tricky to assert positively (Body is rendered as a
        // normal Text, no Rectangle). Instead assert dashes
        // didn't leak as paragraph text.
        assert!(!qml.contains("text: \"--------"),
            "underline leaked as paragraph: {}", qml);
    }

    #[test]
    fn to_qml_hr_after_paragraph_with_blank_line_still_hr() {
        // "para\n\n---\n\npara" — the blank lines separate the
        // dashes from the paragraph, so setext doesn't apply
        // and the dashes ARE an HR.
        let sty = test_style();
        let md = "Para A.\n\n---\n\nPara B.";
        let qml = to_qml(md, &sty);
        // HR Rectangle should be present.
        assert!(qml.contains("height: 1"),
            "expected HR Rectangle: {}", qml);
        // Both paragraphs should be present.
        assert!(qml.contains("Para A."));
        assert!(qml.contains("Para B."));
    }
}
