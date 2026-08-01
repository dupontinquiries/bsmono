//! inbox — the conversation list, ported from `InboxWidget`.
//!
//! The other half of "the port demonstrates no messaging". `InboxWidget` is the
//! Inbox page's body, and the shell already ports its sub-tab table verbatim
//! (All / Unread / Sent) — but nothing was behind the tabs.
//!
//! The contracts taken from `include/ui/InboxWidget.hpp`:
//!
//! * **`setCurrentSubTab(index)` -> `applyFilterIndex`** — the sub-tabs are a
//!   FILTER over one list, not three lists. Ported as such, because three lists
//!   drift: a snap marked read in one has to be removed from another.
//! * **`addSnap` / `removeSnap` / `clearSnaps`** — the list mutates from network
//!   events, so ordering must be intrinsic rather than insertion-order.
//! * **`receiveConnectionRequest`** — a request appears IN the list, not in a
//!   modal. A modal interrupts; a row waits.
//! * **`showExportReminder`** — a transient banner over the list.
//!
//! Ordering is newest-first by timestamp, with unread lifted above read at equal
//! time — because an inbox exists to show you what needs attention.

/// One row in the inbox.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub id: u64,
    pub peer: String,
    pub preview: String,
    /// Seconds since the epoch; only ordering matters here.
    pub at: i64,
    pub unread: bool,
    /// True when we sent it — the Sent tab's filter.
    pub outgoing: bool,
    pub kind: Kind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Snap,
    Message,
    /// A connection request, which appears IN the list rather than as a modal:
    /// a modal interrupts whatever you were doing, a row waits until you care.
    Request,
}

/// The sub-tabs, verbatim from the Qt table the shell already pins.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Filter {
    #[default]
    All,
    Unread,
    Sent,
}

impl Filter {
    pub const ALL: [Filter; 3] = [Filter::All, Filter::Unread, Filter::Sent];
    pub fn label(self) -> &'static str {
        match self { Filter::All => "All", Filter::Unread => "Unread", Filter::Sent => "Sent" }
    }
    /// `applyFilterIndex` — the index comes from the nav bar, and an unknown one
    /// falls back to All rather than showing an empty list, because an empty
    /// list looks like lost messages.
    pub fn from_index(i: usize) -> Filter {
        *Filter::ALL.get(i).unwrap_or(&Filter::All)
    }
    fn keeps(self, e: &Entry) -> bool {
        match self {
            Filter::All => true,
            Filter::Unread => e.unread,
            Filter::Sent => e.outgoing,
        }
    }
}

/// The inbox model.
#[derive(Debug, Clone, Default)]
pub struct Inbox {
    entries: Vec<Entry>,
    pub filter: Filter,
    /// A transient banner over the list (`showExportReminder`).
    pub banner: Option<String>,
}

impl Inbox {
    /// Add or REPLACE by id. The list mutates from network events, and a snap
    /// arriving twice must not appear twice — which it will if this appends
    /// blindly, since a resend is indistinguishable from a new message.
    pub fn add(&mut self, e: Entry) {
        match self.entries.iter_mut().find(|x| x.id == e.id) {
            Some(slot) => *slot = e,
            None => self.entries.push(e),
        }
    }

    pub fn remove(&mut self, id: u64) -> bool {
        let n = self.entries.len();
        self.entries.retain(|e| e.id != id);
        self.entries.len() != n
    }

    pub fn clear(&mut self) { self.entries.clear(); }

    pub fn mark_read(&mut self, id: u64) {
        if let Some(e) = self.entries.iter_mut().find(|e| e.id == id) { e.unread = false; }
    }

    pub fn unread_count(&self) -> usize { self.entries.iter().filter(|e| e.unread).count() }

    /// A connection request lands in the list like anything else.
    pub fn connection_request(&mut self, id: u64, peer: &str, at: i64) {
        self.add(Entry { id, peer: peer.into(), preview: "wants to connect".into(),
                         at, unread: true, outgoing: false, kind: Kind::Request });
    }

    /// The rows to draw, filtered and ordered.
    ///
    /// Newest first, and unread lifted above read at equal time — an inbox
    /// exists to show what needs attention, and a request outranks both because
    /// it is the only row that expires.
    pub fn visible(&self) -> Vec<&Entry> {
        let mut v: Vec<&Entry> = self.entries.iter().filter(|e| self.filter.keeps(e)).collect();
        v.sort_by(|a, b| {
            let rank = |e: &Entry| match e.kind { Kind::Request => 0, _ => 1 };
            rank(a).cmp(&rank(b))
                .then(b.at.cmp(&a.at))
                .then(b.unread.cmp(&a.unread))
                .then(a.id.cmp(&b.id))          // stable: the list must not shuffle
        });
        v
    }

    pub fn len(&self) -> usize { self.entries.len() }
    pub fn is_empty(&self) -> bool { self.entries.is_empty() }
}

/// Draw the list. Returns the entry clicked, if any.
pub fn inbox_ui(ui: &mut egui::Ui, inbox: &mut Inbox,
                colors: &uikit_egui::PaletteColors) -> Option<u64> {
    let mut clicked = None;
    if let Some(b) = inbox.banner.clone() {
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(&b).color(colors.warning).small());
            if ui.small_button("dismiss").clicked() { inbox.banner = None; }
        });
        ui.separator();
    }
    ui.horizontal(|ui| {
        for f in Filter::ALL {
            let n = if f == Filter::Unread { inbox.unread_count() } else { 0 };
            let label = if n > 0 { format!("{} ({n})", f.label()) } else { f.label().to_string() };
            if ui.selectable_label(inbox.filter == f, label).clicked() { inbox.filter = f; }
        }
    });
    ui.separator();
    let rows = inbox.visible().into_iter().cloned().collect::<Vec<_>>();
    if rows.is_empty() {
        // An empty state is a STATE: saying nothing looks like a broken list.
        ui.vertical_centered(|ui| {
            ui.add_space(24.0);
            ui.label(egui::RichText::new(match inbox.filter {
                Filter::Unread => "nothing unread",
                Filter::Sent => "nothing sent yet",
                Filter::All => "no messages yet",
            }).weak());
        });
        return None;
    }
    egui::ScrollArea::vertical().show(ui, |ui| {
        for e in rows {
            let resp = ui.horizontal(|ui| {
                let dot = if e.unread { colors.accent } else { colors.bg_card };
                let (r, _) = ui.allocate_exact_size(egui::Vec2::splat(8.0), egui::Sense::hover());
                ui.painter().circle_filled(r.center(), 4.0, dot);
                ui.vertical(|ui| {
                    ui.label(egui::RichText::new(&e.peer).strong());
                    ui.label(egui::RichText::new(&e.preview).weak().small());
                });
                if e.kind == Kind::Request {
                    ui.label(egui::RichText::new("request").color(colors.warning).small());
                }
            }).response;
            if resp.interact(egui::Sense::click()).clicked() { clicked = Some(e.id); }
            ui.separator();
        }
    });
    clicked
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(id: u64, at: i64, unread: bool, outgoing: bool) -> Entry {
        Entry { id, peer: format!("peer{id}"), preview: "hi".into(), at, unread, outgoing,
                kind: Kind::Message }
    }

    #[test]
    fn the_sub_tabs_filter_one_list_rather_than_being_three_lists() {
        // Three lists drift: a snap marked read in one has to be removed from
        // another, and eventually is not.
        let mut ib = Inbox::default();
        ib.add(e(1, 100, true, false));
        ib.add(e(2, 200, false, true));
        ib.add(e(3, 300, true, true));
        assert_eq!(ib.visible().len(), 3);
        ib.filter = Filter::Unread;
        assert_eq!(ib.visible().len(), 2);
        ib.filter = Filter::Sent;
        assert_eq!(ib.visible().len(), 2);
        // and marking read updates every view at once
        ib.mark_read(3);
        ib.filter = Filter::Unread;
        assert_eq!(ib.visible().len(), 1);
        assert_eq!(ib.len(), 3, "filtering never removes anything");
    }

    #[test]
    fn an_unknown_sub_tab_index_falls_back_to_all() {
        // An empty list looks like lost messages; All looks like All.
        assert_eq!(Filter::from_index(0), Filter::All);
        assert_eq!(Filter::from_index(1), Filter::Unread);
        assert_eq!(Filter::from_index(2), Filter::Sent);
        assert_eq!(Filter::from_index(99), Filter::All);
    }

    #[test]
    fn a_resent_snap_does_not_appear_twice() {
        // The list mutates from network events, and a resend is
        // indistinguishable from a new message if add() appends blindly.
        let mut ib = Inbox::default();
        ib.add(e(7, 100, true, false));
        ib.add(e(7, 100, true, false));
        assert_eq!(ib.len(), 1);
        // and a genuine update replaces rather than duplicating
        let mut updated = e(7, 100, false, false);
        updated.preview = "edited".into();
        ib.add(updated);
        assert_eq!(ib.len(), 1);
        assert_eq!(ib.visible()[0].preview, "edited");
        assert!(!ib.visible()[0].unread);
    }

    #[test]
    fn the_newest_message_is_at_the_top_and_requests_outrank_everything() {
        // An inbox exists to show what needs attention, and a request is the
        // only row that expires.
        let mut ib = Inbox::default();
        ib.add(e(1, 100, false, false));
        ib.add(e(2, 300, false, false));
        ib.add(e(3, 200, false, false));
        let v = ib.visible();
        assert_eq!(v.iter().map(|x| x.id).collect::<Vec<_>>(), vec![2, 3, 1]);

        ib.connection_request(9, "stranger", 50);   // OLD, but a request
        assert_eq!(ib.visible()[0].id, 9, "a request goes first regardless of age");
        assert_eq!(ib.visible()[0].kind, Kind::Request);
    }

    #[test]
    fn ordering_is_stable_so_the_list_does_not_shuffle() {
        // Two messages in the same second must not swap places between frames.
        let mut ib = Inbox::default();
        for id in [5u64, 3, 4] { ib.add(e(id, 100, false, false)); }
        let a: Vec<u64> = ib.visible().iter().map(|x| x.id).collect();
        let b: Vec<u64> = ib.visible().iter().map(|x| x.id).collect();
        assert_eq!(a, b);
        assert_eq!(a, vec![3, 4, 5], "ties break on id, deterministically");
    }

    #[test]
    fn removing_and_clearing_do_what_they_say() {
        let mut ib = Inbox::default();
        ib.add(e(1, 100, true, false));
        ib.add(e(2, 200, true, false));
        assert!(ib.remove(1));
        assert!(!ib.remove(1), "removing twice is not an error, just false");
        assert_eq!(ib.len(), 1);
        assert_eq!(ib.unread_count(), 1);
        ib.clear();
        assert!(ib.is_empty());
        assert_eq!(ib.unread_count(), 0);
    }
}
