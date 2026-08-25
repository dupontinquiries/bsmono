//! Terminal-scene fences: markdown ```` ```term ```` blocks rendered as a
//! styled command/output demo instead of a flat code block.
//!
//! ## Why
//!
//! Course/book content (the Asahi stream textbook arc) recovers terminal
//! sessions from video frames via OCR. Storing them as text instead of
//! frame images is a ~200x size win and makes them searchable and
//! copyable — but a flat code block loses the command/output structure
//! that `notebook::classify` and `term_blocks::CommandBlock` already
//! treat as first-class elsewhere in the app. This module gives bundled
//! and on-disk markdown the same command-vs-output visual language.
//!
//! ## Format (degrades gracefully everywhere else)
//!
//! ````text
//! ```term t=1:23:45 title="hv boot"
//! $ python -m m1n1.hv boot.bin
//! [hv] loading boot.bin...
//! TTBR0 set, jumping to payload
//! $ dmesg | tail -2
//! ...
//! ```
//! ````
//!
//! - The info string is `term` plus optional `key=value` pairs. Values
//!   may be double-quoted to contain spaces. Unknown keys are ignored
//!   (forward compatibility for `step=`, `src=`, ...).
//! - A body line starting with `"$ "` is a command; everything else is
//!   output. Consecutive output lines merge into one block. OCR
//!   normalization upstream rewrites `user@host:path$ cmd` prompts to
//!   the bare `$ cmd` form, so this parser stays dumb on purpose.
//! - Renderers that don't know the tag (GitHub, mdBook, our own
//!   `to_html` legacy path) show a plain fenced code block — the base
//!   information is never lost, only the styling.
//!
//! ## Rendering
//!
//! `emit_qml` follows `emit_code_block_qml`'s conventions: outer
//! Rectangle sized off a uniquely-id'd inner Column
//! (`qml_gen::next_paragraph_id` — same duplicate-id discipline as
//! v0.1.223), `write_esc_pub` for text escaping, colors from
//! `RenderStyle`. Commands render bold with the leading `$ `; output
//! renders dimmed (opacity) — no RichText, so no second escaping
//! context to get wrong. Header row shows title/timestamp when present.
//!
//! Follow-ups deliberately not in this slice: tap-to-copy on command
//! lines, multi-step replay (`step=` key), linking `t=` to a media
//! seek. All are additive on this format.

/// One parsed entry of a terminal scene.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TermEntry {
    /// A command line (the `"$ "` prefix has been stripped).
    Cmd(String),
    /// A run of one or more output lines, joined with `\n`.
    Output(String),
}

/// A parsed ```term fence.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TermScene {
    /// `t=` value from the info string (source-video timestamp).
    pub t: Option<String>,
    /// `title=` value from the info string.
    pub title: Option<String>,
    pub entries: Vec<TermEntry>,
}

/// Parse the fence info string (everything after the backticks on the
/// opening fence line, e.g. `term t=1:23:45 title="hv boot"`).
/// Returns (t, title). Tokens are `key=value`; a value beginning with
/// `"` runs (spaces included) to the next `"`. Unknown keys ignored.
pub fn parse_info(info: &str) -> (Option<String>, Option<String>) {
    let mut t = None;
    let mut title = None;
    let mut rest = info.trim();
    // skip the leading language token ("term")
    if let Some(sp) = rest.find(char::is_whitespace) {
        rest = rest[sp..].trim_start();
    } else {
        return (t, title);
    }
    let bytes = rest.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        // key
        let key_start = i;
        while i < bytes.len() && bytes[i] != b'=' && !bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        let key = &rest[key_start..i];
        if i >= bytes.len() || bytes[i] != b'=' {
            // bare token without '=': skip it
            while i < bytes.len() && bytes[i].is_ascii_whitespace() { i += 1; }
            continue;
        }
        i += 1; // '='
        let value: String;
        if i < bytes.len() && bytes[i] == b'"' {
            i += 1;
            let vstart = i;
            while i < bytes.len() && bytes[i] != b'"' { i += 1; }
            value = rest[vstart..i].to_string();
            if i < bytes.len() { i += 1; } // closing quote
        } else {
            let vstart = i;
            while i < bytes.len() && !bytes[i].is_ascii_whitespace() { i += 1; }
            value = rest[vstart..i].to_string();
        }
        match key {
            "t" => t = Some(value),
            "title" => title = Some(value),
            _ => {}
        }
        while i < bytes.len() && bytes[i].is_ascii_whitespace() { i += 1; }
    }
    (t, title)
}

/// Parse a fence body into command/output entries. Trailing newline
/// tolerated. Blank lines inside an output run are kept (they are part
/// of real terminal output); leading/trailing blank output lines are
/// trimmed per run.
pub fn parse_body(body: &str) -> Vec<TermEntry> {
    let mut entries: Vec<TermEntry> = Vec::new();
    let mut out_run: Vec<&str> = Vec::new();
    fn flush(run: &mut Vec<&str>, entries: &mut Vec<TermEntry>) {
        while run.first().is_some_and(|l| l.trim().is_empty()) { run.remove(0); }
        while run.last().is_some_and(|l| l.trim().is_empty()) { run.pop(); }
        if !run.is_empty() {
            entries.push(TermEntry::Output(run.join("\n")));
            run.clear();
        }
    }
    for line in body.lines() {
        if let Some(cmd) = line.strip_prefix("$ ") {
            flush(&mut out_run, &mut entries);
            entries.push(TermEntry::Cmd(cmd.to_string()));
        } else if line == "$" {
            // a bare prompt line — treat as an empty command boundary
            flush(&mut out_run, &mut entries);
        } else {
            out_run.push(line);
        }
    }
    flush(&mut out_run, &mut entries);
    entries
}

/// Convenience: parse info string + body in one call.
pub fn parse(info: &str, body: &str) -> TermScene {
    let (t, title) = parse_info(info);
    TermScene { t, title, entries: parse_body(body) }
}

/// Emit the styled QML for a scene. Mirrors `emit_code_block_qml`'s
/// structure: Rectangle sized off a uniquely-id'd inner Column.
pub fn emit_qml(scene: &TermScene, sty: &crate::qml_gen::RenderStyle, out: &mut String) {
    let nid = crate::qml_gen::next_paragraph_id();
    let col_id = format!("_termCol_{}", nid);
    out.push_str("Rectangle {\nwidth: parent.width\n");
    out.push_str(&format!("color: \"{}\"\n", sty.input_bg));
    out.push_str(&format!("border.color: \"{}\"\n", sty.border_color));
    out.push_str("border.width: 1\nradius: 4\n");
    out.push_str(&format!("height: {}.implicitHeight + 12\n", col_id));
    out.push_str(&format!("Column {{\nid: {}\n", col_id));
    out.push_str("anchors.left: parent.left\nanchors.right: parent.right\nanchors.top: parent.top\n");
    out.push_str("anchors.margins: 6\nspacing: 2\n");

    // header row: "⏱ t — title" when either is present
    if scene.t.is_some() || scene.title.is_some() {
        let mut header = String::new();
        if let Some(t) = &scene.t {
            header.push_str("⏱ ");
            header.push_str(t);
        }
        if let Some(title) = &scene.title {
            if !header.is_empty() { header.push_str(" — "); }
            header.push_str(title);
        }
        out.push_str("Text {\nwidth: parent.width\n");
        out.push_str(&format!("color: \"{}\"\n", sty.text_color));
        out.push_str("opacity: 0.6\n");
        out.push_str(&format!("font.pixelSize: {}\n", sty.font_size - 2));
        out.push_str("font.family: \"monospace\"\n");
        out.push_str("wrapMode: Text.WrapAtWordBoundaryOrAnywhere\n");
        out.push_str("text: \"");
        crate::qml_gen::write_esc_pub(out, &header);
        out.push_str("\"\n}\n");
    }

    for entry in &scene.entries {
        match entry {
            TermEntry::Cmd(cmd) => {
                out.push_str("Text {\nwidth: parent.width\n");
                out.push_str(&format!("color: \"{}\"\n", sty.text_color));
                out.push_str("font.bold: true\n");
                out.push_str(&format!("font.pixelSize: {}\n", sty.font_size));
                out.push_str("font.family: \"monospace\"\n");
                out.push_str("wrapMode: Text.WrapAtWordBoundaryOrAnywhere\n");
                out.push_str("text: \"");
                let mut line = String::with_capacity(cmd.len() + 2);
                line.push_str("$ ");
                line.push_str(cmd);
                crate::qml_gen::write_esc_pub(out, &line);
                out.push_str("\"\n}\n");
            }
            TermEntry::Output(text) => {
                out.push_str("Text {\nwidth: parent.width\n");
                out.push_str(&format!("color: \"{}\"\n", sty.text_color));
                out.push_str("opacity: 0.78\n");
                out.push_str(&format!("font.pixelSize: {}\n", sty.font_size));
                out.push_str("font.family: \"monospace\"\n");
                out.push_str("wrapMode: Text.WrapAtWordBoundaryOrAnywhere\n");
                out.push_str("text: \"");
                crate::qml_gen::write_esc_pub(out, text);
                out.push_str("\"\n}\n");
            }
        }
    }
    out.push_str("}\n}\n");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn info_parses_t_and_quoted_title() {
        let (t, title) = parse_info("term t=1:23:45 title=\"hv boot demo\"");
        assert_eq!(t.as_deref(), Some("1:23:45"));
        assert_eq!(title.as_deref(), Some("hv boot demo"));
    }

    #[test]
    fn info_bare_term_is_empty() {
        assert_eq!(parse_info("term"), (None, None));
    }

    #[test]
    fn info_unknown_keys_ignored() {
        let (t, title) = parse_info("term step=3 t=0:10");
        assert_eq!(t.as_deref(), Some("0:10"));
        assert_eq!(title, None);
    }

    #[test]
    fn body_splits_cmd_and_output_runs() {
        let e = parse_body("$ make hv\nCC hv.c\nLD hv.bin\n$ ls\nhv.bin\n");
        assert_eq!(e, vec![
            TermEntry::Cmd("make hv".into()),
            TermEntry::Output("CC hv.c\nLD hv.bin".into()),
            TermEntry::Cmd("ls".into()),
            TermEntry::Output("hv.bin".into()),
        ]);
    }

    #[test]
    fn body_keeps_interior_blank_lines_trims_edges() {
        let e = parse_body("\nfoo\n\nbar\n\n");
        assert_eq!(e, vec![TermEntry::Output("foo\n\nbar".into())]);
    }

    #[test]
    fn body_output_only_is_one_run() {
        let e = parse_body("kernel: booting\nkernel: ok\n");
        assert_eq!(e.len(), 1);
    }

    #[test]
    fn emit_has_unique_ids_and_escaped_text() {
        let sty = crate::qml_gen::RenderStyle::from_theme(
            false, 14, false, crate::ad_filter::FilterMode::Standard,
        );
        let scene = parse("term t=0:05", "$ echo \"hi\"\nhi\n");
        let mut a = String::new();
        emit_qml(&scene, &sty, &mut a);
        let mut b = String::new();
        emit_qml(&scene, &sty, &mut b);
        // two emissions must not share a Column id (v0.1.223 discipline)
        let id_of = |s: &str| s.lines().find(|l| l.starts_with("id: _termCol_"))
            .unwrap().to_string();
        assert_ne!(id_of(&a), id_of(&b));
        // the quote inside the command must be escaped for the QML
        // string literal — a raw `"hi"` would terminate it early
        assert!(a.contains("\\\"hi\\\""), "quotes must be escaped: {}", a);
        assert!(a.contains("font.bold: true"));
        assert!(a.contains("opacity: 0.78"));
        assert!(a.contains("⏱ 0:05"));
    }
}
