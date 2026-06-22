//! markdown.rs — lightweight Markdown → GTK4 widget renderer
//!
//! Renders assistant messages as a vertical stack of widgets:
//!   • Paragraph text with Pango markup (bold, italic, inline code, links)
//!   • Fenced code blocks → monospace TextView with language badge + copy button
//!   • Headers → larger bold label
//!   • Blockquotes → indented dim label
//!   • Bullet/numbered lists → indented labels
//!
//! No external parser dep — a single-pass state machine handles the grammar
//! we actually see in LLM output. Toggle between rendered and raw source is
//! handled by the caller (chat_tab) by swapping the widget.

use gtk4::prelude::*;
use gtk4::{
    Box as GtkBox, Button, Label, Orientation,
    PolicyType, ScrolledWindow, TextBuffer, TextView, WrapMode,
};
use gtk4::glib;

// ── Parsed block types ────────────────────────────────────────────────────────

#[derive(Debug)]
enum Block {
    Heading { level: u8, text: String },
    Para(String),
    Code { lang: String, text: String },
    Blockquote(String),
    ListItem { ordered: bool, index: usize, text: String },
    HRule,
}

// ── Parse markdown into blocks ────────────────────────────────────────────────

// parse() intentionally removed — use render() or has_markdown() directly

fn parse_blocks(md: &str) -> Vec<Block> {
    let mut blocks = Vec::new();
    let mut lines  = md.lines().peekable();
    let mut list_idx = 0usize;

    while let Some(line) = lines.next() {
        // Fenced code block
        if line.starts_with("```") {
            let lang = line.trim_start_matches('`').trim().to_string();
            let mut code = String::new();
            for l in lines.by_ref() {
                if l.starts_with("```") { break; }
                code.push_str(l);
                code.push('\n');
            }
            blocks.push(Block::Code { lang, text: code });
            list_idx = 0;
            continue;
        }

        // Horizontal rule
        if line.trim() == "---" || line.trim() == "***" || line.trim() == "___" {
            blocks.push(Block::HRule);
            continue;
        }

        // Heading
        let hashes = line.chars().take_while(|&c| c == '#').count();
        if hashes > 0 && hashes <= 6 && line.chars().nth(hashes) == Some(' ') {
            blocks.push(Block::Heading {
                level: hashes as u8,
                text: line[hashes+1..].to_string(),
            });
            list_idx = 0;
            continue;
        }

        // Blockquote
        if line.starts_with("> ") {
            blocks.push(Block::Blockquote(line[2..].to_string()));
            continue;
        }

        // Ordered list item
        if let Some(rest) = line.strip_prefix(|c: char| c.is_ascii_digit())
            .and_then(|s| s.strip_prefix(". ").or_else(|| s.strip_prefix(") "))) {
            list_idx += 1;
            blocks.push(Block::ListItem { ordered: true, index: list_idx, text: rest.to_string() });
            continue;
        }

        // Unordered list item
        if let Some(rest) = line.strip_prefix("- ").or_else(|| line.strip_prefix("* "))
                                .or_else(|| line.strip_prefix("+ ")) {
            blocks.push(Block::ListItem { ordered: false, index: 0, text: rest.to_string() });
            list_idx = 0;
            continue;
        }

        // Empty line
        if line.trim().is_empty() {
            list_idx = 0;
            continue;
        }

        // Regular paragraph
        blocks.push(Block::Para(line.to_string()));
        list_idx = 0;
    }

    // Merge consecutive Para blocks into single paragraphs
    let mut merged: Vec<Block> = Vec::new();
    for block in blocks {
        if let Block::Para(ref text) = block {
            if let Some(Block::Para(prev)) = merged.last_mut() {
                prev.push(' ');
                prev.push_str(text);
                continue;
            }
        }
        merged.push(block);
    }
    merged
}

// ── Inline markdown → Pango markup ───────────────────────────────────────────

fn inline_to_pango(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 32);
    let chars: Vec<char> = text.chars().collect();
    let n = chars.len();
    let mut i = 0;

    while i < n {
        // Escape XML special chars first
        match chars[i] {
            '&' => { out.push_str("&amp;"); i += 1; continue; }
            '<' => { out.push_str("&lt;");  i += 1; continue; }
            '>' => { out.push_str("&gt;");  i += 1; continue; }
            _ => {}
        }

        // Inline code: `code`
        if chars[i] == '`' {
            let end = chars[i+1..].iter().position(|&c| c == '`');
            if let Some(rel) = end {
                let code: String = chars[i+1..i+1+rel].iter().collect();
                out.push_str(&format!("<tt>{}</tt>",
                    code.replace('&',"&amp;").replace('<',"&lt;").replace('>',"&gt;")));
                i += rel + 2;
                continue;
            }
        }

        // Bold+italic: ***text***
        if i + 2 < n && chars[i] == '*' && chars[i+1] == '*' && chars[i+2] == '*' {
            let end = text[i+3..].find("***");
            if let Some(rel) = end {
                let inner = inline_to_pango(&text[i+3..i+3+rel]);
                out.push_str(&format!("<b><i>{inner}</i></b>"));
                i += rel + 6;
                continue;
            }
        }

        // Bold: **text**
        if i + 1 < n && chars[i] == '*' && chars[i+1] == '*' {
            let end = text[i+2..].find("**");
            if let Some(rel) = end {
                let inner = inline_to_pango(&text[i+2..i+2+rel]);
                out.push_str(&format!("<b>{inner}</b>"));
                i += rel + 4;
                continue;
            }
        }

        // Italic: *text* or _text_
        if (chars[i] == '*' || chars[i] == '_') && (i == 0 || chars[i-1] == ' ') {
            let delim = chars[i];
            let end = chars[i+1..].iter().position(|&c| c == delim);
            if let Some(rel) = end {
                let inner = inline_to_pango(&text[i+1..i+1+rel]);
                out.push_str(&format!("<i>{inner}</i>"));
                i += rel + 2;
                continue;
            }
        }

        // Strikethrough: ~~text~~
        if i + 1 < n && chars[i] == '~' && chars[i+1] == '~' {
            let end = text[i+2..].find("~~");
            if let Some(rel) = end {
                let inner = inline_to_pango(&text[i+2..i+2+rel]);
                out.push_str(&format!("<s>{inner}</s>"));
                i += rel + 4;
                continue;
            }
        }

        out.push(chars[i]);
        i += 1;
    }
    out
}

// ── Render to GTK4 widget stack ───────────────────────────────────────────────

/// Render markdown text into a GtkBox of mixed widgets.
/// The returned widget can replace a plain Label in a chat bubble.
pub fn render(md: &str) -> GtkBox {
    let vbox = GtkBox::new(Orientation::Vertical, 4);
    vbox.set_hexpand(true);

    let blocks = parse_blocks(md);

    for block in blocks {
        match block {
            Block::Heading { level, text } => {
                let markup = format!("{}<b>{}</b>",
                    match level { 1 => "<span size='x-large'>", 2 => "<span size='large'>",
                                  _ => "<span size='medium'>" },
                    inline_to_pango(&text)
                ) + "</span>";
                let lbl = Label::builder()
                    .use_markup(true).label(&markup)
                    .halign(gtk4::Align::Start).wrap(true)
                    .selectable(true).build();
                lbl.add_css_class("chat-md-heading");
                vbox.append(&lbl);
            }

            Block::Para(text) => {
                let markup = inline_to_pango(&text);
                let lbl = Label::builder()
                    .use_markup(true).label(&markup)
                    .halign(gtk4::Align::Start).wrap(true)
                    .wrap_mode(gtk4::pango::WrapMode::WordChar)
                    .selectable(true).hexpand(true).build();
                lbl.add_css_class("chat-bubble-text");
                vbox.append(&lbl);
            }

            Block::Code { lang, text } => {
                let code_card = GtkBox::new(Orientation::Vertical, 0);
                code_card.add_css_class("chat-code-block");

                // Language badge + copy button header
                let code_hdr = GtkBox::new(Orientation::Horizontal, 0);
                code_hdr.add_css_class("chat-code-header");
                let lang_lbl = Label::builder()
                    .label(if lang.is_empty() { "code" } else { &lang })
                    .halign(gtk4::Align::Start).hexpand(true).build();
                lang_lbl.add_css_class("chat-code-lang");
                let copy_btn = Button::with_label("Copy");
                copy_btn.add_css_class("flat"); copy_btn.add_css_class("chat-code-copy");

                let text_for_copy = text.clone();
                copy_btn.connect_clicked(move |btn| {
                    if let Some(display) = gtk4::gdk::Display::default() {
                        display.clipboard().set_text(&text_for_copy);
                        btn.set_label("Copied ✓");
                        let b = btn.clone();
                        glib::timeout_add_local_once(
                            std::time::Duration::from_secs(2),
                            move || { b.set_label("Copy"); }
                        );
                    }
                });

                code_hdr.append(&lang_lbl);
                code_hdr.append(&copy_btn);
                code_card.append(&code_hdr);

                // Code text
                let buf = TextBuffer::new(None);
                buf.set_text(text.trim_end_matches('\n'));
                let tv = TextView::builder()
                    .buffer(&buf).editable(false).monospace(true)
                    .wrap_mode(WrapMode::None).build();
                tv.add_css_class("chat-code-view");

                let sw = ScrolledWindow::builder()
                    .child(&tv)
                    .hscrollbar_policy(PolicyType::Automatic)
                    .vscrollbar_policy(PolicyType::Never)
                    .max_content_height(320)
                    .build();
                code_card.append(&sw);
                vbox.append(&code_card);
            }

            Block::Blockquote(text) => {
                let row = GtkBox::new(Orientation::Horizontal, 8);
                let bar = GtkBox::new(Orientation::Vertical, 0);
                bar.add_css_class("chat-bq-bar");
                bar.set_size_request(3, -1);
                let lbl = Label::builder()
                    .use_markup(true).label(&inline_to_pango(&text))
                    .halign(gtk4::Align::Start).wrap(true)
                    .selectable(true).hexpand(true).build();
                lbl.add_css_class("chat-bq-text");
                row.append(&bar);
                row.append(&lbl);
                row.add_css_class("chat-bq-row");
                vbox.append(&row);
            }

            Block::ListItem { ordered, index, text } => {
                let row = GtkBox::new(Orientation::Horizontal, 6);
                let bullet = if ordered {
                    Label::new(Some(&format!("{index}.")))
                } else {
                    Label::new(Some("•"))
                };
                bullet.add_css_class("chat-list-bullet");
                bullet.set_valign(gtk4::Align::Start);
                let lbl = Label::builder()
                    .use_markup(true).label(&inline_to_pango(&text))
                    .halign(gtk4::Align::Start).wrap(true)
                    .selectable(true).hexpand(true).build();
                lbl.add_css_class("chat-bubble-text");
                row.set_margin_start(8);
                row.append(&bullet);
                row.append(&lbl);
                vbox.append(&row);
            }

            Block::HRule => {
                let sep = gtk4::Separator::new(Orientation::Horizontal);
                sep.set_margin_top(4); sep.set_margin_bottom(4);
                vbox.append(&sep);
            }
        }
    }

    if vbox.first_child().is_none() {
        // Empty — at least show something
        let lbl = Label::builder().label("").halign(gtk4::Align::Start).build();
        vbox.append(&lbl);
    }

    vbox
}

/// Returns true if the text looks like it has meaningful markdown formatting.
/// Plain conversational text shouldn't trigger the markdown renderer.
pub fn has_markdown(text: &str) -> bool {
    text.contains("```")
        || text.contains("**")
        || text.contains("# ")
        || text.contains("\n- ")
        || text.contains("\n* ")
        || text.contains("\n> ")
        || text.contains("> ")
        || text.contains("~~")
        || (text.contains('`') && text.matches('`').count() >= 2)
}
