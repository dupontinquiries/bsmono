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
        self.nav.go_page(next);
    }

    /// Advance the animation; call once per frame with the frame delta.
    pub fn tick(&mut self, dt: f32) {
        self.bg.tick(dt);
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
        ui.horizontal_wrapped(|ui| {
            ui.colored_label(c.text_weak, "background:");
            for s in BgStyle::all() {
                if ui.selectable_label(self.bg.style == s, s.as_str()).clicked() {
                    self.bg.style = s;
                }
            }
        });
    }

    /// Wayfinder: the group voice channel and the landmark tools. Was a dead
    /// tab.
    fn wayfinder_page(&mut self, ui: &mut Ui, c: &PaletteColors) {
        self.nav.title_bar(ui, "Wayfinder");
        ui.horizontal(|ui| {
            if ui.selectable_label(self.group_voice.joined, "join channel").clicked() {
                if self.group_voice.joined { self.group_voice.leave(); }
                else { self.group_voice.join(); }
            }
            if ui.selectable_label(self.group_voice.muted, "mute").clicked() {
                self.group_voice.muted = !self.group_voice.muted;
            }
            let ptt = self.group_voice.mode == crate::group_voice::TalkMode::PushToTalk;
            if ui.selectable_label(ptt, "push to talk").clicked() {
                self.group_voice.mode = if ptt { crate::group_voice::TalkMode::Open }
                                        else { crate::group_voice::TalkMode::PushToTalk };
            }
        });
        ui.separator();
        crate::group_voice::roster_ui(ui, &self.group_voice, c);
    }

    /// The inbox, on its own page. Was absent entirely: the app had a Peers
    /// list and no way to see a message, which is the gap that made a
    /// messaging app read as a demo.
    fn inbox_page(&mut self, ui: &mut Ui, c: &PaletteColors) {
        self.nav.title_bar(ui, "Messages");
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
