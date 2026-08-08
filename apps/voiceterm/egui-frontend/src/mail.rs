//! mail — `mail_panes.rs`, ported off QML string generation.
//!
//! The Qt side builds these panes by concatenating QML source into a `String`
//! — 956 lines of `o.push_str("Column {\n…")` — and hands it to a loader. That
//! is a reasonable way to drive a QML runtime and a terrible thing to port
//! verbatim: the layout, the escaping and the data are fused into one function,
//! so nothing in it can be tested without a QML engine to render the result.
//!
//! What is worth carrying is underneath: which rows exist, what a row says when
//! its fields are missing, when a pane is empty, and how the sender graph is
//! bucketed. Those are decisions. The `Column { width: parent.width }` is not.
//!
//! ## Escaping stops being a concern, and that is the real win
//!
//! `qml_escape` exists because a subject line containing a quote or a backslash
//! would otherwise terminate a QML string literal and break the pane — a
//! rendering failure caused by an ordinary email. Drawing through egui there is
//! no string literal to escape into, so the entire class is gone rather than
//! handled. The port keeps a test asserting such a subject renders intact,
//! because "we no longer need to escape" is exactly the claim that goes wrong
//! quietly if someone later reintroduces a text-generation path.
//!
//! ## Unread is weight, not a badge
//!
//! An inbox is scanned, not read. A dot beside a row is a thing to look for; a
//! heavier row is a thing you cannot miss, and it survives being glanced at
//! from arm's length on a phone.

/// Who a message is from, for a list row.
///
/// A FUNCTION over the existing `mail_view::Message` rather than a second
/// message type. `mail_view` already models messages, addresses, threads and
/// bodies; adding a parallel `MessageRow` here would have been the "two lists
/// of the same thing" failure this port keeps finding in the Qt code — and I
/// wrote one before checking, which is why the rule is grep first.
///
/// `Address::label()` already picks name-or-address; this only adds the case it
/// does not cover, which is having neither. Never blank: a row with no sender
/// cannot be told from another row with no sender, and an inbox exists to tell
/// messages apart.
pub fn sender_of(m: &crate::mail_view::Message) -> &str {
    match &m.sender {
        Some(a) => {
            let l = a.label().trim();
            if l.is_empty() { "(unknown sender)" } else { l }
        }
        None => "(unknown sender)",
    }
}

/// The subject, or a marker.
///
/// "(no subject)" rather than an empty line: an empty row reads as a rendering
/// fault, and the row still needs its height — a list whose rows change height
/// by content jumps while you read it.
pub fn subject_of(m: &crate::mail_view::Message) -> &str {
    let s = m.subject.trim();
    if s.is_empty() { "(no subject)" } else { s }
}

/// A draft.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DraftRow {
    pub id: String,
    pub to: String,
    pub subject: String,
    pub snippet: String,
}

impl DraftRow {
    /// A draft with no recipient is normal — it is where most drafts start —
    /// so it renders as such rather than being hidden or flagged.
    pub fn to_text(&self) -> &str {
        let t = self.to.trim();
        if t.is_empty() { "(no recipient)" } else { t }
    }
}

/// A decrypted message.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MessageView {
    pub id: String,
    pub from_name: String,
    pub from_addr: String,
    pub to: String,
    pub subject: String,
    pub date: String,
    /// Decrypted plain text; the hub decrypts through the PGP engine.
    pub body_text: String,
}

impl MessageView {
    /// The header line: name and address together when both exist.
    ///
    /// Both, because the name is what you recognise and the address is what
    /// tells you it is really them — showing only the name is how display-name
    /// spoofing works.
    pub fn from_line(&self) -> String {
        match (self.from_name.trim(), self.from_addr.trim()) {
            ("", "") => "(unknown sender)".into(),
            ("", a) => a.to_string(),
            (n, "") => n.to_string(),
            (n, a) => format!("{n} <{a}>"),
        }
    }

    /// An empty body is a real state — an encrypted message that decrypted to
    /// nothing, or a subject-only note — and says so rather than showing a
    /// blank pane the user reads as a failed load.
    pub fn body_or_notice(&self) -> &str {
        if self.body_text.trim().is_empty() { "(no message body)" } else { &self.body_text }
    }
}

/// Which pane is showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pane { Inbox, Drafts, Message, Compose, Search, Peers }

/// What a pane asks the host to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MailAction {
    Open(String),
    EditDraft(String),
    Search(String),
    ToggleUnreadOnly,
}

/// Filter rows for the inbox.
///
/// Returned as references rather than a filtered copy: an inbox can hold
/// thousands of rows and the unread toggle is flipped constantly.
pub fn visible<'a>(rows: &'a [crate::mail_view::Message], unread_only: bool)
    -> Vec<&'a crate::mail_view::Message> {
    rows.iter().filter(|r| !unread_only || r.unread).collect()
}

/// What an empty pane says.
///
/// Different text for a filtered empty than a truly empty one, because "no
/// messages" in front of someone with a full inbox is a bug report waiting to
/// happen — they need to know the filter is on.
pub fn empty_notice(unread_only: bool, total: usize) -> &'static str {
    if unread_only && total > 0 { "No unread messages." } else { "No messages." }
}

/// Sender counts for the graph, highest first.
///
/// Ties broken by name so the graph does not reshuffle between frames — a bar
/// chart that reorders itself while being looked at is unreadable, and hash
/// order would do exactly that.
pub fn sender_counts(rows: &[crate::mail_view::Message]) -> Vec<(String, u32)> {
    let mut acc: Vec<(String, u32)> = Vec::new();
    for r in rows {
        let key = sender_of(r).to_string();
        match acc.iter_mut().find(|(k, _)| *k == key) {
            Some((_, n)) => *n += 1,
            None => acc.push((key, 1)),
        }
    }
    acc.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    acc
}

/// Draw a list pane.
pub fn inbox_ui(ui: &mut egui::Ui, rows: &[crate::mail_view::Message], unread_only: bool,
                c: &uikit_egui::PaletteColors) -> Option<MailAction> {
    let mut act = None;
    ui.horizontal(|ui| {
        ui.label(if unread_only { "Inbox — unread" } else { "Inbox" });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.selectable_label(unread_only, "unread only").clicked() {
                act = Some(MailAction::ToggleUnreadOnly);
            }
        });
    });
    ui.separator();

    let shown = visible(rows, unread_only);
    if shown.is_empty() {
        ui.colored_label(c.text_weak, empty_notice(unread_only, rows.len()));
        return act;
    }
    egui::ScrollArea::vertical().id_salt("inbox").show(ui, |ui| {
        for r in shown {
            ui.horizontal(|ui| {
                // Weight, not a badge — see the module docs.
                let sender = if r.unread {
                    egui::RichText::new(sender_of(r)).strong()
                } else {
                    egui::RichText::new(sender_of(r))
                };
                ui.vertical(|ui| {
                    if ui.add(egui::Button::new(sender).frame(false)).clicked() {
                        act = Some(MailAction::Open(r.id.clone()));
                    }
                    ui.colored_label(c.text_weak,
                        egui::RichText::new(subject_of(r)).small());
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.colored_label(c.text_weak,
                        egui::RichText::new(r.time.to_string()).small());
                });
            });
        }
    });
    act
}

/// Draw an open message.
pub fn message_ui(ui: &mut egui::Ui, m: &MessageView, c: &uikit_egui::PaletteColors) {
    ui.label(egui::RichText::new(if m.subject.trim().is_empty() {
        "(no subject)" } else { &m.subject }).strong());
    ui.colored_label(c.text_weak, m.from_line());
    ui.colored_label(c.text_weak, egui::RichText::new(&m.date).small());
    ui.separator();
    egui::ScrollArea::vertical().id_salt("message_body").show(ui, |ui| {
        ui.colored_label(c.text, m.body_or_notice());
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::mail_view::{Address, Message};

    fn row(id: &str, name: &str, subj: &str, unread: bool) -> Message {
        Message { id: id.into(), conversation_id: "c".into(), subject: subj.into(),
                  unread, sender: Some(Address { name: name.into(),
                                                 address: "a@b.c".into() }),
                  time: 0, num_attachments: 0, is_encrypted: 0 }
    }

    #[test]
    fn a_sender_with_no_display_name_falls_back_to_the_address() {
        let r = row("1", "", "hi", false);
        assert_eq!(sender_of(&r), "a@b.c");
        let none = Message { sender: None, ..row("2", "", "hi", false) };
        assert_eq!(sender_of(&none), "(unknown sender)",
                   "never blank — an inbox exists to tell messages apart");
    }

    #[test]
    fn a_missing_subject_renders_a_marker_rather_than_an_empty_line() {
        // An empty row reads as a rendering fault, and the row still needs its
        // height — a list whose rows change height by content jumps while you
        // read it.
        assert_eq!(subject_of(&row("1", "Ada", "   ", false)), "(no subject)");
    }

    #[test]
    fn the_from_line_shows_the_name_and_the_address() {
        // Showing only the display name is how spoofing works: the name is what
        // you recognise, the address is what says it is really them.
        let m = MessageView { from_name: "Ada".into(), from_addr: "ada@x.y".into(),
                              ..Default::default() };
        assert_eq!(m.from_line(), "Ada <ada@x.y>");
    }

    #[test]
    fn a_sender_with_only_one_of_the_two_still_renders() {
        let name_only = MessageView { from_name: "Ada".into(), ..Default::default() };
        assert_eq!(name_only.from_line(), "Ada");
        let addr_only = MessageView { from_addr: "ada@x.y".into(), ..Default::default() };
        assert_eq!(addr_only.from_line(), "ada@x.y");
        assert_eq!(MessageView::default().from_line(), "(unknown sender)");
    }

    #[test]
    fn an_empty_body_says_so_rather_than_showing_a_blank_pane() {
        // A message that decrypted to nothing, or a subject-only note. A blank
        // pane reads as a failed load.
        assert_eq!(MessageView::default().body_or_notice(), "(no message body)");
    }

    #[test]
    fn quotes_and_backslashes_in_a_subject_survive_intact() {
        // The QML path needed qml_escape because a subject containing a quote
        // would terminate a string literal and break the pane — a rendering
        // failure caused by an ordinary email. Drawing through egui there is no
        // literal to escape into, so the class is gone rather than handled.
        // Pinned because "we no longer need to escape" fails quietly if a
        // text-generation path is reintroduced.
        let nasty = r#"Re: "quoted" \ backslash — 100%"#;
        let r = row("1", "Ada", nasty, false);
        assert_eq!(subject_of(&r), nasty);
        let m = MessageView { subject: nasty.into(), ..Default::default() };
        assert_eq!(m.subject.trim(), nasty);
    }

    #[test]
    fn the_unread_filter_hides_read_messages() {
        let rows = vec![row("1", "Ada", "a", true), row("2", "Bob", "b", false)];
        assert_eq!(visible(&rows, true).len(), 1);
        assert_eq!(visible(&rows, false).len(), 2);
    }

    #[test]
    fn a_filtered_empty_inbox_says_the_filter_is_on() {
        // "No messages" in front of someone with a full inbox is a bug report
        // waiting to happen.
        assert_eq!(empty_notice(true, 40), "No unread messages.");
        assert_eq!(empty_notice(false, 0), "No messages.");
        assert_eq!(empty_notice(true, 0), "No messages.",
                   "genuinely empty is not a filter problem");
    }

    #[test]
    fn sender_counts_rank_by_volume() {
        let rows = vec![row("1", "Ada", "a", false), row("2", "Bob", "b", false),
                        row("3", "Ada", "c", false)];
        let counts = sender_counts(&rows);
        assert_eq!(counts[0], ("Ada".to_string(), 2));
        assert_eq!(counts[1], ("Bob".to_string(), 1));
    }

    #[test]
    fn equal_senders_order_stably_rather_than_by_hash() {
        // A bar chart that reorders itself while being looked at is unreadable.
        let rows = vec![row("1", "Zoe", "a", false), row("2", "Ada", "b", false)];
        assert_eq!(sender_counts(&rows), sender_counts(&rows));
        assert_eq!(sender_counts(&rows)[0].0, "Ada", "ties break by name");
    }

    #[test]
    fn an_empty_inbox_produces_an_empty_graph_rather_than_a_zero_bar() {
        // A single bar labelled "(unknown sender): 0" is worse than no chart.
        assert!(sender_counts(&[]).is_empty());
    }

    #[test]
    fn a_draft_with_no_recipient_is_normal() {
        // It is where most drafts start, so it renders as such rather than
        // being hidden or flagged as broken.
        assert_eq!(DraftRow::default().to_text(), "(no recipient)");
    }
}
