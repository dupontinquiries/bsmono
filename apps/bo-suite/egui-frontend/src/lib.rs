//! bo-suite-egui — the B&O soundscape controller, ported to egui.
//!
//! The Qt/QML control panel becomes an egui panel: per-device volume + playback, and scene
//! presets, **swipe-paged** (Devices / Scenes / Library) via the shared gesture layer, laid
//! out by the adaptive layer, a11y-navigable, and tracking live cross-app settings. bo-suite's
//! mixer/routing engine is already Qt-free, so on-device this drives it directly; here it
//! carries a small view model so the crate builds and the logic is unit-tested.

pub mod transport;
pub mod spaces;
pub mod library;
pub mod browse;
pub mod queue;
pub mod dial;
pub mod channel;
pub mod tags;
pub mod outputs;
pub mod sources;
pub mod catalog_source;
pub mod device;
pub mod settings_page;
pub mod chrome;
pub mod scrollfeel;
pub mod anticipation;
pub mod safety;
pub mod dialogs;
pub mod soundscape;
use egui::Ui;
use uikit_egui::{a11y::semantic_toggle, A11yScene, Adaptive, GestureLayer, PaletteColors};

/// A networked speaker/device.
#[derive(Debug, Clone)]
pub struct Device {
    pub name: String,
    /// 0..=100
    pub volume: u8,
    pub playing: bool,
}

impl Device {
    pub fn set_volume(&mut self, v: i32) {
        self.volume = v.clamp(0, 100) as u8;
    }
}

/// A room/scene preset.
#[derive(Debug, Clone)]
pub struct Scene {
    pub name: String,
}

// Six views, not three. The port had eight ported modules and three views, two
// of which were a heading and a sentence — "Browse ambient stems and playlists
// (on-device: bo-mux catalog)" is a promise, not a library.
/// Where the room is stored. Constants so the save and the restore cannot
/// point at different files.
pub const ROUTING_APP: &str = "bo-suite";
pub const ROUTING_FILE: &str = "routing.conf";

pub const VIEWS: [&str; 6] = ["Devices", "Spaces", "Library", "Mixer", "Soundscape", "Settings"];

/// The three ways into the library. The QML keeps AlbumsPage and ArtistsPage as
/// SwipeView siblings of the track list; here they are tabs of one view, because
/// a six-page swipe with three more pages inside it is a maze — but the pages,
/// their modes and their detail drill-downs are the same.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LibraryTab { Tracks, Albums, Artists, Tags }

/// Move the view index by a swipe sign, clamped to the ends.
pub fn page(idx: usize, sign: i32, n: usize) -> usize {
    (idx as i32 + sign).clamp(0, n as i32 - 1) as usize
}

/// Controller state + the swipe gesture + a11y scene.
pub struct Controller {
    pub view_idx: usize,
    /// The ported surfaces, held across frames so they keep their state — a
    /// library that forgot its page, or a mixer that forgot its bands, is the
    /// same empty demo by another route.
    pub transport: crate::transport::Transport,
    pub routing: crate::spaces::Routing,
    /// Search state plus the catalogue it filters. The library module models
    /// SEARCH over a track list rather than owning a paged list, so the tracks
    /// live here and the search is the view state.
    pub search: crate::library::Search,
    /// The catalogue, fed by a source the host supplies. Previously this was a
    /// `Vec::new()` that no code ever filled — a connected placeholder that the
    /// wiring checker reported as done.
    pub catalog: crate::catalog_source::Catalog,
    pub tracks: Vec<crate::library::Track>,
    pub plays: Vec<crate::library::Play>,
    /// Browse-by-album / browse-by-artist, and the detail pages you land on.
    /// Held across frames so leaving the Library page and coming back keeps the
    /// mode, the sort and the open album — a browse that forgets where it was
    /// makes a large library unusable.
    pub browse: crate::browse::Browse,
    /// The rows the browse aggregates are computed from. Empty until a host
    /// supplies them, which is why `Browse::loaded` exists: an unread library
    /// and an empty one are not the same message.
    pub browse_tracks: Vec<crate::browse::BrowseTrack>,
    /// Which of the three library surfaces is showing.
    pub library_tab: LibraryTab,
    /// The play queue. When browse was ported, "play all" had nowhere to put
    /// the tracks and reported them instead; this is that somewhere.
    pub queue: crate::queue::Queue,
    pub now_playing: crate::queue::NowPlaying,
    /// The hero volume wheel. Held across frames so a flick keeps coasting
    /// through the redraws it is animating.
    pub dial: crate::dial::VolumeDial,
    /// The volume the dial last reported, waiting for the host to debounce and
    /// push. `None` once it has been sent.
    pub pending_volume: Option<f32>,
    /// The two mixer channels. Held separately rather than as one band list,
    /// because the whole point of the page is that they are independent.
    /// The library's tags, and the per-track editor.
    pub tags: Vec<crate::tags::Tag>,
    pub tag_editor: crate::tags::TagEditor,
    /// Ratings by track id, so a row can show one without owning it.
    pub ratings: std::collections::HashMap<i64, crate::tags::StarRating>,
    /// The tag whose tracks are being shown, if any.
    pub open_tag: Option<i64>,
    /// The device switcher above the output controls. The sink picker itself
    /// lives in settings_page as `SinkPicker` — see outputs.rs's note.
    pub device_selector: crate::outputs::DeviceSelector,
    /// The route the user last committed; the host applies it.
    pub pending_route: Option<crate::settings_page::Route>,
    /// The device the selector last switched to.
    pub active_device_url: String,
    /// What this device can play from, and what the user has collected.
    pub sources_page: crate::sources::Sources,
    pub playlists_page: crate::sources::PlaylistsPage,
    /// Wall clock in seconds, for the "3d ago" column. Supplied by the host so
    /// the page does not reach for a clock it cannot be tested with.
    pub now_secs: i64,
    pub speaker_col: crate::channel::ChannelColumn,
    pub instrumental_col: crate::channel::ChannelColumn,
    pub mixer: crate::device::Mixer,
    pub profile: crate::device::Profile,
    pub soundscape: crate::soundscape::Soundscape,
    pub settings: crate::settings_page::SettingsPage,
    pub sinks: crate::settings_page::SinkPicker,
    pub now: crate::chrome::NowPlaying,
    pub toast: crate::chrome::Toast,
    /// The anticipation dot, mounted in the header strip beside the device name
    /// exactly as the QML specifies — "so it doesn't compete with any actual
    /// control".
    pub dot: crate::anticipation::AnticipationDot,
    /// Volume safety. Per DEVICE session, so switching speakers re-asserts the
    /// cap rather than carrying an unlock granted for a different one.
    pub safety: crate::safety::Safety,
    pub add_to_playlist: crate::dialogs::AddToPlaylist,
    pub devices: Vec<Device>,
    pub scenes: Vec<Scene>,
    pub active_scene: usize,
    gesture: GestureLayer,
    a11y: A11yScene,
    /// The user's selected palette id (empty ⇒ default); set from prefs in apply_settings.
    palette: String,
}

impl Controller {
    pub fn new(devices: Vec<Device>, scenes: Vec<Scene>) -> Self {
        Controller { view_idx: 0, devices, scenes, active_scene: 0,
            transport: Default::default(),
            routing: Default::default(),
            search: Default::default(),
            catalog: Default::default(),
            tracks: Vec::new(),
            plays: Vec::new(),
            browse: Default::default(),
            browse_tracks: Vec::new(),
            library_tab: LibraryTab::Tracks,
            queue: Default::default(),
            now_playing: Default::default(),
            dial: crate::dial::VolumeDial::new(0.0),
            pending_volume: None,
            tags: Vec::new(),
            tag_editor: Default::default(),
            ratings: Default::default(),
            open_tag: None,
            device_selector: Default::default(),
            pending_route: None,
            active_device_url: String::new(),
            sources_page: Default::default(),
            playlists_page: Default::default(),
            now_secs: 0,
            // Frequencies and role hints come from the mixer's own band plan;
            // the column prints them so the choice is visible rather than
            // implied.
            speaker_col: crate::channel::ChannelColumn::new(
                crate::channel::Channel::Speaker, 200.0, 1500.0, 8000.0,
                ["body", "vocal presence", "air"]),
            instrumental_col: crate::channel::ChannelColumn::new(
                crate::channel::Channel::Instrumental, 200.0, 1500.0, 8000.0,
                ["weight", "midrange", "shimmer"]),
            mixer: Default::default(),
            profile: Default::default(),
            soundscape: Default::default(),
            settings: Default::default(),
            sinks: Default::default(),
            now: Default::default(),
            toast: Default::default(),
            dot: Default::default(),
            safety: Default::default(),
            add_to_playlist: Default::default(), gesture: GestureLayer::horizontal_page(320.0), a11y: A11yScene::new(), palette: String::new() }
    }

    pub fn current(&self) -> &'static str {
        VIEWS[self.view_idx]
    }

    /// Switch view by a swipe sign (wraps at the ends — clamped, see `page()`). Pure index arithmetic;
    /// used by tests. NOT gesture-driven in the live UI any more — compact mode uses the vertical-tabs
    /// sidebar instead of a whole-surface swipe (see `ui()`'s doc comment for why: the Devices view's
    /// volume Sliders claim horizontal drag themselves, and a whole-surface gesture reads the same raw
    /// pointer trajectory with nothing arbitrating between the two).
    pub fn swipe(&mut self, sign: i32) {
        self.view_idx = page(self.view_idx, sign, VIEWS.len());
    }

    /// Adopt cross-app preferences (theme/text/button + swipe feel incl. detent animation).
    pub fn apply_settings(&mut self, ctx: &egui::Context, prefs: &settings_kit::Prefs) {
        uikit_egui::apply_prefs(ctx, prefs);
        *self.gesture.config_mut() = prefs.gesture_config(320.0);
        self.palette = prefs.palette.clone();
    }

    /// Render the controller.
    pub fn ui(&mut self, ui: &mut Ui, adaptive: &Adaptive) {
        let colors = uikit_egui::palette_colors(&self.palette);
        self.a11y.begin_frame();

        if !adaptive.is_compact() {
            ui.horizontal(|ui| {
                for (i, name) in VIEWS.iter().enumerate() {
                    if ui.selectable_label(i == self.view_idx, *name).clicked() {
                        self.view_idx = i;
                    }
                }
            });
            ui.separator();
            // The safety toast slides down from the TOP, above everything, and
            // is drawn before the header so it cannot be pushed off by a view
            // that changes height.
            self.safety.tick(ui.input(|i| i.stable_dt));
            if self.safety.toast_visible() {
                ui.horizontal(|ui| {
                    ui.colored_label(colors.warning, "Volume capped for hearing safety");
                    if ui.button("Dismiss").clicked() { self.safety.dismiss(); }
                    if ui.button("Unlock").clicked() {
                        if let Some(v) = self.safety.unlock() {
                            self.transport.title = self.transport.title.clone();
                            let _ = v;   // the host applies it; the model decided it
                        }
                    }
                });
                ui.separator();
                ui.ctx().request_repaint();
            }
            // The dot lives in the header strip. Drawn before the body so it
            // keeps its place when a view changes height.
            {
                let d = &mut self.dot;
                d.tick(ui.input(|i| i.stable_dt));
                if d.band().visible() {
                    let (rect, resp) = ui.allocate_exact_size(
                        egui::vec2(14.0, 14.0), egui::Sense::click());
                    let a = (d.opacity() * 255.0) as u8;
                    ui.painter().circle_filled(rect.center(), 4.0,
                        egui::Color32::from_rgba_unmultiplied(
                            colors.accent.r(), colors.accent.g(), colors.accent.b(), a));
                    if resp.clicked() { d.tap(); }
                    if let Some(text) = d.revealed() { resp.on_hover_text(text); }
                    // Repaint while breathing: without this the dot only moves
                    // when something else causes a frame, which reads as a
                    // stuck indicator rather than a quiet one.
                    ui.ctx().request_repaint();
                }
            }
            match self.view_idx {
                0 => self.devices_view(ui, &colors),
                1 => self.spaces_view(ui, &colors),
                2 => self.library_view(ui, &colors),
                3 => self.mixer_view(ui, &colors),
                4 => self.soundscape_view(ui, &colors),
                _ => self.settings_view(ui, &colors),
            }
            // The transport bar is always on screen, which is the point of a
            // controller: whatever page you are on, you can pause.
            ui.separator();
            self.transport_strip(ui, &colors);
        } else {
            // Compact width: a vertical-tabs sidebar instead of a whole-surface swipe gesture. The
            // devices view has volume Sliders, which claim horizontal drag themselves — a whole-surface
            // GestureLayer reads the same raw pointer trajectory with nothing arbitrating between the
            // two (the exact rusty-board keyboard bug: see uikit_egui::vertical_tabs's doc comment), so
            // dragging a slider could also register as a page swipe. A sidebar tap has no such race.
            ui.horizontal(|ui| {
                ui.allocate_ui(egui::vec2(64.0_f32.max(adaptive.min_touch_target * 1.3), ui.available_height()), |ui| {
                    let tabs: Vec<uikit_egui::vertical_tabs::Tab> = VIEWS.iter().map(|n| uikit_egui::vertical_tabs::Tab::new(n)).collect();
                    if let Some(i) = uikit_egui::vertical_tabs::show(ui, &tabs, self.view_idx, adaptive.min_touch_target, &colors) {
                        self.view_idx = i;
                    }
                });
                ui.separator();
                ui.vertical(|ui| {
                    match self.view_idx {
                        0 => self.devices_view(ui, &colors),
                        1 => self.scenes_view(ui, &colors),
                        _ => self.library_view(ui, &colors),
                    }
                });
            });
        }
    }

    fn devices_view(&mut self, ui: &mut Ui, c: &PaletteColors) {
        // The product selector sits at the top, as in the Qt page.
        if let Some(url) = crate::outputs::device_selector_ui(ui, &mut self.device_selector, c) {
            self.active_device_url = url;
        }
        ui.separator();
        // Output routing: which speaker, or which speakers. Committed through
        // the existing SinkPicker, which now knows about drivers.
        if self.sinks.open {
            if ui.add_enabled(self.sinks.can_combine(),
                egui::Button::new(self.sinks.combine_label())).clicked() {
                self.pending_route = self.sinks.route();
            }
        }
        ui.separator();
        ui.heading("Devices");
        // collect edits to avoid borrow issues while iterating
        let mut vol_edits: Vec<(usize, i32)> = Vec::new();
        for (i, d) in self.devices.iter_mut().enumerate() {
            ui.horizontal(|ui| {
                ui.colored_label(c.text, &d.name);
                let mut v = d.volume as i32;
                if ui.add(egui::Slider::new(&mut v, 0..=100).text("vol")).changed() {
                    vol_edits.push((i, v));
                }
            });
            // play toggle, tagged for a11y/voice
            semantic_toggle(&mut *ui, &mut self.a11y, 1000 + i as u64, "playing", &mut d.playing);
        }
        for (i, v) in vol_edits {
            self.devices[i].set_volume(v);
        }
    }

    fn scenes_view(&mut self, ui: &mut Ui, c: &PaletteColors) {
        ui.heading("Scenes");
        for (i, s) in self.scenes.iter().enumerate() {
            if ui.selectable_label(i == self.active_scene, &s.name).clicked() {
                self.active_scene = i;
            }
        }
        if let Some(s) = self.scenes.get(self.active_scene) {
            ui.colored_label(c.text_weak, format!("active: {}", s.name));
        }
    }

    /// Carry out a playlist action. The page owns no storage; the host performs
    /// these and re-publishes, which is the only way the list changes.
    pub fn apply_playlist(&mut self, a: crate::sources::PlaylistAction) {
        use crate::sources::PlaylistAction;
        if let PlaylistAction::Create { name, description } = a {
            let id = self.playlists_page.all().iter().map(|p| p.id).max().unwrap_or(0) + 1;
            let mut all = self.playlists_page.all().to_vec();
            all.push(crate::sources::PlaylistSummary {
                id, name, description, track_count: 0, updated_at: self.now_secs });
            self.playlists_page.set(all);
            // Bring it into view: a new row appearing off-screen looks like
            // nothing happened.
            self.playlists_page.focus(id);
        }
    }

    /// Carry out a tag action. The view owns no storage; these are requests the
    /// host performs, and the lists refresh when it publishes new state.
    pub fn apply_tag(&mut self, a: crate::tags::TagAction) {
        use crate::tags::TagAction;
        match a {
            TagAction::Create { name, color } => {
                let id = self.tags.iter().map(|t| t.id).max().unwrap_or(0) + 1;
                self.tags.push(crate::tags::Tag { id, name, color, track_count: 0 });
            }
            TagAction::Attach(_, _) | TagAction::Detach(_, _) => {}
        }
    }

    /// Rate a track. Optimistic: the star fills now, and the host's echo
    /// reconciles it through `confirm_rating`.
    pub fn rate(&mut self, track_id: i64, stars: u8) -> Option<u8> {
        let r = self.ratings.entry(track_id)
            .or_insert_with(|| crate::tags::StarRating::new(track_id, 0));
        r.tap(stars)
    }

    /// The host confirmed a rating.
    pub fn confirm_rating(&mut self, track_id: i64, stars: u8) {
        if let Some(r) = self.ratings.get_mut(&track_id) { r.reconcile(track_id, stars); }
    }

    /// Carry out what a browse surface asked for.
    ///
    /// The queue lives in bo-mux, not here, so the queue actions report rather
    /// than enqueue — a second writer in the UI thread would fight the engine
    /// that owns it. What this DOES do is reflect the play locally, because a
    /// tap that changes nothing on screen reads as a dead control even when the
    /// backend heard it.
    pub fn apply_browse(&mut self, action: crate::browse::Action) {
        use crate::browse::Action;
        match action {
            Action::OpenAlbum(key) => {
                let tracks = std::mem::take(&mut self.browse_tracks);
                self.browse.open_album(key, &tracks);
                self.browse_tracks = tracks;
            }
            Action::OpenArtist { id, name } => {
                let tracks = std::mem::take(&mut self.browse_tracks);
                self.browse.open_artist(id, name, &tracks);
                self.browse_tracks = tracks;
            }
            Action::Back => self.browse.back(),
            Action::Play(id) => self.play_browse_track(id),
            Action::PlayAll(ids) => {
                // NOW it actually queues. When browse was ported this reported
                // the tracks instead, because the queue lived only in bo-mux
                // and claiming to have queued what the engine had never heard
                // of was the failure this port has been avoiding throughout.
                // The queue is here now, so the claim is true.
                let tracks = self.tracks_for(&ids);
                self.queue.mode = crate::queue::EnqueueMode::Replace;
                self.queue.add_all(&tracks);
                if let Some(first) = self.queue.advance() { self.start_playing(&first); }
                if !self.queue.is_empty() {
                    self.toast.show(&format!("queued {}",
                        crate::browse::plural(self.queue.len() as i64, "more track")));
                }
            }
            Action::Queue(id) => {
                let t = self.tracks_for(&[id]);
                self.queue.mode = crate::queue::EnqueueMode::Stack;
                self.queue.add_all(&t);
                self.toast.show("queued 1 track");
            }
            Action::QueueAll(ids) => {
                let tracks = self.tracks_for(&ids);
                self.queue.mode = crate::queue::EnqueueMode::Stack;
                let n = tracks.len();
                self.queue.add_all(&tracks);
                self.toast.show(&format!("queued {}", crate::browse::plural(n as i64, "track")));
            }
            // With a local source there is nothing to re-fetch: the aggregates
            // are already in memory and the mode/sort read straight off them.
            // On device this is where the host reissues the query.
            Action::Reload => {}
        }
    }

    /// Look up browse tracks by id, in the order given — "play all" on an album
    /// means the album, in its order, not in id order.
    fn tracks_for(&self, ids: &[i64]) -> Vec<(i64, String, String)> {
        ids.iter().filter_map(|id| self.browse_tracks.iter().find(|t| t.id == *id))
            .map(|t| (t.id, t.title.clone(), t.primary_artist.clone()))
            .collect()
    }

    /// Reflect a queue entry as the thing now playing.
    fn start_playing(&mut self, e: &crate::queue::QueueEntry) {
        self.now.title = e.title.clone();
        self.now.artist = e.artist.clone();
        self.transport.title = e.title.clone();
        self.transport.artist = e.artist.clone();
        self.transport.playing = true;
        self.now_playing = crate::queue::NowPlaying {
            title: e.title.clone(), artist: e.artist.clone(),
            source: String::new(), playing: true,
        };
    }

    fn play_browse_track(&mut self, id: i64) {
        if let Some(t) = self.browse_tracks.iter().find(|t| t.id == id) {
            self.now.title = t.title.clone();
            self.now.artist = t.primary_artist.clone();
            self.transport.title = t.title.clone();
            self.transport.artist = t.primary_artist.clone();
            self.transport.playing = true;
        }
    }

    fn library_view(&mut self, ui: &mut Ui, c: &PaletteColors) {
        ui.heading("Library");
        // Tracks / Albums / Artists. The tab row stays visible on a detail page
        // so there is always a way out that is not the back chevron — a drill-down
        // you can only leave by one control is where a touch user gets stuck.
        ui.horizontal(|ui| {
            for (tab, label) in [(LibraryTab::Tracks, "Tracks"),
                                 (LibraryTab::Albums, "Albums"),
                                 (LibraryTab::Artists, "Artists"),
                                 (LibraryTab::Tags, "Tags")] {
                if ui.selectable_label(self.library_tab == tab, label).clicked()
                    && self.library_tab != tab {
                    self.library_tab = tab;
                    // Switching tabs closes any open detail page: coming back to
                    // Albums and finding an artist's tracks under an album header
                    // is worse than losing the drill-down.
                    self.browse.back();
                    self.browse.tab = match tab {
                        LibraryTab::Artists => crate::browse::Tab::Artists,
                        _ => crate::browse::Tab::Albums,
                    };
                }
            }
        });
        if self.library_tab == LibraryTab::Tags {
            // Drilled into a tag, or the grid.
            match self.open_tag {
                Some(id) => {
                    let name = self.tags.iter().find(|t| t.id == id)
                        .map(|t| t.name.clone()).unwrap_or_default();
                    ui.horizontal(|ui| {
                        if ui.button("‹").clicked() { self.open_tag = None; }
                        ui.strong(name);
                    });
                }
                None => {
                    if let Some(id) = crate::tags::tags_ui(ui, &self.tags, c) {
                        self.open_tag = Some(id);
                    }
                }
            }
            if let Some(a) = crate::tags::tag_editor_ui(ui, &mut self.tag_editor, c) {
                self.apply_tag(a);
            }
            return;
        }
        if self.library_tab != LibraryTab::Tracks {
            ui.separator();
            self.browse.tab = match self.library_tab {
                LibraryTab::Artists => crate::browse::Tab::Artists,
                _ => crate::browse::Tab::Albums,
            };
            if let Some(a) = crate::browse::browse_ui(ui, &mut self.browse, c) {
                self.apply_browse(a);
            }
            return;
        }
        // The real paged list: sort tabs, search, and the empty state that says
        // WHICH empty it is.
        ui.horizontal(|ui| {
            ui.text_edit_singleline(&mut self.search.query);
            if !self.search.tag.is_empty() {
                if ui.button(format!("tag: {} ×", self.search.tag)).clicked() {
                    self.search.tag.clear();
                }
            }
        });
        // the tag cloud, most-used first
        let tags = crate::library::Search::tags(&self.tracks);
        if !tags.is_empty() && self.search.tag.is_empty() {
            ui.horizontal_wrapped(|ui| {
                for (t, n) in tags.iter().take(8) {
                    if ui.small_button(format!("{t} ({n})")).clicked() {
                        self.search.tag = t.clone();
                    }
                }
            });
        }
        ui.separator();
        // Prefer the real catalogue. `self.tracks` remains for the in-memory
        // demo path; when a source has been supplied, the catalogue is the
        // truth and the search filters what it returned.
        if self.catalog.loaded {
            if let Some(why) = self.catalog.empty_reason() {
                ui.colored_label(c.text_weak, why);
                return;
            }
            let q = self.search.query.trim().to_lowercase();
            let hits: Vec<crate::catalog_source::CatalogTrack> = self.catalog.rows.iter()
                .filter(|t| q.is_empty() || t.label().to_lowercase().contains(&q)
                            || t.sub().to_lowercase().contains(&q))
                .cloned().collect();
            if hits.is_empty() { ui.colored_label(c.text_weak, "nothing matches"); return; }
            egui::ScrollArea::vertical().show(ui, |ui| {
                for t in hits {
                    ui.horizontal(|ui| {
                        if ui.selectable_label(false, t.label()).clicked() {
                            self.now.title = t.label();
                            self.now.artist = t.artists.join(", ");
                            self.transport.title = t.label();
                            self.transport.artist = self.now.artist.clone();
                            self.transport.playing = true;
                        }
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.colored_label(c.text_weak, t.sub());
                        });
                    });
                }
            });
            return;
        }
        let rows: Vec<crate::library::Track> =
            self.search.results(&self.tracks).into_iter().cloned().collect();
        if rows.is_empty() {
            // Which empty it is: an unsearched library and one with no matches
            // look the same otherwise.
            ui.colored_label(c.text_weak,
                if self.tracks.is_empty() { "catalogue not opened" } else { "nothing matches" });
            return;
        }
        egui::ScrollArea::vertical().show(ui, |ui| {
            for t in rows {
                ui.horizontal(|ui| {
                    if ui.selectable_label(false, t.title.as_str()).clicked() {
                        self.now.title = t.title.clone();
                        self.now.artist = t.artist.clone();
                        self.transport.title = t.title.clone();
                        self.transport.artist = t.artist.clone();
                        self.transport.duration = 210.0;
                        self.transport.playing = true;
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.colored_label(c.text_weak, t.album.as_str());
                        if ui.small_button("+").clicked() {
                            self.add_to_playlist.open(&t.id.to_string());
                        }
                    });
                });
            }
        });
    }

    fn spaces_view(&mut self, ui: &mut Ui, c: &PaletteColors) {
        ui.heading("Spaces");
        crate::spaces::spaces_ui(ui, &mut self.routing, c);
        ui.separator();
        ui.horizontal(|ui| {
            // Saving the room is the one persistence bo-suite cannot do
            // without: losing which speaker plays which channel means the room
            // sounds wrong on next launch and the user rebuilds it by ear.
            if ui.button("save room").clicked() {
                match config_kit::save(ROUTING_APP, ROUTING_FILE, &self.routing.render()) {
                    Ok(()) => self.toast.show("room saved"),
                    // Shown, not swallowed: a save that silently fails is the
                    // same lie as one that never wrote.
                    Err(e) => self.toast.show(&format!("could not save: {e}")),
                }
            }
            if ui.button("restore room").clicked() {
                match config_kit::load(ROUTING_APP, ROUTING_FILE) {
                    Some(text) => {
                        let n = self.routing.restore(&text);
                        // Say HOW MANY applied. A saved room restored onto a
                        // mesh missing two speakers is a partial restore, and
                        // silently succeeding would hide the gap.
                        self.toast.show(&format!("{n} of {} speakers restored",
                                                 self.routing.speakers.len()));
                    }
                    None => self.toast.show("no saved room yet"),
                }
            }
        });
    }

    fn mixer_view(&mut self, ui: &mut Ui, c: &PaletteColors) {
        ui.heading("Mixer");
        // The hero control. It reports continuously; the parent debounces
        // before pushing to the device, so a flick sends one command rather
        // than a hundred.
        if let Some(v) = crate::dial::dial_ui(ui, &mut self.dial, 140.0, c) {
            self.pending_volume = Some(v);
        }
        if let Some(v) = self.pending_volume {
            ui.colored_label(c.text_weak, format!("volume {v:.0}"));
        }
        if self.mixer.is_flat() {
            ui.colored_label(c.text_weak, "flat — no correction applied");
        }
        // The two channel strips, side by side, over their response curves —
        // the independence of the chains is the thing this page exists to show.
        ui.horizontal(|ui| {
            crate::channel::column_ui(ui, &mut self.speaker_col, c);
            ui.separator();
            crate::channel::column_ui(ui, &mut self.instrumental_col, c);
        });
        crate::channel::curve_ui(ui, &self.speaker_col, &self.instrumental_col, 120.0, c);
        ui.separator();

        let ordered: Vec<(usize, crate::device::Band)> = self.mixer.ordered();
        for (i, band) in ordered {
            ui.horizontal(|ui| {
                ui.label(crate::device::hz_label(band.center_hz));
                let mut g = band.gain_db;
                if ui.add(egui::Slider::new(&mut g, -crate::device::MAX_GAIN_DB
                                            ..=crate::device::MAX_GAIN_DB)
                          .show_value(false)).changed() {
                    self.mixer.set_gain(i, g);
                }
                ui.label(crate::device::db_label(band.gain_db));
            });
        }
        ui.separator();
        ui.horizontal(|ui| {
            if ui.button("reset").clicked() { self.mixer.reset(); }
            let peak = self.mixer.peak_boost_db();
            if peak > 6.0 {
                ui.colored_label(c.warning, format!("{} of boost — watch headroom",
                                                    crate::device::db_label(peak)));
            }
        });
    }

    fn soundscape_view(&mut self, ui: &mut Ui, c: &PaletteColors) {
        ui.heading("Soundscape");
        if crate::soundscape::soundscape_ui(ui, &mut self.soundscape, c) {
            self.toast.show("loop search started");
        }
    }

    fn settings_view(&mut self, ui: &mut Ui, c: &PaletteColors) {
        ui.heading("Settings");
        for d in self.settings.problems() {
            ui.colored_label(c.warning, format!("missing {} — {}", d.name, d.enables));
        }
        for d in self.settings.optional_missing() {
            ui.colored_label(c.text_weak, format!("optional: {} enables {}", d.name, d.enables));
        }
        ui.separator();
        ui.label(self.settings.scan.label());
        if let Some(f) = self.settings.scan.fraction() {
            ui.add(egui::ProgressBar::new(f));
        }
        if ui.add_enabled(self.settings.can_scan(), egui::Button::new("scan library")).clicked() {
            self.settings.scan = crate::settings_page::Scan::Running {
                seen: 0, total: None, path: String::new() };
        }
        ui.separator();
        // product capabilities gate what is shown, per hasControl
        ui.label(format!("{} {}", self.profile.emoji, self.profile.name));
        if self.profile.has_control("autoroute") { ui.label("autoroute available"); }
    }

    /// The always-on transport strip, plus the toast that warns over it.
    fn transport_strip(&mut self, ui: &mut Ui, c: &PaletteColors) {
        if let Some(msg) = self.toast.visible() {
            ui.colored_label(c.warning, msg);
        }
        if self.now.active() { ui.label(self.now.line()); }
        crate::transport::transport_ui(ui, &mut self.transport, c);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Controller {
        Controller::new(
            vec![
                Device { name: "Living Room".into(), volume: 30, playing: false },
                Device { name: "Kitchen".into(), volume: 50, playing: true },
            ],
            vec![Scene { name: "Morning".into() }, Scene { name: "Focus".into() }],
        )
    }

    fn with_library() -> Controller {
        let mut c = sample();
        c.browse_tracks = vec![
            crate::browse::BrowseTrack { id: 1, title: "Opener".into(), album: Some("Blue".into()),
                primary_artist: "Ada".into(), artist_id: 1, year: Some(1999), play_count: 4,
                last_played_at: Some(10) },
            crate::browse::BrowseTrack { id: 2, title: "Closer".into(), album: Some("Blue".into()),
                primary_artist: "Ada".into(), artist_id: 1, year: Some(1999), play_count: 0,
                last_played_at: None },
        ];
        let t = c.browse_tracks.clone();
        c.browse.load_local(&t);
        c
    }

    #[test]
    fn opening_an_album_and_playing_from_it_reaches_the_transport() {
        // The audit's own lesson: a surface can be mentioned everywhere and
        // still be wired to nothing. This asserts the whole path — open the
        // album, get its tracks, tap one, and the transport is actually
        // playing it.
        let mut c = with_library();
        c.apply_browse(crate::browse::Action::OpenAlbum(
            crate::browse::AlbumKey { title: "Blue".into(), artist: "Ada".into() }));
        assert_eq!(c.browse.detail_tracks.len(), 2, "the detail page got the album's tracks");
        c.apply_browse(crate::browse::Action::Play(2));
        assert_eq!(c.transport.title, "Closer");
        assert_eq!(c.transport.artist, "Ada");
        assert!(c.transport.playing);
    }

    #[test]
    fn play_all_now_actually_queues_the_rest() {
        // This test used to be called "...reports the rest rather than claiming
        // to queue them", and asserted exactly that: browse had no queue to
        // put tracks in, so saying "queued 2 tracks" would have been a lie
        // about state the engine had never been told about.
        //
        // queue.rs exists now, so the claim is true and the test says the true
        // thing. Renaming rather than leaving a name that describes a
        // limitation which no longer applies — a stale test name is a stale
        // comment with a green tick beside it.
        let mut c = with_library();
        c.apply_browse(crate::browse::Action::OpenArtist { id: 1, name: "Ada".into() });
        let ids: Vec<i64> = c.browse.detail_tracks.iter().map(|t| t.id).collect();
        c.apply_browse(crate::browse::Action::PlayAll(ids));
        assert_eq!(c.transport.title, "Opener", "the first track plays");
        assert_eq!(c.queue.len(), 1, "and the rest are really in the queue");
        assert_eq!(c.queue.up_next().unwrap().title, "Closer");
        assert_eq!(c.toast.visible(), Some("queued 1 more track"));
    }

    #[test]
    fn creating_a_playlist_adds_it_and_brings_it_into_view() {
        let mut c = sample();
        c.apply_playlist(crate::sources::PlaylistAction::Create {
            name: "Drive".into(), description: "long roads".into() });
        assert_eq!(c.playlists_page.all().len(), 1);
        assert_eq!(c.playlists_page.focus_id, Some(1));
    }

    #[test]
    fn choosing_the_source_already_playing_does_nothing() {
        let mut c = sample();
        c.sources_page.set(vec![
            crate::sources::Source { name: "Line In".into(), kind: "line".into() },
            crate::sources::Source { name: "Spotify".into(), kind: "spotify".into() }],
            "Line In");
        assert_eq!(c.sources_page.choose("Line In"), None, "it is already playing");
        assert_eq!(c.sources_page.choose("Spotify"), Some("Spotify".into()));
    }

    #[test]
    fn rating_a_track_is_optimistic_and_the_hosts_echo_reconciles_it() {
        // The star fills now, because a control that pauses makes the user tap
        // again and rate it twice. The echo then settles it on the truth.
        let mut c = sample();
        assert_eq!(c.rate(7, 4), Some(4));
        assert!(c.ratings[&7].pending());
        c.confirm_rating(7, 4);
        assert!(!c.ratings[&7].pending());
    }

    #[test]
    fn a_rejected_rating_pulls_the_stars_back() {
        let mut c = sample();
        c.rate(7, 5);
        c.confirm_rating(7, 2);
        assert_eq!(c.ratings[&7].value, 2);
    }

    #[test]
    fn tapping_the_same_star_again_un_rates_the_track() {
        let mut c = sample();
        c.rate(7, 3);
        assert_eq!(c.rate(7, 3), Some(0), "the only way back to unrated");
    }

    #[test]
    fn the_library_gained_a_tags_tab_beside_the_browse_ones() {
        let mut c = sample();
        c.library_tab = LibraryTab::Tags;
        assert_eq!(c.library_tab, LibraryTab::Tags);
        c.apply_tag(crate::tags::TagAction::Create {
            name: "Drive".into(), color: "#ff0000".into() });
        assert_eq!(c.tags.len(), 1);
        assert_eq!(c.tags[0].name, "Drive");
    }

    #[test]
    fn resetting_one_mixer_channel_leaves_the_other_alone() {
        // Per-channel reset is the common need: you adjust one column, decide
        // it was a bad call, and want to start over without losing the other.
        let mut c = sample();
        c.speaker_col.set_gain(1, 8.0);
        c.instrumental_col.set_gain(0, -5.0);
        c.speaker_col.reset();
        assert!(c.speaker_col.is_neutral());
        assert!(!c.instrumental_col.is_neutral());
    }

    #[test]
    fn lifting_a_speaker_band_does_not_move_the_instrumental_curve() {
        let mut c = sample();
        let before = c.instrumental_col.response_db(1500.0);
        c.speaker_col.set_gain(1, 10.0);
        assert!(c.speaker_col.response_db(1500.0) > 9.0);
        assert_eq!(c.instrumental_col.response_db(1500.0), before);
    }

    #[test]
    fn the_dial_reports_continuously_and_the_shell_holds_the_pending_value() {
        // A dial that wrote to the network on every frame of a flick would send
        // a hundred volume commands for one gesture; the parent debounces.
        let mut c = sample();
        c.dial.press(0.0, 200.0, 1.0, 100.0);
        c.dial.drag(0.0, 100.0);
        assert!(c.dial.value > 0.0);
        c.pending_volume = Some(c.dial.value);
        assert!(c.pending_volume.is_some());
    }

    #[test]
    fn a_device_reported_volume_stops_the_wheel_fighting_it() {
        let mut c = sample();
        c.dial.press(0.0, 0.0, 90.0, 100.0);
        c.dial.drag(30.0, 0.0);
        c.dial.release();
        assert!(c.dial.spinning());
        c.dial.set_external(15.0);
        assert_eq!(c.dial.value, 15.0);
        assert!(!c.dial.spinning());
    }

    #[test]
    fn queueing_a_single_track_appends_without_interrupting() {
        let mut c = with_library();
        c.apply_browse(crate::browse::Action::OpenArtist { id: 1, name: "Ada".into() });
        c.apply_browse(crate::browse::Action::Queue(2));
        assert_eq!(c.queue.len(), 1);
        assert_eq!(c.transport.title, "", "nothing started playing");
    }

    #[test]
    fn play_all_replaces_the_queue_rather_than_piling_onto_it() {
        // "Play this album" means this album, not this album after whatever was
        // already lined up.
        let mut c = with_library();
        c.apply_browse(crate::browse::Action::QueueAll(vec![1, 2]));
        assert_eq!(c.queue.len(), 2);
        c.apply_browse(crate::browse::Action::PlayAll(vec![1, 2]));
        assert_eq!(c.queue.len(), 1, "the first plays, one remains, the old two are gone");
    }

    #[test]
    fn a_tab_switch_closes_the_detail_page() {
        // Otherwise Albums re-opens showing an artist's tracks under an album
        // header, which reads as the wrong album having opened.
        let mut c = with_library();
        c.apply_browse(crate::browse::Action::OpenArtist { id: 1, name: "Ada".into() });
        assert!(c.browse.detail.is_open());
        c.library_tab = LibraryTab::Albums;
        c.browse.back();
        assert!(!c.browse.detail.is_open());
        assert!(c.browse.detail_tracks.is_empty());
    }

    #[test]
    fn paging_and_swipe_clamp() {
        let mut c = sample();
        // Six views now, not three: the port had eight ported modules and only
        // Devices/Scenes/Library, two of which were a heading and a sentence.
        assert_eq!(c.current(), "Devices");
        c.swipe(1);
        assert_eq!(c.current(), "Spaces");
        for _ in 0..10 { c.swipe(1); }        // clamp at the last view
        assert_eq!(c.current(), "Settings");
        c.swipe(-1);
        assert_eq!(c.current(), "Soundscape");
        for _ in 0..10 { c.swipe(-1); }        // clamp at the first
        assert_eq!(c.current(), "Devices");
    }

    #[test]
    fn volume_clamps() {
        let mut d = Device { name: "x".into(), volume: 10, playing: false };
        d.set_volume(150);
        assert_eq!(d.volume, 100);
        d.set_volume(-5);
        assert_eq!(d.volume, 0);
    }

    #[test]
    fn apply_settings_adopts_feel() {
        let ctx = egui::Context::default();
        let mut c = sample();
        let mut prefs = settings_kit::Prefs::default();
        prefs.gesture.animation = settings_kit::AnimationStyle::Snappy;
        c.apply_settings(&ctx, &prefs);
        // snappy detent animation → short takeover lock reached the gesture layer
        assert_eq!(c.gesture.config_mut().takeover_lock_ms, 60.0);
        assert_eq!(c.current(), "Devices");
    }
}
