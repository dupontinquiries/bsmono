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

    /// Threads matching a SPOKEN selector — "my last couple emails from bob".
    ///
    /// Separate from `visible()` rather than replacing it, because the two are
    /// different acts: a typed query is a substring search the user refines by
    /// looking, while a spoken selector is a sentence parsed once. Folding the
    /// selector into `visible` would mean typing "from" in the box silently
    /// changed what the box means.
    ///
    /// The count is applied LAST, after filtering: "my last two emails from
    /// bob" means two of bob's, not bob's within the last two overall.
    pub fn selected_by(&self, sel: &crate::mail_selector::Selector) -> Vec<&Thread> {
        let mut out: Vec<&Thread> = self.threads.iter().filter(|t| {
            let from_ok = match &sel.from {
                None => true,
                Some(f) => {
                    let f = f.to_lowercase();
                    t.participants.iter().any(|p| p.to_lowercase().contains(&f))
                }
            };
            let about_ok = match &sel.about {
                None => true,
                Some(a) => t.subject.to_lowercase().contains(&a.to_lowercase()),
            };
            from_ok && about_ok
        }).collect();
        // "all" means no cap; otherwise take the newest `count`. Threads are
        // already in the order set_messages produced, so this is a take rather
        // than a re-sort — re-sorting here would silently disagree with the
        // list the user is looking at.
        if !sel.all && sel.count > 0 && out.len() > sel.count {
            out.truncate(sel.count);
        }
        out
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
        ui.add(egui::TextEdit::singleline(&mut view.query).id_salt("mail_view-view_query"));
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
                egui::Stroke::new(1.0_f32, colors.accent), egui::StrokeKind::Inside); }
            // A FRESH INTERACTION WITH A STABLE, PER-ROW ID.
            //
            // This was `resp.interact(Sense::click())` on the response of
            // `ui.horizontal(..)`. A layout response carries an
            // auto-generated id, and re-interacting through it did not
            // register a clickable region here at all — a frame-driven test
            // clicking the row's own drawn position never produced a
            // selection, so opening a thread by clicking it did nothing for
            // the life of this port. Every model test passed throughout,
            // because none of them drew anything.
            //
            // Keyed by conversation id, not index — `row_click`'s own doc
            // says why.
            if uikit_egui::dock::row_click(ui, resp.rect, &t.conversation_id).clicked() {
                clicked = Some(t.conversation_id.clone());
            }
            ui.separator();
        }
    });
    if let Some(id) = &clicked { view.select(id); }
    clicked
}

/// Tests that drive `list_ui` through a REAL egui frame.
///
/// Everything else in this file tests the model, and the model was never
/// the part that was broken: `set_messages` had no caller for the life of
/// the port, and every model test passed the whole time the list showed
/// nothing. A renderer nobody runs is a renderer nobody has checked, and
/// the same gap in snapclone's swipe work hid two real bugs — a panicking
/// layer order and a double page turn — that no unit test could see.
#[cfg(test)]
mod frame_tests {
    use super::*;
    use egui::{Context, Event, PointerButton, Pos2, RawInput};

    fn view_with(n: usize, unread: usize) -> MailView {
        let msgs: Vec<Message> = (0..n).map(|i| Message {
            id: format!("m{i}"),
            conversation_id: format!("c{i}"),
            subject: format!("subject {i}"),
            unread: i < unread,
            sender: Some(Address { name: format!("sender{i}"), address: "a@b".into() }),
            time: i as i64,
            num_attachments: 0,
            is_encrypted: 0,
        }).collect();
        let mut v = MailView::default();
        v.set_messages(&msgs);
        v
    }

    /// Run one frame and hand back the text shapes it drew, plus anything
    /// `list_ui` returned.
    fn frame(ctx: &Context, view: &mut MailView, events: Vec<Event>)
        -> (Vec<String>, Option<String>) {
        let colors = uikit_egui::palette_colors("voice");
        let mut out = None;
        let done = ctx.run(RawInput {
            events,
            screen_rect: Some(egui::Rect::from_min_size(Pos2::ZERO, egui::vec2(400.0, 800.0))),
            ..Default::default()
        }, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                out = list_ui(ui, view, &colors);
            });
        });
        let texts = done.shapes.iter().filter_map(|s| match &s.shape {
            egui::epaint::Shape::Text(t) => Some(t.galley.text().to_string()),
            _ => None,
        }).collect();
        (texts, out)
    }

    /// Where a piece of drawn text ended up.
    ///
    /// Clicking a guessed coordinate is how a UI test passes on a layout it
    /// never checked, or fails for a reason that has nothing to do with the
    /// behaviour under test. Asking the frame where the row actually is
    /// removes both.
    fn text_pos(ctx: &Context, view: &mut MailView, needle: &str) -> Option<Pos2> {
        let colors = uikit_egui::palette_colors("voice");
        let done = ctx.run(RawInput {
            screen_rect: Some(egui::Rect::from_min_size(Pos2::ZERO, egui::vec2(400.0, 800.0))),
            ..Default::default()
        }, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| { list_ui(ui, view, &colors); });
        });
        done.shapes.iter().find_map(|s| match &s.shape {
            egui::epaint::Shape::Text(t) if t.galley.text().contains(needle) => Some(t.pos),
            _ => None,
        })
    }

    #[test]
    fn an_empty_view_says_so_rather_than_drawing_nothing() {
        // A blank panel and a broken panel look identical, and for most of
        // this port's life the list WAS blank — so the empty state is the
        // one a person was most likely to be looking at.
        let ctx = Context::default();
        let mut v = MailView::default();
        let (texts, out) = frame(&ctx, &mut v, vec![]);
        assert!(texts.iter().any(|t| t.contains("no mail")), "{texts:?}");
        assert_eq!(out, None);
    }

    #[test]
    fn a_query_that_matches_nothing_is_distinguishable_from_an_empty_inbox() {
        // "no mail" when you have typed a filter is a lie about your
        // mailbox. Different words, and the difference only exists in the
        // renderer.
        let ctx = Context::default();
        let mut v = view_with(3, 0);
        v.query = "zzz".into();
        let (texts, _) = frame(&ctx, &mut v, vec![]);
        assert!(texts.iter().any(|t| t.contains("nothing matches")), "{texts:?}");
        assert!(!texts.iter().any(|t| t.contains("no mail")), "{texts:?}");
    }

    #[test]
    fn every_thread_is_drawn_and_the_unread_count_with_them() {
        let ctx = Context::default();
        let mut v = view_with(3, 2);
        let (texts, _) = frame(&ctx, &mut v, vec![]);
        for i in 0..3 {
            assert!(texts.iter().any(|t| t.contains(&format!("subject {i}"))),
                    "subject {i} missing: {texts:?}");
        }
        assert!(texts.iter().any(|t| t.contains("2 unread")), "{texts:?}");
    }

    #[test]
    fn clicking_a_row_selects_that_thread_and_returns_it() {
        // The return value is what the caller opens a thread on. Driving it
        // through a frame is the only way to know a click reaches it —
        // `select` on its own would pass with no rows drawn at all.
        let ctx = Context::default();
        let mut v = view_with(2, 0);
        // First frame lays the rows out; a click needs somewhere to land.
        let (_, none) = frame(&ctx, &mut v, vec![]);
        assert_eq!(none, None);

        // Asked, not guessed — see `text_pos`.
        let p = text_pos(&ctx, &mut v, "subject 0").expect("the row was never drawn");
        let at = Pos2::new(p.x + 4.0, p.y + 4.0);
        // Press and release in SEPARATE frames. egui decides a click from
        // state that spans frames, and collapsing the whole gesture into
        // one is a way to write a passing-looking test that never clicks.
        let btn = |pressed: bool| Event::PointerButton {
            pos: at, button: PointerButton::Primary, pressed,
            modifiers: Default::default() };
        let mut selected = None;
        frame(&ctx, &mut v, vec![Event::PointerMoved(at)]);
        frame(&ctx, &mut v, vec![btn(true)]);
        for _ in 0..3 {
            let (_, out) = frame(&ctx, &mut v, vec![btn(false)]);
            if out.is_some() { selected = out; break; }
        }
        let id = selected.expect(&format!(
            "a click at {at:?} on a row returned nothing"));
        assert!(id.starts_with('c'), "{id}");
        assert_eq!(v.selected.as_deref(), Some(id.as_str()),
                   "the click returned an id the view did not select");
    }

    #[test]
    fn a_thread_of_several_messages_shows_its_count() {
        // The difference between one mail and a conversation, and it is
        // drawn only when there is more than one — a "1" on every row is
        // noise.
        let ctx = Context::default();
        let msgs = vec![
            Message { id: "a".into(), conversation_id: "c".into(), subject: "topic".into(),
                      unread: false, sender: None, time: 1, num_attachments: 0,
                      is_encrypted: 0 },
            Message { id: "b".into(), conversation_id: "c".into(), subject: "topic".into(),
                      unread: false, sender: None, time: 2, num_attachments: 0,
                      is_encrypted: 0 },
        ];
        let mut v = MailView::default();
        v.set_messages(&msgs);
        let (texts, _) = frame(&ctx, &mut v, vec![]);
        assert!(texts.iter().any(|t| t == "2"), "no thread count drawn: {texts:?}");

        let mut single = view_with(1, 0);
        let (t2, _) = frame(&ctx, &mut single, vec![]);
        assert!(!t2.iter().any(|t| t == "1"), "a single-message thread was labelled: {t2:?}");
    }
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
