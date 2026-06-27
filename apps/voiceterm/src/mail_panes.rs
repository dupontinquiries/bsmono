//! QML pane generation for the mail bridge: inbox list, drafts list, single
//! message view, compose, and search results — plus niftyplot source
//! generators so panes can render a sender-relationship graph or a
//! message-volume plot natively through w2q's existing niftyplot canvas.
//!
//! v0.1.862: These functions emit self-contained QML `Item` trees in the
//! same idiom the rest of w2q uses — children laid out by a `Column`, links
//! and buttons calling `if (backend) backend.<method>(...)`. The backend
//! method names referenced here (`mail_open_message`, `mail_compose`,
//! `mail_send_draft`, `mail_open_inbox`, `niftyplot_source_clicked`) are the
//! integration seam: wire them as `qt_method!`s on the main Backend that call
//! through to [`crate::mail::MailHub`].
//!
//! niftyplot support: [`sender_graph_niftyplot`] turns a set of inbox
//! headers into a `%kind: graph` source string (senders → you, edge labels =
//! message counts) that the existing `backend.niftyplot_to_layout(source)`
//! renders on the cards canvas. The inbox pane embeds that source and a
//! "View as niftyplot" affordance, mirroring how fenced code blocks expose
//! the same canvas.

/// The styling a pane needs. A trimmed projection of
/// [`crate::qml_gen::RenderStyle`] so the pane builders are pure and unit-
/// testable without constructing the full render style (which depends on the
/// HTML/theme machinery).
#[derive(Clone, Debug)]
pub struct PaneStyle {
    pub bg: String,
    pub fg: String,
    pub muted: String,
    pub accent: String,
    pub link: String,
    pub border: String,
    pub button_bg: String,
    pub button_fg: String,
    pub font_family: String,
    pub font_px: i32,
}

impl PaneStyle {
    /// Adapt from the project's render style. Reads only the fields the
    /// panes use; everything else in `RenderStyle` is ignored.
    pub fn from_render_style(rs: &crate::qml_gen::RenderStyle) -> Self {
        PaneStyle {
            bg: rs.input_bg.clone(),
            fg: rs.text_color.clone(),
            muted: rs.border_color.clone(),
            accent: rs.link_color.clone(),  // w2q RenderStyle has no selection_accent; link_color is the accent
            link: rs.link_color.clone(),
            border: rs.border_color.clone(),
            button_bg: rs.button_bg.clone(),
            button_fg: rs.button_fg.clone(),
            font_family: rs.font_family.clone(),
            font_px: rs.font_size,
        }
    }

    /// A dark default, handy for standalone use and tests.
    pub fn dark_default() -> Self {
        PaneStyle {
            bg: "#1e1e1e".into(),
            fg: "#e0e0e0".into(),
            muted: "#3a3a3a".into(),
            accent: "#ffaa44".into(),
            link: "#5aa0ff".into(),
            border: "#3a3a3a".into(),
            button_bg: "#2d2d2d".into(),
            button_fg: "#e0e0e0".into(),
            font_family: "sans-serif".into(),
            font_px: 14,
        }
    }
}

// ---------------------------------------------------------------------
// Row/view data the hub fills in from `mail_proton` models
// ---------------------------------------------------------------------

#[derive(Clone, Debug, Default)]
pub struct MessageRow {
    pub id: String,
    pub from_name: String,
    pub from_addr: String,
    pub subject: String,
    pub date: String,
    pub unread: bool,
}

#[derive(Clone, Debug, Default)]
pub struct DraftRow {
    pub id: String,
    pub to: String,
    pub subject: String,
    pub snippet: String,
}

#[derive(Clone, Debug, Default)]
pub struct MessageView {
    pub id: String,
    pub from_name: String,
    pub from_addr: String,
    pub to: String,
    pub subject: String,
    pub date: String,
    /// Decrypted, plain-text body (the hub decrypts via the PGP engine).
    pub body_text: String,
}

// ---------------------------------------------------------------------
// QML panes
// ---------------------------------------------------------------------

/// Inbox list pane. Embeds a niftyplot sender-graph at the top (collapsible)
/// and a tappable list of message rows below.
pub fn inbox_pane_qml(style: &PaneStyle, rows: &[MessageRow], unread_only: bool) -> String {
    let mut o = String::with_capacity(rows.len() * 256 + 1024);
    pane_header(&mut o, style, if unread_only { "Inbox — unread" } else { "Inbox" });

    // niftyplot sender graph affordance.
    let graph_src = sender_graph_niftyplot(rows);
    push_niftyplot_affordance(&mut o, style, &graph_src, "View senders as niftyplot graph");

    o.push_str("Column {\nwidth: parent.width\nspacing: 0\n");
    if rows.is_empty() {
        push_empty_notice(&mut o, style, "No messages.");
    }
    for r in rows {
        push_message_row(&mut o, style, r);
    }
    o.push_str("}\n"); // inner Column
    o.push_str("}\n}\n"); // close outer Column + root Item
    o
}

/// Drafts list pane: each row opens the compose pane pre-filled, with a
/// "Send" affordance.
pub fn draft_pane_qml(style: &PaneStyle, drafts: &[DraftRow]) -> String {
    let mut o = String::with_capacity(drafts.len() * 256 + 512);
    pane_header(&mut o, style, "Drafts");
    o.push_str("Column {\nwidth: parent.width\nspacing: 6\n");
    if drafts.is_empty() {
        push_empty_notice(&mut o, style, "No drafts.");
    }
    for d in drafts {
        let _ = std::fmt::Write::write_fmt(
            &mut o,
            format_args!(
                "Rectangle {{\nwidth: parent.width\nheight: childrenRect.height + 12\ncolor: \"transparent\"\nborder.color: \"{border}\"\nborder.width: 1\nradius: 4\nColumn {{\nx: 8; y: 6\nwidth: parent.width - 16\nspacing: 2\nText {{ text: \"{subj}\"; color: \"{fg}\"; font.bold: true; font.pixelSize: {fs}; font.family: \"{ff}\"; elide: Text.ElideRight; width: parent.width }}\nText {{ text: \"To: {to}\"; color: \"{muted}\"; font.pixelSize: {fss}; font.family: \"{ff}\" }}\nText {{ text: \"{snip}\"; color: \"{muted}\"; font.pixelSize: {fss}; font.family: \"{ff}\"; elide: Text.ElideRight; width: parent.width; maximumLineCount: 2; wrapMode: Text.WordWrap }}\nRow {{\nspacing: 8\n",
                border = q(&style.border),
                subj = q(&blank_if_empty(&d.subject, "(no subject)")),
                fg = q(&style.fg),
                fs = style.font_px,
                fss = style.font_px - 2,
                ff = q(&style.font_family),
                to = q(&d.to),
                muted = q(&style.muted),
                snip = q(&d.snippet),
            ),
        );
        push_button(&mut o, style, "Edit", &format!("if (backend) backend.mail_open_draft(\"{}\")", q(&d.id)));
        push_button(&mut o, style, "Send", &format!("if (backend) backend.mail_send_draft(\"{}\")", q(&d.id)));
        o.push_str("}\n}\n}\n"); // Row, Column, Rectangle
    }
    o.push_str("}\n}\n}\n"); // list Column + header Column + root Item
    o
}

/// Single message view pane.
pub fn message_pane_qml(style: &PaneStyle, m: &MessageView) -> String {
    let mut o = String::with_capacity(m.body_text.len() + 1024);
    pane_header(&mut o, style, &blank_if_empty(&m.subject, "(no subject)"));
    let _ = std::fmt::Write::write_fmt(
        &mut o,
        format_args!(
            "Column {{\nwidth: parent.width\nspacing: 4\nText {{ text: \"From: {fname} <{faddr}>\"; color: \"{muted}\"; font.pixelSize: {fss}; font.family: \"{ff}\"; width: parent.width; elide: Text.ElideRight }}\nText {{ text: \"To: {to}\"; color: \"{muted}\"; font.pixelSize: {fss}; font.family: \"{ff}\"; width: parent.width; elide: Text.ElideRight }}\nText {{ text: \"{date}\"; color: \"{muted}\"; font.pixelSize: {fss}; font.family: \"{ff}\" }}\nRectangle {{ width: parent.width; height: 1; color: \"{border}\" }}\nText {{ text: \"{body}\"; color: \"{fg}\"; font.pixelSize: {fs}; font.family: \"{ff}\"; width: parent.width; wrapMode: Text.WordWrap; textFormat: Text.PlainText }}\nRow {{ spacing: 8\n",
            fname = q(&m.from_name),
            faddr = q(&m.from_addr),
            muted = q(&style.muted),
            fss = style.font_px - 2,
            ff = q(&style.font_family),
            to = q(&m.to),
            date = q(&m.date),
            border = q(&style.border),
            body = q(&m.body_text),
            fg = q(&style.fg),
            fs = style.font_px,
        ),
    );
    push_button(&mut o, style, "Reply", &format!("if (backend) backend.mail_reply(\"{}\")", q(&m.id)));
    push_button(&mut o, style, "Forward", &format!("if (backend) backend.mail_forward(\"{}\")", q(&m.id)));
    o.push_str("}\n}\n}\n}\n"); // Row + body Column + header Column + root Item
    o
}

/// Compose pane: a minimal To/Subject/Body form with a Send button.
pub fn compose_pane_qml(
    style: &PaneStyle,
    to: Option<&str>,
    subject: Option<&str>,
    body: Option<&str>,
) -> String {
    let mut o = String::with_capacity(896);
    pane_header(&mut o, style, "Compose");
    let _ = std::fmt::Write::write_fmt(
        &mut o,
        format_args!(
            "Column {{\nwidth: parent.width\nspacing: 6\nTextField {{ objectName: \"mail_to\"; width: parent.width; placeholderText: \"To\"; text: \"{to}\"; color: \"{fg}\"; font.family: \"{ff}\"; font.pixelSize: {fs} }}\nTextField {{ objectName: \"mail_subject\"; width: parent.width; placeholderText: \"Subject\"; text: \"{subject}\"; color: \"{fg}\"; font.family: \"{ff}\"; font.pixelSize: {fs} }}\nTextArea {{ objectName: \"mail_body\"; width: parent.width; height: 240; placeholderText: \"Message\"; text: \"{body}\"; wrapMode: TextArea.Wrap; color: \"{fg}\"; font.family: \"{ff}\"; font.pixelSize: {fs} }}\nRow {{ spacing: 8\n",
            to = q(to.unwrap_or("")),
            subject = q(subject.unwrap_or("")),
            body = q(body.unwrap_or("")),
            fg = q(&style.fg),
            ff = q(&style.font_family),
            fs = style.font_px,
        ),
    );
    // Send/Save capture the backend's `status\tdetail` wire and show it
    // on the status line (same pattern as the login form), so the user
    // sees "Draft saved" or the honest "sending not available" note
    // rather than a silent no-op.
    push_button(
        &mut o,
        style,
        "Send",
        "if (backend) _setStatus(backend.mail_send(_root_to(), _root_subject(), _root_body()))",
    );
    push_button(
        &mut o,
        style,
        "Save draft",
        "if (backend) _setStatus(backend.mail_save_draft(_root_to(), _root_subject(), _root_body()))",
    );
    o.push_str("}\n");
    // Status line + field readers (kept inline so the snippet is
    // self-contained when injected into a Loader).
    let _ = std::fmt::Write::write_fmt(
        &mut o,
        format_args!(
            "Text {{ id: _status; objectName: \"compose_status\"; width: parent.width; text: \"\"; visible: text.length > 0; wrapMode: Text.WordWrap; color: \"{fg}\"; font.family: \"{ff}\"; font.pixelSize: {fs} }}\n",
            fg = q(&style.fg),
            ff = q(&style.font_family),
            fs = style.font_px,
        ),
    );
    o.push_str(
        "function _setStatus(r) { if (!r) { _status.text=\"\"; return } var p=r.split(\"\\t\"); if (p[0]===\"ok\") { _status.color=\"#86c986\"; _status.text=(p[1]||\"Done\") } else { _status.color=\"#e0928f\"; _status.text=(p[1]||\"Failed\") } }\n\
         function _root_to() { return _field(\"mail_to\") }\n\
         function _root_subject() { return _field(\"mail_subject\") }\n\
         function _root_body() { return _field(\"mail_body\") }\n\
         function _field(n) { for (var i=0;i<children.length;i++){ var c=children[i]; if(c.objectName===n) return c.text||\"\" } return \"\" }\n",
    );
    o.push_str("}\n}\n}\n"); // form Column + header Column + root Item
    o
}

/// Search-results pane: a header plus message rows (already resolved by the
/// hub) and the original query echoed.
pub fn search_pane_qml(style: &PaneStyle, query: &str, rows: &[MessageRow]) -> String {
    let mut o = String::with_capacity(rows.len() * 256 + 512);
    pane_header(&mut o, style, &format!("Search: {}", query));
    o.push_str("Column {\nwidth: parent.width\nspacing: 0\n");
    if rows.is_empty() {
        push_empty_notice(&mut o, style, "No matching messages.");
    }
    for r in rows {
        push_message_row(&mut o, style, r);
    }
    o.push_str("}\n}\n}\n"); // list Column + header Column + root Item
    o
}

// ---------------------------------------------------------------------
// niftyplot source generation
// ---------------------------------------------------------------------

/// Build a `%kind: graph` niftyplot source describing who's been mailing
/// you. Each distinct sender becomes a node; an edge `sender → You` is
/// labeled with the message count. Unread-heavy senders are drawn with a
/// heavy edge (`=>`). Feed the result to `backend.niftyplot_to_layout`.
pub fn sender_graph_niftyplot(rows: &[MessageRow]) -> String {
    use std::collections::BTreeMap;
    // (display, total, unread) keyed by a sanitized node id.
    let mut agg: BTreeMap<String, (String, u32, u32)> = BTreeMap::new();
    for r in rows {
        let key = nifty_id(if !r.from_addr.is_empty() { &r.from_addr } else { &r.from_name });
        let display = if !r.from_name.is_empty() {
            r.from_name.clone()
        } else {
            r.from_addr.clone()
        };
        let e = agg.entry(key).or_insert((display, 0, 0));
        e.1 += 1;
        if r.unread {
            e.2 += 1;
        }
    }

    let mut s = String::from("%kind: graph\n%title: \"Inbox senders\"\nYou: circle \"You\"\n");
    for (id, (display, total, unread)) in &agg {
        // Node with a count-bearing label.
        let label = if *unread > 0 {
            format!("{display} ({total}, {unread} unread)")
        } else {
            format!("{display} ({total})")
        };
        let _ = std::fmt::Write::write_fmt(
            &mut s,
            format_args!("{id}: \"{}\"\n", nifty_label(&label)),
        );
        // Heavy edge when there's unread mail, else solid; edge label = count.
        let arrow = if *unread > 0 { "=>" } else { "->" };
        let _ = std::fmt::Write::write_fmt(
            &mut s,
            format_args!("{id} {arrow} You : \"{total}\"\n"),
        );
    }
    s
}

/// Build a simple message-volume graph: one node per date bucket chained in
/// order, labeled with the count. A lightweight stand-in for a bar chart
/// using the graph kind the canvas already supports.
pub fn volume_plot_niftyplot(buckets: &[(String, u32)]) -> String {
    let mut s = String::from("%kind: graph\n%title: \"Message volume\"\n");
    let mut prev: Option<String> = None;
    for (i, (bucket, count)) in buckets.iter().enumerate() {
        let id = format!("d{i}");
        let _ = std::fmt::Write::write_fmt(
            &mut s,
            format_args!("{id}: \"{} ({count})\"\n", nifty_label(bucket)),
        );
        if let Some(p) = prev {
            let _ = std::fmt::Write::write_fmt(&mut s, format_args!("{p} -> {id}\n"));
        }
        prev = Some(id);
    }
    s
}

// ---------------------------------------------------------------------
// shared emit helpers
// ---------------------------------------------------------------------

fn pane_header(o: &mut String, style: &PaneStyle, title: &str) {
    // Root Item + a header Text, then an outer Column the callers extend.
    let _ = std::fmt::Write::write_fmt(
        o,
        format_args!(
            "Item {{\nwidth: parent ? parent.width : 600\nimplicitHeight: childrenRect.height\nColumn {{\nwidth: parent.width\nspacing: 8\npadding: 8\nText {{ text: \"{title}\"; color: \"{accent}\"; font.bold: true; font.pixelSize: {fs}; font.family: \"{ff}\" }}\n",
            title = q(title),
            accent = q(&style.accent),
            fs = style.font_px + 3,
            ff = q(&style.font_family),
        ),
    );
}

fn push_message_row(o: &mut String, style: &PaneStyle, r: &MessageRow) {
    let dot = if r.unread { "● " } else { "" };
    let who = if !r.from_name.is_empty() { &r.from_name } else { &r.from_addr };
    let _ = std::fmt::Write::write_fmt(
        o,
        format_args!(
            "Rectangle {{\nwidth: parent.width\nheight: childrenRect.height + 10\ncolor: \"transparent\"\nRectangle {{ anchors.bottom: parent.bottom; width: parent.width; height: 1; color: \"{border}\" }}\nColumn {{\nx: 4; y: 5\nwidth: parent.width - 8\nspacing: 1\nText {{ text: \"{dot}{who}\"; color: \"{fg}\"; font.bold: {bold}; font.pixelSize: {fs}; font.family: \"{ff}\"; elide: Text.ElideRight; width: parent.width }}\nText {{ text: \"{subj}\"; color: \"{fg}\"; font.pixelSize: {fs}; font.family: \"{ff}\"; elide: Text.ElideRight; width: parent.width }}\nText {{ text: \"{date}\"; color: \"{muted}\"; font.pixelSize: {fss}; font.family: \"{ff}\" }}\n}}\nMouseArea {{ anchors.fill: parent; cursorShape: Qt.PointingHandCursor; onClicked: {{ if (backend) backend.mail_open_message(\"{id}\") }} }}\n}}\n",
            border = q(&style.border),
            dot = dot,
            who = q(who),
            fg = q(&style.fg),
            bold = if r.unread { "true" } else { "false" },
            fs = style.font_px,
            ff = q(&style.font_family),
            subj = q(&blank_if_empty(&r.subject, "(no subject)")),
            date = q(&r.date),
            muted = q(&style.muted),
            fss = style.font_px - 3,
            id = q(&r.id),
        ),
    );
}

fn push_button(o: &mut String, style: &PaneStyle, label: &str, on_clicked: &str) {
    let _ = std::fmt::Write::write_fmt(
        o,
        format_args!(
            "Rectangle {{\nradius: 4\ncolor: \"{bg}\"\nborder.color: \"{border}\"\nborder.width: 1\nimplicitWidth: _t_{key}.width + 20\nimplicitHeight: _t_{key}.height + 12\nText {{ id: _t_{key}; anchors.centerIn: parent; text: \"{label}\"; color: \"{fg}\"; font.pixelSize: {fs}; font.family: \"{ff}\" }}\nMouseArea {{ anchors.fill: parent; cursorShape: Qt.PointingHandCursor; onClicked: {{ {onc} }} }}\n}}\n",
            bg = q(&style.button_bg),
            border = q(&style.border),
            key = sanitize_id(label),
            label = q(label),
            fg = q(&style.button_fg),
            fs = style.font_px,
            ff = q(&style.font_family),
            onc = on_clicked,
        ),
    );
}

fn push_niftyplot_affordance(o: &mut String, style: &PaneStyle, source: &str, label: &str) {
    // Stash the niftyplot source in a hidden property and expose a link that
    // hands it to the existing canvas via the backend signal.
    let _ = std::fmt::Write::write_fmt(
        o,
        format_args!(
            "Text {{\ntext: \"{label}\"\ncolor: \"{link}\"\nfont.pixelSize: {fs}\nfont.family: \"{ff}\"\nfont.underline: true\nproperty string niftySource: \"{src}\"\nMouseArea {{ anchors.fill: parent; cursorShape: Qt.PointingHandCursor; onClicked: {{ if (backend) backend.niftyplot_source_clicked(parent.niftySource) }} }}\n}}\n",
            label = q(label),
            link = q(&style.link),
            fs = style.font_px - 1,
            ff = q(&style.font_family),
            src = q(source), // q() escapes the newlines in the DSL into \n
        ),
    );
}

fn push_empty_notice(o: &mut String, style: &PaneStyle, msg: &str) {
    let _ = std::fmt::Write::write_fmt(
        o,
        format_args!(
            "Text {{ text: \"{msg}\"; color: \"{muted}\"; font.pixelSize: {fs}; font.family: \"{ff}\"; padding: 8 }}\n",
            msg = q(msg),
            muted = q(&style.muted),
            fs = style.font_px,
            ff = q(&style.font_family),
        ),
    );
}

// ---------------------------------------------------------------------
// escaping / id sanitation
// ---------------------------------------------------------------------

/// Escape a string for embedding inside a QML double-quoted string literal.
/// Public so sibling pane modules (e.g. `mail_login`) share one escaper.
pub fn qml_escape(s: &str) -> String {
    q(s)
}

/// Escape a string for embedding inside a QML double-quoted string literal.
/// Backslash and quote are escaped; newlines become `\n` (important: the
/// niftyplot source we stash in a property is multi-line).
fn q(s: &str) -> String {
    let mut o = String::with_capacity(s.len() + 8);
    for c in s.chars() {
        match c {
            '\\' => o.push_str("\\\\"),
            '"' => o.push_str("\\\""),
            '\n' => o.push_str("\\n"),
            '\r' => {}
            '\t' => o.push_str("\\t"),
            _ => o.push(c),
        }
    }
    o
}

fn blank_if_empty(s: &str, fallback: &str) -> String {
    if s.trim().is_empty() {
        fallback.to_string()
    } else {
        s.to_string()
    }
}

/// QML id-safe key from an arbitrary label (for object ids like `_t_send`).
fn sanitize_id(s: &str) -> String {
    let mut out: String = s
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '_' })
        .collect();
    if out.is_empty() || out.chars().next().unwrap().is_ascii_digit() {
        out.insert(0, '_');
    }
    out
}

/// niftyplot node id from an email/name: alnum + underscore only, never
/// empty, never starting with a digit.
fn nifty_id(s: &str) -> String {
    let mut out: String = s
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    if out.is_empty() {
        out.push('n');
    }
    if out.chars().next().unwrap().is_ascii_digit() {
        out.insert(0, 'n');
    }
    out
}

/// Sanitize a niftyplot quoted label: strip embedded quotes/newlines that
/// would break the DSL line.
fn nifty_label(s: &str) -> String {
    s.replace('"', "'").replace(['\n', '\r'], " ")
}

// ---------------------------------------------------------------------
// attachments
// ---------------------------------------------------------------------

#[derive(Clone, Debug, Default)]
pub struct AttachmentItem {
    pub id: String,
    pub message_id: String,
    pub name: String,
    pub size: i64,
    pub mime: String,
    /// Sender display, used as the message-node label in the merge graph.
    pub from: String,
}

/// niftyplot graph for a *single* message's attachments: a central message
/// node with one edge to each attachment, labeled name + human size.
pub fn attachment_graph_niftyplot(message_label: &str, items: &[AttachmentItem]) -> String {
    let mut s = String::from("%kind: graph\n%title: \"Attachments\"\n");
    let _ = std::fmt::Write::write_fmt(
        &mut s,
        format_args!("msg: note \"{}\"\n", nifty_label(message_label)),
    );
    for (i, a) in items.iter().enumerate() {
        let id = format!("a{i}");
        let _ = std::fmt::Write::write_fmt(
            &mut s,
            format_args!(
                "{id}: {shape} \"{}\"\nmsg -> {id} : \"{}\"\n",
                nifty_label(&a.name),
                nifty_label(&human_size(a.size)),
                shape = shape_for_mime(&a.mime),
            ),
        );
    }
    s
}

/// The **attachment merge format**: one graph that merges the attachments of
/// many messages. Each message is a node; each *distinct* attachment (keyed
/// by name + size, so the same file shared across a thread collapses to one
/// node) is an attachment node, with an edge from every message that carries
/// it. Duplicates and shared files across a sender/thread become visible at
/// a glance — the same "contract.pdf" forwarded three times shows as a
/// single node with three incoming edges.
pub fn attachments_merge_niftyplot(messages: &[(String, Vec<AttachmentItem>)]) -> String {
    use std::collections::BTreeMap;
    let mut s = String::from("%kind: graph\n%title: \"Attachment merge\"\n");
    let mut att_ids: BTreeMap<(String, i64), String> = BTreeMap::new();
    let mut edges = String::new();

    for (mi, (label, items)) in messages.iter().enumerate() {
        let mnode = format!("m{mi}");
        let _ = std::fmt::Write::write_fmt(
            &mut s,
            format_args!("{mnode}: note \"{}\"\n", nifty_label(label)),
        );
        for a in items {
            let key = (a.name.clone(), a.size);
            let next = att_ids.len();
            let anode = att_ids
                .entry(key)
                .or_insert_with(|| format!("f{next}"))
                .clone();
            let _ = std::fmt::Write::write_fmt(&mut edges, format_args!("{mnode} -> {anode}\n"));
        }
    }
    for ((name, size), id) in &att_ids {
        let _ = std::fmt::Write::write_fmt(
            &mut s,
            format_args!(
                "{id}: cylinder \"{} ({})\"\n",
                nifty_label(name),
                nifty_label(&human_size(*size)),
            ),
        );
    }
    s.push_str(&edges);
    s
}

/// Attachments list pane: a niftyplot affordance plus a tappable row per
/// attachment with Save/Open actions.
pub fn attachments_pane_qml(
    style: &PaneStyle,
    title: &str,
    items: &[AttachmentItem],
    niftyplot_src: &str,
) -> String {
    let mut o = String::with_capacity(items.len() * 200 + 512);
    pane_header(&mut o, style, title);
    push_niftyplot_affordance(&mut o, style, niftyplot_src, "View attachment graph");
    o.push_str("Column {\nwidth: parent.width\nspacing: 6\n");
    if items.is_empty() {
        push_empty_notice(&mut o, style, "No attachments.");
    }
    for a in items {
        let _ = std::fmt::Write::write_fmt(
            &mut o,
            format_args!(
                "Rectangle {{\nwidth: parent.width\nimplicitHeight: _ar_{key}.height + 12\ncolor: \"transparent\"\nborder.color: \"{border}\"\nborder.width: 1\nradius: 4\nRow {{\nid: _ar_{key}\nx: 8; y: 6\nwidth: parent.width - 16\nspacing: 10\nColumn {{\nwidth: parent.width - 150\nText {{ text: \"{name}\"; color: \"{fg}\"; font.pixelSize: {fs}; font.family: \"{ff}\"; elide: Text.ElideRight; width: parent.width }}\nText {{ text: \"{mime} · {size}\"; color: \"{muted}\"; font.pixelSize: {fss}; font.family: \"{ff}\" }}\n}}\n",
                key = sanitize_id(&a.id),
                border = q(&style.border),
                name = q(&blank_if_empty(&a.name, "(unnamed)")),
                fg = q(&style.fg),
                fs = style.font_px,
                ff = q(&style.font_family),
                mime = q(&blank_if_empty(&a.mime, "application/octet-stream")),
                size = q(&human_size(a.size)),
                muted = q(&style.muted),
                fss = style.font_px - 2,
            ),
        );
        push_button(&mut o, style, "Save", &format!("if (backend) backend.mail_save_attachment(\"{}\", \"{}\")", q(&a.message_id), q(&a.id)));
        push_button(&mut o, style, "Open", &format!("if (backend) backend.mail_open_attachment(\"{}\", \"{}\")", q(&a.message_id), q(&a.id)));
        o.push_str("}\n}\n"); // Row, Rectangle
    }
    o.push_str("}\n}\n}\n"); // list Column + header Column + root Item
    o
}

/// Human-readable byte size (e.g. "1.2 MB"). Dependency-free.
pub fn human_size(bytes: i64) -> String {
    if bytes < 0 {
        return "?".into();
    }
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = bytes as f64;
    let mut u = 0;
    while v >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    if u == 0 {
        format!("{} {}", bytes, UNITS[0])
    } else {
        format!("{v:.1} {}", UNITS[u])
    }
}

/// A niftyplot node shape hint per MIME family.
fn shape_for_mime(mime: &str) -> &'static str {
    let m = mime.to_ascii_lowercase();
    if m.starts_with("image/") {
        "circle"
    } else if m.starts_with("text/") || m.contains("pdf") {
        "rect"
    } else if m.contains("zip") || m.contains("tar") || m.contains("compress") {
        "cylinder"
    } else {
        "note"
    }
}

#[cfg(test)]
mod attachment_tests {
    use super::*;

    fn items() -> Vec<AttachmentItem> {
        vec![
            AttachmentItem { id: "a1".into(), message_id: "m1".into(), name: "contract.pdf".into(), size: 2_500_000, mime: "application/pdf".into(), from: "Bob".into() },
            AttachmentItem { id: "a2".into(), message_id: "m1".into(), name: "logo.png".into(), size: 14_336, mime: "image/png".into(), from: "Bob".into() },
        ]
    }

    #[test]
    fn single_message_attachment_graph() {
        let s = attachment_graph_niftyplot("Bob - Lunch", &items());
        assert!(s.starts_with("%kind: graph"));
        assert!(s.contains("msg -> a0"));
        assert!(s.contains("2.4 MB"));
    }

    #[test]
    fn merge_dedups_shared_files() {
        let shared = AttachmentItem { id: "x".into(), message_id: "m2".into(), name: "contract.pdf".into(), size: 2_500_000, mime: "application/pdf".into(), from: "Alice".into() };
        let msgs = vec![
            ("Bob - Lunch".to_string(), items()),
            ("Alice - Fwd".to_string(), vec![shared]),
        ];
        let s = attachments_merge_niftyplot(&msgs);
        let contract_node = s.lines().find(|l| l.contains("contract.pdf")).unwrap();
        let node_id = contract_node.split(':').next().unwrap();
        let incoming = s.matches(&format!("-> {node_id}\n")).count();
        assert_eq!(incoming, 2, "shared file should have 2 incoming edges:\n{s}");
    }

    #[test]
    fn attachments_pane_balanced() {
        let src = attachment_graph_niftyplot("m", &items());
        let qml = attachments_pane_qml(&PaneStyle::dark_default(), "Attachments from Bob", &items(), &src);
        assert!(qml.contains("mail_save_attachment(\"m1\", \"a1\")"));
        assert_eq!(qml.matches('{').count(), qml.matches('}').count(), "{qml}");
    }

    #[test]
    fn human_size_units() {
        assert_eq!(human_size(512), "512 B");
        assert_eq!(human_size(14_336), "14.0 KB");
        assert_eq!(human_size(2_500_000), "2.4 MB");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows() -> Vec<MessageRow> {
        vec![
            MessageRow { id: "m1".into(), from_name: "Bob".into(), from_addr: "bob@proton.me".into(), subject: "Lunch".into(), date: "Mon".into(), unread: true },
            MessageRow { id: "m2".into(), from_name: "Bob".into(), from_addr: "bob@proton.me".into(), subject: "Re: Lunch".into(), date: "Tue".into(), unread: false },
            MessageRow { id: "m3".into(), from_name: "Alice".into(), from_addr: "alice@proton.me".into(), subject: "Invoice".into(), date: "Wed".into(), unread: false },
        ]
    }

    #[test]
    fn inbox_pane_is_nonempty_and_references_ids() {
        let qml = inbox_pane_qml(&PaneStyle::dark_default(), &rows(), false);
        assert!(qml.contains("Inbox"));
        assert!(qml.contains("mail_open_message(\"m1\")"));
        assert!(qml.contains("niftyplot_source_clicked"));
        // balanced-ish: equal number of '{' and '}' (cheap structural sanity)
        assert_eq!(qml.matches('{').count(), qml.matches('}').count(), "unbalanced braces:\n{qml}");
    }

    #[test]
    fn sender_graph_aggregates_counts() {
        let src = sender_graph_niftyplot(&rows());
        assert!(src.starts_with("%kind: graph"));
        assert!(src.contains("You: circle"));
        // Bob has 2 messages, 1 unread → heavy edge with count 2.
        assert!(src.contains("=> You : \"2\""), "src:\n{src}");
        // Alice has 1, none unread → solid edge count 1.
        assert!(src.contains("-> You : \"1\""), "src:\n{src}");
    }

    #[test]
    fn compose_pane_has_fields_and_send() {
        let qml = compose_pane_qml(&PaneStyle::dark_default(), Some("a@b.c"), None, None);
        assert!(qml.contains("objectName: \"mail_to\""));
        assert!(qml.contains("text: \"a@b.c\""));
        assert!(qml.contains("mail_send("));
        assert_eq!(qml.matches('{').count(), qml.matches('}').count());
    }

    #[test]
    fn message_pane_escapes_body() {
        let mut m = MessageView::default();
        m.subject = "Hi \"there\"".into();
        m.body_text = "line1\nline2 with \"quotes\" and \\slash".into();
        let qml = message_pane_qml(&PaneStyle::dark_default(), &m);
        assert!(qml.contains("\\\"there\\\""));
        assert!(qml.contains("line1\\nline2"));
        assert!(!qml.contains("\n\nline2")); // raw newline did not leak into literal
        assert_eq!(qml.matches('{').count(), qml.matches('}').count());
    }

    #[test]
    fn draft_pane_balanced() {
        let drafts = vec![DraftRow { id: "d1".into(), to: "x@y.z".into(), subject: "".into(), snippet: "hello".into() }];
        let qml = draft_pane_qml(&PaneStyle::dark_default(), &drafts);
        assert!(qml.contains("(no subject)"));
        assert!(qml.contains("mail_send_draft(\"d1\")"));
        assert_eq!(qml.matches('{').count(), qml.matches('}').count());
    }

    #[test]
    fn nifty_ids_are_safe() {
        assert_eq!(nifty_id("bob@proton.me"), "bob_proton_me");
        assert_eq!(nifty_id(""), "n");
        assert_eq!(nifty_id("123"), "n123");
    }
}
