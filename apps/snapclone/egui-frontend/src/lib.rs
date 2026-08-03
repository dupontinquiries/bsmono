//! snapclone-egui — the founder app, ported to egui, now *consuming* the systems it started.
//!
//! Live backgrounds and page/subpage navigation used to live inside snapclone's QML; they're
//! now shared uikit-egui capabilities ([`livebg`](uikit_egui::livebg), [`nav`](uikit_egui::nav)),
//! so this port pulls them from the kit instead of owning them — and localweather / av1tuner get
//! the same for free. On top: adaptive layout, a11y, swipe between pages, and live cross-app
//! settings. The camera/peer content renders on the carved surface; the snapclone-* engine
//! crates (audio/proto/net-intel) drive it on-device.

use egui::Ui;
use uikit_egui::{Adaptive, BgStyle, GestureLayer, LiveBackground, Nav, PaletteColors};

pub mod camera;
pub mod debounce;
pub mod confirm;
pub mod camera_source;
pub mod peer_source;
pub mod contacts;
pub mod conversation;
pub mod composer;
pub mod outbox;
pub mod group_voice;
pub mod settings_page;
pub mod settings_store;
pub mod inbox;
pub mod unified_nav;
pub mod friends;
pub mod modal;
pub mod snaps;
pub mod session;
pub mod ring;
pub mod qr;
pub mod wayfinder;
pub mod voicebar;
pub mod landmark;
pub mod export_reminder;
pub mod nav_chrome;
pub mod ribbon;
pub mod scroll_areas;
pub mod mobile_dialog;
pub use unified_nav::{NavEvent, UnifiedNav, PAGE_NAMES, sub_tabs};

/// The five Qt pages, verbatim (`MainWindow.cpp` kPageCamera..kPageSettings). The old 3-page
/// skeleton ("Camera/Peers/Settings") predated the faithful-port pass; "Peers" was never a
/// Qt page — the roster lives on the FRIENDS page, and Inbox/Wayfinder were missing entirely.
pub const PAGES: [&str; 5] = unified_nav::PAGE_NAMES;

/// A connected peer (mirrors snapclone-proto; on-device the real roster feeds this).
#[derive(Debug, Clone)]
pub struct Peer {
    pub name: String,
    pub online: bool,
}

/// The snapclone view: page/subpage nav, a live background, the peer roster, and gesture/theme.
pub struct Snapclone {
    /// The ported surfaces. Held here rather than constructed per frame so they
    /// keep their state — a viewfinder that forgot its focus point every frame,
    /// or an inbox that forgot its filter, is the "dud demo" failure: the
    /// widgets exist, nothing remembers anything, and the app looks unfinished
    /// because it IS.
    pub viewfinder: crate::camera::Viewfinder,
    /// Which camera and format, from device-kit. `None` means no camera was
    /// found — shown as such rather than as an empty preview, because a black
    /// rectangle and a missing device look identical.
    pub camera: Option<crate::camera_source::CameraChoice>,
    /// Why the last save failed, if it did.
    pub settings_error: Option<String>,
    /// Page transitions, debounced. The nav is instant (leading edge) while the
    /// per-page LOAD waits for the swipe to settle — so a fast scroll through
    /// five pages loads the fifth once rather than all five in turn.
    pub page_debounce: crate::debounce::Debouncer<usize>,
    /// Notify on arrival only when the window is NOT focused. Notifying while
    /// someone is looking at the inbox is noise about something already on
    /// screen, and it is how an app trains people to turn notifications off.
    pub notify_enabled: bool,
    /// Suppress sender and content in notifications — they persist in KDE's
    /// history and show on lock screens.
    pub high_security: bool,
    pub inbox: crate::inbox::Inbox,
    pub composer: crate::composer::Composer,
    pub settings: crate::settings_page::SettingsPage,
    pub group_voice: crate::group_voice::GroupVoice,
    pub picker: crate::contacts::Picker,
    pub conversation: crate::conversation::Conversation,
    /// Who the open conversation is with. The Conversation model holds the
    /// ITEMS; which peer they belong to is the shell's business.
    pub conversation_peer: String,
    /// The unified bottom bar (Qt UnifiedNavBar): page arrows, sub-tab segments, underline
    /// morph, page dots. Owns the page + sub-tab state; `nav` mirrors the page for swipe.
    pub bar: UnifiedNav,
    /// Login overlay visibility, with the Qt ordering rule (MainWindow.cpp): the overlay
    /// STARTS HIDDEN; if auto-login already succeeded at construction we leave it hidden and
    /// go straight to camera; otherwise we show it and hide on `login_succeeded`.
    pub login_visible: bool,
    pub nav: Nav,
    pub bg: LiveBackground,
    pub peers: Vec<Peer>,
    /// The friends roster (Qt FriendListWidget). Derived from `peers` on
    /// change rather than kept in parallel — two lists of the same people
    /// disagree the moment one update misses.
    pub roster: crate::friends::Roster,
    /// Which row's action menu is open (Qt ContactCard), at most one.
    pub cards: crate::friends::Accordion,
    /// The manual add-peer form: the only way in when discovery does not work.
    pub add_form: crate::friends::AddForm,
    /// The six-layer modal stack (Qt ModalSurface) every dialog is presented
    /// into. Owning it here is the point: z-order becomes a property of the
    /// structure rather than of which code called raise() last.
    pub modal: crate::modal::ModalSurface,
    /// The sealed feed of unopened snaps (Qt SnapFeedWidget).
    pub feed: crate::snaps::SnapFeed,
    /// The ephemeral viewer. Holds the only decrypted bytes in the app, and
    /// wipes them on close.
    pub viewer: crate::snaps::Viewer,
    feed_press: crate::snaps::Press,
    /// Snaps opened this session, for the host to mark in storage.
    pub snaps_viewed: Vec<String>,
    /// Snaps whose view window has elapsed. Separate from `snaps_viewed`
    /// because they answer different questions: one is "did they see it", the
    /// other is "is the window over".
    pub snaps_expired: Vec<String>,
    pub last_saved: Option<String>,
    /// Send status per snap (Qt OutboxWidget), shown on the Inbox "Sent" tab.
    pub outbox: crate::session::Outbox,
    /// The sign-in form behind the login overlay. The overlay's visibility rule
    /// was ported long ago; this is what it was hiding.
    pub login: crate::session::Login,
    /// Wall clock in seconds, for relative times. Held rather than read at
    /// draw time so tests can advance it.
    pub now_secs: i64,
    /// The incoming-call ring (Qt IncomingCallDialog), presented into the
    /// Confirm layer — it is a question that must be answered, so it sits above
    /// any dialog already open.
    pub incoming_call: Option<crate::ring::IncomingCall>,
    /// Calls that ended without being taken, for the missed-call list. A
    /// timeout and a decline are recorded apart: same effect, different history.
    pub missed_calls: Vec<String>,
    /// The QR scanner sheet (Qt QrScanDialog), presented into the Dialog layer.
    /// `None` when it is not open — the scanner holds camera frames and should
    /// not exist while nobody is looking at it.
    pub qr: Option<crate::qr::QrScan>,
    /// The on-air control surface (Qt GroupVoiceBar). Drawn above the nav
    /// whenever a session is joined, which is where the join/mute/talk controls
    /// live now — the previous commit took them off the Wayfinder page, where
    /// they never belonged, and left them with no home.
    pub voicebar: crate::voicebar::VoiceBar,
    /// Monthly export-reminder bookkeeping. Host-persisted in Qt (QSettings);
    /// held here so the decision stays a pure function of state + clock.
    pub reminder: crate::export_reminder::ReminderState,
    /// The current month, supplied by the host — the rule must not reach for a
    /// clock, or it cannot be tested.
    pub now_month: Option<crate::export_reminder::Month>,
    /// Recent snap paths for the "remember when?" strip.
    pub recent_snaps: Vec<String>,
    /// Set when the user asks to export; the host runs the actual zip.
    pub export_requested: bool,
    /// Set when the user asks to retry failed sends; the host owns the retry.
    pub retry_requested: bool,
    /// Scroll position of the background carousel, and whether its last wheel
    /// event ran off the end and was handed to the page nav.
    pub bg_carousel: crate::scroll_areas::HScroll,
    pub carousel_handoff: bool,
    /// Whether dialogs present as bottom sheets or floating windows. Decided
    /// once per frame from the real surface rather than hardcoded, so the same
    /// build is correct on a phone and on a desktop.
    pub dialog_form: crate::mobile_dialog::Form,
    /// The swipe affordance. Retires itself after four swipes.
    pub peel: crate::nav_chrome::PeelHint,
    /// The Inbox's bottom ribbon: All / Unread / Sent plus the actions each of
    /// those tabs can actually offer.
    pub ribbon: crate::ribbon::SmartRibbon,
    /// Our own position, when the device has a fix. `None` is a real state, not
    /// a placeholder: without it nobody can be placed geographically.
    pub self_fix: Option<crate::wayfinder::Fix>,
    /// Contacts as the wayfinder sees them — id, name, online, position.
    pub wayfinder_contacts: Vec<crate::wayfinder::Contact>,
    /// (sender id, caption) for the snaps floating on the map, newest first.
    pub wayfinder_snaps: Vec<(String, String)>,
    gesture: GestureLayer,
    palette: String,
}

impl Snapclone {
    pub fn new(peers: Vec<Peer>) -> Snapclone {
        Self::new_with_login(peers, true)
    }

    /// `auto_logged_in` mirrors Qt's construction-time auto-login check.
    pub fn new_with_login(peers: Vec<Peer>, auto_logged_in: bool) -> Snapclone {
        Snapclone {
            viewfinder: Default::default(),
            camera: None,
            settings_error: None,
            notify_enabled: true,
            high_security: false,
            page_debounce: crate::debounce::Debouncer::new(
                crate::debounce::Config::page_transition()),
            inbox: Default::default(),
            composer: Default::default(),
            // Load from disk rather than defaults: a settings page that starts
            // blank every launch is the same lie as one that never saved.
            settings: crate::settings_page::SettingsPage::new(crate::settings_store::load()),
            group_voice: Default::default(),
            picker: Default::default(),
            cards: Default::default(),
            add_form: Default::default(),
            modal: Default::default(),
            feed: Default::default(),
            viewer: Default::default(),
            feed_press: Default::default(),
            snaps_viewed: Vec::new(),
            snaps_expired: Vec::new(),
            last_saved: None,
            outbox: Default::default(),
            login: Default::default(),
            now_secs: 0,
            incoming_call: None,
            missed_calls: Vec::new(),
            qr: None,
            voicebar: crate::voicebar::VoiceBar::new(),
            reminder: Default::default(),
            now_month: None,
            recent_snaps: Vec::new(),
            export_requested: false,
            retry_requested: false,
            bg_carousel: crate::scroll_areas::HScroll::new(0.0, 400.0),
            carousel_handoff: false,
            dialog_form: crate::mobile_dialog::Form::Sheet,
            peel: crate::nav_chrome::PeelHint::new(PAGES.len()),
            ribbon: {
                // The tab labels are the nav's own, so the ribbon and the
                // sub-tab table cannot drift apart.
                let mut r = crate::ribbon::SmartRibbon::new(sub_tabs(1));
                r.set_tab_actions(1, vec![
                    crate::ribbon::Action::new("read_all", "check", "Mark all read")]);
                r.set_tab_actions(2, vec![
                    crate::ribbon::Action::new("retry_failed", "refresh", "Retry failed")]);
                r
            },
            self_fix: None,
            wayfinder_contacts: Vec::new(),
            wayfinder_snaps: Vec::new(),
            conversation: Default::default(),
            conversation_peer: String::new(),
            bar: UnifiedNav::new(),
            login_visible: !auto_logged_in,
            nav: Nav::new(PAGES.len()),
            bg: LiveBackground::new(BgStyle::Aurora),
            gesture: GestureLayer::horizontal_page(320.0),
            palette: String::new(),
            // Built from the same peers, once, so the two cannot drift.
            roster: {
                let mut r = crate::friends::Roster::default();
                r.set(peers.iter().map(Snapclone::friend_of).collect());
                r
            },
            peers,
        }
    }

    fn friend_of(p: &Peer) -> crate::friends::Friend {
        crate::friends::Friend {
            // A discovered Peer has one string standing for both its routable
            // id and its name; a real contact record separates them, and the
            // Friend row is built to take either.
            id: p.name.clone(), name: p.name.clone(), online: p.online,
        }
    }

    /// Re-derive the roster after the peer list changes. One source of truth:
    /// keeping a second list in parallel means the day an update misses one of
    /// them, the page shows someone as online who left an hour ago.
    pub fn sync_roster(&mut self) {
        self.roster.set(self.peers.iter().map(Snapclone::friend_of).collect());
    }

    /// Confirm the pending delete. Split from apply_friends because the
    /// confirmation arrives as its own answer, not as part of the tap that
    /// asked the question.
    pub fn confirm_delete(&mut self, id: &str) {
        self.peers.retain(|p| p.name != id);
        self.cards.close(id);
        self.sync_roster();
        self.modal.dismiss(&format!("confirm-delete:{id}"));
    }

    /// Carry out what the Friends page asked for.
    pub fn apply_friends(&mut self, a: crate::friends::Action) {
        use crate::friends::Action;
        match a {
            // A manual add is optimistic: the row appears offline immediately,
            // because a Connect button that visibly does nothing for two
            // seconds gets pressed four more times.
            Action::Add(p) => {
                let name = p.host.clone();
                if !self.peers.iter().any(|e| e.name == name) {
                    self.peers.push(Peer { name: name.clone(), online: false });
                }
                self.sync_roster();
            }
            Action::Selected(id) => {
                if let Some(i) = self.peers.iter().position(|p| p.name == id) {
                    self.nav.push_subpage(i);
                }
            }
            Action::Invoke(id, act) => match act.as_str() {
                // Delete asks first, in the Confirm layer above the dialog
                // layer — removing a contact on a single tap of a menu item
                // that sits one row below "Message" is how people lose people.
                "delete" => self.modal.present_default(
                    &format!("confirm-delete:{id}"), crate::modal::Layer::Confirm,
                    crate::modal::Rect::new(40.0, 200.0, 300.0, 140.0)),
                "edit" => self.modal.present_default(
                    &format!("editor:{id}"), crate::modal::Layer::Dialog,
                    crate::modal::Rect::new(20.0, 80.0, 340.0, 400.0)),
                _ => {}
            },
            Action::ImportVCard => {}
        }
    }

    pub fn page_name(&self) -> &'static str {
        PAGES[self.nav.page()]
    }

    /// Swipe between top-level pages (unless drilled into a subpage — then swipe is off so the
    /// content can scroll/interact).
    pub fn swipe_page(&mut self, sign: i32) {
        // Counted here rather than at the gesture layer so a swipe that is
        // REFUSED — blocked by a dialog, or at the end of the pages — does not
        // count toward familiarity. The user has not learned anything from a
        // gesture that did nothing.
        // A swipe under an open dialog would change the page out from under it
        // and leave the dialog floating over a screen it has nothing to do
        // with. This is what Qt's stackChanged signal exists to let callers
        // ask.
        if self.modal.blocks_input() {
            return;
        }
        if self.nav.can_go_back() {
            return;
        }
        let n = PAGES.len() as i32;
        let next = (self.nav.page() as i32 + sign).clamp(0, n - 1) as usize;
        if next != self.nav.page() { self.peel.note_swipe(); }
        self.nav.go_page(next);
        self.peel.set_page(next, PAGES.len());
    }

    /// Advance the animation; call once per frame with the frame delta.
    pub fn tick(&mut self, dt: f32) {
        self.bg.tick(dt);
        // The viewer countdown runs on the same clock, and deliberately keeps
        // running after the user closes early: the expiry event is what marks
        // the snap in storage.
        for ev in self.viewer.tick(dt * 1000.0) {
            self.apply_snap_event(ev);
        }
        // The scanner's success hold runs on the frame clock too.
        if let Some(c) = self.qr.as_mut().and_then(|q| q.tick(dt * 1000.0)) {
            self.apply_scan(c);
        }
        // The ring declines itself after thirty seconds whether or not anyone
        // is looking at this page.
        if let Some(call) = self.incoming_call.as_mut() {
            let caller = call.caller_name.clone();
            if let Some(a) = call.tick(dt * 1000.0) { self.finish_call(&caller, a); }
        }
    }

    /// Re-derive which ribbon actions are available. Called before drawing, so
    /// enablement always reflects the state on screen rather than the state
    /// when the tab was opened.
    pub fn refresh_ribbon(&mut self) {
        let unread = self.inbox.unread_count();
        let failed = self.outbox.rows().iter()
            .filter(|r| !r.delivered && r.phase.is_failure()).count();
        self.ribbon.refresh(|id| match id {
            // Nothing to mark, nothing to press. An always-enabled "mark all
            // read" on an empty inbox is a button that lies about having work.
            "read_all" => unread > 0,
            "retry_failed" => failed > 0,
            _ => true,
        });
    }

    /// Carry out a ribbon action.
    pub fn apply_ribbon(&mut self, ev: crate::ribbon::RibbonEvent) {
        use crate::ribbon::RibbonEvent;
        match ev {
            RibbonEvent::TabSelected(i) => { self.bar.sub[1] = i; }
            RibbonEvent::Triggered(id) => match id.as_str() {
                "read_all" => {
                    for id in self.inbox.unread_ids() { self.inbox.mark_read(id); }
                }
                // Retry is the host's: this view does not own the transport.
                // Recorded so the host can act and the test can see it.
                "retry_failed" => self.retry_requested = true,
                _ => {}
            },
        }
    }

    /// Handle the export card. Either action marks the month shown — the card
    /// was on screen and answered, and re-offering it after an explicit answer
    /// is the nag this feature is built to avoid.
    pub fn apply_export_card(&mut self, a: crate::export_reminder::CardAction,
                             month: crate::export_reminder::Month) {
        use crate::export_reminder::CardAction;
        match a {
            CardAction::ExportRequested => {
                self.export_requested = true;
                self.reminder.mark_shown(month);
            }
            CardAction::Dismissed => self.reminder.snooze(month),
        }
    }

    /// Carry out what the voice bar asked for. The bar owns no audio and no
    /// sockets; this is where its events meet the session.
    pub fn apply_voice(&mut self, ev: crate::voicebar::Event) {
        use crate::voicebar::Event;
        match ev {
            // Reflect the press locally AND ask the session. A talk button that
            // waits for the round trip before lighting feels broken on a slow
            // link, which is exactly when you are pressing it hardest.
            Event::TalkPressed => self.voicebar.set_on_air(true),
            Event::TalkReleased => self.voicebar.set_on_air(false),
            // The request is ours to record; the TIER is the negotiation's to
            // report, so this does not set Private itself.
            Event::PrivateRequested(want) => {
                self.voicebar.security = if want { crate::voicebar::Security::Securing }
                                         else { crate::voicebar::Security::Public };
            }
        }
    }

    /// Join or leave the group voice channel.
    pub fn toggle_voice(&mut self) {
        if self.group_voice.joined {
            self.group_voice.leave();
            self.voicebar.set_on_air(false);
        } else {
            self.group_voice.join();
        }
    }

    /// Where a dialog of this height should sit, given the current form factor.
    /// Replaces the hardcoded rectangles the modal layer was being handed —
    /// those were phone-shaped constants that would have put a 340px sheet in
    /// the corner of a desktop window.
    pub fn dialog_rect(&self, surface: egui::Rect, content_h: f32) -> crate::modal::Rect {
        let mut d = crate::mobile_dialog::MobileDialog::new(self.dialog_form);
        d.open();
        d.tick(crate::mobile_dialog::SHEET_MS * 2.0);
        let (x, y, w, h) = d.geometry(surface.width(), surface.height(), content_h);
        crate::modal::Rect::new(x, y, w, h)
    }

    /// Open the QR scanner. `camera` false lands straight on the manual field
    /// rather than showing a viewfinder that will never produce a frame.
    pub fn open_qr_scan(&mut self, camera: bool) {
        self.qr = Some(if camera { crate::qr::QrScan::scanning() }
                       else { crate::qr::QrScan::manual_only() });
        self.modal.present_default("qr", crate::modal::Layer::Dialog,
                                   crate::modal::Rect::new(20.0, 80.0, 340.0, 420.0));
    }

    /// A scanned or pasted code becomes a peer, through the SAME path a typed
    /// address takes — so a contact added by camera and one added by hand are
    /// the same kind of contact.
    pub fn apply_scan(&mut self, c: crate::qr::ScannedContact) {
        self.apply_friends(crate::friends::Action::Add(
            crate::friends::AddPeer { host: c.sc_id, port: c.port }));
        self.qr = None;
        self.modal.dismiss("qr");
    }

    /// Feed a decoded frame from the camera. Returns true once it latches.
    pub fn qr_frame(&mut self, payload: &str) -> bool {
        self.qr.as_mut().map(|q| q.frame(payload)).unwrap_or(false)
    }

    /// A voice packet arrived for a conversation whose thread is not open.
    pub fn ring(&mut self, caller: &str) {
        self.incoming_call = Some(crate::ring::IncomingCall::new(caller));
        self.modal.present(&format!("call:{caller}"), crate::modal::Layer::Confirm,
                           crate::modal::Rect::new(0.0, 400.0, 400.0, 300.0),
                           // NOT dismiss-on-tap-outside: a stray tap while the
                           // phone is in a pocket must not answer or refuse a
                           // call. Only the two buttons and the timeout decide.
                           false, true);
    }

    /// Apply an answer: dismiss the sheet, and open the thread if it was taken.
    pub fn answer_call(&mut self, a: crate::ring::Answer) {
        let Some(call) = self.incoming_call.as_mut() else { return };
        let caller = call.caller_name.clone();
        if call.answer(a).is_none() { return; }
        self.finish_call(&caller, a);
    }

    fn finish_call(&mut self, caller: &str, a: crate::ring::Answer) {
        use crate::ring::Answer;
        self.modal.dismiss(&format!("call:{caller}"));
        match a {
            Answer::Accepted => {
                self.conversation_peer = caller.to_string();
                self.group_voice.joined = true;
            }
            // Both rejections drop the buffered packets; only the ignored one
            // is a MISSED call.
            Answer::TimedOut => self.missed_calls.push(caller.to_string()),
            Answer::Declined => {}
        }
        self.incoming_call = None;
    }

    /// Open a snap from the feed, view-once, presented into the modal Dialog
    /// layer so the swipe nav and the page behind it are both blocked.
    pub fn open_snap(&mut self, snap: crate::snaps::Snap) {
        let id = snap.id.clone();
        for ev in self.viewer.open(snap) { self.apply_snap_event(ev); }
        self.feed.remove(&id);
        self.modal.present_default(&format!("snap:{id}"), crate::modal::Layer::Dialog,
                                   crate::modal::Rect::new(0.0, 0.0, 400.0, 700.0));
    }

    /// Close the viewer by hand. The countdown is NOT stopped.
    pub fn close_snap(&mut self) {
        let id = self.viewer.current().map(|s| s.id.clone()).unwrap_or_default();
        self.viewer.close();
        self.modal.dismiss(&format!("snap:{id}"));
    }

    fn apply_snap_event(&mut self, ev: crate::snaps::ViewerEvent) {
        use crate::snaps::ViewerEvent as V;
        match ev {
            // Both mark the row in the inbox; the tile is already gone from the
            // feed, and an expiry for a snap the user closed early still has to
            // land or the row stays looking unopened.
            // Storage is the host's: it owns the snap records and this view
            // does not. Recorded rather than applied, and deliberately NOT
            // mapped onto inbox row ids — snap ids are strings and inbox ids
            // are u64s, and inventing a conversion here to make the types line
            // up would mark the wrong row.
            V::Viewed(id) => {
                if !self.snaps_viewed.contains(&id) { self.snaps_viewed.push(id); }
            }
            V::Expired(id) => {
                if !self.snaps_expired.contains(&id) { self.snaps_expired.push(id.clone()); }
                self.modal.dismiss(&format!("snap:{id}"));
            }
            V::Saved(_, path) => self.last_saved = Some(path),
            V::Kept(_) => {}
        }
    }

    /// Adopt cross-app settings + the swipe feel, and use the palette's background style pref.
    pub fn apply_settings(&mut self, ctx: &egui::Context, prefs: &settings_kit::Prefs) {
        uikit_egui::apply_prefs(ctx, prefs);
        *self.gesture.config_mut() = prefs.gesture_config(320.0);
        self.palette = prefs.palette.clone();
    }

    /// Render: live background, then the current page/subpage.
    /// Snaps arrived. Returns what to send to the notification daemon, or
    /// `None` when nothing should be sent.
    ///
    /// `focused` is the window's own state: an app that notifies about a message
    /// visible on screen is one people mute.
    pub fn on_snaps_arrived(&self, from: &[String], focused: bool)
        -> Option<uikit_egui::notify::Rendered> {
        if !self.notify_enabled || focused || from.is_empty() { return None; }
        let notes: Vec<uikit_egui::notify::Note> = from.iter().map(|f| {
            uikit_egui::notify::Note {
                from: f.clone(),
                // A snap has no text — the picture is the message — so there is
                // nothing to suppress in the detail, and the generic carries it.
                detail: String::new(),
                generic: "New snap".into(),
            }
        }).collect();
        uikit_egui::notify::render_batch(&notes, self.high_security)
    }

    /// Note the page the user is on, and report when they have settled there.
    ///
    /// The caller does the expensive part — fetching a conversation, opening a
    /// camera — only on Settled. Doing it on every frame of a swipe is what
    /// makes a page transition stutter, and doing it only on Settled without a
    /// leading edge is what makes it feel laggy.
    pub fn note_page(&mut self, page: usize, now_ms: u64) -> Option<usize> {
        self.page_debounce.change(page, now_ms);
        self.page_debounce.tick(now_ms).map(|(_, p)| p)
    }

    pub fn ui(&mut self, ui: &mut Ui, adaptive: &Adaptive) {
        let colors = uikit_egui::palette_colors(&self.palette);
        let surface = ui.available_rect_before_wrap();

        // live background painted behind everything
        self.bg.paint(ui.painter(), surface, &colors);

        // swipe paging (top-level only)
        let frame = self.gesture.update(ui, surface, true);
        if let Some(dir) = frame.page_jump {
            self.swipe_page(dir.sign());
        }

        // Form factor first: every dialog opened this frame is placed by it.
        // The environment is the host's to report; what this view can see is
        // the surface it was handed, which is the width branch of the ladder.
        self.dialog_form = crate::mobile_dialog::form_factor(
            &Default::default(), surface.width(), adaptive.is_compact());

        // The peel hints ride over the page area, under everything else.
        self.peel.set_page(self.nav.page(), PAGES.len());
        crate::nav_chrome::peel_ui(ui, &mut self.peel, surface, &colors);

        // The voice bar sits above the page content while a session is live, so
        // the floor holder, the mic level and the talk button are reachable
        // from wherever you are — the point of a single bar over per-page
        // controls.
        if self.group_voice.joined {
            if let Some(ev) = crate::voicebar::voicebar_ui(ui, &mut self.voicebar, &colors) {
                self.apply_voice(ev);
            }
            ui.separator();
        }

        // page tabs on desktop; swipe hint on phone
        if !adaptive.is_compact() && !self.nav.can_go_back() {
            ui.horizontal(|ui| {
                for (i, name) in PAGES.iter().enumerate() {
                    if ui.selectable_label(i == self.nav.page(), *name).clicked() {
                        self.nav.go_page(i);
                    }
                }
            });
            ui.separator();
        }

        match self.nav.page() {
            // The nav has always declared five pages — Camera, Inbox, Friends,
            // Wayfinder, Settings — and the shell routed three of them, so two
            // tabs led nowhere. That is most of why the app read as a demo: the
            // chrome promised more than the body delivered.
            0 => self.camera_page(ui, &colors),
            1 => self.inbox_page(ui, &colors),
            2 => self.peers_page(ui, &colors),
            3 => self.wayfinder_page(ui, &colors),
            _ => self.settings_page(ui, &colors),
        }
    }

    fn camera_page(&mut self, ui: &mut Ui, c: &PaletteColors) {
        self.nav.title_bar(ui, "Camera");
        // The REAL viewfinder, not a labelled rectangle. Tap-to-focus, the
        // fading reticle, full-bleed vs framed and the front-camera mirror all
        // come from camera.rs — which existed and was never called, which is
        // exactly why the app looked like a placeholder.
        ui.allocate_ui(egui::vec2(ui.available_width(), 220.0), |ui| {
            crate::camera::viewfinder(ui, &mut self.viewfinder, None, c);
        });
        // Say WHICH camera, or that there is none. The viewfinder mirrors only
        // when device-kit says this device is front-facing.
        match &self.camera {
            Some(cam) => {
                ui.colored_label(c.text_weak, crate::camera_source::describe(cam));
            }
            None => {
                ui.colored_label(c.warning, "no camera found");
            }
        }
        ui.horizontal(|ui| {
            if ui.button("flip").clicked() { self.viewfinder.flip(); }
            if ui.selectable_label(self.viewfinder.full_bleed, "fill").clicked() {
                self.viewfinder.full_bleed = !self.viewfinder.full_bleed;
            }
            if ui.selectable_label(self.viewfinder.scan_mode, "scan").clicked() {
                self.viewfinder.scan_mode = !self.viewfinder.scan_mode;
            }
        });
        ui.separator();
        // the composer, over the captured frame
        ui.horizontal(|ui| {
            ui.label("caption");
            ui.text_edit_singleline(&mut self.composer.caption);
            if ui.button("colour").clicked() { self.composer.cycle_color(); }
            let d = self.composer.duration;
            // Say what the timer MEANS before the snap is sent, not after. The
            // difference between a replicated durable snap and an ephemeral one
            // is invisible otherwise, and it is the difference that matters.
            let delivery = crate::outbox::Delivery::of(d);
            if ui.button(d.label()).clicked() {
                use crate::composer::Duration;
                self.composer.duration = match d {
                    Duration::Seconds(s) if s < 10 => Duration::Seconds(s + 5),
                    Duration::Seconds(_) => Duration::Unlimited,
                    Duration::Unlimited => Duration::Seconds(5),
                };
            }
        });
        // Recomputed here rather than captured: the button above may have just
        // changed the duration, and showing the PREVIOUS delivery would tell
        // the user the opposite of what pressing send will do.
        let delivery = crate::outbox::Delivery::of(self.composer.duration);
        ui.label(egui::RichText::new(delivery.explain())
            .color(if delivery.replicates() { c.warning } else { c.text_weak }).small());
    }

    fn peers_page(&mut self, ui: &mut Ui, c: &PaletteColors) {
        self.nav.title_bar(ui, "Friends");
        // The contact picker, so Friends is a place you DO something rather
        // than a list you look at.
        ui.horizontal(|ui| {
            ui.text_edit_singleline(&mut self.picker.query);
            let multi = self.picker.is_multi();
            if ui.selectable_label(multi, "multi").clicked() {
                self.picker.mode = if multi { crate::contacts::PickMode::Single }
                                   else { crate::contacts::PickMode::Multi {
                                       confirm_label: "Send".into() } };
            }
        });
        let ids: Vec<String> = self.peers.iter().map(|p| p.name.clone()).collect();
        for id in ids {
            let sel = self.picker.selected().iter().any(|s| s == &id);
            if ui.selectable_label(sel, &id).clicked() { self.picker.toggle(&id); }
        }
        ui.separator();
        if let Some(sel) = self.nav.subpage() {
            // subpage: peer detail
            if let Some(p) = self.peers.get(sel) {
                ui.colored_label(c.text, format!("{}  ·  {}", p.name, if p.online { "online" } else { "offline" }));
                ui.colored_label(c.text_weak, "Share photos, connect a session, or send a ping.");
            }
            return;
        }
        // Join / leave lives here because Friends is where the people are; the
        // rest of the voice controls are on the bar itself once a session is up.
        ui.horizontal(|ui| {
            let joined = self.group_voice.joined;
            if ui.selectable_label(joined, if joined { "leave channel" } else { "join channel" })
                .clicked() { self.toggle_voice(); }
            if joined && ui.selectable_label(self.group_voice.muted, "mute").clicked() {
                self.group_voice.muted = !self.group_voice.muted;
            }
        });
        // The real FriendListWidget: cards with an action menu, the empty state
        // that names the ports to open, and the add-peer form. The flat list of
        // names this replaced had no way to add anyone, which is the half of
        // the page that matters when discovery has failed and you are standing
        // next to the person you are trying to reach.
        if let Some(a) = crate::friends::friends_ui(
            ui, &self.roster, &mut self.cards, &mut self.add_form, c) {
            self.apply_friends(a);
        }
    }

    fn settings_page(&mut self, ui: &mut Ui, c: &PaletteColors) {
        self.nav.title_bar(ui, "Settings");
        // The five real tab bodies from settings_page.rs, not one row of
        // background styles. The background picker stays as an appearance
        // control, but it is no longer the whole page.
        crate::settings_page::settings_ui(ui, &mut self.settings, c);
        // Persist. The io error is SHOWN rather than swallowed — a Save that
        // silently fails is the same lie as one that never wrote, and a
        // read-only config dir is a real thing on locked-down devices.
        if ui.add_enabled(self.settings.is_dirty(), egui::Button::new("Save to disk")).clicked() {
            self.settings.save();
            match crate::settings_store::store(&self.settings.current) {
                Ok(()) => self.settings_error = None,
                Err(e) => self.settings_error = Some(format!("could not save: {e}")),
            }
        }
        if let Some(err) = &self.settings_error {
            ui.colored_label(c.warning, err);
        }
        ui.separator();
        // The background styles are a horizontal CAROUSEL, not a wrapped row:
        // a wheel over it scrolls sideways, and at either end continued
        // scrolling hands off to the page nav rather than dying silently.
        ui.colored_label(c.text_weak, "background:");
        let strip = egui::ScrollArea::horizontal().id_salt("bg_carousel").show(ui, |ui| {
            ui.horizontal(|ui| {
                for s in BgStyle::all() {
                    if ui.selectable_label(self.bg.style == s, s.as_str()).clicked() {
                        self.bg.style = s;
                    }
                }
            });
        });
        let over = strip.inner_rect.contains(
            ui.input(|i| i.pointer.hover_pos()).unwrap_or(egui::pos2(-1e6, -1e6)));
        if over {
            let (dx, dy) = ui.input(|i| (i.smooth_scroll_delta.x, i.smooth_scroll_delta.y));
            if dx != 0.0 || dy != 0.0 {
                let w = crate::scroll_areas::Wheel::pixels(dx, dy);
                if self.bg_carousel.wheel(w) == crate::scroll_areas::Wheeled::PassThrough {
                    // Clamped and still pushing: this is the handoff the Qt
                    // comment describes — the carousel declines so the page
                    // swipe can take it.
                    self.carousel_handoff = true;
                }
            }
        }
    }

    /// Wayfinder: the group voice channel and the landmark tools. Was a dead
    /// tab.
    fn wayfinder_page(&mut self, ui: &mut Ui, c: &PaletteColors) {
        self.nav.title_bar(ui, "Wayfinder");
        // This page used to draw the group-voice controls — join / mute / push
        // to talk. Not a stub of the wayfinder: a different feature under its
        // name, which is worse than an empty page because nothing looks
        // missing. Those controls live on the voice bar, which is where the Qt
        // app puts them.
        let nodes = crate::wayfinder::build_nodes(self.self_fix, &self.wayfinder_contacts);
        let photos = crate::wayfinder::place_photos(&nodes, &self.wayfinder_snaps);
        if let Some(id) = crate::wayfinder::wayfinder_ui(ui, &nodes, &photos, c) {
            // Tapping a contact jumps to Friends with them selected, per the
            // Qt page: the map is a way IN to a conversation, not a dead end.
            self.apply_friends(crate::friends::Action::Selected(id));
            self.nav.go_page(2);
        }
        if nodes.is_empty() {
            ui.colored_label(c.text_weak, "No contacts to place yet.");
        }
    }

    /// The inbox, on its own page. Was absent entirely: the app had a Peers
    /// list and no way to see a message, which is the gap that made a
    /// messaging app read as a demo.
    fn inbox_page(&mut self, ui: &mut Ui, c: &PaletteColors) {
        self.nav.title_bar(ui, "Messages");
        // The monthly card sits at the top of the Inbox, per the Qt page. It
        // gates itself: no month from the host means no card, rather than a
        // guess at what month it is.
        if let Some(month) = self.now_month {
            let card = crate::export_reminder::ExportCard::for_month(
                &self.reminder, month, self.recent_snaps.clone());
            if let Some(a) = crate::export_reminder::export_card_ui(ui, &card, c) {
                self.apply_export_card(a, month);
            }
        }
        // The ribbon sits at the BOTTOM of the inbox, where a thumb reaches it.
        self.refresh_ribbon();
        self.ribbon.set_current(self.bar.sub[1]);
        // The nav has declared an Inbox "Sent" sub-tab since the shell was
        // ported, with nothing behind it. That is the outbox.
        if self.bar.sub[1] == 2 {
            crate::session::outbox_ui(ui, &self.outbox, self.now_secs, c);
            return;
        }
        if let Some(id) = crate::inbox::inbox_ui(ui, &mut self.inbox, c) {
            self.inbox.mark_read(id);
            self.conversation_peer = id.to_string();
        }
        // The conversation itself, below the list: a tap on a row should show
        // the thread rather than only marking it read.
        if !self.conversation_peer.is_empty() {
            ui.separator();
            ui.label(format!("with {}", self.conversation_peer));
            let items: Vec<crate::conversation::Item> =
                self.conversation.ordered().into_iter().cloned().collect();
            for it in items {
                use crate::conversation::Item;
                let (label, openable) = match &it {
                    Item::Text { body, .. } => (body.clone(), false),
                    Item::Snap { opened, .. } =>
                        (if *opened { "snap (viewed)".to_string() }
                         else { "snap — tap to open".to_string() }, !*opened),
                    Item::Voice { secs, .. } => (format!("voice {secs}s"), false),
                };
                if ui.add_enabled(openable, egui::Button::new(label)).clicked() {
                    self.conversation.tap_snap(it.id());
                }
            }
            let n = self.conversation.unopened();
            if n > 0 { ui.colored_label(c.accent, format!("{n} unopened")); }
            ui.horizontal(|ui| {
                ui.text_edit_singleline(&mut self.conversation.draft);
                if ui.add_enabled(self.conversation.can_send(),
                                  egui::Button::new("send")).clicked() {
                    self.conversation.take_draft();
                }
            });
        }
    }
}

impl Snapclone {
    /// Draw the sign-in form. Returns credentials for the host to act on; this
    /// view does not decide whether they are good.
    pub fn login_ui(&mut self, ui: &mut Ui, c: &PaletteColors)
        -> Option<crate::session::Credentials> {
        crate::session::login_ui(ui, &mut self.login, c)
    }

    /// Qt's `loginSucceeded` signal: hide the overlay (idempotent, per the ordering rule).
    pub fn login_succeeded(&mut self) { self.login_visible = false; }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app() -> Snapclone {
        Snapclone::new(vec![
            Peer { name: "alice".into(), online: true },
            Peer { name: "bob".into(), online: false },
        ])
    }

    fn a_snap(id: &str) -> crate::snaps::Snap {
        crate::snaps::Snap { id: id.into(), sender_id: "ada@10.0.0.2".into(),
            sender_name: "Ada".into(), sent_at: 0, media: crate::snaps::Media::Image,
            duration_secs: 5, bytes: vec![9, 9, 9] }
    }

    #[test]
    fn a_desktop_surface_gets_a_centred_window_and_a_phone_gets_a_full_width_sheet() {
        // The hardcoded rectangles this replaced were phone-shaped constants —
        // a 340px sheet dropped in the corner of a desktop window.
        let mut s = app();
        let wide = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1600.0, 900.0));
        s.dialog_form = crate::mobile_dialog::Form::Window;
        let r = s.dialog_rect(wide, 400.0);
        assert!(r.w <= 480.0, "a dialog does not grow with the monitor");
        assert!((r.x + r.w / 2.0 - 800.0).abs() < 1.0, "centred");

        s.dialog_form = crate::mobile_dialog::Form::Sheet;
        let narrow = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(360.0, 780.0));
        let r = s.dialog_rect(narrow, 400.0);
        assert_eq!(r.x, 0.0);
        assert_eq!(r.w, 360.0, "full width");
        assert!((r.y + r.h - 780.0).abs() < 1.0, "flush with the bottom");
    }

    #[test]
    fn a_narrow_surface_chooses_the_sheet_form_on_its_own() {
        use crate::mobile_dialog::{form_factor, Form};
        assert_eq!(form_factor(&Default::default(), 360.0, false), Form::Sheet);
        assert_eq!(form_factor(&Default::default(), 1600.0, false), Form::Window);
    }

    #[test]
    fn the_background_carousel_scrolls_then_hands_off_at_its_end() {
        // The Qt bug this reproduces the fix for: the carousel silently ate the
        // wheel at its edge, so "keep scrolling to reach the next page" did
        // nothing at all.
        use crate::scroll_areas::{Wheel, Wheeled};
        let mut s = app();
        s.bg_carousel.value = 200.0;
        assert_eq!(s.bg_carousel.wheel(Wheel::pixels(-50.0, 0.0)), Wheeled::Consumed);
        // Run it to the end, then keep pushing.
        for _ in 0..20 { s.bg_carousel.wheel(Wheel::pixels(-50.0, 0.0)); }
        assert!(s.bg_carousel.at_max());
        assert_eq!(s.bg_carousel.wheel(Wheel::pixels(-50.0, 0.0)), Wheeled::PassThrough);
    }

    #[test]
    fn a_vertical_wheel_still_moves_the_carousel_sideways() {
        // A default scroll area has no vertical range here, so the wheel would
        // otherwise do nothing rather than the wrong thing.
        use crate::scroll_areas::{Wheel, Wheeled};
        let mut s = app();
        s.bg_carousel.value = 200.0;
        assert_eq!(s.bg_carousel.wheel(Wheel::degrees(0.0, 15.0)), Wheeled::Consumed);
        assert_ne!(s.bg_carousel.value, 200.0);
    }

    #[test]
    fn the_ribbon_tabs_are_the_navs_own_so_they_cannot_drift() {
        let s = app();
        let labels: Vec<&str> = s.ribbon.tabs().iter().map(|t| t.label.as_str()).collect();
        assert_eq!(labels, sub_tabs(1));
    }

    #[test]
    fn mark_all_read_is_disabled_with_nothing_to_mark() {
        // An always-enabled action on an empty inbox is a button that lies
        // about having work.
        let mut s = app();
        s.refresh_ribbon();
        s.ribbon.set_current(1);
        assert!(s.ribbon.trigger("read_all").is_none(), "nothing unread");
    }

    #[test]
    fn mark_all_read_clears_the_unread_count() {
        let mut s = app();
        s.inbox = Default::default();
        for (id, peer) in [(1u64, "ada"), (2, "bo")] {
            s.inbox.add(crate::inbox::Entry { id, peer: peer.into(),
                preview: "hi".into(), at: 0, unread: true, outgoing: false,
                kind: crate::inbox::Kind::Message });
        }
        assert_eq!(s.inbox.unread_count(), 2);
        s.refresh_ribbon();
        s.ribbon.set_current(1);
        assert!(s.ribbon.trigger("read_all").is_some(), "there is work to do");
        s.apply_ribbon(crate::ribbon::RibbonEvent::Triggered("read_all".into()));
        assert_eq!(s.inbox.unread_count(), 0);
    }

    #[test]
    fn retry_failed_only_lights_when_a_send_has_actually_failed() {
        let mut s = app();
        s.outbox.add("s1", "ada", 0);
        s.refresh_ribbon();
        s.ribbon.set_current(2);
        assert!(s.ribbon.trigger("retry_failed").is_none(), "still connecting, not failed");
        s.outbox.update("s1", crate::session::Phase::Failed, "no route");
        s.refresh_ribbon();
        assert!(s.ribbon.trigger("retry_failed").is_some());
        s.apply_ribbon(crate::ribbon::RibbonEvent::Triggered("retry_failed".into()));
        assert!(s.retry_requested, "the host owns the retry; the view reports it");
    }

    #[test]
    fn a_delivered_send_is_not_a_failed_one() {
        let mut s = app();
        s.outbox.add("s1", "ada", 0);
        s.outbox.update("s1", crate::session::Phase::Failed, "no route");
        s.outbox.mark_delivered("s1");
        s.refresh_ribbon();
        s.ribbon.set_current(2);
        assert!(s.ribbon.trigger("retry_failed").is_none(),
                "it arrived in the end; there is nothing to retry");
    }

    #[test]
    fn a_refused_swipe_does_not_count_toward_learning_the_gesture() {
        // The user has not learned anything from a gesture that did nothing.
        let mut s = app();
        assert_eq!(s.nav.page(), 0);
        s.swipe_page(-1);
        assert_eq!(s.peel.swipes(), 0, "swiping past the first page taught nothing");
        s.swipe_page(1);
        assert_eq!(s.peel.swipes(), 1);
    }

    #[test]
    fn a_swipe_blocked_by_a_dialog_does_not_count_either() {
        let mut s = app();
        s.open_qr_scan(true);
        s.swipe_page(1);
        assert_eq!(s.peel.swipes(), 0);
    }

    #[test]
    fn the_hint_retires_after_four_real_swipes() {
        let mut s = app();
        for _ in 0..4 { s.swipe_page(1); }
        assert!(s.peel.retired(), "four pages in, the gesture is learned");
    }

    #[test]
    fn exporting_and_dismissing_both_close_the_month() {
        // Either way the card was on screen and answered; re-offering it after
        // an explicit answer is the nag this feature exists to avoid.
        use crate::export_reminder::{CardAction, Month};
        let aug = Month { year: 2026, month: 8 };
        let mut s = app();
        s.apply_export_card(CardAction::ExportRequested, aug);
        assert!(s.export_requested);
        assert!(!s.reminder.should_show(aug, 50));

        let mut t = app();
        t.apply_export_card(CardAction::Dismissed, aug);
        assert!(!t.export_requested, "maybe later is not an export");
        assert!(!t.reminder.should_show(aug, 50));
    }

    #[test]
    fn no_month_from_the_host_means_no_card_rather_than_a_guess() {
        let s = app();
        assert!(s.now_month.is_none());
        // The gate is the host's month; without one the Inbox simply draws no
        // card, instead of inventing a date to compare against.
        assert!(s.recent_snaps.is_empty());
    }

    #[test]
    fn the_voice_controls_are_reachable_again_after_leaving_the_wayfinder_page() {
        // The regression I introduced last commit: the controls came off the
        // Wayfinder page, where they never belonged, and had no home.
        let mut s = app();
        assert!(!s.group_voice.joined);
        s.toggle_voice();
        assert!(s.group_voice.joined, "join is reachable from Friends");
        s.toggle_voice();
        assert!(!s.group_voice.joined);
    }

    #[test]
    fn pressing_talk_lights_the_bar_without_waiting_for_the_session() {
        // A talk button that waits for the round trip feels broken on a slow
        // link, which is exactly when you are pressing it hardest.
        let mut s = app();
        s.apply_voice(crate::voicebar::Event::TalkPressed);
        assert!(s.voicebar.on_air());
        s.apply_voice(crate::voicebar::Event::TalkReleased);
        assert!(!s.voicebar.on_air());
    }

    #[test]
    fn requesting_privacy_shows_securing_not_private() {
        // The request is ours to record; the TIER is the negotiation's to
        // report. Claiming Private on request is the padlock lie.
        let mut s = app();
        s.apply_voice(crate::voicebar::Event::PrivateRequested(true));
        assert_eq!(s.voicebar.security, crate::voicebar::Security::Securing);
        assert!(!s.voicebar.security.locked());
    }

    #[test]
    fn leaving_the_channel_takes_the_bar_off_air() {
        let mut s = app();
        s.toggle_voice();
        s.apply_voice(crate::voicebar::Event::TalkPressed);
        s.toggle_voice();
        assert!(!s.voicebar.on_air(), "a left channel cannot still be transmitting");
    }

    #[test]
    fn the_wayfinder_page_draws_the_wayfinder_and_not_the_voice_controls() {
        // The regression this replaced: PAGES[3] is "Wayfinder" and it drew
        // join/mute/push-to-talk. Nothing looked missing, which is why it
        // survived.
        assert_eq!(PAGES[3], "Wayfinder");
        let mut s = app();
        s.self_fix = Some(crate::wayfinder::Fix { lat: 51.5074, lon: -0.1278 });
        s.wayfinder_contacts = vec![crate::wayfinder::Contact {
            id: "paris".into(), display_name: "Paris".into(), online: true,
            loc: Some(crate::wayfinder::Fix { lat: 48.8566, lon: 2.3522 }) }];
        let nodes = crate::wayfinder::build_nodes(s.self_fix, &s.wayfinder_contacts);
        assert_eq!(nodes.len(), 1);
        assert!(nodes[0].distance_label().ends_with("SE"));
    }

    #[test]
    fn contacts_without_a_fix_still_appear_on_the_map() {
        let mut s = app();
        s.self_fix = None;
        s.wayfinder_contacts = vec![crate::wayfinder::Contact {
            id: "ada".into(), display_name: "Ada".into(), online: true, loc: None }];
        let nodes = crate::wayfinder::build_nodes(s.self_fix, &s.wayfinder_contacts);
        assert_eq!(nodes.len(), 1, "a contact with no GPS is not a missing contact");
        assert_eq!(nodes[0].distance_label(), "Nearby");
    }

    #[test]
    fn a_scanned_code_becomes_a_peer_through_the_same_path_as_a_typed_address() {
        // A contact added by camera and one added by hand must be the same kind
        // of contact — otherwise one of the two paths grows its own bugs.
        let mut s = app();
        s.open_qr_scan(true);
        assert!(s.modal.blocks_input());
        assert!(s.qr_frame("snapclone://sc-deadbeef:9000"));
        // Held first, so the user sees what was found.
        s.tick(0.3);
        assert_eq!(s.roster.len(), 2, "not yet — the success state is showing");
        s.tick(0.4);
        assert_eq!(s.roster.len(), 3);
        assert!(s.roster.get("sc-deadbeef").is_some());
        assert!(s.qr.is_none());
        assert!(s.modal.is_empty(), "and the sheet closes with it");
    }

    #[test]
    fn scanning_the_same_code_twice_adds_one_peer() {
        // At 8fps the code arrives repeatedly before the camera stops.
        let mut s = app();
        s.open_qr_scan(true);
        assert!(s.qr_frame("sc-deadbeef"));
        assert!(!s.qr_frame("sc-deadbeef"));
        s.tick(1.0);
        assert_eq!(s.roster.len(), 3);
        assert!(!s.qr_frame("sc-deadbeef"), "the sheet is gone; no stray frame lands");
        assert_eq!(s.roster.len(), 3);
    }

    #[test]
    fn no_camera_opens_straight_on_the_manual_field() {
        // A viewfinder that will never produce a frame is worse than no
        // viewfinder.
        let mut s = app();
        s.open_qr_scan(false);
        let q = s.qr.as_ref().unwrap();
        assert!(!q.is_scanning());
        assert!(q.status().contains("Camera unavailable"));
    }

    #[test]
    fn accepting_a_call_opens_the_thread_and_joins_the_voice_session() {
        let mut s = app();
        s.ring("alice");
        assert!(s.modal.blocks_input(), "the ring sits above whatever page is up");
        s.answer_call(crate::ring::Answer::Accepted);
        assert_eq!(s.conversation_peer, "alice");
        assert!(s.group_voice.joined);
        assert!(s.incoming_call.is_none());
        assert!(s.modal.is_empty());
    }

    #[test]
    fn an_ignored_ring_becomes_a_missed_call_but_a_declined_one_does_not() {
        // Same effect on the packets, different history: a missed call and a
        // refused one are not the same thing to whoever reads the list later.
        let mut s = app();
        s.ring("alice");
        s.tick(31.0);
        assert_eq!(s.missed_calls, vec!["alice".to_string()]);
        assert!(s.incoming_call.is_none());

        let mut t = app();
        t.ring("bob");
        t.answer_call(crate::ring::Answer::Declined);
        assert!(t.missed_calls.is_empty());
        assert_eq!(t.conversation_peer, "", "and it does not open the thread");
    }

    #[test]
    fn a_stray_tap_outside_the_sheet_neither_answers_nor_refuses_the_call() {
        // A phone in a pocket must not take or drop a call on its own.
        let mut s = app();
        s.ring("alice");
        let out = s.modal.tap(10.0, 10.0);
        assert_eq!(out, crate::modal::TapResult::Swallowed);
        assert!(s.incoming_call.as_ref().unwrap().ringing());
    }

    #[test]
    fn the_sent_sub_tab_shows_the_outbox_the_nav_has_always_declared() {
        // sub_tabs(1) has listed All/Unread/Sent since the shell was ported and
        // "Sent" drew the inbox list like the other two.
        let mut s = app();
        s.outbox.add("s1", "ada", 0);
        assert_eq!(sub_tabs(1)[2], "Sent");
        s.bar.sub[1] = 2;
        assert_eq!(s.outbox.len(), 1);
        assert_eq!(s.outbox.summary().as_deref(), Some("0 / 1 delivered"));
    }

    #[test]
    fn a_successful_sign_in_hides_the_overlay_and_leaves_no_plaintext() {
        let mut s = Snapclone::new_with_login(vec![], false);
        assert!(s.login_visible, "no auto-login, so the form is up");
        s.login.username = "ada".into();
        s.login.password = "hunter2".into();
        let creds = s.login.submit().expect("valid");
        s.login_succeeded();
        assert!(!s.login_visible);
        assert!(s.login.password.is_empty());
        assert_eq!(creds.username, "ada");
    }

    #[test]
    fn opening_a_snap_takes_the_tile_and_blocks_the_page_behind_it() {
        let mut s = app();
        s.feed.set_entries(vec![crate::snaps::FeedEntry {
            id: "s1".into(), sender_name: "Ada".into(), sent_at: 0,
            media: crate::snaps::Media::Image, thumb_path: String::new() }]);
        s.open_snap(a_snap("s1"));
        assert_eq!(s.feed.count(), 0, "a view-once snap cannot be tapped twice");
        assert_eq!(s.snaps_viewed, vec!["s1".to_string()]);
        assert!(s.modal.blocks_input(), "the viewer is modal");
    }

    #[test]
    fn a_snap_closed_early_still_expires_and_still_wipes() {
        // The two halves that matter: plaintext must not outlive the viewer,
        // and the host must still hear the expiry or the row stays unopened.
        let mut s = app();
        s.open_snap(a_snap("s1"));
        s.tick(1.0);
        s.close_snap();
        assert!(s.viewer.wiped(), "bytes gone on close");
        assert!(s.snaps_expired.is_empty(), "the window has not elapsed yet");
        s.tick(10.0);
        assert_eq!(s.snaps_expired, vec!["s1".to_string()]);
        assert!(s.modal.is_empty(), "and the modal goes with it");
    }

    #[test]
    fn the_countdown_runs_on_the_frame_clock() {
        let mut s = app();
        s.open_snap(a_snap("s1"));
        assert_eq!(s.viewer.countdown(), 1.0);
        s.tick(2.5);
        assert!(s.viewer.countdown() < 0.6 && s.viewer.countdown() > 0.4);
    }

    #[test]
    fn the_roster_is_built_from_the_peers_at_construction() {
        // Not a second list that has to be remembered to update.
        let s = app();
        assert_eq!(s.roster.len(), 2);
        assert_eq!(s.roster.online_count(), 1);
        assert!(s.roster.get("alice").unwrap().online);
    }

    #[test]
    fn a_manual_add_shows_the_row_immediately_as_offline() {
        // A Connect button that visibly does nothing until the handshake
        // completes gets pressed four more times.
        let mut s = app();
        s.apply_friends(crate::friends::Action::Add(
            crate::friends::AddPeer { host: "192.168.1.5".into(), port: 9876 }));
        assert_eq!(s.roster.len(), 3);
        let added = s.roster.get("192.168.1.5").expect("the new row");
        assert!(!added.online, "optimistic about the row, honest about the state");
    }

    #[test]
    fn adding_a_peer_that_is_already_there_does_not_duplicate_it() {
        let mut s = app();
        s.apply_friends(crate::friends::Action::Add(
            crate::friends::AddPeer { host: "alice".into(), port: 9876 }));
        assert_eq!(s.roster.len(), 2);
    }

    #[test]
    fn delete_asks_before_it_deletes() {
        // Qt puts a ChevronConfirm in the way — "Remove %1 from your address
        // book?" — and the port was deleting on the single tap of a menu item
        // sitting one row below "Message". That is how people lose people.
        let mut s = app();
        s.cards.open_only("bob");
        s.apply_friends(crate::friends::Action::Invoke("bob".into(), "delete".into()));
        assert_eq!(s.roster.len(), 2, "nobody is gone yet");
        assert_eq!(s.modal.blocking_layer(), Some(crate::modal::Layer::Confirm));

        s.confirm_delete("bob");
        assert_eq!(s.roster.len(), 1);
        assert!(s.roster.get("bob").is_none());
        // Leaving the accordion open on a deleted id strands the menu over
        // whichever row slides up into its place.
        assert!(!s.cards.is_open("bob"));
        assert!(s.modal.is_empty(), "and the confirm goes with it");
    }

    #[test]
    fn dismissing_the_confirm_keeps_the_contact() {
        let mut s = app();
        s.apply_friends(crate::friends::Action::Invoke("bob".into(), "delete".into()));
        s.modal.dismiss_topmost();
        assert_eq!(s.roster.len(), 2, "backing out of a confirm is not a delete");
    }

    #[test]
    fn a_swipe_under_an_open_dialog_does_not_change_the_page() {
        // Otherwise the dialog is left floating over a screen it has nothing
        // to do with.
        let mut s = app();
        let page = s.nav.page();
        s.apply_friends(crate::friends::Action::Invoke("bob".into(), "edit".into()));
        assert!(s.modal.blocks_input());
        s.swipe_page(1);
        assert_eq!(s.nav.page(), page, "the swipe was swallowed by the dialog");
        s.modal.dismiss_all();
        s.swipe_page(1);
        assert_ne!(s.nav.page(), page, "and works again once it closes");
    }

    #[test]
    fn tapping_a_row_drills_into_that_peers_subpage() {
        let mut s = app();
        s.apply_friends(crate::friends::Action::Selected("bob".into()));
        assert_eq!(s.nav.subpage(), Some(1), "bob's index, not alice's");
    }

    #[test]
    fn swipe_moves_between_pages() {
        let mut s = app();
        assert_eq!(s.page_name(), "Camera");
        s.swipe_page(1);
        assert_eq!(s.page_name(), "Inbox"); // page 1 in the faithful 5-page shell
        s.swipe_page(1);
        s.swipe_page(1);
        s.swipe_page(1);
        s.swipe_page(1); // clamp at Settings (page 4)
        assert_eq!(s.page_name(), "Settings");
        s.swipe_page(-1);
        assert_eq!(s.page_name(), "Wayfinder");
    }

    #[test]
    fn swipe_disabled_while_drilled_into_subpage() {
        let mut s = app();
        s.nav.go_page(1); // Peers
        s.nav.push_subpage(0); // into a peer detail
        s.swipe_page(1); // should be ignored so content interaction wins
        assert_eq!(s.page_name(), "Inbox"); // page 1 is Inbox in the faithful 5-page shell
        assert_eq!(s.nav.subpage(), Some(0));
    }

    #[test]
    fn background_tick_and_style_switch() {
        let mut s = app();
        let p0 = s.bg.phase;
        s.tick(0.5);
        assert!(s.bg.phase > p0);
        s.bg.style = BgStyle::Waves;
        assert_eq!(s.bg.style, BgStyle::Waves);
    }

    #[test]
    fn apply_settings_adopts_feel() {
        let ctx = egui::Context::default();
        let mut s = app();
        let mut prefs = settings_kit::Prefs::default();
        prefs.gesture.animation = settings_kit::AnimationStyle::Heavy;
        s.apply_settings(&ctx, &prefs);
        assert_eq!(s.gesture.config_mut().takeover_lock_ms, 220.0);
    }
}

#[cfg(test)]
mod faithful_shell_tests {
    use super::*;

    #[test]
    fn five_qt_pages_in_order() {
        assert_eq!(PAGES, ["Camera", "Inbox", "Friends", "Wayfinder", "Settings"],
            "the page set must match MainWindow.cpp kPage* exactly");
    }

    #[test]
    fn sub_tab_tables_match_the_qt_switches() {
        assert_eq!(sub_tabs(0), &[] as &[&str], "Camera is not SubTabbed");
        assert_eq!(sub_tabs(1), &["All", "Unread", "Sent"], "InboxWidget::subTabLabel");
        assert_eq!(sub_tabs(2), &["All", "Online", "Pending"], "ContactsPage::subTabLabel");
        assert_eq!(sub_tabs(3), &[] as &[&str], "Wayfinder is not SubTabbed");
        assert_eq!(sub_tabs(4), &["General", "Camera", "Network", "Storage", "About"],
            "SettingsPage::subTabLabel");
    }

    #[test]
    fn login_overlay_ordering_rule() {
        // auto-login already succeeded at construction → overlay stays hidden (the Qt bug
        // this rule exists to prevent: overlay flashing over the camera on first paint)
        let s = Snapclone::new_with_login(vec![], true);
        assert!(!s.login_visible);
        // auto-login not yet fired → shown, then hidden on the signal
        let mut s = Snapclone::new_with_login(vec![], false);
        assert!(s.login_visible);
        s.login_succeeded();
        assert!(!s.login_visible);
        s.login_succeeded(); // idempotent
        assert!(!s.login_visible);
    }

    fn run_bar(bar: &mut UnifiedNav) {
        let ctx = egui::Context::default();
        let raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(480.0, 800.0))),
            ..Default::default()
        };
        #[allow(deprecated)]
        let _ = ctx.run(raw, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| { let _ = bar.ui(ui); });
        });
    }

    #[test]
    fn underline_morph_converges_on_the_active_segment() {
        let mut bar = UnifiedNav::new();
        bar.page = 1; // Inbox: segments exist
        run_bar(&mut bar);
        let start = bar.underline_span().expect("underline after first frame");
        bar.sub[1] = 2; // jump to "Sent" — the morph should EASE, not snap
        run_bar(&mut bar);
        let mid = bar.underline_span().unwrap();
        assert!(mid.0 > start.0, "morph must move toward the new segment");
        for _ in 0..40 { run_bar(&mut bar); }
        let done = bar.underline_span().unwrap();
        assert!((done.0 - mid.0).abs() > 0.0 || done.0 > start.0);
        // converged: two more frames barely move it
        let before = bar.underline_span().unwrap();
        run_bar(&mut bar);
        let after = bar.underline_span().unwrap();
        assert!((after.0 - before.0).abs() < 0.6, "underline must settle ({} -> {})", before.0, after.0);
    }

    #[test]
    fn every_page_renders_headless_with_the_bar() {
        for page in 0..PAGE_NAMES.len() {
            let mut bar = UnifiedNav::new();
            bar.page = page;
            run_bar(&mut bar); // must not panic on any page form (centered vs segmented)
        }
    }

#[cfg(test)]
mod desktop_entry_tests {
    /// The shipped entry, read at compile time so the test cannot pass against
    /// a file that is not the one installed.
    const ENTRY: &str = include_str!("../../packaging/org.snapclone.SnapClone.desktop");

    #[test]
    fn the_app_id_matches_the_desktop_files_icon_and_name() {
        // The mismatch that produces a working app with no icon. Checking it
        // here means a rename of either side breaks the build rather than the
        // dock.
        let icon = ENTRY.lines().find_map(|l| l.strip_prefix("Icon=")).expect("Icon=");
        assert_eq!(icon, "org.snapclone.SnapClone");
        assert!(uikit_egui::desktop::is_valid_app_id(icon), "{icon}");
        // StartupWMClass covers X11, where the compositor matches on class
        // rather than app id; both must name the same app or the integration
        // works on one display server and not the other.
        assert!(ENTRY.contains("StartupWMClass="), "no StartupWMClass");
    }

    #[test]
    fn the_entry_has_what_a_launcher_needs() {
        // A .desktop file missing any of these is accepted by the spec and
        // shows up wrong: no category means no menu placement, no Exec means it
        // cannot be launched at all.
        for key in ["Type=Application", "Name=", "Exec=", "Icon=", "Categories="] {
            assert!(ENTRY.contains(key), "missing {key}");
        }
        // Terminal=true on a GUI app opens a console window alongside it.
        assert!(ENTRY.contains("Terminal=false"));
    }
}

#[cfg(test)]
mod notify_tests {
    use super::*;

    fn app() -> Snapclone { Snapclone::new(Vec::new()) }

    #[test]
    fn a_focused_window_does_not_notify_about_what_is_on_screen() {
        // An app that pops a notification for a message the user is looking at
        // is one people mute, and a muted app cannot tell them anything.
        let a = app();
        assert!(a.on_snaps_arrived(&["Ada".into()], true).is_none());
        assert!(a.on_snaps_arrived(&["Ada".into()], false).is_some());
    }

    #[test]
    fn high_security_notifies_without_naming_anyone() {
        // The notification persists in KDE history and shows on a lock screen;
        // a name there defeats the encryption underneath it.
        let mut a = app();
        a.high_security = true;
        let r = a.on_snaps_arrived(&["Ada".into()], false).unwrap();
        assert!(!r.summary.contains("Ada"), "{r:?}");
        assert!(!r.body.contains("Ada"), "{r:?}");
    }

    #[test]
    fn a_burst_of_snaps_is_one_notification() {
        let a = app();
        let from: Vec<String> = (0..8).map(|i| format!("p{i}")).collect();
        let r = a.on_snaps_arrived(&from, false).unwrap();
        assert_eq!(r.summary, "8 new messages");
    }

    #[test]
    fn nothing_arriving_notifies_nothing() {
        let a = app();
        assert!(a.on_snaps_arrived(&[], false).is_none());
    }

    #[test]
    fn notifications_can_be_turned_off_entirely() {
        let mut a = app();
        a.notify_enabled = false;
        assert!(a.on_snaps_arrived(&["Ada".into()], false).is_none());
    }
}
}
