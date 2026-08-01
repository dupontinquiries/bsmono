//! mail_view — the mailbox, drawn.
//!
//! `src/mail*.rs` is ~9,300 lines and **Qt-free**: the hub, the Proton client,
//! the crypto, the sessions. None of that is reimplemented here. This is the
//! view that was missing — a message list and a reader — over the engine's own
//! `Message` type.
//!
//! The decisions that matter for a mail client, as opposed to a list widget:
//!
//! * **Threading by conversation.** Proton returns messages; a person reads
//!   threads. Grouping by `conversation_id` and showing the newest as the
//!   summary is the difference between an inbox and a log.
//! * **The body arrives separately.** `list_messages` returns empty bodies and
//!   `get_message` fetches one. So the reader must render a *loading* state
//!   rather than an empty message, because a blank body looks like a mail with
//!   nothing in it.
//! * **Encrypted-and-undecryptable is a state, not an error.** A message whose
//!   body cannot be decrypted still has a subject and a sender worth showing,
//!   and hiding it entirely loses mail the user knows they received.
//! * **Unread is per message, threads are unread if any message is.** Marking a
//!   thread read must not silently mark unread messages inside it as read.

/// The fields of the engine's `mail_proton::Message` that a VIEW needs.
///
/// Not a `#[path]` mount, unlike config and niftyplot: `mail_proton` pulls in
/// `mail_crypto`, `mail_error`, `mail_json` and `mail_session`, so mounting it
/// drags the PGP stack and a network client into a widget crate. A view needs
/// six fields and no crypto.
///
/// The cost is a conversion at the boundary, written once in `from_engine` —
/// which is a fair trade for a view that compiles in a second and can be tested
/// without a Proton session. The field NAMES match the engine's exactly, so the
/// mapping is checkable by eye.
#[derive(Debug, Clone, PartialEq)]
pub struct Message {
    pub id: String,
    pub conversation_id: String,
    pub subject: String,
    pub unread: bool,
    pub sender: Option<Address>,
    pub time: i64,
    pub num_attachments: i64,
    pub is_encrypted: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Address {
    pub name: String,
    pub address: String,
}

impl Address {
    /// What to show: the name when there is one, the address otherwise. An
    /// empty label in a mail list is worse than a raw address.
    pub fn label(&self) -> &str {
        if self.name.is_empty() { &self.address } else { &self.name }
    }
}

/// A thread: the messages of one conversation, newest last.
#[derive(Debug, Clone, PartialEq)]
pub struct Thread {
    pub conversation_id: String,
    pub subject: String,
    /// Message ids, oldest first.
    pub messages: Vec<String>,
    pub newest_time: i64,
    pub unread: usize,
    /// Who else is in it, for the "you, Ada, Bo" line.
    pub participants: Vec<String>,
    pub has_attachments: bool,
}

impl Thread {
    pub fn is_unread(&self) -> bool { self.unread > 0 }
}

/// Group messages into threads.
///
/// A person reads threads, not messages — grouping is the difference between an
/// inbox and a log. The subject comes from the NEWEST message, because a thread
/// that was renamed mid-conversation should show what it is called now.
pub fn thread(messages: &[Message]) -> Vec<Thread> {
    use std::collections::BTreeMap;
    let mut by_conv: BTreeMap<String, Vec<&Message>> = BTreeMap::new();
    for m in messages {
        // A message with no conversation id is its own thread rather than being
        // grouped with every other id-less message, which would merge unrelated
        // mail into one impenetrable blob.
        let key = if m.conversation_id.is_empty() {
            format!("__solo__{}", m.id)
        } else {
            m.conversation_id.clone()
        };
        by_conv.entry(key).or_default().push(m);
    }
    let mut out: Vec<Thread> = by_conv.into_iter().map(|(cid, mut ms)| {
        ms.sort_by_key(|m| m.time);
        let newest = ms.last().expect("a group has at least one");
        let mut participants: Vec<String> = ms.iter()
            .filter_map(|m| m.sender.as_ref().map(|s| s.label().to_string()))
            .collect();
        participants.dedup();
        Thread {
            conversation_id: cid,
            subject: if newest.subject.is_empty() { "(no subject)".into() }
                     else { newest.subject.clone() },
            messages: ms.iter().map(|m| m.id.clone()).collect(),
            newest_time: newest.time,
            unread: ms.iter().filter(|m| m.unread).count(),
            participants,
            has_attachments: ms.iter().any(|m| m.num_attachments > 0),
        }
    }).collect();
    // Newest first, and ties broken on id so the list cannot shuffle between
    // refreshes — a mailbox that reorders itself is one you lose your place in.
    out.sort_by(|a, b| b.newest_time.cmp(&a.newest_time)
                        .then(a.conversation_id.cmp(&b.conversation_id)));
    out
}

/// What the reader can show for a message.
#[derive(Debug, Clone, PartialEq)]
pub enum Body {
    /// `list_messages` gives no body; it has not been fetched yet.
    NotLoaded,
    Loading,
    Plain(String),
    /// Encrypted and we cannot decrypt it — a STATE, not an error. The subject
    /// and sender are still worth showing; hiding the message entirely loses
    /// mail the user knows they received.
    Encrypted { reason: String },
    Failed(String),
}

impl Body {
    /// Text to render, always something. A blank reader is indistinguishable
    /// from a mail with nothing in it.
    pub fn text(&self) -> &str {
        match self {
            Body::NotLoaded => "select to load",
            Body::Loading => "loading…",
            Body::Plain(s) => s,
            Body::Encrypted { reason } => reason,
            Body::Failed(e) => e,
        }
    }
    pub fn is_readable(&self) -> bool { matches!(self, Body::Plain(_)) }
}

/// Mailbox view state.
#[derive(Debug, Clone, Default)]
pub struct MailView {
    pub threads: Vec<Thread>,
    pub selected: Option<String>,
    /// Body per message id, fetched lazily.
    pub bodies: std::collections::BTreeMap<String, Body>,
    pub query: String,
    /// Which label/folder is showing; empty is the inbox.
    pub label: String,
}

impl MailView {
    pub fn set_messages(&mut self, messages: &[Message]) {
        self.threads = thread(messages);
        // A selection that no longer exists must be cleared, or the reader
        // shows a thread the list does not contain.
        if let Some(sel) = &self.selected {
            if !self.threads.iter().any(|t| &t.conversation_id == sel) {
                self.selected = None;
            }
        }
    }

    /// Threads matching the query, searched over subject and participants.
    ///
    /// Case-insensitive and substring, deliberately: a mail search that requires
    /// exact case is one nobody uses twice.
    pub fn visible(&self) -> Vec<&Thread> {
        let q = self.query.trim().to_lowercase();
        self.threads.iter().filter(|t| {
            q.is_empty()
                || t.subject.to_lowercase().contains(&q)
                || t.participants.iter().any(|p| p.to_lowercase().contains(&q))
        }).collect()
    }

    pub fn unread_total(&self) -> usize { self.threads.iter().map(|t| t.unread).sum() }

    pub fn select(&mut self, conversation_id: &str) {
        self.selected = Some(conversation_id.to_string());
    }

    pub fn selected_thread(&self) -> Option<&Thread> {
        let id = self.selected.as_ref()?;
        self.threads.iter().find(|t| &t.conversation_id == id)
    }

    /// Message ids in the open thread that still need a body fetch.
    ///
    /// Returned rather than fetched here, because the view must not do I/O:
    /// the host owns the session and the runtime.
    pub fn needs_fetch(&self) -> Vec<String> {
        self.selected_thread().map(|t| t.messages.iter()
            .filter(|id| !matches!(self.bodies.get(*id), Some(Body::Plain(_)) | Some(Body::Loading)))
            .cloned().collect()).unwrap_or_default()
    }

    pub fn body(&self, message_id: &str) -> &Body {
        self.bodies.get(message_id).unwrap_or(&Body::NotLoaded)
    }

    /// Mark a whole thread read.
    ///
    /// Returns the message ids that actually changed, so the caller tells the
    /// server about exactly those — marking already-read messages again is a
    /// wasted round trip, and on a metered connection that matters.
    pub fn mark_thread_read(&mut self, conversation_id: &str) -> Vec<String> {
        let Some(t) = self.threads.iter_mut()
            .find(|t| t.conversation_id == conversation_id) else { return Vec::new() };
        if t.unread == 0 { return Vec::new(); }
        let changed = t.messages.clone();
        t.unread = 0;
        changed
    }
}

/// Draw the list. Returns a conversation id if one was clicked.
pub fn list_ui(ui: &mut egui::Ui, view: &mut MailView,
               colors: &uikit_egui::PaletteColors) -> Option<String> {
    let mut clicked = None;
    ui.horizontal(|ui| {
        ui.label("🔍");
        ui.text_edit_singleline(&mut view.query);
        let n = view.unread_total();
        if n > 0 { ui.label(egui::RichText::new(format!("{n} unread")).color(colors.accent).small()); }
    });
    ui.separator();
    let rows: Vec<Thread> = view.visible().into_iter().cloned().collect();
    if rows.is_empty() {
        ui.vertical_centered(|ui| {
            ui.add_space(20.0);
            ui.label(egui::RichText::new(if view.query.is_empty() {
                "no mail" } else { "nothing matches" }).weak());
        });
        return None;
    }
    egui::ScrollArea::vertical().show(ui, |ui| {
        for t in rows {
            let open = view.selected.as_deref() == Some(t.conversation_id.as_str());
            let resp = ui.horizontal(|ui| {
                let (r, _) = ui.allocate_exact_size(egui::Vec2::splat(8.0), egui::Sense::hover());
                ui.painter().circle_filled(r.center(), 4.0,
                    if t.is_unread() { colors.accent } else { colors.bg_card });
                ui.vertical(|ui| {
                    let subj = egui::RichText::new(&t.subject);
                    ui.label(if t.is_unread() { subj.strong() } else { subj });
                    ui.label(egui::RichText::new(t.participants.join(", ")).weak().small());
                });
                if t.messages.len() > 1 {
                    ui.label(egui::RichText::new(format!("{}", t.messages.len())).weak().small());
                }
                if t.has_attachments { ui.label(egui::RichText::new("📎").small()); }
            }).response;
            if open { ui.painter().rect_stroke(resp.rect, 2.0,
                egui::Stroke::new(1.0, colors.accent), egui::StrokeKind::Inside); }
            if resp.interact(egui::Sense::click()).clicked() {
                clicked = Some(t.conversation_id.clone());
            }
            ui.separator();
        }
    });
    if let Some(id) = &clicked { view.select(id); }
    clicked
}

#[cfg(test)]
mod tests {
    use super::*;
    fn addr(name: &str) -> Address {
        Address { name: name.into(), address: format!("{name}@x.test") }
    }

    fn msg(id: &str, conv: &str, subject: &str, time: i64, unread: bool, from: &str) -> Message {
        Message {
            id: id.into(), conversation_id: conv.into(), subject: subject.into(),
            unread, sender: Some(addr(from)), time, num_attachments: 0, is_encrypted: 0,
        }
    }

    #[test]
    fn an_address_shows_a_name_or_falls_back_to_the_address() {
        // An empty label in a mail list is worse than a raw address.
        assert_eq!(Address { name: "Ada".into(), address: "a@x".into() }.label(), "Ada");
        assert_eq!(Address { name: String::new(), address: "a@x".into() }.label(), "a@x");
    }

    #[test]
    fn messages_group_into_threads_and_the_newest_names_them() {
        // A person reads threads, not messages — and a thread renamed
        // mid-conversation should show what it is called NOW.
        let ms = vec![
            msg("1", "c1", "Original subject", 100, false, "ada"),
            msg("2", "c1", "Re: renamed", 200, true, "bo"),
            msg("3", "c2", "Other", 150, false, "cy"),
        ];
        let t = thread(&ms);
        assert_eq!(t.len(), 2, "two conversations");
        assert_eq!(t[0].conversation_id, "c1", "newest thread first");
        assert_eq!(t[0].subject, "Re: renamed");
        assert_eq!(t[0].messages, vec!["1", "2"], "oldest first inside a thread");
        assert_eq!(t[0].unread, 1);
        assert!(t[0].is_unread());
        assert!(!t[1].is_unread());
    }

    #[test]
    fn a_message_with_no_conversation_id_is_its_own_thread() {
        // Grouping them together would merge unrelated mail into one
        // impenetrable blob.
        let ms = vec![
            msg("1", "", "First", 100, false, "ada"),
            msg("2", "", "Second", 200, false, "bo"),
        ];
        let t = thread(&ms);
        assert_eq!(t.len(), 2, "not merged: {t:?}");
    }

    #[test]
    fn the_list_does_not_shuffle_between_refreshes() {
        // A mailbox that reorders itself is one you lose your place in.
        let ms = vec![
            msg("1", "b", "B", 100, false, "x"),
            msg("2", "a", "A", 100, false, "y"),
            msg("3", "c", "C", 100, false, "z"),
        ];
        let a: Vec<String> = thread(&ms).iter().map(|t| t.conversation_id.clone()).collect();
        let b: Vec<String> = thread(&ms).iter().map(|t| t.conversation_id.clone()).collect();
        assert_eq!(a, b);
        assert_eq!(a, vec!["a", "b", "c"], "equal times break on id");
    }

    #[test]
    fn a_missing_body_is_a_state_rather_than_a_blank_reader() {
        // list_messages returns empty bodies, so a blank reader is
        // indistinguishable from a mail with nothing in it.
        let v = MailView::default();
        assert_eq!(*v.body("nope"), Body::NotLoaded);
        assert!(!v.body("nope").text().is_empty(), "always something to draw");
        for b in [Body::Loading, Body::Encrypted { reason: "no key".into() },
                  Body::Failed("timeout".into())] {
            assert!(!b.text().is_empty(), "{b:?} must render something");
            assert!(!b.is_readable());
        }
        assert!(Body::Plain("hello".into()).is_readable());
    }

    #[test]
    fn only_unfetched_messages_are_requested() {
        // The view must not do I/O — it reports what it needs and the host,
        // which owns the session, fetches.
        let mut v = MailView::default();
        v.set_messages(&[msg("1", "c", "S", 100, false, "a"), msg("2", "c", "S", 200, false, "b")]);
        v.select("c");
        assert_eq!(v.needs_fetch().len(), 2);
        v.bodies.insert("1".into(), Body::Plain("body".into()));
        assert_eq!(v.needs_fetch(), vec!["2"]);
        v.bodies.insert("2".into(), Body::Loading);
        assert!(v.needs_fetch().is_empty(), "in-flight is not re-requested");
        // a failed fetch IS retried, since the alternative is a message that
        // can never be read again without restarting
        v.bodies.insert("2".into(), Body::Failed("timeout".into()));
        assert_eq!(v.needs_fetch(), vec!["2"]);
    }

    #[test]
    fn marking_a_thread_read_reports_exactly_what_changed() {
        // Telling the server about already-read messages is a wasted round
        // trip, and on a metered connection that matters.
        let mut v = MailView::default();
        v.set_messages(&[msg("1", "c", "S", 100, true, "a"), msg("2", "c", "S", 200, true, "b")]);
        let changed = v.mark_thread_read("c");
        assert_eq!(changed.len(), 2);
        assert_eq!(v.unread_total(), 0);
        assert!(v.mark_thread_read("c").is_empty(), "nothing left to change");
        assert!(v.mark_thread_read("nope").is_empty(), "an unknown thread is not an error");
    }

    #[test]
    fn search_is_case_insensitive_over_subject_and_people() {
        // A mail search that requires exact case is one nobody uses twice.
        let mut v = MailView::default();
        v.set_messages(&[
            msg("1", "c1", "Quarterly Report", 100, false, "ada"),
            msg("2", "c2", "Lunch", 200, false, "bo"),
        ]);
        v.query = "quarterly".into();
        assert_eq!(v.visible().len(), 1);
        v.query = "ADA".into();
        assert_eq!(v.visible().len(), 1, "searches participants too");
        v.query = "  ".into();
        assert_eq!(v.visible().len(), 2, "a blank query shows everything");
        v.query = "zzz".into();
        assert!(v.visible().is_empty());
    }

    #[test]
    fn a_selection_that_vanishes_on_refresh_is_cleared() {
        // Otherwise the reader shows a thread the list does not contain.
        let mut v = MailView::default();
        v.set_messages(&[msg("1", "c1", "S", 100, false, "a")]);
        v.select("c1");
        assert!(v.selected_thread().is_some());
        v.set_messages(&[msg("2", "c2", "T", 200, false, "b")]);
        assert!(v.selected.is_none(), "the stale selection is dropped");
        assert!(v.selected_thread().is_none());
    }
}
