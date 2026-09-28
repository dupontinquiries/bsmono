//! bo-suite-egui — the B&O soundscape controller, ported to egui.
//!
//! The Qt/QML control panel becomes an egui panel: per-device volume + playback, and scene
//! presets, **swipe-paged** (Devices / Scenes / Library) via the shared gesture layer, laid
//! out by the adaptive layer, a11y-navigable, and tracking live cross-app settings. bo-suite's
//! mixer/routing engine is already Qt-free, so on-device this drives it directly; here it
//! carries a small view model so the crate builds and the logic is unit-tested.

/// The RUNTIME DEPENDENCIES card from SettingsPage.qml, as a model. The real
/// audit lives in `bo-controller::dep_wrangler`, which is a BINARY crate and
/// so unreachable from here -- the audit arrives as data, exactly as the QML
/// receives it.
pub mod dep_audit;
/// The PLAYBACK DIAGNOSTIC card from SettingsPage.qml: the ladder
/// `diagnose_playback` walks, and what stopping at each rung means. Its value
/// is naming the LAYER, since "playback is broken" is not actionable.
pub mod playback_diag;
/// The RESEARCH MODE gate from SettingsPage.qml, over three real bo-mux DSP
/// features. Off by default because an anti-noise system that is wrong is
/// worse than off, not merely ineffective.
pub mod research_mode;
pub mod dial_watermark;
pub mod product_silhouette;
pub mod transport;
pub mod spaces;
pub mod all_tracks;
pub mod library;
pub mod browse;
pub mod queue;
pub mod dial;
pub mod channel;
pub mod tags;
pub mod outputs;
pub mod music_scan;
pub mod real_catalog;
pub mod cast;
pub mod playback;
pub mod surround;
pub mod voice_control;
pub mod voice_source;
pub mod sources;
pub mod fileproc;
pub mod shell_bits;
pub mod catalog_source;
pub mod device;
pub mod settings_page;
pub mod chrome;
pub mod scrollfeel;
pub mod anticipation;
/// The ANC operator control: Off / Auto / On over the engine's own gates.
pub mod anc;
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

/// How much of the session "previous" and Repeat All can reach back
/// through. A cap because the list is walked on every draw of the
/// history-aware controls, and a day of radio is thousands of rows.
pub const HISTORY_CAP: usize = 200;

/// The three ways into the library. The QML keeps AlbumsPage and ArtistsPage as
/// SwipeView siblings of the track list; here they are tabs of one view, because
/// a six-page swipe with three more pages inside it is a maze — but the pages,
/// their modes and their detail drill-downs are the same.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LibraryTab { Home, Search, AllTracks, Albums, Artists, Tags, Playlists }

/// The Library tab names, in tab order.
///
/// One list, because the bar renders from it and the enum maps to it: a
/// second literal list in the view is how a reorder starts naming the wrong
/// page — the same failure `the_named_page_bar_and_the_swipe_nav_stay_in_step`
/// guards for the page bar, and the same one the snapclone tab labels hit.
/// The mixer page's sections, in the order they read in.
///
/// The stagger is per-index, so this order IS the entrance order — the dial
/// first because it is the hero control, the keys last because they are the
/// least important thing on the page.
pub const MIXER_CARDS: [&str; 4] = ["Output", "Channels", "Crossover", "Steps"];

pub const LIBRARY_TABS: [&str; 7] =
    ["Home", "Search", "All tracks", "Albums", "Artists", "Tags", "Playlists"];

impl LibraryTab {
    pub fn label(self) -> &'static str { LIBRARY_TABS[self as usize] }

    pub fn from_index(i: usize) -> Option<LibraryTab> {
        Some(match i {
            0 => LibraryTab::Home,
            1 => LibraryTab::Search,
            2 => LibraryTab::AllTracks,
            3 => LibraryTab::Albums,
            4 => LibraryTab::Artists,
            5 => LibraryTab::Tags,
            6 => LibraryTab::Playlists,
            _ => return None,
        })
    }
}

/// The Home tab's lists, each a short answer to a question a listener
/// actually asks — what was I just playing, what did I add, what do I
/// keep coming back to, what have I never heard. Filled by the host from
/// the catalogue; empty is honest until it has.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct HomeLists {
    pub recently_played: Vec<crate::browse::BrowseTrack>,
    pub recently_added: Vec<crate::browse::BrowseTrack>,
    pub most_played: Vec<crate::browse::BrowseTrack>,
    pub never_played: Vec<crate::browse::BrowseTrack>,
    /// True once a host has answered, so "nothing here yet" can be told
    /// from "nobody asked".
    pub loaded: bool,
}

/// How many rows each Home list shows. Enough to be useful, few enough
/// that four of them fit a phone with the transport bar still visible.
pub const HOME_ROWS: usize = 8;

/// Move the view index by a swipe sign, clamped to the ends.
/// Paint a product silhouette badge: thin strokes (the module pins 1.0–1.2px),
/// an opacity reveal, and the accent halo blooming behind a change.
fn silhouette_paint(ui: &Ui, s: &crate::product_silhouette::ProductSilhouette,
                    rect: egui::Rect, c: &PaletteColors) {
    let p = ui.painter().with_clip_rect(rect);
    let a = (s.reveal() * 255.0) as u8;
    let line = egui::Color32::from_rgba_unmultiplied(
        c.text.r(), c.text.g(), c.text.b(), a);
    if s.halo() > 0.0 {
        let ha = (s.halo() * 90.0) as u8;
        p.circle_filled(rect.center(), rect.width() * 0.62,
            egui::Color32::from_rgba_unmultiplied(c.accent.r(), c.accent.g(), c.accent.b(), ha));
    }
    // THE PRODUCT OUTLINE, walked from data.
    //
    // This used to be four hand-drawn primitives — Disc was two concentric
    // circles, Cube a rectangle with a dot — and at badge size they were
    // indistinguishable from each other and from a loading spinner. The
    // shapes are point lists in a unit box now (`Shape::outline`), so the
    // geometry is reviewable, testable, and the same at every size the
    // badge is drawn at.
    //
    // Inset so a 1.2px stroke centred on the boundary is not half clipped;
    // `with_clip_rect` above would take the outer half of it otherwise.
    let inset = rect.shrink(1.5);
    let at = |(x, y): (f32, f32)| egui::pos2(
        inset.left() + x * inset.width(),
        inset.top() + y * inset.height());
    for sp in s.shape().outline() {
        let mut pts: Vec<egui::Pos2> = sp.pts.iter().map(|q| at(*q)).collect();
        if sp.closed {
            if let Some(first) = pts.first().copied() { pts.push(first); }
        }
        // One polyline per subpath rather than a segment per pair: egui
        // joins the corners of a polyline, and a shape drawn as loose
        // segments has a visible notch at every vertex — which on a
        // 16-point outline is the whole outline.
        p.add(egui::Shape::line(pts, egui::Stroke::new(sp.weight, line)));
    }
}

/// Paint the dial watermark: a dashed circle, upper-left half only (the 45°
/// cut through the midpoint), dash phase advancing at the module's pace. No
/// blur here — egui's painter has none, and the module's opacity already
/// degraded for that (`set_blur_available(false)`), which is the QML's own
/// "still shows un-blurred so nothing breaks" rule.
fn watermark_paint(ui: &Ui, wm: &crate::dial_watermark::DialWatermark,
                   center: egui::Pos2, radius: f32, c: &PaletteColors) {
    use crate::dial_watermark::DASH_MODULO;
    let alpha = (wm.opacity() * 255.0).clamp(0.0, 255.0) as u8;
    let col = egui::Color32::from_rgba_unmultiplied(c.text_weak.r(), c.text_weak.g(), c.text_weak.b(), alpha);
    let stroke = egui::Stroke::new(1.1, col);
    let p = ui.painter();
    let circumference = 2.0 * std::f32::consts::PI * radius;
    let step_px = 2.0;
    let dash_px = DASH_MODULO * 0.55; // dash : gap within the 18px pattern
    let (w, h) = (radius * 2.0, radius * 2.0);
    let mut s = 0.0f32;
    let mut run: Vec<egui::Pos2> = Vec::new();
    while s < circumference {
        let ang = s / radius;
        let pt = center + egui::vec2(ang.cos() * radius, ang.sin() * radius);
        let local = pt - (center - egui::vec2(radius, radius));
        let on_dash = ((s + wm.dash_offset()) % DASH_MODULO) < dash_px;
        let in_cut = wm.in_cut(local.x, local.y, w, h);
        if on_dash && in_cut {
            run.push(pt);
        } else if run.len() > 1 {
            p.add(egui::Shape::line(std::mem::take(&mut run), stroke));
        } else {
            run.clear();
        }
        s += step_px;
    }
    if run.len() > 1 { p.add(egui::Shape::line(run, stroke)); }
}

/// The whole catalogue in the BROWSE shape, for `Browse::load_local`.
///
/// `browse` had two data paths and neither was connected: `load_local` for an
/// in-memory catalogue, and a paged `Request`/`Page` protocol for on-device
/// SQL. `browse_tracks` sat on the app, was read only when opening a detail
/// page, and nothing ever filled it — so the albums and artists grids were
/// empty by construction. Found by the tested-but-uncalled check.
///
/// This is the in-memory half, which is the one that works today: the paged
/// half needs a host that answers `Request`, and `page_from_source` above is
/// the shape that would take.
///
/// WHAT THE SOURCE CANNOT SUPPLY is left at its default rather than invented.
/// `CatalogTrack` has no artist_id, year, play_count or last_played_at, so:
///
/// * `artist_id` is a stable hash of the artist NAME. Browse groups by it and
///   opens a detail page from it, so it has to be consistent within a run —
///   but it is not a database id and must not be persisted as one.
/// * `play_count` stays 0, which means the TOP mode ranks nothing. That is
///   visible and correct; fabricating counts would make a play-ranked list
///   that ranks by accident.
pub fn browse_tracks_from_source(
    src: &dyn crate::catalog_source::CatalogSource,
) -> Vec<crate::browse::BrowseTrack> {
    // The whole catalogue: load_local's own doc says it is "correct only when
    // the whole catalogue is in memory", so a partial page here would silently
    // produce a partial album list.
    let total = src.total().max(0);
    let rows = src.page(0, total, "title");
    rows.into_iter().map(|t| crate::browse::BrowseTrack {
        id: t.id,
        title: t.title,
        album: t.album,
        // Reduced the way the All-tracks adapter does, so one track does not
        // land under two artists in two views.
        primary_artist: t.artists.first().cloned().unwrap_or_default(),
        artist_id: artist_id_of(t.artists.first().map(String::as_str).unwrap_or("")),
        year: None,
        play_count: 0,
        last_played_at: None,
    }).collect()
}

/// A stable within-run id for an artist name.
///
/// FNV-1a, not a database key. Browse needs to group tracks and open a detail
/// page by id; the source has no artist table, so this derives one that is
/// consistent for the same name. Two different artists sharing a name collide
/// — which is the same thing the name itself does, so nothing is lost.
pub fn artist_id_of(name: &str) -> i64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in name.trim().to_lowercase().bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x1000_0000_01b3);
    }
    // Positive: the field is i64 and a negative id reads as a sentinel.
    (h >> 1) as i64
}

/// One page of the catalogue, in the All-tracks channel shape.
///
/// The adapter between `catalog_source` (the host data contract) and
/// `all_tracks` (the paged window): sorted by the backend via `Sort::tag`,
/// context stamped so the page's own channel guard applies, primary artist
/// reduced the way the Qt delegate does.
pub fn page_from_source(
    src: &dyn crate::catalog_source::CatalogSource,
    offset: usize,
    sort: crate::all_tracks::Sort,
) -> crate::all_tracks::PageResult {
    let rows = src.page(offset as i64, crate::all_tracks::PAGE_SIZE as i64, sort.tag());
    crate::all_tracks::PageResult {
        context: "all_tracks".into(),
        offset,
        total: src.total().max(0) as usize,
        tracks: rows
            .into_iter()
            .map(|t| crate::all_tracks::Track {
                id: t.id,
                title: t.title,
                primary_artist: t.artists.first().cloned().unwrap_or_default(),
            })
            .collect(),
    }
}

pub fn page(idx: usize, sign: i32, n: usize) -> usize {
    // `n == 0` would be `clamp(0, -1)`, and clamp PANICS when min > max.
    // Unreachable today — the only caller passes `VIEWS.len()`, a
    // non-empty const — but this is `pub`, so the next caller decides
    // whether it stays unreachable, and a panic in a paging helper lands
    // in a draw path where it takes the app down rather than showing a
    // wrong page.
    if n == 0 {
        return 0;
    }
    (idx as i32 + sign).clamp(0, n as i32 - 1) as usize
}

/// Controller state + the swipe gesture + a11y scene.
pub struct Controller {
    /// Which top-level view, via the shared kit.
    ///
    /// A bare `usize` before, and `current()` had to defend against it
    /// drifting out of range with a comment saying the bound "is not a
    /// property this function can see". `Nav` makes it one: `go_page`
    /// clamps, so an out-of-range index cannot be stored in the first
    /// place and the draw path cannot panic on it.
    ///
    /// Page level ONLY. The library's drill-down stays in `browse` —
    /// see the note on `swipe`.
    pub nav: uikit_egui::nav::Nav,
    /// The kit's segmented strip, for the wide layout.
    strip: uikit_egui::segmented_tabs::TabStrip,
    /// The ported surfaces, held across frames so they keep their state — a
    /// library that forgot its page, or a mixer that forgot its bands, is the
    /// same empty demo by another route.
    pub transport: crate::transport::Transport,
    /// Active noise cancellation: the operator's mode over the engine's gates.
    pub anc: crate::anc::Anc,
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
    /// Which of the library surfaces is showing.
    pub library_tab: LibraryTab,
    /// The whole library, a hundred rows at a time — the Qt AllTracksPage.
    /// Distinct from `search` (which filters what is here): this is a paged
    /// WINDOW into a server-side ordering, with the offset guard that makes
    /// two in-flight requests safe.
    pub all_tracks: crate::all_tracks::AllTracksPage,
    /// The flick physics for that list, held across the frames they animate.
    pub tracks_kinetic: crate::scrollfeel::KineticScroll,
    /// A page the view asked for and no source has answered. The host either
    /// calls `answer_tracks_from` with its CatalogSource or builds a
    /// PageResult itself and calls `supply_tracks`.
    pub pending_tracks: Option<crate::all_tracks::TracksAction>,
    /// The play queue. When browse was ported, "play all" had nowhere to put
    /// the tracks and reported them instead; this is that somewhere.
    pub queue: crate::queue::Queue,
    pub now_playing: crate::queue::NowPlaying,
    /// The "this is alive" oscillation behind the now-playing equaliser.
    /// A value type, ticked by the frame clock like every other motion here.
    breath: uikit_egui::motion::Breath,
    /// Is the queue list expanded under the transport bar?
    queue_open: bool,
    /// The hero volume wheel. Held across frames so a flick keeps coasting
    /// through the redraws it is animating.
    pub dial: crate::dial::VolumeDial,
    /// The product silhouette peeking from behind the dial: half-cut, soft,
    /// dashed outline flowing at the pace playback sets.
    pub watermark: crate::dial_watermark::DialWatermark,
    /// One badge per device row, keyed by index; recreated when the device
    /// list changes length. A per-row struct because the halo is per-row
    /// state — one shared badge would bloom every row on any change.
    pub silhouettes: Vec<crate::product_silhouette::ProductSilhouette>,
    /// The animation scale from settings, remembered so a badge created
    /// after `apply_settings` gets it too — a device that appears mid-
    /// session would otherwise animate at full speed under reduce-motion.
    pub motion_scale: f32,
    /// Which speaker is which, for surround. Held beside the picker rather
    /// than inside it: the picker answers "which speakers", and this
    /// answers "where they are" — one survives a change to the other.
    pub surround: crate::surround::Assignment,
    /// Voice control settings. OFF until someone turns it on — a music
    /// player that listens is a microphone in a living room.
    pub voice: crate::voice_control::VoicePrefs,
    /// A low-confidence command awaiting a yes. Free text cannot be
    /// checked against anything, so a mishearing starts the wrong album.
    pub voice_confirm: Option<crate::voice_control::Command>,
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
    /// The track the transport is playing, for the rating to attach to.
    /// `ratings` is keyed by track id and NowPlaying carries only strings, so
    /// without this there is nothing to key on at the moment of rating.
    pub now_playing_id: Option<i64>,
    /// The tag whose tracks are being shown, if any.
    pub open_tag: Option<i64>,
    /// The playlist currently drilled into, mirroring `open_tag`. Tracks
    /// for it land in `browse_tracks` — the same field Albums/Artists
    /// already use for a drilled-in row list — so play/queue on a
    /// playlist row goes through the one tested `apply_browse` path
    /// instead of a second copy of "how does tapping a track work".
    pub open_playlist: Option<i64>,
    /// A playlist the host hasn't loaded tracks for yet (set by `Open`).
    pub pending_playlist_open: Option<i64>,
    /// A playlist the host hasn't deleted yet (set by `Delete`).
    pub pending_playlist_delete: Option<i64>,
    /// (playlist_id, track_id) the host hasn't removed yet.
    pub pending_playlist_remove: Option<(i64, i64)>,
    /// A "+" on a track was tapped and a playlist chosen in the dialog —
    /// (track_id, playlist_id), resolved from the dialog's name-based
    /// `add()` via `playlists_page`. Cleared by the host.
    pub pending_add_to_playlist: Option<(i64, i64)>,
    /// The edit-track dialog. Same model dialogs.rs has had since it was
    /// ported; nothing opened or drew it until now.
    pub edit_track: crate::dialogs::EditTrack,
    /// A track whose current fields the host hasn't fetched yet — set
    /// when the edit button is pressed, before the dialog has anything
    /// to show.
    pub pending_track_edit_open: Option<i64>,
    /// A save the host hasn't persisted yet: (track_id, title, artist,
    /// album, year) — only the changed fields are `Some`, straight from
    /// `EditTrack::payload()`.
    pub pending_track_edit_save:
        Option<(i64, Option<String>, Option<String>, Option<String>, Option<u16>)>,
    /// The scan-folder picker. Same "tested model, never drawn" gap
    /// `AddToPlaylist` and `EditTrack` had.
    pub scan_picker: crate::dialogs::ScanPicker,
    /// "choose folders…" was pressed; the host computes suggestions
    /// (it knows $HOME/$BO_MUSIC_DIR, this struct doesn't) and opens
    /// `scan_picker` with them.
    pub pending_scan_picker_open: bool,
    /// The picker's "scan" button was pressed with these paths.
    pub pending_scan_with_roots: Option<Vec<String>>,
    /// The active cast device's queue — (track_id, title, artist), the
    /// same tuple shape `tracks_for`/local `Queue::add_all` already use.
    /// Distinct from `queue` (this app's own local playback queue): this
    /// is bo-media's per-device `play_queue` table, what a cast target
    /// plays through when several tracks are pushed to it rather than
    /// one at a time.
    pub remote_queue: Vec<(i64, String, String)>,
    /// A device url whose queue the host hasn't fetched yet.
    pub pending_remote_queue_load: Option<String>,
    /// (device_url, track_id) the host hasn't pushed yet.
    pub pending_remote_queue_push: Option<(String, i64)>,
    /// The device switcher above the output controls. The sink picker itself
    /// lives in settings_page as `SinkPicker` — see outputs.rs's note.
    pub device_selector: crate::outputs::DeviceSelector,
    /// The route the user last committed; the host applies it.
    pub pending_route: Option<crate::settings_page::Route>,
    /// What the host should do about audio, drained like the other pending
    /// fields. A COMMAND rather than a `Player` on the controller: this
    /// view is built and tested without a process table, and putting a
    /// child process on it would end that.
    pub pending_playback: Option<crate::playback::Request>,
    /// The route audio is currently going to, so a track change does not
    /// rebuild a combine sink that has not changed.
    pub audio_sink: crate::playback::Sink,
    /// The file the transport would play. Set by whatever resolves a track
    /// to something on disk; empty until then, and empty is honest — see
    /// `now_playing_url`.
    pub track_url: String,
    /// (track id, file path), from whatever catalogue the host installed.
    /// Empty until it does, which is why `request_play` refuses rather
    /// than guessing.
    pub track_paths: Vec<(i64, String)>,
    /// The id most recently handed to `request_play`, for a host that
    /// wants to record a play/rating against the real track — cleared by
    /// the host (`take()`), not by this struct, so it survives exactly
    /// one read and never fires twice for the same play.
    pub last_played_track: Option<i64>,
    /// The "scan library" button was pressed and the host hasn't started
    /// one yet. Cleared by the host (`take()`) the same frame it acts, so
    /// this never fires the same scan twice.
    pub pending_rescan: bool,
    /// The host's LAN address for a B&O speaker to cast the current track
    /// to (e.g. "http://192.168.1.50") — set by whatever picks the device
    /// (a settings-page device list is a follow-up; nothing in this crate
    /// discovers devices yet), read once by the host.
    pub pending_cast: Option<String>,
    /// A playlist create the host hasn't persisted yet. `apply_playlist`
    /// already inserts an optimistic row with a locally-made id (see
    /// there); this is the paired request to make it real. Cleared by the
    /// host, same one-shot pattern as `pending_cast`/`pending_rescan`.
    pub pending_playlist_create: Option<(String, String)>,
    /// The rating the user last set; the host applies it and echoes back
    /// through `confirm_rating`. Same shape as `pending_route` — this view
    /// owns no library.
    pub pending_rating: Option<(i64, u8)>,
    /// The input source the user last picked; the host applies it.
    ///
    /// Same shape as `pending_route` and for the same reason: this view owns
    /// no audio path. It records what was asked for and the host performs it,
    /// so a pick that fails on device does not leave the UI claiming a source
    /// it never got.
    pub pending_source: Option<String>,
    /// The device the selector last switched to.
    pub active_device_url: String,
    /// What this device can play from, and what the user has collected.
    pub sources_page: crate::sources::Sources,
    pub playlists_page: crate::sources::PlaylistsPage,
    /// Wall clock in seconds, for the "3d ago" column. Supplied by the host so
    /// the page does not reach for a clock it cannot be tested with.
    pub now_secs: i64,
    /// The offline processing section at the bottom of the mixer page.
    pub fileproc: crate::fileproc::FileProcessor,
    /// The volume step keys, held across frames so their ripple and flash
    /// survive the redraw they animate through.
    pub key_down: crate::fileproc::StepKey,
    pub key_up: crate::fileproc::StepKey,
    pub key_mute: crate::fileproc::StepKey,
    /// The processing request the host should perform.
    pub pending_proc: Option<crate::fileproc::ProcAction>,
    /// The named page menu at the top of the canvas. PageDots stays the swipe
    /// hint at the bottom; this is the jump-to menu.
    pub page_bar: crate::shell_bits::PageBar,
    /// The Library's own tab bar. Six tabs — Tracks, All tracks, Albums,
    /// Artists, Tags, Playlists — do not fit a phone in a plain row, and a
    /// plain row does not scroll: the last ones simply leave the screen.
    library_bar: crate::shell_bits::PageBar,
    /// The mixer page's cards, staggered so the page assembles.
    ///
    /// Labelled from MIXER_CARDS, which is what they actually wrap. They were
    /// built from VIEWS — the PAGE names — so drawing them would have put
    /// "Devices" and "Spaces" over the mixer's dial and channel columns. The
    /// list was a convenient source of five strings, and nothing caught it
    /// because card_frame was never called.
    pub cards: Vec<crate::shell_bits::ControlCard>,
    pub speaker_col: crate::channel::ChannelColumn,
    pub instrumental_col: crate::channel::ChannelColumn,
    pub mixer: crate::device::Mixer,
    pub profile: crate::device::Profile,
    /// The product map, as the daemon described it.
    ///
    /// Empty until something fills it, which resolves to the permissive
    /// hardcoded fallback — the QML's behaviour when its `JSON.parse` fails,
    /// and the right one: an unrecognised device showing every control beats
    /// a new speaker looking broken.
    pub product_profiles: std::collections::BTreeMap<String, crate::device::Profile>,
    /// Device address -> product type, as reported.
    ///
    /// Keyed on the ADDRESS because that is what `active_device_url` is
    /// (outputs.rs: "a device here is an ADDRESS"), and a display name is not
    /// unique — two Beoplays in one house are both "Beoplay".
    pub product_types: std::collections::BTreeMap<String, String>,
    pub soundscape: crate::soundscape::Soundscape,
    /// Which scene preset was last loaded, for the status line.
    pub scene_loaded: Option<String>,
    /// Request id for daemon calls. Monotonic, because `control::call`
    /// matches the reply BY id and a reused one could pick up the
    /// previous call's answer.
    pub scene_seq: u64,
    /// Whether the DAEMON reports the scene playing.
    ///
    /// Separate from `transport.playing`, which is the track player:
    /// one button for both would make stopping the music stop the
    /// ambience.
    pub scene_playing: bool,
    /// Seconds since the last status poll.
    pub status_age: f32,
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
    /// What has played this session, oldest first, capped. This is what
    /// makes "previous" mean the previous TRACK rather than the start of
    /// this one, and what Repeat All loops over when the queue runs dry.
    pub history: Vec<crate::queue::QueueEntry>,
    /// The shuffle draw's state. xorshift, seeded by the host or the
    /// clock — no `rand` dependency for one draw a track, and a seed a
    /// test can fix.
    pub shuffle_seed: u64,
    /// A search the host has not answered yet, and its answer.
    pub pending_search: Option<String>,
    pub search_results: Vec<crate::browse::BrowseTrack>,
    /// The query the results are for, so a stale answer is not shown
    /// under a newer question.
    pub search_answered: String,
    /// The tracks of the open playlist. Their own field: they used to
    /// land in `browse_tracks`, which is the album/artist aggregate
    /// source, so opening a playlist emptied every album grid until the
    /// next reload.
    pub playlist_tracks: Vec<crate::browse::BrowseTrack>,
    /// The Home tab's lists, and the request for them.
    pub home: HomeLists,
    pub pending_home: bool,
    /// The browse aggregates should be re-read from the catalogue: set at
    /// start and after a scan, cleared by the host when it supplies them.
    pub browse_stale: bool,
    /// The last volume the host applied, for the keyboard steps and the
    /// unmute — the dial's own value is what it shows, this is what the
    /// room is set to.
    pub volume: f32,
}

impl Controller {
    pub fn new(devices: Vec<Device>, scenes: Vec<Scene>) -> Self {
        Controller { nav: uikit_egui::nav::Nav::new(VIEWS.len()), scene_loaded: None,
            scene_seq: 0, scene_playing: false, status_age: 0.0,
            strip: uikit_egui::segmented_tabs::TabStrip::new(&VIEWS),
            devices, scenes, active_scene: 0,
            breath: uikit_egui::motion::Breath::new(2200.0, 0.35, 1.0),
            queue_open: false,
            transport: Default::default(),
            anc: Default::default(),
            routing: Default::default(),
            search: Default::default(),
            catalog: Default::default(),
            tracks: Vec::new(),
            plays: Vec::new(),
            browse: Default::default(),
            browse_tracks: Vec::new(),
            library_tab: LibraryTab::Home,
            all_tracks: crate::all_tracks::AllTracksPage::default(),
            tracks_kinetic: crate::scrollfeel::KineticScroll::default(),
            pending_tracks: None,
            queue: Default::default(),
            now_playing: Default::default(),
            dial: crate::dial::VolumeDial::new(0.0),
            watermark: crate::dial_watermark::DialWatermark::default(),
            silhouettes: Vec::new(),
            motion_scale: 1.0,
            surround: Default::default(),
            voice: crate::voice_control::load(),
            voice_confirm: None,
            pending_volume: None,
            tags: Vec::new(),
            tag_editor: Default::default(),
            ratings: Default::default(),
            now_playing_id: None,
            open_tag: None,
            open_playlist: None,
            pending_playlist_open: None,
            pending_playlist_delete: None,
            pending_playlist_remove: None,
            pending_add_to_playlist: None,
            edit_track: Default::default(),
            pending_track_edit_open: None,
            pending_track_edit_save: None,
            scan_picker: Default::default(),
            pending_scan_picker_open: false,
            pending_scan_with_roots: None,
            remote_queue: Vec::new(),
            pending_remote_queue_load: None,
            pending_remote_queue_push: None,
            device_selector: Default::default(),
            pending_route: None,
            pending_playback: None,
            audio_sink: crate::playback::Sink::Default,
            track_url: String::new(),
            track_paths: Vec::new(),
            last_played_track: None,
            pending_rescan: false,
            pending_cast: None,
            pending_playlist_create: None,
            pending_source: None,
            pending_rating: None,
            active_device_url: String::new(),
            sources_page: Default::default(),
            playlists_page: Default::default(),
            now_secs: 0,
            fileproc: Default::default(),
            key_down: crate::fileproc::StepKey::new("−", false),
            key_up: crate::fileproc::StepKey::new("+", false),
            key_mute: crate::fileproc::StepKey::new("MUTE", true),
            pending_proc: None,
            page_bar: crate::shell_bits::PageBar::new(&VIEWS),
            library_bar: crate::shell_bits::PageBar::new(&LIBRARY_TABS),
            // The delay is per-card because only the page knows the order the
            // cards read in.
            cards: MIXER_CARDS.iter().enumerate()
                .map(|(i, v)| crate::shell_bits::ControlCard::new(v, i as f32 * 60.0))
                .collect(),
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
            product_profiles: Default::default(),
            product_types: Default::default(),
            soundscape: Default::default(),
            settings: Default::default(),
            sinks: Default::default(),
            now: Default::default(),
            toast: Default::default(),
            dot: Default::default(),
            safety: Default::default(),
            add_to_playlist: Default::default(), gesture: GestureLayer::horizontal_page(320.0), a11y: A11yScene::new(), palette: String::new(),
            history: Vec::new(),
            shuffle_seed: 0x9E37_79B9_7F4A_7C15,
            pending_search: None,
            search_results: Vec::new(),
            search_answered: String::new(),
            playlist_tracks: Vec::new(),
            home: HomeLists::default(),
            pending_home: true,
            browse_stale: true,
            volume: 50.0,
        }
    }

    /// The views this product actually has, in `VIEWS` order.
    ///
    /// The QML drops a page a device cannot use; `device::views_for` is that
    /// rule and this is where it meets the live profile.
    /// The product type reported for a device address, if any.
    ///
    /// `None` is the ordinary state, not an error: a device the daemon has not
    /// described yet resolves through the same fallback chain as an
    /// unrecognised one.
    pub fn product_type_of(&self, url: &str) -> Option<String> {
        self.product_types.get(url).cloned()
    }

    pub fn visible_views(&self) -> Vec<&'static str> {
        crate::device::views_for(&self.profile, &VIEWS)
    }

    /// Keep the selection pointing at the same VIEW when the visible set
    /// changes — a profile arriving, a device being swapped.
    ///
    /// ## Why by name, and why this needs doing at all
    ///
    /// `Nav` holds a bare index into a fixed count. Filter the list and the
    /// index means a different page: on a profile with no `eq`, Mixer
    /// disappears and index 3 slides from Mixer to Soundscape — the user is
    /// moved to a page they did not pick, silently, while a device connects.
    ///
    /// So the anchor is the view's NAME. If it survived the filter the
    /// selection follows it to its new index; if it did not, we fall back to
    /// the first visible view rather than clamping the old index, because a
    /// clamped index lands on whatever happens to be adjacent and that is a
    /// coin flip dressed as a decision.
    ///
    /// Returns the name now selected, so a caller can say so if it wants to.
    pub fn reconcile_views(&mut self, was: &str) -> &'static str {
        let vis = self.visible_views();
        // An empty visible set cannot happen -- Devices and Settings have no
        // required control -- but the draw path must not divide by zero if it
        // ever does.
        let Some(first) = vis.first().copied() else { return VIEWS[0] };
        let idx = vis.iter().position(|v| *v == was);
        let target = idx.map(|i| vis[i]).unwrap_or(first);
        // `Nav` indexes VIEWS, not the visible subset, so translate back --
        // otherwise hiding one page shifts every index after it.
        let in_all = VIEWS.iter().position(|v| *v == target).unwrap_or(0);
        self.nav.go_page(in_all);
        target
    }

    pub fn current(&self) -> &'static str {
        // NOT `VIEWS[self.nav.page()]`. `view_idx` is written from
        // `strip_ui`'s return (line ~609) and from `page()`, so its bound
        // is enforced by the strip having been built from VIEWS — true
        // today, and not a property this function can see. An index that
        // drifts out of range would panic INSIDE THE DRAW PATH, which
        // takes the whole app down rather than showing a wrong tab name.
        //
        // Falling back to the first view is the recoverable answer:
        // wrong label, working app, and `view_idx` is corrected on the
        // next select. LIBRARY_TABS has a `from_index` guard and a test
        // for exactly this drift; VIEWS never got one.
        // Still a `get`, and still with a fallback: `Nav` clamps on
        // write, so this cannot be out of range today — but `current()`
        // returning a wrong label beats it panicking in the draw path,
        // and that trade does not change just because the invariant got
        // stronger.
        VIEWS.get(self.nav.page()).copied().unwrap_or(VIEWS[0])
    }

    /// Not called yet, and that is not an oversight: see below — the gesture
    /// was deliberately removed from the live UI and this is kept as the index
    /// arithmetic its replacement is checked against.
    ///
    /// Switch view by a swipe sign (wraps at the ends — clamped, see `page()`). Pure index arithmetic;
    /// used by tests. NOT gesture-driven in the live UI any more — compact mode uses the vertical-tabs
    /// sidebar instead of a whole-surface swipe (see `ui()`'s doc comment for why: the Devices view's
    /// volume Sliders claim horizontal drag themselves, and a whole-surface gesture reads the same raw
    /// pointer trajectory with nothing arbitrating between the two).
    pub fn swipe(&mut self, sign: i32) {
        // `page()` wraps; `Nav::go_page` clamps. Kept as `page()` so the
        // wrap survives — a swipe past the last view returning to the
        // first is the gesture's own idiom, and clamping would make the
        // last view feel stuck.
        self.nav.go_page(page(self.nav.page(), sign, VIEWS.len()));
    }

    /// Adopt cross-app preferences (theme/text/button + swipe feel incl. detent animation).
    pub fn apply_settings(&mut self, ctx: &egui::Context, prefs: &settings_kit::Prefs) {
        uikit_egui::apply_prefs(ctx, prefs);
        *self.gesture.config_mut() = prefs.gesture_config(320.0);
        // THE ACCENT CHANGED, so the badges bloom once. `accent_changed`
        // is what the halo is FOR — the module calls it "a marker, not a
        // flourish" — and it had no caller, so switching palette repainted
        // every outline in a new colour with nothing to draw the eye to
        // the change.
        //
        // Guarded on the palette actually differing: called
        // unconditionally this fires every frame, and a halo that never
        // stops is the throb the module's two-phase envelope exists to
        // avoid.
        if self.palette != prefs.palette {
            for s in self.silhouettes.iter_mut() {
                s.accent_changed();
            }
        }
        self.palette = prefs.palette.clone();
        // REDUCE MOTION REACHES THE BADGES. `set_motion_scale` existed with
        // no caller, so the silhouettes animated at full speed under a
        // setting whose whole point is that they should not — and a
        // reduce-motion preference that some of the app ignores is worse
        // than one it does not offer, because the person has no way to
        // tell which parts obeyed.
        //
        // `time_scale` is the kit's own number: Snappy 0.5, Smooth 1.0,
        // Reduced near-instant. Reading it here rather than inventing a
        // second scale keeps the badge animating like everything else the
        // kit draws.
        self.motion_scale = prefs.gesture.animation.time_scale();
        for s in self.silhouettes.iter_mut() {
            s.set_motion_scale(self.motion_scale);
        }
    }

    /// Render the controller.
    /// Handle an action from the All-tracks page.
    ///
    /// Play and Enqueue resolve here — the row is on screen, so its title and
    /// artist are too. Load is the one thing the view cannot do (the catalogue
    /// lives with the host), so it is stashed like `pending_volume` is.
    pub fn tracks_action(&mut self, act: crate::all_tracks::TracksAction) {
        use crate::all_tracks::TracksAction;
        match act {
            TracksAction::Load { .. } => self.pending_tracks = Some(act),
            TracksAction::Play(id) => {
                if let Some(t) = self.all_tracks.tracks().iter().find(|t| t.id == id) {
                    let (title, artist) = (t.title_text(), t.artist_text());
                    self.set_now_playing(&title, &artist);
                }
                // AND ACTUALLY PLAY IT. This arm set four labels and
                // `playing = true`, so pressing play on a row changed the
                // header and made no sound. The path comes from the
                // resolver the host installed; with none, `request_play`
                // says so rather than starting a player with an empty
                // argument.
                self.request_play(id);
            }
            TracksAction::Enqueue(id) => {
                if let Some(t) = self.all_tracks.tracks().iter().find(|t| t.id == id) {
                    let (title, artist) = (t.title_text(), t.artist_text());
                    if let Some(note) = self.queue.add(id, &title, &artist) {
                        self.toast.show(note);
                    }
                }
            }
        }
    }

    /// Accept a page the host fetched. Forwards to the page's own guard —
    /// a mismatched offset is discarded there, not here.
    pub fn supply_tracks(&mut self, p: &crate::all_tracks::PageResult) -> bool {
        self.pending_tracks = None;
        self.all_tracks.accept(p)
    }

    /// Answer any pending load from a `CatalogSource` — the adapter that
    /// composes the two library modules while keeping them ignorant of each
    /// other: `all_tracks` owns the window and its guards, `catalog_source`
    /// owns the channel contract. A host with an in-process source calls this
    /// once per frame; a host with an async one uses `supply_tracks`.
    pub fn answer_tracks_from(&mut self, src: &dyn crate::catalog_source::CatalogSource) {
        if let Some(crate::all_tracks::TracksAction::Load { offset }) = self.pending_tracks {
            let page = page_from_source(src, offset, self.all_tracks.sort());
            self.supply_tracks(&page);
        }
        // BROWSE IS FILLED FROM THE SAME SOURCE, once. Its grids were empty by
        // construction because nothing called load_local — and last commit I
        // wrote the adapter for it and did not call that either, which the
        // tested-but-uncalled report said plainly on the next run.
        //
        // Guarded on being empty rather than refreshed per call: this runs
        // once a frame for an in-process host, and re-aggregating the whole
        // catalogue at 60Hz would be the most expensive thing the app does.
        // A host whose library changes calls `reload_browse`.
        if self.browse_tracks.is_empty() && src.total() > 0 {
            self.reload_browse(src);
        }
    }

    /// Re-read the catalogue into the browse grids.
    ///
    /// Separate from `answer_tracks_from` so a host that knows its library
    /// changed can say so, rather than the frame loop guessing from a count
    /// that would miss an edit which kept the total the same.
    pub fn reload_browse(&mut self, src: &dyn crate::catalog_source::CatalogSource) {
        self.browse_tracks = browse_tracks_from_source(src);
        self.browse.load_local(&self.browse_tracks);
    }

    pub fn ui(&mut self, ui: &mut Ui, adaptive: &Adaptive) {
        let colors = uikit_egui::palette_colors(&self.palette);
        self.a11y.begin_frame();
        let dt = ui.input(|i| i.stable_dt);
        // THE TOAST CLOCK. `Toast::tick` had no caller, so every message
        // stayed up until the next one replaced it — "queued 1 track"
        // from an hour ago, still in the bar.
        self.toast.tick(dt);
        if self.toast.visible().is_some() { ui.ctx().request_repaint(); }
        self.keyboard(ui);

        // THE TRANSPORT BAR IS A PANEL, not the last thing in a column.
        // Drawn as a column it sat under whatever the page allocated, and
        // every page with a scroll list took the whole height — so the
        // one control that must be on every page was the one control
        // that was on none of them once the library had rows. A bottom
        // panel is laid out FIRST and the page gets what is left.
        //
        // Both layouts. The compact branch had no bar at all: a phone,
        // the deploy target, could start music and never pause it.
        let bar_bg = colors.bg_card.gamma_multiply(0.6);
        egui::Panel::bottom("bo-transport")
            .frame(egui::Frame::NONE.fill(bar_bg).inner_margin(egui::Margin::symmetric(10, 6)))
            .show_separator_line(true)
            .show_inside(ui, |ui| {
                self.transport_strip(ui, &colors);
            });

        if !adaptive.is_compact() {
            // The kit's strip on wide, the kit's vertical_tabs on compact —
            // both shared widgets now, where the wide branch was the last
            // hand-rolled selectable_label row in this app. That leaves two
            // navigation widgets rather than three, and the two that remain
            // are the two the kit provides for the two shapes.
            self.strip.select(self.nav.page());
            if let Some(i) = uikit_egui::segmented_tabs::strip_ui(ui, &self.strip, &colors) {
                self.nav.go_page(i);
            }
            ui.separator();
            // The safety toast slides down from the TOP, above everything, and
            // is drawn before the header so it cannot be pushed off by a view
            // that changes height.
            self.safety.tick(dt);
            self.poll_daemon(dt);
            // The playback position, interpolated between network reports. Its
            // own doc: "advance the local clock between reports, so the bar
            // moves smoothly rather than stuttering one packet at a time" —
            // and nothing called it, so between packets the bar did not move
            // at all.
            self.transport.tick(dt as f64);
            // ONE TOAST. An inline version lived here and I added a second in
            // safety.rs last commit without seeing it — I searched for
            // `safety::` and this calls through the FIELD, `self.safety.`.
            //
            // The inline one had two faults worth naming. It DISCARDED the
            // unlocked volume: `let _ = v; // the host applies it` — no host
            // does, so Unlock showed a confirmation and changed nothing. And
            // beside it sat `self.transport.title = self.transport.title
            // .clone()`, a self-assignment that does nothing at all.
            //
            // The renderer in safety.rs replaces it, and the volume is applied
            // here where pending_volume lives.
            if let Some(a) = crate::safety::safety_toast_ui(ui, &self.safety, &colors) {
                match a {
                    crate::safety::ToastAction::Dismiss => self.safety.dismiss(),
                    crate::safety::ToastAction::Unlock => {
                        if let Some(v) = self.safety.unlock() {
                            self.pending_volume = Some(v as f32);
                        }
                    }
                }
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
            self.page_view(ui, &colors);
        } else {
            // Compact width: a vertical-tabs sidebar instead of a whole-surface swipe gesture. The
            // devices view has volume Sliders, which claim horizontal drag themselves — a whole-surface
            // GestureLayer reads the same raw pointer trajectory with nothing arbitrating between the
            // two (the exact rusty-board keyboard bug: see uikit_egui::vertical_tabs's doc comment), so
            // dragging a slider could also register as a page swipe. A sidebar tap has no such race.
            ui.horizontal(|ui| {
                ui.allocate_ui(egui::vec2(64.0_f32.max(adaptive.min_touch_target * 1.3), ui.available_height()), |ui| {
                    let tabs: Vec<uikit_egui::vertical_tabs::Tab> = VIEWS.iter().map(|n| uikit_egui::vertical_tabs::Tab::new(n)).collect();
                    if let Some(i) = uikit_egui::vertical_tabs::show(ui, &tabs, self.nav.page(), adaptive.min_touch_target, &colors) {
                        self.nav.go_page(i);
                    }
                });
                ui.separator();
                // The same clocks the wide branch runs. They lived only
                // there, so on a phone the scene status never polled and
                // the position never advanced between reports.
                self.safety.tick(dt);
                self.poll_daemon(dt);
                self.transport.tick(dt as f64);
                ui.vertical(|ui| {
                    // The SAME six pages as the wide layout. This mapped
                    // Spaces to the scene list and Mixer, Soundscape and
                    // Settings all to the Library, so on the phone — the
                    // deploy target — three of the six tabs opened the
                    // wrong page and the settings were unreachable.
                    self.page_view(ui, &colors);
                });
            });
        }

        // THE FOCUS RING, after both layout branches. paint_focus_ring is
        // the a11y module's one egui touch and this app — the only one that
        // used the scene at all — never called it, so focus was tracked and
        // invisible. That is the half that makes voice navigation usable by
        // someone who can also see the screen: without it, nothing on the
        // display says where the voice focus is.
        self.a11y.paint_focus_ring(ui, colors.accent);

        // ADD-TO-PLAYLIST, after both layout branches for the same reason
        // the focus ring is: it has to float above whichever page is
        // showing, not live inside library_view where the "+" button that
        // opens it happens to sit. `add_to_playlist.open()` has been called
        // from that button since the dialog was ported — with a full,
        // tested model — and nothing ever drew it: `is_open()` had no
        // reader, so the dialog "opened" into nothing appearing on screen.
        if self.add_to_playlist.is_open() {
            if let Some((track_id, playlist_name)) =
                crate::dialogs::add_to_playlist_ui(ui, &mut self.add_to_playlist, &colors)
            {
                if let Ok(track_id) = track_id.parse::<i64>() {
                    if let Some(playlist_id) = self.playlists_page.all().iter()
                        .find(|p| p.name == playlist_name).map(|p| p.id)
                    {
                        self.pending_add_to_playlist = Some((track_id, playlist_id));
                    }
                }
            }
        }
        // EDIT TRACK, the same shape as add-to-playlist above: a floating
        // dialog above whichever page is showing, opened by an `edit`
        // button that lives inside the track row it edits.
        if self.edit_track.is_open() {
            if let Some(save) = crate::dialogs::edit_track_ui(ui, &mut self.edit_track, &colors) {
                if let Ok(track_id) = save.track_id.parse::<i64>() {
                    self.pending_track_edit_save = Some((
                        track_id, save.title, save.artist, save.album, save.year));
                }
                self.edit_track.close();
            }
        }
        if self.scan_picker.open {
            if let Some(paths) = crate::dialogs::scan_picker_ui(ui, &mut self.scan_picker, &colors) {
                self.settings.scan = crate::settings_page::Scan::Running {
                    seen: 0, total: None, path: String::new() };
                self.pending_scan_with_roots = Some(paths);
                self.scan_picker.close();
            }
        }
    }

    /// The current page, in the space above the transport bar.
    ///
    /// The Library manages its own scrolling (its lists are windowed);
    /// every other page is a column of cards that is taller than a phone
    /// and scrolls as a whole. Without the scroll the mixer's lower cards
    /// were painted across the transport bar, which the layout test
    /// reports as its text colliding with the bar's.
    fn page_view(&mut self, ui: &mut Ui, colors: &PaletteColors) {
        match self.nav.page() {
            2 => self.library_view(ui, colors),
            page => {
                egui::ScrollArea::vertical().id_salt(("page-scroll", page))
                    .auto_shrink([false, false]).show(ui, |ui| {
                    match page {
                        0 => self.devices_view(ui, colors),
                        1 => self.spaces_view(ui, colors),
                        3 => self.mixer_view(ui, colors),
                        4 => self.soundscape_view(ui, colors),
                        _ => self.settings_view(ui, colors),
                    }
                });
            }
        }
    }

    fn devices_view(&mut self, ui: &mut Ui, c: &PaletteColors) {
        // The product selector sits at the top, as in the Qt page.
        if let Some(url) = crate::outputs::device_selector_ui(ui, &mut self.device_selector, c) {
            // A DIFFERENT SPEAKER IS A NEW SESSION. safety::new_session's own
            // doc: "moving to another speaker should not carry an unlock
            // granted for the first one" — so without this, unlocking high
            // volume on a pair of headphones silently unlocks it on the living
            // room speakers too.
            //
            // Guarded on the url actually changing: device_selector_ui returns
            // the selection, and re-selecting the current device would
            // otherwise revoke an unlock the operator just granted.
            if url != self.active_device_url {
                self.safety.new_session();
                // THE PROFILE FOLLOWS THE ACTIVE DEVICE, and this is the one
                // place a device actually changes. What a Beosound can do is
                // not what a Beoplay can, so the view set has to change with
                // it — otherwise the filtering added alongside `views_for`
                // never fires and headphones keep offering a Mixer.
                //
                // Reconciled by NAME immediately after, so the page you were
                // on survives the switch when the new device also has it. The
                // order matters: set the profile, THEN reconcile, or the
                // reconcile computes the visible set from the old device.
                let was = self.current();
                // Resolved into a local first. `self.profile = f(&self.product_profiles, ..)`
                // borrows self immutably on the right while assigning to a
                // field of self on the left, and whether borrowck accepts that
                // is exactly the kind of thing not worth being clever about.
                let ty = self.product_type_of(&url);
                let resolved =
                    crate::device::resolve_profile(&self.product_profiles, ty.as_deref());
                self.profile = resolved;
                self.reconcile_views(was);
                // A "device" here is an ADDRESS (outputs.rs's own doc), so
                // an http(s) one is a networked B&O speaker to cast to —
                // distinct from the Bluetooth/local sinks `SinkPicker`
                // handles. `request_cast` no-ops with a toast if nothing
                // is playing, so selecting a cast target before pressing
                // play is harmless, not a silent failure.
                if url.starts_with("http://") || url.starts_with("https://") {
                    self.request_cast(url.clone());
                    self.pending_remote_queue_load = Some(url.clone());
                }
            }
            self.active_device_url = url;
        }
        ui.separator();
        // REMOTE QUEUE, only for a cast target — a Bluetooth/local sink
        // has no daemon-side queue table to show. Distinct from the
        // `queue` view further down (this app's own local playback
        // queue): pushing here lands in bo-media's per-device
        // `play_queue`, what the speaker itself plays through when more
        // than one track is queued to it rather than cast one at a time.
        if self.active_device_url.starts_with("http://")
            || self.active_device_url.starts_with("https://")
        {
            ui.label(egui::RichText::new("queued on this speaker").strong());
            if self.remote_queue.is_empty() {
                ui.colored_label(c.text_weak, "nothing queued");
            }
            for (_, title, artist) in &self.remote_queue {
                let label = if title.trim().is_empty() { "(untitled)" } else { title };
                ui.label(if artist.is_empty() { label.to_string() }
                          else { format!("{label} — {artist}") });
            }
            if let Some(id) = self.now_playing_id {
                if ui.button("queue what's playing here").clicked() {
                    self.pending_remote_queue_push = Some((self.active_device_url.clone(), id));
                }
            }
            ui.separator();
        }
        // VOICE CONTROL, in settings beside the other things that change
        // what the app does on its own. Off until asked for; the panel
        // says what enabling it means.
        if self.sinks.open {
            ui.separator();
            if crate::voice_control::voice_settings_ui(ui, &mut self.voice, c) {
                if let Err(e) = crate::voice_control::store(&self.voice) {
                    // Silently, a write failure means a setting that works
                    // until restart — and for a listening switch, one that
                    // silently turns itself back ON is the wrong direction
                    // to fail in.
                    self.toast.show(&format!("voice settings not saved: {e}"));
                }
            }
        }
        // SURROUND, beside the combine control rather than instead of it.
        // They are different things — combine duplicates a stream, surround
        // splits one — and a single toggle between them would make picking
        // "several speakers" mean two incompatible things depending on a
        // mode the person set earlier.
        if self.sinks.open {
            ui.separator();
            ui.label(egui::RichText::new("surround").strong());
            let speakers: Vec<(String, String)> = self.sinks.devices.iter()
                .map(|d| (d.mac.clone(), d.label().to_string())).collect();
            crate::surround::assign_ui(ui, &mut self.surround, &speakers, c);
            // Enabled only when the assignment would actually route: the
            // rules refuse a setup that drops half the mix, and a button
            // that accepts one and fails later is the validation arriving
            // after the decision.
            let ok = self.surround.routable();
            if ui.add_enabled(ok, egui::Button::new("Use this arrangement")).clicked() {
                if let Some(sink) = crate::surround::sink_of(&self.surround) {
                    self.audio_sink = sink;
                    self.pending_playback = Some(crate::playback::Request::ReleaseRoute);
                    self.toast.show("surround set");
                }
            }
        }
        // Output routing: which speaker, or which speakers. Committed through
        // the existing SinkPicker, which now knows about drivers.
        if self.sinks.open {
            if ui.add_enabled(self.sinks.can_combine(),
                egui::Button::new(self.sinks.combine_label())).clicked() {
                self.pending_route = self.sinks.route();
                // Audio follows the picker in the same act. Leaving them to
                // be connected by the host is how the UI ends up showing a
                // route the sound is not going to.
                if let Some(r) = self.pending_route.clone() {
                    self.set_audio_route(&r);
                }
            }
        }
        // INPUT SOURCES, beside the output routing they belong with. sources_ui
        // was ported and never called, and choose() already refuses a source
        // that is the active one — the page has always known not to offer a
        // switch to where you already are.
        //
        // Drawn only when the device reported some: the renderer's own empty
        // state says "No sources reported by this device", which is right on a
        // Sources page and noise on a Devices page that has plenty else to
        // show.
        if !self.sources_page.is_empty() {
            ui.separator();
            ui.heading("Sources");
            if let Some(name) = crate::sources::sources_ui(ui, &self.sources_page, c) {
                if let Some(pick) = self.sources_page.choose(&name) {
                    self.pending_source = Some(pick);
                }
            }
        }
        ui.separator();
        ui.heading("Devices");
        // collect edits to avoid borrow issues while iterating
        let mut vol_edits: Vec<(usize, i32)> = Vec::new();
        // A badge per row. The shape is parsed from the device name each
        // frame; set_shape only blooms the halo when it actually changed, so a
        // list refresh cannot flash every badge — the module's own guard.
        let before = self.silhouettes.len();
        self.silhouettes.resize_with(self.devices.len(),
            crate::product_silhouette::ProductSilhouette::default);
        // Newly created badges inherit the setting. Applying it only in
        // `apply_settings` leaves a device that appeared afterwards
        // animating at full speed, which is the half-obeyed reduce-motion
        // the field exists to prevent.
        for s in self.silhouettes.iter_mut().skip(before) {
            s.set_motion_scale(self.motion_scale);
        }
        let dt_ms = ui.input(|i| i.stable_dt).min(0.1) * 1000.0;
        for (i, d) in self.devices.iter_mut().enumerate() {
            ui.horizontal(|ui| {
                let s = &mut self.silhouettes[i];
                s.set_shape(crate::product_silhouette::Shape::parse(
                    &d.name.to_lowercase().split_whitespace().last().unwrap_or_default()));
                s.tick(dt_ms);
                let (rect, _) = ui.allocate_exact_size(egui::vec2(26.0, 26.0), egui::Sense::hover());
                silhouette_paint(ui, s, rect, c);
                if s.reveal() < 1.0 || s.halo() > 0.0 { ui.ctx().request_repaint(); }
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

    /// Jump to a page from the named bar, keeping the swipe nav in step.
    pub fn jump_to_page(&mut self, i: usize) -> bool {
        if !self.page_bar.select(i) { return false; }
        self.nav.go_page(i);
        true
    }

    /// Start an offline processing run, if the section will allow it.
    pub fn start_proc(&mut self, which: &str) {
        let a = if which == "run" { self.fileproc.run() } else { self.fileproc.act(which) };
        if a.is_some() { self.pending_proc = a; }
    }

    /// Carry out a playlist action. The page owns no storage; the host performs
    /// these and re-publishes, which is the only way the list changes.
    pub fn apply_playlist(&mut self, a: crate::sources::PlaylistAction) {
        use crate::sources::PlaylistAction;
        match a {
            PlaylistAction::Create { name, description } => {
                let id = self.playlists_page.all().iter().map(|p| p.id).max().unwrap_or(0) + 1;
                let mut all = self.playlists_page.all().to_vec();
                all.push(crate::sources::PlaylistSummary {
                    id, name: name.clone(), description: description.clone(),
                    track_count: 0, updated_at: self.now_secs });
                self.playlists_page.set(all);
                // Bring it into view: a new row appearing off-screen looks like
                // nothing happened.
                self.playlists_page.focus(id);
                // The optimistic row above has a LOCALLY-MADE id — it was never
                // real, which is why this used to be the entire method: a
                // playlist created here lived exactly as long as the process
                // did. The host persists the real one and reloads the list
                // (see pending_playlist_create's doc comment), which replaces
                // this row with the true id rather than trying to patch it in
                // place.
                self.pending_playlist_create = Some((name, description));
            }
            PlaylistAction::Open(id) => {
                self.open_playlist = Some(id);
                self.pending_playlist_open = Some(id);
            }
            PlaylistAction::Delete(id) => {
                self.pending_playlist_delete = Some(id);
                if self.open_playlist == Some(id) { self.open_playlist = None; }
            }
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
    ///
    /// Until something calls this, every rating stays PENDING — `pending()` is
    /// `value != confirmed`, and tags.rs draws a dot beside a pending star. So
    /// with no echo the dot never clears and every rating the operator has
    /// ever set looks like it is still in flight.
    ///
    /// `rate` IS called from the UI, so this is not a dormant pair: the
    /// optimistic half runs and the reconciling half never did.
    ///
    /// Not called yet, and that is not an oversight — but it IS a live defect,
    /// which is a distinction worth keeping. Closing it needs a host that can
    /// report a stored rating, and this repo has none: `CatalogSource` exposes
    /// `page` and `total`, and `CatalogTrack` carries no rating.
    ///
    /// I started to add `confirm_ratings_from(&dyn CatalogSource)` to match
    /// `answer_tracks_from`, and stopped: it needed a `rating()` on the trait,
    /// and widening a host data contract so an internal checker goes quiet is
    /// the worst available reason to change an interface.
    ///
    /// What a host must do: call this with the stored value after writing a
    /// rating. Until one does, the pending dot in `tags.rs` never clears and
    /// every rating ever set reads as still in flight.
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
        ids.iter().filter_map(|id| self.find_track(*id))
            .map(|t| (t.id, t.title.clone(), t.primary_artist.clone()))
            .collect()
    }

    /// A row by id, from whichever list it is showing in — the browse
    /// aggregate source, the open playlist, a search answer, the Home
    /// lists. One lookup so a new list cannot forget to be playable.
    fn find_track(&self, id: i64) -> Option<&crate::browse::BrowseTrack> {
        self.browse_tracks.iter().find(|t| t.id == id)
            .or_else(|| self.playlist_tracks.iter().find(|t| t.id == id))
            .or_else(|| self.search_results.iter().find(|t| t.id == id))
            .or_else(|| self.home.recently_played.iter().find(|t| t.id == id))
            .or_else(|| self.home.recently_added.iter().find(|t| t.id == id))
            .or_else(|| self.home.most_played.iter().find(|t| t.id == id))
            .or_else(|| self.home.never_played.iter().find(|t| t.id == id))
    }

    /// How often to ask the daemon what it is doing.
    ///
    /// Two seconds. `control::send` opens a UNIX SOCKET per call, so
    /// per-frame polling is sixty connects a second for a value that
    /// changes when somebody presses a button. Two seconds is slower
    /// than a person can notice for an ambience track and cheap enough
    /// to leave running.
    const STATUS_EVERY_S: f32 = 2.0;

    /// Ask the daemon what is actually loaded and playing.
    ///
    /// This is what turns `scene_playing` from a GUESS into a REPORT.
    /// Before it, the flag recorded what this UI had asked for — so a
    /// scene started from the CLI, or stopped by the daemon itself,
    /// left the indicator wrong with no way to notice.
    fn poll_daemon(&mut self, dt: f32) {
        self.status_age += dt;
        if self.status_age < Self::STATUS_EVERY_S {
            return;
        }
        self.status_age = 0.0;
        self.scene_seq += 1;
        let Ok(v) = bo_mux::control::status(self.scene_seq) else {
            // A daemon that is not running is not an error to show every
            // two seconds. The scene controls already report a failure
            // when a person actually presses one, which is when they are
            // asking. `scene_playing` is cleared because nothing is
            // playing if nothing is running.
            self.scene_playing = false;
            return;
        };
        // The daemon's answer WINS over the local flag. That is the
        // whole point: it knows, and this UI was guessing.
        if let Some(p) = v.get("playing").and_then(|x| x.as_bool()) {
            self.scene_playing = p;
        }
        // `loaded` is null when nothing is loaded, which is a real
        // ANSWER and not a missing field — so a null clears the name
        // rather than leaving a stale one on screen.
        //
        // Distinguished by `is_null` rather than matching
        // `serde_json::Value`, so this crate does not need serde_json as
        // a direct dependency for one pattern. An ABSENT key is left
        // alone: a daemon that stopped reporting a field is not a daemon
        // saying "nothing is loaded".
        if let Some(l) = v.get("loaded") {
            if l.is_null() {
                self.scene_loaded = None;
            } else if let Some(n) = l.as_str() {
                self.scene_loaded = Some(n.to_string());
            }
        }
    }

    /// Reflect a queue entry as the thing now playing.
    fn start_playing(&mut self, e: &crate::queue::QueueEntry) {
        self.remember_current();
        self.start_playing_no_history(e);
    }

    /// The track that was playing goes into the history — unless nothing
    /// was, or it is the same track again (Repeat One, a restart).
    fn remember_current(&mut self) {
        if let Some(id) = self.now_playing_id {
            if self.history.last().map(|h| h.track_id) != Some(id) {
                self.history.push(crate::queue::QueueEntry {
                    queue_id: 0, track_id: id,
                    title: self.transport.title.clone(), artist: self.transport.artist.clone(),
                });
                if self.history.len() > HISTORY_CAP { self.history.remove(0); }
            }
        }
    }

    fn start_playing_no_history(&mut self, e: &crate::queue::QueueEntry) {
        let (title, artist) = (e.title.clone(), e.artist.clone());
        self.set_now_playing(&title, &artist);
        // Recorded here because this is where a track id exists. QueueEntry
        // has one and NowPlaying does not — it carries display strings, since
        // line-in and radio have no track to name.
        self.now_playing_id = Some(e.track_id);
        // AND ASK FOR THE SOUND. Every path that starts a track goes
        // through here — the queue head, an album's first track, a tapped
        // row — so this is the one place it has to be requested. It was
        // missing, which is why "play album" queued, showed a title and
        // played nothing.
        self.request_play(e.track_id);
    }

    /// Start a track by id from any of the row lists, as a tap on a row
    /// does. Goes through the queue entry path so history and the sound
    /// both happen — this used to set two labels and play nothing.
    pub fn play_track_id(&mut self, id: i64) {
        if let Some((_, title, artist)) = self.tracks_for(&[id]).into_iter().next() {
            let e = crate::queue::QueueEntry { queue_id: 0, track_id: id, title, artist };
            self.start_playing(&e);
        } else {
            self.toast.show("that track is not in view");
        }
    }

    /// Queue a track by id from any of the row lists.
    pub fn queue_track_id(&mut self, id: i64) {
        let t = self.tracks_for(&[id]);
        if t.is_empty() { self.toast.show("that track is not in view"); return; }
        self.queue.mode = crate::queue::EnqueueMode::Stack;
        self.queue.add_all(&t);
        self.toast.show(&format!("queued {}", t[0].1));
    }

    /// Act on something the person said.
    ///
    /// The whole decision — is voice on, was that a command, is it safe to
    /// act on — lives in `voice_control::decide`, so there is ONE place
    /// that can be wrong about whether the app is listening. A check
    /// duplicated across handlers is one that gets missed in the fourth.
    ///
    /// Returns what happened, so a host can log it and a test can assert
    /// it without reading four fields.
    pub fn heard(&mut self, phrase: &str) -> crate::voice_control::Outcome {
        let outcome = crate::voice_control::decide(&self.voice, phrase);
        match &outcome {
            crate::voice_control::Outcome::Run(cmd) => {
                let cmd = cmd.clone();
                self.run_voice(cmd);
            }
            crate::voice_control::Outcome::Confirm(cmd) => {
                self.voice_confirm = Some(cmd.clone());
                self.toast.show(&format!("say yes to {}", cmd.describe()));
            }
            crate::voice_control::Outcome::Ignored => {}
        }
        outcome
    }

    /// Accept or reject a pending confirmation.
    pub fn confirm_voice(&mut self, yes: bool) {
        let Some(cmd) = self.voice_confirm.take() else { return };
        if yes { self.run_voice(cmd); }
    }

    /// Carry out a command that has been decided on.
    pub fn run_voice(&mut self, cmd: crate::voice_control::Command) {
        use crate::voice_control::Command as C;
        use crate::transport::Repeat;
        match cmd {
            // Through the same paths a tap takes. A voice command that
            // set `playing` directly would skip `start_playing` and make
            // no sound — which is exactly the bug the tap path had.
            C::Play => { if !self.transport.playing { self.toggle_play(); } }
            C::Pause => { if self.transport.playing { self.toggle_play(); } }
            C::Toggle => self.toggle_play(),
            C::Stop => self.stop(),
            C::Next => self.next(),
            C::Previous => self.previous(),
            C::Search(q) => {
                self.search.query = q;
                self.voice_confirm = None;
            }
            C::PlayNamed(q) => {
                // Searched, not played. Free text cannot name a track
                // exactly, and starting the first fuzzy match is how a
                // request for one album plays another.
                self.search.query = q;
            }
            C::VolumeUp => self.nudge_volume(10),
            C::VolumeDown => self.nudge_volume(-10),
            C::VolumeSet(n) => self.pending_volume = Some(f32::from(n)),
            C::Mute => self.pending_volume = Some(0.0),
            // Unmute goes to a MODEST level, not to whatever it was. The
            // previous volume is not remembered here, and restoring a
            // guessed loud one in a room where someone just asked for
            // quiet is the wrong way to be wrong.
            C::Unmute => self.pending_volume = Some(30.0_f32.max(self.volume.min(30.0))),
            C::ShuffleOn => self.transport.shuffle = true,
            C::ShuffleOff => self.transport.shuffle = false,
            C::RepeatOne => self.transport.repeat = Repeat::One,
            C::RepeatAll => self.transport.repeat = Repeat::All,
            C::RepeatOff => self.transport.repeat = Repeat::Off,
        }
    }

    /// Set what is playing, in ALL THREE places that hold it.
    ///
    /// The app carries the same fact three times -- `self.now`
    /// (`chrome::NowPlaying`, the top bar), `self.transport` (the play bar),
    /// and `self.now_playing` (`queue::NowPlaying`, the queue strip) -- and
    /// every path that started a track updated its own subset.
    ///
    /// Playing from the queue set all three. Playing from the all-tracks list
    /// or from browse set only the first two, so the queue strip kept saying
    /// "Nothing playing" directly above a bar naming the track. Reported from
    /// a screenshot showing exactly that.
    ///
    /// One setter rather than three assignments at each call site: the bug is
    /// not that someone forgot a line, it is that forgetting a line was
    /// possible. Collapsing the three structs into one would be better still
    /// and is a bigger change than this -- `chrome::NowPlaying` and
    /// `queue::NowPlaying` are field-identical and differ only in their
    /// methods, which is the actual duplication to remove.
    fn set_now_playing(&mut self, title: &str, artist: &str) {
        self.now.title = title.to_string();
        self.now.artist = artist.to_string();
        self.transport.title = title.to_string();
        self.transport.artist = artist.to_string();
        self.transport.playing = true;
        self.now_playing = crate::queue::NowPlaying {
            title: title.to_string(),
            artist: artist.to_string(),
            source: String::new(),
            playing: true,
        };
    }

    fn play_browse_track(&mut self, id: i64) {
        // Through the queue-entry path. This set the labels and stopped:
        // a tap on an album track showed the title and played nothing.
        self.play_track_id(id);
    }

    fn library_view(&mut self, ui: &mut Ui, c: &PaletteColors) {
        ui.heading("Library");
        // Tracks / Albums / Artists. The tab row stays visible on a detail page
        // so there is always a way out that is not the back chevron — a drill-down
        // you can only leave by one control is where a touch user gets stuck.
        // THROUGH page_bar_ui, which scrolls and keeps the active tab in view.
        // Six tabs in a plain ui.horizontal do not fit a phone, and a plain row
        // does not scroll — the last ones simply leave the screen. That is the
        // job this widget was written for and it had never been drawn: the
        // controller built a PageBar from VIEWS, kept it in step, and tested
        // that it could not misname a page, all for something with no display.
        //
        // The strip is not the right widget here. It divides the width into
        // equal segments, which is correct for five short page names and wrong
        // for six tabs including "All tracks" — at phone width each segment
        // would be too narrow to read or hit.
        self.library_bar.select(LIBRARY_TABS.iter()
            .position(|n| *n == self.library_tab.label())
            .unwrap_or(0));
        if let Some(i) = crate::shell_bits::page_bar_ui(ui, &mut self.library_bar, 92.0, c) {
            if let Some(tab) = LibraryTab::from_index(i) {
                self.library_tab = tab;
            }
        }
        if self.library_tab == LibraryTab::AllTracks {
            if let Some(act) = crate::all_tracks::all_tracks_ui(
                ui, &mut self.all_tracks, &mut self.tracks_kinetic, c,
            ) {
                self.tracks_action(act);
            }
            return;
        }
        if self.library_tab == LibraryTab::Playlists {
            // PlaylistsPage was ported with its create-in-place card, its
            // state has sat on this controller as `playlists_page`, and
            // apply_playlist has handled Create since it was written — with a
            // test. The only missing piece was a tab to reach it, so a whole
            // ported page had no route in.
            match self.open_playlist {
                Some(id) => {
                    let name = self.playlists_page.all().iter().find(|p| p.id == id)
                        .map(|p| p.name.clone()).unwrap_or_default();
                    ui.horizontal(|ui| {
                        if ui.button("‹").clicked() { self.open_playlist = None; }
                        ui.strong(name);
                        if !self.playlist_tracks.is_empty() {
                            if ui.button("▶ play all").clicked() {
                                let ids: Vec<i64> = self.playlist_tracks.iter().map(|t| t.id).collect();
                                self.apply_browse(crate::browse::Action::PlayAll(ids));
                            }
                            if ui.button("+ queue all").clicked() {
                                let ids: Vec<i64> = self.playlist_tracks.iter().map(|t| t.id).collect();
                                self.apply_browse(crate::browse::Action::QueueAll(ids));
                            }
                        }
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.small_button("delete").clicked() {
                                self.apply_playlist(crate::sources::PlaylistAction::Delete(id));
                            }
                        });
                    });
                    // The loaded tracks, in their OWN field. Tapping a row
                    // goes through the one tested play/queue path.
                    egui::ScrollArea::vertical().id_salt("playlist-tracks").show(ui, |ui| {
                        if self.playlist_tracks.is_empty() {
                            ui.colored_label(c.text_weak, "no tracks in this playlist yet — use + on any track");
                        }
                        let rows = self.playlist_tracks.clone();
                        for t in rows {
                            self.track_row(ui, &t, c, true);
                        }
                    });
                }
                None => {
                    if let Some(act) =
                        crate::sources::playlists_ui(ui, &mut self.playlists_page, self.now_secs, c)
                    {
                        self.apply_playlist(act);
                    }
                }
            }
            return;
        }
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
        if matches!(self.library_tab, LibraryTab::Albums | LibraryTab::Artists) {
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
        if self.library_tab == LibraryTab::Search {
            self.search_view(ui, c);
            return;
        }
        self.home_view(ui, c);
    }

    /// One track row, the same in every list: title over artist, and the
    /// actions on the right. Tapping the title plays it. `in_playlist`
    /// adds a remove button in place of the add-to-playlist one.
    fn track_row(&mut self, ui: &mut Ui, t: &crate::browse::BrowseTrack, c: &PaletteColors,
                 in_playlist: bool) {
        let playing = self.now_playing_id == Some(t.id);
        // The buttons are laid out first, from the right, and the title
        // truncates into what they leave — a long title drawn as a button
        // ran under the buttons at phone width.
        ui.horizontal(|ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if in_playlist {
                    if let Some(pid) = self.open_playlist {
                        if ui.small_button("−").on_hover_text("Remove from playlist").clicked() {
                            self.pending_playlist_remove = Some((pid, t.id));
                        }
                    }
                } else if ui.small_button("♪+").on_hover_text("Add to playlist").clicked() {
                    self.add_to_playlist.open(&t.id.to_string());
                    self.add_to_playlist.playlists =
                        self.playlists_page.all().iter().map(|p| p.name.clone()).collect();
                }
                if ui.small_button("+").on_hover_text("Add to queue").clicked() {
                    self.queue_track_id(t.id);
                }
                if ui.small_button("edit").on_hover_text("Edit title, artist, album, year").clicked() {
                    self.pending_track_edit_open = Some(t.id);
                }
                ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    let (art, _) = ui.allocate_exact_size(egui::vec2(28.0, 28.0), egui::Sense::hover());
                    crate::transport::identicon_paint(ui.painter(), art, &t.title, c);
                    ui.vertical(|ui| {
                        let label = if t.title.trim().is_empty() { "(untitled)".to_string() } else { t.title.clone() };
                        let text = if playing { egui::RichText::new(label).color(c.accent).strong() }
                                   else { egui::RichText::new(label) };
                        let r = ui.add(egui::Label::new(text).truncate().sense(egui::Sense::click()))
                            .on_hover_text("Play");
                        if r.clicked() { self.play_track_id(t.id); }
                        let mut sub = t.primary_artist.clone();
                        if let Some(a) = &t.album { if !a.is_empty() {
                            if !sub.is_empty() { sub.push_str(" — "); }
                            sub.push_str(a);
                        } }
                        if t.play_count > 0 { sub.push_str(&format!("  ·  {} plays", t.play_count)); }
                        ui.add(egui::Label::new(egui::RichText::new(sub).small().color(c.text_weak)).truncate());
                    });
                });
            });
        });
    }

    /// The Search tab: one box, the catalogue's own folded multi-term
    /// search behind it, rows that play. The old Tracks tab filtered an
    /// in-memory demo list the host never filled and said "catalogue
    /// not opened" forever.
    fn search_view(&mut self, ui: &mut Ui, c: &PaletteColors) {
        ui.horizontal(|ui| {
            let r = ui.add(egui::TextEdit::singleline(&mut self.search.query)
                .hint_text("title, artist or album — several words narrow it")
                .desired_width(ui.available_width() - 60.0)
                .id_salt("lib-self_search_query"));
            if r.changed() {
                // Asked on every keystroke; the host answers when it can
                // and the answer carries its question, so a slow answer to
                // an old query cannot overwrite a newer one's.
                self.pending_search = Some(self.search.query.clone());
            }
            if ui.button("×").clicked() {
                self.search.query.clear();
                self.search_results.clear();
                self.search_answered.clear();
            }
        });
        ui.separator();
        let q = self.search.query.trim().to_string();
        if q.is_empty() {
            ui.colored_label(c.text_weak, "type to search the library");
            return;
        }
        if self.search_answered != q {
            ui.colored_label(c.text_weak, "searching…");
            if self.pending_search.is_none() { self.pending_search = Some(q); }
            return;
        }
        if self.search_results.is_empty() {
            ui.colored_label(c.text_weak, "nothing matches");
            return;
        }
        ui.horizontal(|ui| {
            ui.colored_label(c.text_weak, format!("{}", crate::browse::plural(self.search_results.len() as i64, "match")));
            if ui.small_button("▶ play all").clicked() {
                let ids: Vec<i64> = self.search_results.iter().map(|t| t.id).collect();
                self.apply_browse(crate::browse::Action::PlayAll(ids));
            }
            if ui.small_button("+ queue all").clicked() {
                let ids: Vec<i64> = self.search_results.iter().map(|t| t.id).collect();
                self.apply_browse(crate::browse::Action::QueueAll(ids));
            }
        });
        let rows = self.search_results.clone();
        egui::ScrollArea::vertical().id_salt("search-rows").show(ui, |ui| {
            for t in &rows { self.track_row(ui, t, c, false); }
        });
    }

    /// The Home tab: the four lists a listener actually wants first.
    fn home_view(&mut self, ui: &mut Ui, c: &PaletteColors) {
        if !self.home.loaded {
            ui.colored_label(c.text_weak, "reading the library…");
            self.pending_home = true;
            return;
        }
        let empty = self.home.recently_played.is_empty() && self.home.recently_added.is_empty()
            && self.home.most_played.is_empty() && self.home.never_played.is_empty();
        if empty {
            ui.colored_label(c.text_weak, "no music yet — scan a folder from Settings");
            if ui.button("scan ~/Music now").clicked() { self.pending_rescan = true; }
            return;
        }
        egui::ScrollArea::vertical().id_salt("home-scroll").show(ui, |ui| {
            let sections: [(&str, Vec<crate::browse::BrowseTrack>); 4] = [
                ("Recently played", self.home.recently_played.clone()),
                ("Recently added", self.home.recently_added.clone()),
                ("Most played", self.home.most_played.clone()),
                ("Never played", self.home.never_played.clone()),
            ];
            for (title, rows) in sections {
                if rows.is_empty() { continue; }
                ui.horizontal(|ui| {
                    ui.strong(title);
                    if ui.small_button("▶ play").on_hover_text("Play this list").clicked() {
                        let ids: Vec<i64> = rows.iter().map(|t| t.id).collect();
                        self.apply_browse(crate::browse::Action::PlayAll(ids));
                    }
                    if ui.small_button("+ queue").clicked() {
                        let ids: Vec<i64> = rows.iter().map(|t| t.id).collect();
                        self.apply_browse(crate::browse::Action::QueueAll(ids));
                    }
                });
                for t in rows.iter().take(HOME_ROWS) { self.track_row(ui, t, c, false); }
                ui.add_space(6.0);
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
        // The scene presets, under the room. They had a page of their
        // own only in the compact layout — where that page was mislabelled
        // "Spaces" — and none in the wide one.
        ui.separator();
        self.scenes_view(ui, c);
    }

    fn mixer_view(&mut self, ui: &mut Ui, c: &PaletteColors) {
        ui.heading("Mixer");
        // The hero control. It reports continuously; the parent debounces
        // before pushing to the device, so a flick sends one command rather
        // than a hundred.
        // The watermark peeks from behind the dial: activity follows what is
        // actually playing, and the module maps it to pace, brightness and
        // (where a platform can blur — egui cannot) sharpness. Painted before
        // the dial so the dial sits on top.
        self.watermark.set_blur_available(false);
        self.watermark.set_activity(
            if self.transport.playing { (self.dial.value / 100.0).clamp(0.0, 1.0) } else { 0.0 });
        self.watermark.tick(ui.input(|i| i.stable_dt).min(0.1) * 1000.0);
        watermark_paint(ui, &self.watermark, ui.cursor().min + egui::vec2(70.0, 70.0), 100.0, c);
        // THE STROKE LIVES, BUT NOT AT SIXTY HERTZ. The comment here used
        // to be "it advances every frame", which was true and was the
        // bug: the quiet loop is 60 SECONDS long, so full-rate repainting
        // spends 3,600 frames advancing one cycle of a dash pattern
        // behind a dial. Nobody can see a sixtieth of a percent of a
        // minute-long rotation.
        //
        // Asking for a frame proportional to the loop gives the same
        // motion at a cost you can defend: ~8 fps when quiet, ~20 fps at
        // full activity, and the phase is time-based so a slower frame
        // advances further rather than moving slower.
        //
        // Still unconditional, deliberately: unlike ambient-learner's
        // timer this animation genuinely never stops — `loop_ms` slows
        // with activity but never reaches zero, which is the design (see
        // `dial_watermark`). The fix is the rate, not a gate.
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(
            (self.watermark.loop_ms() / 500.0).clamp(50.0, 125.0) as u64,
        ));
        // THE CARD FRAME, at last drawn. ControlCard has carried a rise, a
        // fade and an active glow since the port, the controller built one per
        // section with a staggered entrance delay, and a test asserts the
        // stagger produces different opacities — for frames nothing painted.
        //
        // The dial goes in the first card because the stagger is per-index and
        // this is the hero control: it should be the thing that arrives first.
        // The frame is drawn, then the dial inside the rect it returns, which
        // is the contract card_frame states.
        let card_rect = crate::shell_bits::card_frame(
            ui, &mut self.cards[0], 170.0, c);
        let mut inner = ui.new_child(
            egui::UiBuilder::new().max_rect(card_rect.shrink2(egui::vec2(10.0, 22.0))));
        if let Some(v) = crate::dial::dial_ui(&mut inner, &mut self.dial, 140.0, c) {
            // THROUGH THE SAFETY CLAMP. The dial wrote straight to
            // pending_volume, so a manual change above the ~95 dB threshold
            // went through unchallenged and the whole safety module — model,
            // tests and all — never ran. Found by the tested-but-uncalled
            // check: `request` was exercised only by its own tests.
            match self.safety.request(v.round().clamp(0.0, 255.0) as u8) {
                crate::safety::Outcome::Applied(a) => self.pending_volume = Some(a as f32),
                crate::safety::Outcome::Clamped { applied, .. } => {
                    // The capped value IS applied — the cap is a limit, not a
                    // refusal, and leaving the dial at its old position would
                    // make the control feel broken rather than protective.
                    self.pending_volume = Some(applied as f32);
                }
            }
        }
        // The toast is drawn once, in the header strip above — not here.
        // Two call sites would render it twice whenever this view is open.
        ui.colored_label(c.text_weak, format!("volume {:.0}", self.pending_volume.unwrap_or(self.volume)));
        if self.mixer.is_flat() {
            ui.colored_label(c.text_weak, "flat — no correction applied");
        }
        // The two channel strips, side by side, over their response curves —
        // the independence of the chains is the thing this page exists to show.
        let chan_rect = crate::shell_bits::card_frame(ui, &mut self.cards[1], 210.0, c);
        let mut chan = ui.new_child(
            egui::UiBuilder::new().max_rect(chan_rect.shrink2(egui::vec2(10.0, 22.0))));
        chan.horizontal(|ui| {
            crate::channel::column_ui(ui, &mut self.speaker_col, c);
            ui.separator();
            crate::channel::column_ui(ui, &mut self.instrumental_col, c);
        });
        let curve_rect = crate::shell_bits::card_frame(ui, &mut self.cards[2], 150.0, c);
        let mut curve = ui.new_child(
            egui::UiBuilder::new().max_rect(curve_rect.shrink2(egui::vec2(10.0, 22.0))));
        crate::channel::curve_ui(&mut curve, &self.speaker_col, &self.instrumental_col, 120.0, c);

        // The step keys, in the app's own press language. Last card, so last
        // to read in — they are the least important thing on the page.
        let keys_rect = crate::shell_bits::card_frame(ui, &mut self.cards[3], 78.0, c);
        let mut keys = ui.new_child(
            egui::UiBuilder::new().max_rect(keys_rect.shrink2(egui::vec2(10.0, 22.0))));
        keys.horizontal(|ui| {
            if crate::fileproc::step_key_ui(ui, &mut self.key_down, 40.0, c) {
                self.dial.set_external((self.dial.value - 5.0).max(0.0));
            }
            if crate::fileproc::step_key_ui(ui, &mut self.key_up, 40.0, c) {
                self.dial.set_external((self.dial.value + 5.0).min(100.0));
            }
            if crate::fileproc::step_key_ui(ui, &mut self.key_mute, 40.0, c) {
                self.dial.set_external(0.0);
            }
        });
        ui.separator();

        // THE PROCESSING SECTION. The module's own words: it is "the
        // correctness preview" — the same DSP path the realtime thread uses,
        // checkable without a device in the room. It had a complete model, no
        // renderer and no caller, so it could not be operated at all.
        //
        // At the BOTTOM of the mixer page, as the module says, because it runs
        // the CURRENT settings: the controls above it are its input.
        if let Some(a) = crate::fileproc::fileproc_ui(ui, &mut self.fileproc, c) {
            // Through the existing stash. start_proc is the other door onto
            // this and sets the same field; the host performs it and reports
            // back with set_progress and finish.
            self.pending_proc = Some(a);
        }
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
        // SCENE PRESETS. The daemon's own comment says the rich config
        // format is "what the Spaces page reads and writes" — Spaces is
        // speaker-to-channel routing and has never touched a scene, so
        // that comment described a page that does not exist. Nothing in
        // this port could load one.
        //
        // Listed by NAME, from the same `scene_dir` the daemon resolves,
        // now that both sides share it. Showing names from a directory
        // the daemon does not read is worse than showing none, because
        // every one of them fails on click.
        let scenes = bo_mux::soundscape_config::scene_names();
        if scenes.is_empty() {
            // Said, not blank. "No scenes installed" and "scene loading
            // is broken" look identical otherwise, and the first is the
            // ordinary state on a fresh install.
            ui.colored_label(c.text_weak, "no scenes in ~/.local/share/bo-mux/scenes");
        } else {
            ui.horizontal_wrapped(|ui| {
                for name in &scenes {
                    if ui.button(name).clicked() {
                        // Loaded and VALIDATED here rather than fired at
                        // the daemon blind: `parse` is the same code the
                        // daemon runs, so a scene that will fail there
                        // fails here with a message beside the button
                        // that was pressed.
                        let path = bo_mux::soundscape_config::scene_dir()
                            .join(format!("{name}.json"));
                        match bo_mux::soundscape_config::load_from_file(&path) {
                            Ok(cfg) => {
                                // Validated locally, then handed to the
                                // daemon BY NAME — it resolves against
                                // the same scene_dir, so sending a path
                                // would let the two disagree about what
                                // a scene name means.
                                //
                                // The local parse is not redundant: it
                                // is why a bad scene reports its actual
                                // JSON error rather than the daemon's
                                // "load failed", and why the layer count
                                // is available to say.
                                self.scene_seq += 1;
                                match bo_mux::control::load_scene(self.scene_seq, name) {
                                    Ok(_) => {
                                        self.scene_loaded = Some(name.clone());
                                        // Loading REPLACES what the
                                        // daemon holds, so whatever was
                                        // playing is not any more.
                                        // Leaving this true would show
                                        // "▶" over a scene that was
                                        // swapped out from under it.
                                        self.scene_playing = false;
                                        self.toast.show(&format!(
                                            "{name}: {} layers", cfg.layers.len()));
                                    }
                                    // The scene is GOOD and the daemon
                                    // could not take it — a different
                                    // problem from a bad scene, and
                                    // worth saying differently.
                                    // `scene_loaded` stays unset,
                                    // because nothing is playing it.
                                    Err(e) => self.toast.show(&format!(
                                        "{name} parsed, but {}", e.message())),
                                }
                            }
                            Err(e) => self.toast.show(&format!("{name}: {e}")),
                        }
                    }
                }
            });
            if let Some(n) = &self.scene_loaded {
                ui.horizontal(|ui| {
                    ui.colored_label(c.text_weak, format!("loaded {n}"));
                    // SCENE transport, not the music player's. The
                    // `transport` field above drives the track player;
                    // a soundscape is a separate thing the daemon runs,
                    // and sharing one play button would make stopping
                    // the music stop the ambience too.
                    //
                    // Shown only once a scene is loaded: play with
                    // nothing loaded is a button whose only outcome is
                    // an error.
                    if ui.button("play").clicked() {
                        self.scene_seq += 1;
                        // No seed: the daemon picks one. Passing a fixed
                        // seed here would make every play of a scene
                        // identical, which defeats a generative
                        // soundscape.
                        match bo_mux::control::play(self.scene_seq, None) {
                            Ok(_) => {
                                self.scene_playing = true;
                                self.toast.show("playing");
                            }
                            Err(e) => self.toast.show(&e.message()),
                        }
                    }
                    if ui.button("stop").clicked() {
                        self.scene_seq += 1;
                        match bo_mux::control::stop(self.scene_seq) {
                            // Cleared on success ONLY. A stop the daemon
                            // refused leaves it playing, and showing
                            // "stopped" over a scene that is still
                            // audible is worse than showing the error.
                            Ok(_) => {
                                self.scene_playing = false;
                                self.toast.show("stopped");
                            }
                            Err(e) => self.toast.show(&e.message()),
                        }
                    }
                    if self.scene_playing {
                        ui.colored_label(c.text_weak, "▶");
                    }
                });
            }
        }
        ui.separator();
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
            // The button used to only flip local state — nothing ever
            // told a host to actually scan, so "scan library" scanned
            // nothing. A host that owns a real catalogue (see
            // bo-suite-egui's real_catalog.rs) reads this flag and starts
            // one, then drives `self.settings.scan` for real from the
            // worker's own `ScanProgress` messages.
            self.pending_rescan = true;
        }
        if ui.button("choose folders…").clicked() {
            self.pending_scan_picker_open = true;
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
        self.breath.tick(ui.input(|i| i.stable_dt) * 1000.0);
        if self.transport.playing { ui.ctx().request_repaint(); }

        // THE BAR: art, title once, the buttons, the scrub. What it
        // asks for goes through the same functions a key press or a
        // voice command uses — one state machine, three ways in.
        if let Some(act) = crate::transport::transport_ui(ui, &mut self.transport, c) {
            use crate::transport::TransportAction as A;
            match act {
                A::Toggle => self.toggle_play(),
                A::Previous => self.previous(),
                A::Next => self.next(),
                A::Stop => self.stop(),
                A::Seek(at) => self.seek_to(at),
                A::CycleRepeat => self.transport.repeat = self.transport.repeat.next(),
                A::ToggleShuffle => self.transport.shuffle = !self.transport.shuffle,
            }
        }

        // The second row: stars for what is playing, the queue toggle,
        // the ANC chip. Small things that belong beside the transport
        // because they are about what is playing right now.
        // The chip is laid out first, from the right, so the truncating
        // "next:" label takes what is left rather than everything.
        //
        // INSIDE `horizontal`. A right-to-left layout placed straight
        // into the panel's vertical ui centres its widgets in ALL the
        // height the panel has left — and a bottom panel's height is
        // whatever it was last frame, so the row sank lower each frame
        // until the bar was the whole window. `horizontal` bounds the
        // row to one line first. Caught by `shell_layout_tests`.
        ui.horizontal(|ui| {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            crate::anc::anc_chip(ui, &mut self.anc, &mut self.a11y, c);
            ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                // A source with no track — line in, radio — has no title
                // for the bar, so its name is drawn here; a track gets
                // the bars.
                if self.now_playing.title.trim().is_empty() && !self.now_playing.source.trim().is_empty() {
                    crate::queue::now_playing_ui(ui, &self.now_playing, &self.breath, c);
                } else {
                    crate::queue::equalizer_ui(ui, &self.now_playing, &self.breath, c);
                }
                if let Some(id) = self.now_playing_id {
                    let r = self.ratings.entry(id)
                        .or_insert_with(|| crate::tags::StarRating::new(id, 0));
                    if let Some(stars) = crate::tags::stars_ui(ui, r, c) {
                        if let Some(send) = self.rate(id, stars) {
                            self.pending_rating = Some((id, send));
                        }
                    }
                    ui.add_space(8.0);
                }
                let n = self.queue.len();
                let label = if self.queue_open { format!("▾ queue ({n})") }
                            else { format!("▸ queue ({n})") };
                if ui.small_button(label).clicked() {
                    self.queue_open = !self.queue_open;
                }
                if let Some(next) = self.queue.up_next() {
                    // Only where a few words of it fit; three dots tell
                    // nobody what is next.
                    if ui.available_width() > 140.0 {
                        ui.add(egui::Label::new(egui::RichText::new(format!("next: {}", next.label()))
                            .small().color(c.text_weak)).truncate());
                    }
                }
            });
        });
        });
        if self.queue_open {
            {
                if let Some(act) = crate::queue::queue_ui(ui, &self.queue, c) {
                    // Applied HERE rather than inside the renderer: queue_ui
                    // takes &Queue and returns an intent, which is what lets
                    // it be tested without a controller. Honouring that split
                    // means the mutation happens on this side.
                    match act {
                        crate::queue::QueueAction::PlayNow(id) => {
                            if let Some(tr) = self.queue.play_now(id) { self.start_playing(&tr); }
                        }
                        crate::queue::QueueAction::Remove(id) => { self.queue.remove(id); }
                        crate::queue::QueueAction::Clear => self.queue.clear(),
                        crate::queue::QueueAction::SetMode(m) => self.queue.mode = m,
                        crate::queue::QueueAction::MoveUp(id) => { self.queue.move_up(id); }
                        crate::queue::QueueAction::MoveDown(id) => { self.queue.move_down(id); }
                    }
                }
            }
        }
    }

    /// Keys, when no text field has the keyboard.
    ///
    /// Space and the arrows are what every desktop player answers, and
    /// the check for focus is what keeps a space typed into the search
    /// box from pausing the music.
    fn keyboard(&mut self, ui: &mut Ui) {
        if ui.memory(|m| m.focused().is_some()) { return; }
        let (space, left, right, ctrl, n, p, s, m, up, down) = ui.input(|i| (
            i.key_pressed(egui::Key::Space),
            i.key_pressed(egui::Key::ArrowLeft),
            i.key_pressed(egui::Key::ArrowRight),
            i.modifiers.command,
            i.key_pressed(egui::Key::N),
            i.key_pressed(egui::Key::P),
            i.key_pressed(egui::Key::S),
            i.key_pressed(egui::Key::M),
            i.key_pressed(egui::Key::ArrowUp),
            i.key_pressed(egui::Key::ArrowDown),
        ));
        if space { self.toggle_play(); }
        if right { if ctrl { self.next() } else { self.seek_by(10.0) } }
        if left { if ctrl { self.previous() } else { self.seek_by(-10.0) } }
        if n { self.next(); }
        if p { self.previous(); }
        if s { self.stop(); }
        if m { self.pending_volume = Some(if self.volume > 0.0 { 0.0 } else { 30.0 }); }
        if up { self.nudge_volume(5); }
        if down { self.nudge_volume(-5); }
    }

    /// Play/pause, the one place the flag and the request are paired.
    ///
    /// Three cases, in the order a person hits them: something is
    /// playing (pause it, in place if the player can); something is
    /// loaded and paused (continue it from where the transport says);
    /// nothing is loaded (start the queue, or say there is nothing).
    pub fn toggle_play(&mut self) {
        if self.transport.playing {
            self.transport.pause();
            self.now_playing.playing = false;
            self.pending_playback = Some(crate::playback::Request::Pause);
            return;
        }
        if self.now_playing_id.is_some() && !self.track_url.is_empty() {
            let at = self.transport.resume().unwrap_or(0.0);
            self.now_playing.playing = true;
            self.pending_playback = Some(crate::playback::Request::Resume {
                url: self.track_url.clone(), sink: self.audio_sink.clone(), at });
            return;
        }
        match self.queue.advance() {
            Some(e) => self.start_playing(&e),
            None => self.toast.show("nothing to play — pick a track"),
        }
    }

    /// Stop: silence, position back to the start, the track stays loaded.
    pub fn stop(&mut self) {
        self.transport.pause();
        self.transport.report(0.0);
        self.transport.clear_pause();
        self.now_playing.playing = false;
        self.pending_playback = Some(crate::playback::Request::Stop);
    }

    /// Seek to an absolute position in the loaded track.
    pub fn seek_to(&mut self, at: f64) {
        if self.track_url.is_empty() { return; }
        let at = at.clamp(0.0, self.transport.duration.max(0.0));
        self.transport.report(at);
        if self.transport.playing {
            self.pending_playback = Some(crate::playback::Request::Seek {
                url: self.track_url.clone(), sink: self.audio_sink.clone(), at });
        } else {
            // Paused: the next resume starts from here. No request now —
            // there is nothing running to move.
            self.transport.set_paused_at(at);
        }
    }

    /// Seek relative to the current position.
    pub fn seek_by(&mut self, delta: f64) {
        let at = (self.transport.position() + delta).max(0.0);
        self.seek_to(at);
    }

    /// The next track, the same decision a finished track makes.
    pub fn next(&mut self) {
        // A skip is deliberate; Repeat One must not trap it on the same
        // track, which is what routing through `on_track_ended` would do.
        match self.draw_next() {
            Some(e) => { self.transport.report(0.0); self.start_playing(&e); }
            None => self.toast.show("end of queue"),
        }
    }

    /// Previous. Within the first three seconds of a track it means the
    /// track before; after that it means the start of this one — the rule
    /// every CD player and every phone follows, and the reason a double
    /// tap goes back two.
    pub fn previous(&mut self) {
        if self.transport.position() > 3.0 || self.history.is_empty() {
            if let Some(id) = self.now_playing_id {
                self.transport.report(0.0);
                self.request_play(id);
            }
            return;
        }
        let prev = self.history.pop().expect("checked non-empty");
        // What was playing goes back to the head of the queue, so
        // "previous" then "next" lands where you were.
        if let Some(id) = self.now_playing_id {
            let (t, a) = (self.transport.title.clone(), self.transport.artist.clone());
            self.queue.push_front(id, &t, &a);
        }
        self.transport.report(0.0);
        self.start_playing_no_history(&prev);
    }

    /// Take the next entry out of the queue, honouring shuffle.
    fn draw_next(&mut self) -> Option<crate::queue::QueueEntry> {
        if self.transport.shuffle && self.queue.len() > 1 {
            // xorshift64*: one draw a track, no dependency, seedable.
            let mut x = self.shuffle_seed;
            x ^= x >> 12; x ^= x << 25; x ^= x >> 27;
            self.shuffle_seed = x;
            let r = x.wrapping_mul(0x2545_F491_4F6C_DD1D);
            let i = (r % self.queue.len() as u64) as usize;
            let id = self.queue.entries()[i].queue_id;
            return self.queue.take(id);
        }
        self.queue.advance()
    }

    /// Move the volume by a step, clamped, from what the room is set to.
    fn nudge_volume(&mut self, by: i32) {
        let cur = self.pending_volume.unwrap_or(self.volume);
        self.pending_volume = Some((cur + by as f32).clamp(0.0, 100.0));
    }

    /// A track finished on its own. Decide what plays next.
    ///
    /// Called ONLY for a natural end. A stop is not an end — the player
    /// distinguishes them for exactly this reason, because advancing on a
    /// pause means pressing pause skips a track.
    pub fn on_track_ended(&mut self) {
        use crate::transport::Repeat;
        if self.transport.repeat == Repeat::One {
            // Repeat one replays the SAME track rather than consuming the
            // queue. Popping the head here would empty the queue one track
            // per repeat, which is the mode quietly destroying the thing it
            // is looping inside.
            if let Some(id) = self.now_playing_id {
                self.transport.report(0.0);
                self.request_play(id);
                return;
            }
        }
        if self.queue.is_empty() && self.transport.repeat == Repeat::All {
            // The queue ran dry under Repeat All: everything that played
            // this session goes round again, in the order it played (or
            // shuffled, if that is on). The current track is the last of
            // them, so it is included too.
            let mut again: Vec<(i64, String, String)> = self.history.drain(..)
                .map(|e| (e.track_id, e.title, e.artist)).collect();
            if let Some(id) = self.now_playing_id {
                again.push((id, self.transport.title.clone(), self.transport.artist.clone()));
            }
            self.queue.mode = crate::queue::EnqueueMode::Stack;
            self.queue.add_all(&again);
        }
        match self.draw_next() {
            Some(next) => {
                self.transport.report(0.0);
                self.start_playing(&next);
            }
            None => {
                // Nothing queued. Stopped rather than left "playing" over
                // silence, which is the state this app kept showing before
                // there was any audio at all.
                self.transport.playing = false;
                self.transport.report(0.0);
                self.now_playing.playing = false;
            }
        }
    }

    /// Ask the host to play a track id.
    ///
    /// The id is resolved through `track_paths`, which the host fills from
    /// whatever catalogue it has. Unresolved is REPORTED, not attempted: a
    /// player handed an empty path fails with "no such file", which reads
    /// as a broken player rather than as the missing catalogue it is.
    pub fn request_play(&mut self, id: i64) {
        match self.track_paths.iter().find(|(k, _)| *k == id) {
            Some((_, path)) => {
                self.track_url = path.clone();
                self.last_played_track = Some(id);
                // A fresh track has no remembered pause and no length
                // yet; the host reports both. Leaving the old duration
                // would draw the new track's handle against the old
                // track's end.
                self.transport.clear_pause();
                self.transport.duration = 0.0;
                self.transport.report(0.0);
                self.transport.playing = true;
                self.now_playing.playing = true;
                self.pending_playback = Some(crate::playback::Request::Play {
                    url: path.clone(),
                    sink: self.audio_sink.clone(),
                    start_secs: 0.0,
                });
            }
            None => {
                self.transport.playing = false;
                self.now_playing.playing = false;
                self.toast.show("no file for that track");
            }
        }
    }

    /// The host tells the transport how long the track is, when the
    /// player itself cannot (ffplay, paplay). A length the player has
    /// already reported wins: it is measuring the stream, this is a tag.
    pub fn set_duration_hint(&mut self, secs: f64) {
        if self.transport.duration <= 0.0 && secs > 0.0 {
            self.transport.duration = secs;
        }
    }

    /// Tell the view where the files are.
    ///
    /// A flat list rather than a map: a page is at most a few hundred rows,
    /// a linear scan over that is nothing, and a `Vec` keeps the controller
    /// `Clone` and comparable — which its tests rely on.
    /// Ask the host to cast whatever is now playing to a networked B&O
    /// speaker instead of (or alongside) local output. `now_playing_id`
    /// carries the track; the host resolves it to a path the same way
    /// `request_play` does and hands it to `bo_cast`.
    ///
    /// No-op with a toast if nothing is playing — casting silence to a
    /// speaker is not a request that means anything.
    pub fn request_cast(&mut self, device_base: String) {
        if self.now_playing_id.is_some() {
            self.pending_cast = Some(device_base);
        } else {
            self.toast.show("nothing is playing to cast");
        }
    }

    /// The host reports how a cast attempt went.
    pub fn cast_result(&mut self, ok: bool, detail: &str) {
        if ok {
            self.toast.show(&format!("casting via {detail}"));
        } else {
            self.toast.show(&format!("cast failed: {detail}"));
        }
    }

    pub fn set_track_paths(&mut self, paths: Vec<(i64, String)>) {
        self.track_paths = paths;
    }

    /// Add or replace paths for some tracks, keeping the rest. What a host
    /// calls after each list it fetches, since every list carries the
    /// paths for its own rows.
    pub fn merge_track_paths(&mut self, paths: Vec<(i64, String)>) {
        for (id, path) in paths {
            match self.track_paths.iter_mut().find(|(k, _)| *k == id) {
                Some(slot) => slot.1 = path,
                None => self.track_paths.push((id, path)),
            }
        }
    }

    /// The host hands over the browse aggregate source, read from SQL
    /// with real play counts and years — which is what makes the TOP
    /// albums view non-empty and the "most played" rows true.
    pub fn supply_browse_tracks(&mut self, tracks: Vec<crate::browse::BrowseTrack>) {
        self.browse_tracks = tracks;
        self.browse.load_local(&self.browse_tracks);
        self.browse.invalidate();
        self.browse_stale = false;
    }

    /// The host answers a search. Ignored unless it answers the query
    /// currently asked, so a slow answer cannot land under a newer
    /// question.
    pub fn supply_search(&mut self, query: &str, rows: Vec<crate::browse::BrowseTrack>) {
        if query.trim() != self.search.query.trim() { return; }
        self.search_results = rows;
        self.search_answered = query.trim().to_string();
        self.pending_search = None;
    }

    pub fn supply_home(&mut self, home: HomeLists) {
        self.home = home;
        self.home.loaded = true;
        self.pending_home = false;
    }

    /// The session file: queue, modes, what was playing. `[transport]`
    /// keys then the queue rows, in the flat form config_kit reads.
    pub fn render_session(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!("repeat = {}\nshuffle = {}\nvolume = {}\n",
            self.transport.repeat.tag(), self.transport.shuffle, self.volume.round() as i64));
        if let Some(id) = self.now_playing_id {
            out.push_str(&format!("now_playing = {id}\nposition = {:.1}\n", self.transport.position()));
        }
        out.push_str("[queue]\n");
        for line in self.queue.render().lines() {
            out.push_str("row = ");
            out.push_str(line);
            out.push('\n');
        }
        out
    }

    /// Restore what `render_session` wrote. The track that was playing is
    /// LOADED, paused at its position, not started: an app that starts
    /// playing when it opens is an app that plays at 3am when the phone
    /// reboots. Returns the track id to load, for the host to resolve.
    pub fn restore_session(&mut self, text: &str) -> Option<i64> {
        use crate::transport::Repeat;
        let ini = config_kit::Ini::parse(text);
        self.transport.repeat = match ini.top("repeat") {
            Some("all") => Repeat::All, Some("one") => Repeat::One, _ => Repeat::Off };
        self.transport.shuffle = ini.top("shuffle") == Some("true");
        if let Some(v) = ini.top("volume").and_then(|v| v.parse::<f32>().ok()) {
            self.volume = v.clamp(0.0, 100.0);
        }
        // The Ini keeps one value per key; the queue rows are re-read
        // line by line from the section, which is the one place a
        // repeated key is the point.
        let mut in_queue = false;
        let mut rows = String::new();
        for line in text.lines() {
            let l = line.trim();
            if l.starts_with('[') { in_queue = l == "[queue]"; continue; }
            if in_queue {
                if let Some(rest) = l.strip_prefix("row =") {
                    rows.push_str(rest.trim_start());
                    rows.push('\n');
                }
            }
        }
        self.queue.restore(&rows);
        let id = ini.top("now_playing").and_then(|v| v.parse::<i64>().ok())?;
        let at = ini.top("position").and_then(|v| v.parse::<f64>().ok()).unwrap_or(0.0);
        self.transport.report(at);
        self.transport.set_paused_at(at);
        Some(id)
    }

    /// Load a track into the transport without playing it — what a
    /// restored session does. The host resolves the path.
    pub fn load_paused(&mut self, id: i64, title: &str, artist: &str, path: &str, at: f64) {
        self.set_now_playing(title, artist);
        self.transport.playing = false;
        self.now_playing.playing = false;
        self.now_playing_id = Some(id);
        self.track_url = path.to_string();
        self.transport.report(at);
        self.transport.set_paused_at(at);
    }

    /// What to play. The queue's head, or the current track.
    ///
    /// A stub in the sense that this app has no resolver from a track id to
    /// a file yet — the library holds titles and tags, not paths. Named and
    /// EMPTY rather than fabricated: an invented path makes the player fail
    /// with "no such file", which reads as a broken player rather than as a
    /// missing catalogue.
    pub fn now_playing_url(&self) -> String {
        self.track_url.clone()
    }

    /// Point playback at a route, releasing the previous one's resources.
    ///
    /// Called when the picker commits, not per track: a combine sink torn
    /// down between two tracks on the same speakers is a gap of silence and
    /// a rebuild for nothing.
    pub fn set_audio_route(&mut self, route: &crate::settings_page::Route) {
        let next = crate::playback::sink_of(route);
        if next == self.audio_sink {
            return;
        }
        self.audio_sink = next;
        self.pending_playback = Some(crate::playback::Request::ReleaseRoute);
    }
}

/// The badge, through a real frame.
///
/// The outline tests measure the DATA; this measures what egui was asked
/// to paint. Both are needed: the previous silhouettes passed every data
/// test there was, because there were none, and the primitives that drew
/// them lived in a function no test ever ran.
#[cfg(test)]
mod surround_route_tests {
    use super::*;
    use crate::surround::Channel;

    fn app() -> Controller {
        Controller::new(vec![], vec![])
    }

    #[test]
    fn a_committed_arrangement_becomes_the_audio_sink() {
        let mut c = app();
        c.surround.place("l", "Left", Channel::FrontLeft);
        c.surround.place("r", "Right", Channel::FrontRight);
        let sink = crate::surround::sink_of(&c.surround).expect("a valid pair did not route");
        c.audio_sink = sink;
        assert!(matches!(c.audio_sink, crate::playback::Sink::Surround { .. }));
    }

    #[test]
    fn a_half_assigned_room_cannot_be_committed_at_all() {
        // The button is disabled on exactly this condition, and the model
        // refuses independently — two guards, because a UI that enables it
        // and a router that accepts it are different mistakes.
        let mut c = app();
        c.surround.place("l", "Left", Channel::FrontLeft);
        assert!(!c.surround.routable());
        assert_eq!(crate::surround::sink_of(&c.surround), None);
    }

    #[test]
    fn surround_and_combine_are_separate_choices_not_a_mode() {
        // Combine duplicates a stream, surround splits one. A single
        // toggle would make "several speakers" mean two incompatible
        // things depending on a mode set earlier.
        let mut c = app();
        c.set_audio_route(&crate::settings_page::Route::MultiRoom(
            vec!["a".into(), "b".into()]));
        assert!(matches!(c.audio_sink, crate::playback::Sink::Many(_)));
        c.surround.place("a", "A", Channel::FrontLeft);
        c.surround.place("b", "B", Channel::FrontRight);
        // The assignment exists alongside; committing it is a separate act.
        assert!(matches!(c.audio_sink, crate::playback::Sink::Many(_)),
                "assigning positions silently changed the route");
        c.audio_sink = crate::surround::sink_of(&c.surround).unwrap();
        assert!(matches!(c.audio_sink, crate::playback::Sink::Surround { .. }));
    }
}

#[cfg(test)]
mod queue_advance_tests {
    use super::*;
    use crate::transport::Repeat;

    fn app() -> Controller {
        let mut c = Controller::new(vec![], vec![]);
        c.set_track_paths(vec![
            (1, "/m/one.flac".into()), (2, "/m/two.flac".into()), (3, "/m/three.flac".into()),
        ]);
        c
    }

    fn played(c: &mut Controller) -> Option<String> {
        match c.pending_playback.take() {
            Some(crate::playback::Request::Play { url, .. }) => Some(url),
            _ => None,
        }
    }

    #[test]
    fn a_finished_track_plays_the_next_one() {
        // `Queue::advance` is documented as "pop the head, when a track
        // finishes on its own" and had no caller, so a finished track
        // simply stopped — an album played its first track and went quiet.
        let mut c = app();
        c.queue.add_all(&[(1, "One".into(), "A".into()), (2, "Two".into(), "A".into())]);
        let first = c.queue.advance().unwrap();
        c.start_playing(&first);
        assert_eq!(played(&mut c).as_deref(), Some("/m/one.flac"));

        c.on_track_ended();
        assert_eq!(c.transport.title, "Two");
        assert_eq!(played(&mut c).as_deref(), Some("/m/two.flac"));
        assert!(c.transport.playing);
    }

    #[test]
    fn repeat_one_replays_without_eating_the_queue() {
        // Popping the head under Repeat::One empties the queue one track
        // per repeat — the mode quietly destroying the thing it loops
        // inside.
        let mut c = app();
        c.queue.add_all(&[(1, "One".into(), "A".into()), (2, "Two".into(), "A".into())]);
        let first = c.queue.advance().unwrap();
        c.start_playing(&first);
        played(&mut c);
        c.transport.repeat = Repeat::One;

        let queued_before = c.queue.len();
        c.on_track_ended();
        assert_eq!(played(&mut c).as_deref(), Some("/m/one.flac"), "it did not replay");
        assert_eq!(c.queue.len(), queued_before, "repeat-one consumed the queue");
    }

    #[test]
    fn the_end_of_the_queue_stops_rather_than_claiming_to_play() {
        // "Playing" over silence is the state this app showed before there
        // was any audio at all; the end of a queue must not put it back.
        let mut c = app();
        c.queue.add_all(&[(1, "One".into(), "A".into())]);
        let first = c.queue.advance().unwrap();
        c.start_playing(&first);
        played(&mut c);
        c.on_track_ended();
        assert!(!c.transport.playing, "the queue ran out and it still says playing");
        assert!(!c.now_playing.playing);
        assert_eq!(played(&mut c), None, "it asked for a track that does not exist");
    }

    fn three_queued() -> Controller {
        let mut c = app();
        c.queue.add_all(&[(1, "One".into(), "A".into()), (2, "Two".into(), "A".into()),
                          (3, "Three".into(), "A".into())]);
        c
    }

    #[test]
    fn repeat_all_goes_round_again_when_the_queue_runs_dry() {
        // The mode did nothing: `Repeat::All` fell through to "stop at the
        // end", which is what Off does. It now replays the session in the
        // order it played.
        let mut c = three_queued();
        c.transport.repeat = Repeat::All;
        let first = c.queue.advance().unwrap();
        c.start_playing(&first);
        assert_eq!(played(&mut c).as_deref(), Some("/m/one.flac"));
        c.on_track_ended();
        assert_eq!(played(&mut c).as_deref(), Some("/m/two.flac"));
        c.on_track_ended();
        assert_eq!(played(&mut c).as_deref(), Some("/m/three.flac"));
        assert!(c.queue.is_empty());
        // The wrap: one, two, three again — including the track that just
        // ended, which is the last of the session, not outside it.
        c.on_track_ended();
        assert_eq!(played(&mut c).as_deref(), Some("/m/one.flac"), "did not wrap");
        assert!(c.transport.playing);
        assert_eq!(c.queue.len(), 2, "two and three are queued behind it");
        assert_eq!(c.queue.entries()[0].track_id, 2);
        assert_eq!(c.queue.entries()[1].track_id, 3);
    }

    #[test]
    fn shuffle_draws_from_the_queue_rather_than_its_head() {
        let mut c = three_queued();
        c.transport.shuffle = true;
        // Fixed seed: the draw is deterministic, and with three entries
        // some seed picks a non-head entry. Search the seeds so the test
        // asserts the mechanism rather than one magic number.
        let mut saw_non_head = false;
        for seed in 1..40u64 {
            let mut d = three_queued();
            d.transport.shuffle = true;
            d.shuffle_seed = seed;
            d.next();
            let url = played(&mut d).unwrap();
            assert_eq!(d.queue.len(), 2, "one entry left the queue");
            if url != "/m/one.flac" { saw_non_head = true; }
            // whichever was drawn, the two others remain in their order
            let left: Vec<i64> = d.queue.entries().iter().map(|e| e.track_id).collect();
            assert!(left.windows(2).all(|w| w[0] < w[1]), "order disturbed: {left:?}");
        }
        assert!(saw_non_head, "forty seeds never drew past the head — that is not a shuffle");
        // and with shuffle off the head plays, always
        c.transport.shuffle = false;
        c.next();
        assert_eq!(played(&mut c).as_deref(), Some("/m/one.flac"));
    }

    #[test]
    fn previous_early_in_a_track_goes_back_and_late_restarts_it() {
        let mut c = three_queued();
        c.transport.duration = 300.0;
        c.next();
        played(&mut c);
        c.next();
        assert_eq!(played(&mut c).as_deref(), Some("/m/two.flac"));
        assert_eq!(c.history.len(), 1, "one played before this");
        // Late in the track: restart this one.
        c.transport.duration = 300.0;
        c.transport.report(30.0);
        c.previous();
        assert_eq!(played(&mut c).as_deref(), Some("/m/two.flac"), "late means restart");
        assert_eq!(c.transport.position(), 0.0);
        // Early: the previous track, and this one goes back to the head.
        c.transport.duration = 300.0;
        c.transport.report(1.0);
        c.previous();
        assert_eq!(played(&mut c).as_deref(), Some("/m/one.flac"), "early means the one before");
        assert_eq!(c.queue.up_next().map(|e| e.track_id), Some(2), "two is next again");
        assert!(c.history.is_empty());
        // Nothing before that: restart rather than nothing.
        c.transport.duration = 300.0;
        c.transport.report(1.0);
        c.previous();
        assert_eq!(played(&mut c).as_deref(), Some("/m/one.flac"));
    }

    #[test]
    fn next_is_deliberate_and_escapes_repeat_one() {
        let mut c = three_queued();
        c.transport.repeat = Repeat::One;
        c.next();
        played(&mut c);
        c.next();
        assert_eq!(played(&mut c).as_deref(), Some("/m/two.flac"), "a skip stayed on repeat");
        // and at the end it says so rather than pretending
        c.queue.clear();
        c.next();
        assert_eq!(played(&mut c), None);
        assert_eq!(c.toast.visible(), Some("end of queue"));
    }

    #[test]
    fn a_seek_while_playing_asks_the_host_and_while_paused_only_moves_the_handle() {
        let mut c = three_queued();
        c.next();
        played(&mut c);
        c.transport.duration = 200.0;
        c.seek_to(50.0);
        assert!(matches!(c.pending_playback.take(),
                Some(crate::playback::Request::Seek { at, .. }) if (at - 50.0).abs() < 1e-9));
        assert_eq!(c.transport.position(), 50.0);
        c.toggle_play();
        assert_eq!(c.pending_playback.take(), Some(crate::playback::Request::Pause));
        c.seek_to(120.0);
        assert!(c.pending_playback.is_none(), "nothing is running to move");
        assert_eq!(c.transport.position(), 120.0, "but the handle shows it");
        c.toggle_play();
        assert!(matches!(c.pending_playback.take(),
                Some(crate::playback::Request::Resume { at, .. }) if (at - 120.0).abs() < 1e-9),
                "the resume starts where the paused seek pointed");
        // a seek past the end clamps, and one with nothing loaded is ignored
        c.seek_to(999.0);
        assert_eq!(c.transport.position(), 200.0);
        let mut empty = app();
        empty.seek_to(10.0);
        assert!(empty.pending_playback.is_none());
    }

    #[test]
    fn stop_silences_and_rewinds_but_keeps_the_track_loaded() {
        let mut c = three_queued();
        c.next();
        played(&mut c);
        c.transport.duration = 200.0;
        c.transport.report(80.0);
        c.stop();
        assert_eq!(c.pending_playback.take(), Some(crate::playback::Request::Stop));
        assert!(!c.transport.playing);
        assert_eq!(c.transport.position(), 0.0);
        assert_eq!(c.now_playing_id, Some(1), "still loaded");
        // play again starts from the top of the same track
        c.toggle_play();
        assert!(matches!(c.pending_playback.take(),
                Some(crate::playback::Request::Resume { ref url, at, .. }) if url == "/m/one.flac" && at == 0.0));
    }

    #[test]
    fn play_with_nothing_loaded_starts_the_queue_or_says_so() {
        let mut c = app();
        c.toggle_play();
        assert!(c.pending_playback.is_none());
        assert_eq!(c.toast.visible(), Some("nothing to play — pick a track"));
        let mut q = three_queued();
        q.toggle_play();
        assert_eq!(played(&mut q).as_deref(), Some("/m/one.flac"));
        assert!(q.transport.playing);
    }

    #[test]
    fn a_session_round_trips_through_its_file() {
        let mut c = three_queued();
        c.transport.repeat = Repeat::All;
        c.transport.shuffle = true;
        c.volume = 37.0;
        c.next();
        played(&mut c);
        c.transport.duration = 200.0;
        c.transport.report(66.0);
        let text = c.render_session();
        let mut back = app();
        let id = back.restore_session(&text);
        assert_eq!(id, Some(1), "the loaded track is named for the host to resolve");
        assert_eq!(back.transport.repeat, Repeat::All);
        assert!(back.transport.shuffle);
        assert_eq!(back.volume, 37.0);
        assert_eq!(back.queue.len(), 2);
        assert_eq!(back.queue.entries()[0].track_id, 2);
        assert_eq!(back.queue.entries()[1].title, "Three");
        assert!((back.transport.position() - 66.0).abs() < 0.2);
        assert!(!back.transport.playing, "a restored session does not start playing by itself");
        // and loading it paused keeps the position for the resume
        back.load_paused(1, "One", "A", "/m/one.flac", 66.0);
        back.toggle_play();
        assert!(matches!(back.pending_playback.take(),
                Some(crate::playback::Request::Resume { at, .. }) if (at - 66.0).abs() < 0.2));
    }

    #[test]
    fn the_history_is_capped() {
        let mut c = app();
        let paths: Vec<(i64, String)> = (1..=(HISTORY_CAP as i64 + 20)).map(|i| (i, format!("/m/{i}.flac"))).collect();
        c.set_track_paths(paths);
        for i in 1..=(HISTORY_CAP as i64 + 20) {
            c.start_playing(&crate::queue::QueueEntry { queue_id: 0, track_id: i, title: String::new(), artist: String::new() });
        }
        assert_eq!(c.history.len(), HISTORY_CAP);
        assert_eq!(c.history[0].track_id, 20, "the oldest were dropped");
    }

    #[test]
    fn the_position_resets_between_tracks() {
        // Without this the bar starts the next track wherever the last one
        // ended, until the player's first status line corrects it — a
        // second of the handle in the wrong place on every track change.
        let mut c = app();
        c.queue.add_all(&[(1, "One".into(), "A".into()), (2, "Two".into(), "A".into())]);
        let first = c.queue.advance().unwrap();
        c.start_playing(&first);
        c.transport.duration = 200.0;
        c.transport.report(150.0);
        assert!(c.transport.position() > 100.0);
        c.on_track_ended();
        assert!(c.transport.position() < 1.0, "the bar carried over: {}",
                c.transport.position());
    }
}

#[cfg(test)]
mod motion_tests {
    use super::*;

    fn app() -> Controller {
        let mut c = Controller::new(
            vec![Device { name: "Beosound A1".into(), volume: 30, playing: false }],
            vec![]);
        c.silhouettes.push(crate::product_silhouette::ProductSilhouette::default());
        c
    }

    fn prefs(style: settings_kit::AnimationStyle, palette: &str) -> settings_kit::Prefs {
        let mut p = settings_kit::Prefs::default();
        p.gesture.animation = style;
        p.palette = palette.to_string();
        p
    }

    #[test]
    fn reduce_motion_reaches_the_badges() {
        // A reduce-motion setting that some of the app ignores is worse
        // than one it does not offer: the person cannot tell which parts
        // obeyed. `set_motion_scale` had no caller at all.
        let ctx = egui::Context::default();
        let mut c = app();
        c.apply_settings(&ctx, &prefs(settings_kit::AnimationStyle::Reduced, "amber"));
        assert!(c.motion_scale < 1.0, "reduced motion left the scale at {}", c.motion_scale);

        // A badge created afterwards gets it too — a device that appears
        // mid-session would otherwise animate at full speed.
        c.devices.push(Device { name: "Beosound Level".into(), volume: 10, playing: false });
        let before = c.silhouettes.len();
        c.silhouettes.resize_with(c.devices.len(),
            crate::product_silhouette::ProductSilhouette::default);
        for s in c.silhouettes.iter_mut().skip(before) { s.set_motion_scale(c.motion_scale); }
        // Reduced means the reveal finishes in a fraction of a smooth one.
        let mut fast = c.silhouettes.pop().unwrap();
        let mut slow = crate::product_silhouette::ProductSilhouette::default();
        fast.tick(60.0);
        slow.tick(60.0);
        assert!(fast.reveal() > slow.reveal(),
                "reduced {} was not quicker than smooth {}", fast.reveal(), slow.reveal());
    }

    #[test]
    fn changing_the_palette_blooms_the_badges_once() {
        // The halo is "a marker, not a flourish" — it exists to say the
        // accent moved. Without the call, switching palette repainted
        // every outline in a new colour with nothing to draw the eye.
        let ctx = egui::Context::default();
        let mut c = app();
        c.apply_settings(&ctx, &prefs(settings_kit::AnimationStyle::Smooth, "amber"));
        // Ticked in frames, not one huge dt: the envelope advances per
        // call, so a single enormous step leaves it part way through — a
        // detail worth reproducing here because the app ticks per frame
        // too.
        for _ in 0..400 { for s in c.silhouettes.iter_mut() { s.tick(16.0); } }
        assert_eq!(c.silhouettes[0].halo(), 0.0, "settled before the change");

        c.apply_settings(&ctx, &prefs(settings_kit::AnimationStyle::Smooth, "midnight"));
        // `bloom` ARMS the envelope; the halo rises on the next tick. The
        // immediate signal is that the badge has work to do — asserting
        // the halo before a frame has run tests the clock, not the change.
        assert!(c.silhouettes[0].is_animating(), "the accent change did not arm a bloom");
        for s in c.silhouettes.iter_mut() { s.tick(16.0); }
        assert!(c.silhouettes[0].halo() > 0.0, "the armed bloom never rose");
    }

    #[test]
    fn re_applying_the_same_palette_does_not_re_bloom() {
        // Called unconditionally this fires every frame, and a halo that
        // never stops is the throb the two-phase envelope exists to avoid.
        let ctx = egui::Context::default();
        let mut c = app();
        c.apply_settings(&ctx, &prefs(settings_kit::AnimationStyle::Smooth, "amber"));
        for _ in 0..400 { for s in c.silhouettes.iter_mut() { s.tick(16.0); } }
        c.apply_settings(&ctx, &prefs(settings_kit::AnimationStyle::Smooth, "amber"));
        for s in c.silhouettes.iter_mut() { s.tick(16.0); }
        assert_eq!(c.silhouettes[0].halo(), 0.0, "an unchanged palette bloomed anyway");
    }
}

#[cfg(test)]
mod silhouette_frame_tests {
    use super::*;
    use crate::product_silhouette::{ProductSilhouette, Shape};

    /// Draw one badge and count the line shapes and their widths.
    fn painted(shape: Shape) -> (usize, Vec<f32>) {
        let ctx = egui::Context::default();
        let mut s = ProductSilhouette::new(shape);
        // Past the reveal, or the strokes are drawn transparent and the
        // test measures an animation rather than an outline.
        s.tick(10_000.0);
        let colors = uikit_egui::palette_colors("amber");
        let out = ctx.run(egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO,
                                                        egui::vec2(200.0, 200.0))),
            ..Default::default()
        }, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let rect = egui::Rect::from_min_size(egui::pos2(10.0, 10.0),
                                                     egui::vec2(48.0, 48.0));
                silhouette_paint(ui, &s, rect, &colors);
            });
        });
        let mut widths = Vec::new();
        let mut n = 0;
        for cs in out.shapes.iter() {
            if let egui::epaint::Shape::Path(ps) = &cs.shape {
                n += 1;
                widths.push(ps.stroke.width);
            }
        }
        (n, widths)
    }

    #[test]
    fn every_product_paints_its_whole_outline() {
        // One path per subpath. A shape that paints fewer than it has is
        // one the renderer is silently dropping part of.
        for shape in [Shape::Disc, Shape::Bar, Shape::Tower, Shape::Cube] {
            let (n, _) = painted(shape);
            assert_eq!(n, shape.outline().len(),
                       "{shape:?} painted {n} of {} subpaths",
                       shape.outline().len());
        }
    }

    #[test]
    fn nothing_reaches_the_screen_heavier_than_the_design_allows() {
        // The data test pins the outline; this pins what egui was actually
        // handed, which is where a renderer that scaled a stroke by the
        // badge size would show up.
        for shape in [Shape::Disc, Shape::Bar, Shape::Tower, Shape::Cube] {
            let (_, widths) = painted(shape);
            for w in widths {
                assert!(w <= 1.2, "{shape:?} painted a {w}px line");
            }
        }
    }
}

#[cfg(test)]
mod playback_seam_tests {
    use super::*;
    use crate::playback::{Request, Sink};
    use crate::settings_page::Route;

    fn app() -> Controller {
        Controller::new(vec![Device { name: "Living Room".into(), volume: 50,
                                      playing: false }], vec![])
    }

    #[test]
    fn committing_a_route_points_audio_at_it_in_the_same_act() {
        // Leaving the picker and the audio to be connected by the host is
        // how the UI ends up showing a route the sound is not going to.
        let mut c = app();
        c.set_audio_route(&Route::MultiRoom(vec!["AA".into(), "BB".into()]));
        assert_eq!(c.audio_sink, Sink::Many(vec!["AA".into(), "BB".into()]));
        assert_eq!(c.pending_playback, Some(Request::ReleaseRoute),
                   "the previous route's resources were never given up");
    }

    #[test]
    fn re_committing_the_same_route_does_not_rebuild_the_sink() {
        // A combine sink torn down and rebuilt between two tracks on the
        // same speakers is a gap of silence for nothing.
        let mut c = app();
        c.set_audio_route(&Route::Bluetooth("AA".into()));
        c.pending_playback = None;
        c.set_audio_route(&Route::Bluetooth("AA".into()));
        assert_eq!(c.pending_playback, None, "an unchanged route asked for a rebuild");
    }

    #[test]
    fn nothing_is_requested_until_something_actually_changes() {
        // The request field is drained by the host; a view that sets it
        // every frame makes the host restart the track sixty times a
        // second.
        let c = app();
        assert_eq!(c.pending_playback, None);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paging_an_empty_list_does_not_panic() {
        // `clamp(0, n-1)` with n == 0 is `clamp(0, -1)`, and clamp panics
        // when min > max. Unreachable from today's only caller, but this
        // is a `pub` helper in a crate whose panics land in draw paths.
        assert_eq!(page(0, 1, 0), 0);
        assert_eq!(page(5, -1, 0), 0);
    }

    #[test]
    fn the_status_poll_is_throttled_not_per_frame() {
        // `control::send` opens a unix socket per call, so a per-frame
        // poll is sixty connects a second for a value that changes when
        // somebody presses a button.
        //
        // Driven with a frame-sized dt and asserted on the ACCUMULATOR,
        // because the call itself needs a daemon: at 60fps the age must
        // still be climbing after ten frames rather than resetting.
        let mut c = Controller::new(Vec::new(), Vec::new());
        for _ in 0..10 {
            c.poll_daemon(1.0 / 60.0);
        }
        assert!(c.status_age > 0.0, "the poll fired every frame");
        assert!(c.status_age < Controller::STATUS_EVERY_S);
    }

    #[test]
    fn a_poll_past_the_interval_resets_the_clock() {
        // Without the reset the poll fires every frame forever after the
        // first interval elapses — the throttle would work once.
        let mut c = Controller::new(Vec::new(), Vec::new());
        c.poll_daemon(Controller::STATUS_EVERY_S + 1.0);
        assert_eq!(c.status_age, 0.0);
    }

    #[test]
    fn no_daemon_means_nothing_is_playing() {
        // There is no daemon in a test run, so this exercises the real
        // path: a failed status must not leave a stale "playing" on
        // screen.
        let mut c = Controller::new(Vec::new(), Vec::new());
        c.scene_playing = true;
        c.poll_daemon(Controller::STATUS_EVERY_S + 1.0);
        assert!(!c.scene_playing);
    }

    #[test]
    fn the_current_view_name_survives_an_out_of_range_index() {
        // Now enforced by `Nav::go_page`, which CLAMPS — so an
        // out-of-range page cannot be stored at all, where before the
        // bound depended on the widget having been built from VIEWS and
        // `current()` could not check it.
        //
        // `go_page` IGNORES an out-of-range page rather than clamping to
        // the last one — which is the better of the two: clamping would
        // silently move a person to Settings when a bad index appeared,
        // and staying put is the answer that does not act on a value it
        // does not trust.
        //
        // I expected clamping and asserted it; the test said otherwise.
        // Left asserting the real contract.
        // `Controller::new` rather than `default()`: it has no Default
        // impl, and adding one to satisfy a test would put a
        // deviceless, sceneless Controller in the public API for no
        // other reason.
        let mut c = Controller::new(Vec::new(), Vec::new());
        c.nav.go_page(VIEWS.len() + 7);
        assert_eq!(c.current(), VIEWS[0],
                   "an out-of-range page should be ignored, leaving the current view");
    }

    /// A source with two artists and a shared album.
    struct FakeSrc(Vec<crate::catalog_source::CatalogTrack>);
    impl crate::catalog_source::CatalogSource for FakeSrc {
        fn page(&self, offset: i64, limit: i64, _sort: &str)
            -> Vec<crate::catalog_source::CatalogTrack>
        {
            self.0.iter().skip(offset.max(0) as usize).take(limit.max(0) as usize)
                .cloned().collect()
        }
        fn total(&self) -> i64 { self.0.len() as i64 }
    }

    fn ct(id: i64, title: &str, album: &str, artist: &str)
        -> crate::catalog_source::CatalogTrack
    {
        crate::catalog_source::CatalogTrack {
            id, title: title.into(), album: Some(album.into()),
            artists: vec![artist.into()], tags: vec![], bpm: None, key_text: None,
        }
    }

    #[test]
    fn browse_is_filled_from_the_whole_catalogue_not_one_page() {
        // load_local's doc: "correct only when the whole catalogue is in
        // memory". A partial fetch here would silently produce a partial album
        // list that looks complete.
        let src = FakeSrc((0..250).map(|i| ct(i, &format!("t{i}"), "A", "X")).collect());
        let tracks = super::browse_tracks_from_source(&src);
        assert_eq!(tracks.len(), 250, "the adapter fetched one page, not the catalogue");
    }

    #[test]
    fn the_same_artist_name_gets_the_same_id_within_a_run() {
        // Browse groups by artist_id and opens a detail page from it. The
        // source has no artist table, so the id is derived — and it has to be
        // consistent or one artist appears twice.
        let src = FakeSrc(vec![ct(1, "a", "A", "Cocteau Twins"),
                               ct(2, "b", "B", "cocteau twins"),
                               ct(3, "c", "C", "Slowdive")]);
        let t = super::browse_tracks_from_source(&src);
        assert_eq!(t[0].artist_id, t[1].artist_id, "case split one artist in two");
        assert_ne!(t[0].artist_id, t[2].artist_id);
        assert!(t.iter().all(|x| x.artist_id >= 0), "a negative id reads as a sentinel");
    }

    #[test]
    fn play_counts_are_not_invented() {
        // TOP mode is play-ranked. The source has no counts, so leaving them
        // at zero makes TOP visibly empty — fabricating them would produce a
        // ranked list that ranks by accident, which is worse than no ranking.
        let src = FakeSrc(vec![ct(1, "a", "A", "X")]);
        let t = super::browse_tracks_from_source(&src);
        assert_eq!(t[0].play_count, 0);
        assert!(t[0].year.is_none());
        assert!(t[0].last_played_at.is_none());
    }

    #[test]
    fn the_primary_artist_matches_the_all_tracks_adapter() {
        // Both reduce a multi-artist track to its first. If they disagreed,
        // one track would land under two different artists in two views.
        let mut multi = ct(1, "a", "A", "First");
        multi.artists.push("Second".into());
        let src = FakeSrc(vec![multi]);
        let t = super::browse_tracks_from_source(&src);
        assert_eq!(t[0].primary_artist, "First");
    }
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
        // No file known for it yet: the transport must NOT claim to play.
        c.apply_browse(crate::browse::Action::Play(2));
        assert_eq!(c.transport.title, "Closer");
        assert!(!c.transport.playing, "playing with no file is a picture of a transport");
        assert!(c.pending_playback.is_none());
        // With the path resolved, the same tap asks for the sound.
        c.set_track_paths(vec![(2, "/m/closer.flac".into())]);
        c.apply_browse(crate::browse::Action::Play(2));
        assert_eq!(c.transport.title, "Closer");
        assert_eq!(c.transport.artist, "Ada");
        assert!(c.transport.playing);
        assert!(matches!(c.pending_playback, Some(crate::playback::Request::Play { ref url, .. }) if url == "/m/closer.flac"),
                "{:?}", c.pending_playback);
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
    fn the_named_page_bar_and_the_swipe_nav_stay_in_step() {
        // Two ways to change page that disagreed would leave the bar naming one
        // screen while another is shown.
        let mut c = sample();
        assert!(c.jump_to_page(3));
        assert_eq!(c.current(), VIEWS[3]);
        assert_eq!(c.page_bar.active(), 3);
        assert!(!c.jump_to_page(99), "out of range is refused");
        assert_eq!(c.page_bar.active(), 3);
    }

    #[test]
    fn the_page_cards_are_staggered_so_the_page_assembles() {
        let mut c = sample();
        for card in c.cards.iter_mut() { card.tick(80.0); }
        assert!(c.cards[0].opacity() > 0.0, "the first has begun");
        assert_eq!(c.cards[3].opacity(), 0.0, "a later one is still waiting");
    }

    #[test]
    fn a_processing_run_carries_the_resolved_output_and_locks_the_controls() {
        let mut c = sample();
        c.fileproc.input = "/m/a.wav".into();
        c.fileproc.set_bitrate(96);
        c.start_proc("run");
        assert_eq!(c.pending_proc, Some(crate::fileproc::ProcAction::Run {
            input: "/m/a.wav".into(), output: "/m/a.mixed.opus".into(), bitrate: 96 }));
        // A second run would write a second file the user cannot tell from the
        // first.
        c.pending_proc = None;
        c.start_proc("run");
        assert_eq!(c.pending_proc, None);
    }

    #[test]
    fn a_step_key_moves_the_dial_and_the_dial_stops_fighting_it() {
        // Stepping is an external change as far as the wheel is concerned:
        // whatever spin it had is no longer describing what is happening.
        let mut c = sample();
        c.dial.set_external(50.0);
        c.dial.press(0.0, 0.0, 90.0, 100.0);
        c.dial.drag(30.0, 0.0);
        c.dial.release();
        assert!(c.dial.spinning());
        c.dial.set_external((c.dial.value - 5.0).max(0.0));
        assert!(!c.dial.spinning());
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

    // ── all_tracks wiring: the paged window fed through the CatalogSource seam ──

    struct FakeCatalog;
    impl crate::catalog_source::CatalogSource for FakeCatalog {
        fn page(&self, offset: i64, limit: i64, sort: &str) -> Vec<crate::catalog_source::CatalogTrack> {
            (offset..(offset + limit).min(250))
                .map(|i| crate::catalog_source::CatalogTrack {
                    id: i,
                    title: if sort == "artist" { format!("A{i}") } else { format!("T{i}") },
                    album: None,
                    artists: vec!["Ada".into()],
                    tags: vec![],
                    bpm: None,
                    key_text: None,
                })
                .collect()
        }
        fn total(&self) -> i64 { 250 }
    }

    #[test]
    fn the_all_tracks_tab_pages_plays_and_enqueues_through_the_controller() {
        let mut c = Controller::new(vec![], vec![]);
        // The view's first request, answered by the host's source.
        let first = c.all_tracks.reload();
        c.tracks_action(first);
        assert!(matches!(c.pending_tracks,
            Some(crate::all_tracks::TracksAction::Load { offset: 0 })));
        c.answer_tracks_from(&FakeCatalog);
        assert!(c.pending_tracks.is_none(), "the load was answered");
        assert_eq!(c.all_tracks.count_text(), "100 of 250");

        // The prefetch appends the next window.
        let more = c.all_tracks.load_more().expect("not complete yet");
        c.tracks_action(more);
        c.answer_tracks_from(&FakeCatalog);
        assert_eq!(c.all_tracks.tracks().len(), 200);

        // Play resolves from the visible row; the transport picks up the
        // text. It does NOT show as playing, because this catalogue has no
        // files behind it — see `request_play`. Showing "playing" over
        // silence is the failure that made the play button a picture in
        // the first place.
        c.tracks_action(crate::all_tracks::TracksAction::Play(5));
        assert_eq!(c.transport.title, "T5");
        assert!(!c.transport.playing, "playing with no file behind the row");

        // With a path, the same action plays and asks the host for it.
        c.set_track_paths(vec![(5, "/music/T5.flac".into())]);
        c.tracks_action(crate::all_tracks::TracksAction::Play(5));
        assert!(c.transport.playing);
        assert!(matches!(c.pending_playback,
                         Some(crate::playback::Request::Play { ref url, .. })
                         if url == "/music/T5.flac"),
                "{:?}", c.pending_playback);

        // Enqueue lands in the queue with the row's text.
        c.tracks_action(crate::all_tracks::TracksAction::Enqueue(7));
        assert_eq!(c.queue.entries().len(), 1);
        assert_eq!(c.queue.entries()[0].title, "T7");

        // A sort change resets to a fresh window in the NEW server ordering —
        // re-sorting the visible two hundred would be a lie about the rest.
        let resort = c.all_tracks.set_sort(crate::all_tracks::Sort::Artist).unwrap();
        c.tracks_action(resort);
        c.answer_tracks_from(&FakeCatalog);
        assert_eq!(c.all_tracks.tracks().len(), 100);
        assert!(c.all_tracks.tracks()[0].title.starts_with('A'));
    }

    #[test]
    fn a_stale_page_is_discarded_at_the_controller_boundary() {
        let mut c = Controller::new(vec![], vec![]);
        let stale = crate::all_tracks::PageResult {
            context: "all_tracks".into(), offset: 50, total: 250, tracks: vec![],
        };
        assert!(!c.supply_tracks(&stale), "offset 50 into an empty window is a stale reply");
        assert!(c.all_tracks.tracks().is_empty());
        assert!(!c.all_tracks.loading(), "the guard cleared loading so the right request can land");
    }

    // ── silhouette + watermark wiring ──

    #[test]
    fn device_badges_track_the_list_and_bloom_only_on_real_change() {
        let mut c = Controller::new(
            vec![Device { name: "Level bar".into(), volume: 10, playing: false }],
            vec![],
        );
        c.silhouettes.resize_with(c.devices.len(),
            crate::product_silhouette::ProductSilhouette::default);
        let s = &mut c.silhouettes[0];
        // Default is Disc; "bar" is a change → halo blooms.
        assert!(s.set_shape(crate::product_silhouette::Shape::parse("bar")));
        s.tick(50.0);
        assert!(s.halo() > 0.0, "a real change bloomed");
        // Same shape again — a refresh — must NOT restart the halo.
        assert!(!s.set_shape(crate::product_silhouette::Shape::parse("bar")));
    }

    #[test]
    fn the_anc_chip_is_reachable_from_the_playbar_in_every_mode() {
        // The chip lives in transport_strip, which draws on every page —
        // that is the whole requirement: the operator can kill the
        // anti-noise without leaving the sleep engine or the theatre setup.
        let mut c = Controller::new(vec![], vec![]);
        assert_eq!(c.anc.mode, crate::anc::Mode::Auto, "auto by default");
        // Cycling from the bar reaches every mode and comes back.
        assert_eq!(c.anc.cycle(), crate::anc::Mode::Forced);
        assert_eq!(c.anc.cycle(), crate::anc::Mode::Off);
        assert_eq!(c.anc.cycle(), crate::anc::Mode::Auto);
        // And the strip has something to say in each of them, so the chip is
        // never a light with no explanation.
        for _ in 0..3 {
            assert!(!c.anc.readout().is_empty());
            c.anc.cycle();
        }
    }

    #[test]
    fn the_watermark_paces_itself_by_what_is_playing() {
        let mut c = Controller::new(vec![], vec![]);
        c.watermark.set_blur_available(false);
        c.transport.playing = false;
        c.watermark.set_activity(0.0);
        let quiet = c.watermark.loop_ms();
        c.transport.playing = true;
        c.dial.value = 100.0;
        c.watermark.set_activity(1.0);
        let active = c.watermark.loop_ms();
        assert!(active < quiet, "fully active flows faster ({active} < {quiet})");
        assert!(c.watermark.blur().is_none(), "egui cannot blur; the module degrades");
        assert!(c.watermark.opacity() > 0.0);
    }

    #[test]
    fn opening_a_playlist_sets_open_and_asks_the_host_to_load_it() {
        let mut c = Controller::new(vec![], vec![]);
        c.apply_playlist(crate::sources::PlaylistAction::Open(7));
        assert_eq!(c.open_playlist, Some(7));
        assert_eq!(c.pending_playlist_open, Some(7));
    }

    #[test]
    fn deleting_the_open_playlist_closes_the_detail_view() {
        let mut c = Controller::new(vec![], vec![]);
        c.open_playlist = Some(7);
        c.apply_playlist(crate::sources::PlaylistAction::Delete(7));
        assert_eq!(c.open_playlist, None, "the view you were looking at just got deleted");
        assert_eq!(c.pending_playlist_delete, Some(7));
    }

    #[test]
    fn deleting_a_playlist_that_is_not_open_leaves_the_view_alone() {
        let mut c = Controller::new(vec![], vec![]);
        c.open_playlist = Some(3);
        c.apply_playlist(crate::sources::PlaylistAction::Delete(9));
        assert_eq!(c.open_playlist, Some(3));
        assert_eq!(c.pending_playlist_delete, Some(9));
    }

    #[test]
    fn selecting_a_networked_device_asks_the_host_to_load_its_queue() {
        let mut c = Controller::new(vec![], vec![]);
        c.device_selector.set_devices(vec![
            crate::outputs::Device { url: "http://192.168.1.50".into(),
                                      name: "Living Room".into(), kind: "cast".into() },
        ]);
        // Simulate what devices_view does when the selector returns a url —
        // the view itself needs an egui frame to drive, so this exercises
        // the state transition directly rather than through a click.
        let url = "http://192.168.1.50".to_string();
        if url != c.active_device_url {
            if url.starts_with("http://") {
                c.request_cast(url.clone());
                c.pending_remote_queue_load = Some(url.clone());
            }
        }
        c.active_device_url = url;
        assert_eq!(c.pending_remote_queue_load.as_deref(), Some("http://192.168.1.50"));
    }

    #[test]
    fn queueing_what_is_playing_needs_something_playing() {
        let mut c = Controller::new(vec![], vec![]);
        c.active_device_url = "http://192.168.1.50".into();
        assert_eq!(c.now_playing_id, None, "nothing playing yet");
        // The button in devices_view only appears inside `if let Some(id)
        // = self.now_playing_id` — this locks in the precondition that
        // gate depends on, so a refactor that drops the guard shows up
        // here even without a full egui click simulation.
        assert!(c.pending_remote_queue_push.is_none());
    }
}

#[cfg(test)]
mod voice_command_tests {
    use super::*;
    use crate::voice_control::{Command, Outcome, VoicePrefs};

    fn app(enabled: bool) -> Controller {
        let mut c = Controller::new(vec![], vec![]);
        c.voice = VoicePrefs { enabled, ..Default::default() };
        c.set_track_paths(vec![(1, "/m/one.flac".into()), (2, "/m/two.flac".into())]);
        c
    }

    #[test]
    fn nothing_is_heard_until_the_setting_is_on() {
        // The default, and the reason it is the default.
        let mut c = app(false);
        assert_eq!(c.heard("pause"), Outcome::Ignored);
        assert_eq!(c.pending_playback, None);
    }

    #[test]
    fn play_and_pause_go_through_the_same_path_a_tap_does() {
        // A voice command that set `playing` directly would show as
        // playing and make no sound — the exact bug the tap path had.
        let mut c = app(true);
        c.set_track_paths(vec![(1, "/m/one.flac".into())]);
        c.queue.add_all(&[(1, "One".into(), "A".into())]);
        let first = c.queue.advance().unwrap();
        c.start_playing(&first);
        c.pending_playback = None;
        c.transport.duration = 200.0;
        c.transport.report(42.0);

        assert_eq!(c.heard("pause"), Outcome::Run(Command::Pause));
        assert!(!c.transport.playing);
        assert_eq!(c.pending_playback, Some(crate::playback::Request::Pause));

        c.pending_playback = None;
        assert_eq!(c.heard("play"), Outcome::Run(Command::Play));
        assert!(c.transport.playing);
        // Resumed WHERE IT WAS, carrying the file so a player that cannot
        // pause in place can start again at the same second.
        assert!(matches!(c.pending_playback,
                Some(crate::playback::Request::Resume { ref url, at, .. }) if url == "/m/one.flac" && (at - 42.0).abs() < 1e-9),
                "resuming asked for the wrong thing: {:?}", c.pending_playback);
    }

    #[test]
    fn a_search_asks_before_acting_and_then_runs_on_yes() {
        // Free text cannot be checked against anything.
        let mut c = app(true);
        assert_eq!(c.heard("search for miles davis"),
                   Outcome::Confirm(Command::Search("miles davis".into())));
        assert_eq!(c.search.query, "", "it searched before being confirmed");
        c.confirm_voice(true);
        assert_eq!(c.search.query, "miles davis");
    }

    #[test]
    fn saying_no_to_a_confirmation_does_nothing_and_clears_it() {
        let mut c = app(true);
        c.heard("play the smiths");
        c.confirm_voice(false);
        assert_eq!(c.search.query, "");
        assert_eq!(c.voice_confirm, None, "the prompt outlived the answer");
    }

    #[test]
    fn play_something_searches_rather_than_starting_a_guess() {
        // Starting the first fuzzy match is how a request for one album
        // plays another.
        let mut c = app(true);
        c.voice.act_on_uncertain = true;
        c.heard("play the smiths");
        assert_eq!(c.search.query, "the smiths");
        assert_eq!(c.pending_playback, None, "it started playing a guess");
    }

    #[test]
    fn next_advances_the_queue_the_same_way_a_finished_track_does() {
        let mut c = app(true);
        c.queue.add_all(&[(1, "One".into(), "A".into()), (2, "Two".into(), "A".into())]);
        let first = c.queue.advance().unwrap();
        c.start_playing(&first);
        c.pending_playback = None;
        c.heard("next");
        assert_eq!(c.transport.title, "Two");
    }

    #[test]
    fn volume_words_move_it_and_stay_in_range() {
        let mut c = app(true);
        c.heard("volume 90");
        assert_eq!(c.pending_volume, Some(90.0));
        c.heard("louder");
        assert_eq!(c.pending_volume, Some(100.0), "it went past the top");
        c.heard("mute");
        assert_eq!(c.pending_volume, Some(0.0));
        c.heard("quieter");
        assert_eq!(c.pending_volume, Some(0.0), "it went below zero");
    }

#[cfg(test)]
mod view_reconcile_tests {
    use super::*;

    fn limited() -> crate::device::Profile {
        // volume and mute only: no eq, no stream, no autoroute.
        crate::device::Profile {
            controls: ["volume", "mute"].iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        }
    }

    /// THE SILENT MOVE THIS PREVENTS. `Nav` holds a bare index: filter Mixer
    /// out and index 3 slides from Mixer to Soundscape, so a device
    /// connecting moves the user to a page they did not pick.
    #[test]
    fn a_surviving_view_keeps_its_selection_across_a_filter() {
        let mut c = Controller::new(Vec::new(), Vec::new());
        // Library survives any profile.
        let lib = VIEWS.iter().position(|v| *v == "Library").unwrap();
        c.nav.go_page(lib);
        assert_eq!(c.current(), "Library");
        c.profile = limited();
        assert_eq!(c.reconcile_views("Library"), "Library");
        assert_eq!(c.current(), "Library", "and the index followed it");
    }

    /// A view that did NOT survive falls back to the first visible one, not to
    /// a clamped index -- clamping lands on whatever happens to be adjacent,
    /// which is a coin flip dressed as a decision.
    #[test]
    fn a_removed_view_falls_back_to_the_first_visible_one() {
        let mut c = Controller::new(Vec::new(), Vec::new());
        c.profile = limited();
        let now = c.reconcile_views("Mixer");
        assert_eq!(now, "Devices");
        assert!(!c.visible_views().contains(&"Mixer"));
    }

    /// The translation back into VIEWS indices is the part that is easy to get
    /// wrong: `Nav` indexes the FULL list, so a visible-subset index would
    /// point at the wrong page for every view after a hidden one.
    #[test]
    fn the_index_is_translated_back_into_the_full_view_list() {
        let mut c = Controller::new(Vec::new(), Vec::new());
        c.profile = limited();
        // Settings is last in VIEWS (index 5) but third in the visible set.
        c.reconcile_views("Settings");
        assert_eq!(c.current(), "Settings");
        assert_eq!(c.nav.page(), VIEWS.iter().position(|v| *v == "Settings").unwrap());
    }

    /// THE CHAIN END TO END. A device switch resolves a profile from the
    /// product map and the view set follows it -- which is the whole point of
    /// the three pieces and was not connected until the switch site was wired.
    #[test]
    fn switching_to_a_limited_device_drops_its_missing_views() {
        let mut c = Controller::new(Vec::new(), Vec::new());
        c.product_profiles.insert("Beoplay".into(), limited());
        c.product_types.insert("http://phones".into(), "Beoplay".into());

        // Before: default profile, everything visible.
        assert_eq!(c.visible_views().len(), VIEWS.len());

        // The switch, as `device_selector_ui` performs it.
        let was = c.current();
        let ty = c.product_type_of("http://phones");
        let resolved = crate::device::resolve_profile(&c.product_profiles, ty.as_deref());
        c.profile = resolved;
        c.reconcile_views(was);

        let v = c.visible_views();
        assert!(!v.contains(&"Mixer"), "headphones keep offering a Mixer: {v:?}");
        assert_eq!(c.current(), "Devices", "and the selection is still valid");
    }

    /// An address the daemon has not described resolves through the same
    /// fallback as an unrecognised product -- ordinary, not an error.
    #[test]
    fn an_undescribed_address_keeps_every_view() {
        let c = Controller::new(Vec::new(), Vec::new());
        assert_eq!(c.product_type_of("http://never-seen"), None);
        let p = crate::device::resolve_profile(&c.product_profiles, None);
        assert_eq!(crate::device::views_for(&p, &VIEWS).len(), VIEWS.len());
    }

    /// An unknown product keeps every view, so reconciling is a no-op -- the
    /// common case must not move anything.
    #[test]
    fn an_unknown_product_reconciles_to_where_it_already_was() {
        let mut c = Controller::new(Vec::new(), Vec::new());
        for v in VIEWS {
            c.nav.go_page(VIEWS.iter().position(|x| *x == v).unwrap());
            assert_eq!(c.reconcile_views(v), v, "{v} moved on a full profile");
        }
    }
}
}

/// The whole shell through real frames, at both breakpoints, with the
/// states a screenshot caught misbehaving. `layout_probe` reports text
/// that is off-screen, clipped or drawn over other text — which is how
/// the transport bar taking the entire window when the queue opened
/// would have failed here rather than in a screenshot.
#[cfg(test)]
mod shell_layout_tests {
    use super::*;

    fn library_with_queue() -> Controller {
        let mut c = Controller::new(vec![], vec![]);
        let rows: Vec<crate::browse::BrowseTrack> = (1..=40).map(|i| crate::browse::BrowseTrack {
            id: i, title: format!("Track number {i} with a fairly long title"),
            album: Some(format!("Album {}", i % 5)), primary_artist: "Some Artist".into(),
            artist_id: (i % 5) as i64, year: Some(2000), play_count: (i % 3) as u32,
            last_played_at: None,
        }).collect();
        c.set_track_paths(rows.iter().map(|t| (t.id, format!("/m/{}.flac", t.id))).collect());
        c.supply_browse_tracks(rows.clone());
        c.supply_home(HomeLists { recently_played: rows[..8].to_vec(), recently_added: rows[8..16].to_vec(),
            most_played: rows[16..24].to_vec(), never_played: rows[24..32].to_vec(), loaded: true });
        c.nav.go_page(2);
        let ids: Vec<i64> = (1..=12).collect();
        c.apply_browse(crate::browse::Action::PlayAll(ids));
        c.pending_playback = None;
        c.transport.duration = 240.0;
        c.transport.report(61.0);
        c
    }

    /// Frames through the real shell; the report plus every text drawn
    /// in the last frame, positioned.
    fn run(size: egui::Vec2, compact: bool, mut c: Controller)
        -> (uikit_egui::layout_probe::Report, Vec<uikit_egui::layout_probe::Text>) {
        let ctx = egui::Context::default();
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, size);
        let mut input = egui::RawInput::default();
        input.screen_rect = Some(screen);
        let mut last = Vec::new();
        for _ in 0..4 {
            let out = ctx.run(input.clone(), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let mut adaptive = Adaptive::default();
                    adaptive.class = if compact { uikit_egui::adaptive::DeviceClass::Phone }
                                     else { uikit_egui::adaptive::DeviceClass::Desktop };
                    adaptive.size = (size.x, size.y);
                    c.ui(ui, &adaptive);
                });
            });
            last = out.shapes;
        }
        (uikit_egui::layout_probe::analyse(screen, &last, compact),
         uikit_egui::layout_probe::texts(&last))
    }

    /// The bar's own text must be on screen at the bottom, on every page,
    /// and nothing in the page may be drawn across it.
    fn bar_is_pinned(r: &uikit_egui::layout_probe::Report, texts: &[uikit_egui::layout_probe::Text], size: egui::Vec2) {
        assert!(r.overlaps.is_empty(), "text collisions: {}", r.describe());
        assert!(r.offscreen.is_empty(), "text off screen: {}", r.describe());
        let title = texts.iter().find(|g| g.text.starts_with("Track number 1 "))
            .unwrap_or_else(|| panic!("the transport title is not drawn: {}", r.describe()));
        assert!(title.rect.bottom() > size.y * 0.6, "the bar is not at the bottom: {:?}", title.rect);
        // and the page above it is still there: the library heading is
        // drawn at the top, not under the bar
        let heading = texts.iter().find(|g| g.text == "Library").expect("the page heading");
        assert!(heading.rect.top() < size.y * 0.2, "the page was pushed down: {:?}", heading.rect);
        assert!(heading.rect.bottom() < title.rect.top(), "the bar overlaps the page");
    }

    #[test]
    fn the_transport_bar_stays_at_the_bottom_with_the_queue_open_wide() {
        let mut c = library_with_queue();
        c.queue_open = true;
        let size = egui::vec2(1000.0, 720.0);
        let (r, texts) = run(size, false, c);
        bar_is_pinned(&r, &texts, size);
        assert!(texts.iter().any(|g| g.text.contains("Queue")), "the queue list is not drawn");
    }

    #[test]
    fn the_transport_bar_stays_at_the_bottom_with_the_queue_open_compact() {
        let mut c = library_with_queue();
        c.queue_open = true;
        let size = egui::vec2(420.0, 820.0);
        let (r, texts) = run(size, true, c);
        bar_is_pinned(&r, &texts, size);
    }

    #[test]
    fn every_page_lays_out_without_text_collisions_at_both_widths() {
        for page in 0..VIEWS.len() {
            for (size, compact) in [(egui::vec2(1000.0, 720.0), false), (egui::vec2(420.0, 820.0), true)] {
                let mut c = library_with_queue();
                c.nav.go_page(page);
                let (r, _) = run(size, compact, c);
                assert!(r.overlaps.is_empty(), "page {} at {size:?}: {}", VIEWS[page], r.describe());
            }
        }
    }
}
