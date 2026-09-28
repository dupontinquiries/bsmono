//! bo_suite_egui.rs — the deployable egui bo-suite app (built behind the `app` feature so a plain
//! `cargo build`/`cargo test` on this crate stays eframe-free). Defaults to the Amber palette (warm
//! gold) matching bo-controller's Qt/QML surface, instead of the shared default.
//!
//! Run:  cargo run --bin bo-suite-egui --features app   (from apps/bo-suite/egui-frontend)

use eframe::egui;
use bo_suite_egui::catalog_source::CatalogSource;
use bo_suite_egui::{Controller, Device, Scene};
use uikit_egui::Adaptive;

struct App {
    controller: Controller,
    /// The live backdrop behind the glass panels — snapclone's finish, through the kit.
    backdrop: uikit_egui::Backdrop,
    /// The audio host. The controller returns REQUESTS; this performs them,
    /// which is what keeps the view constructible and testable with no
    /// process table — see `playback::Request`.
    player: bo_suite_egui::playback::Player,
    /// Said once per failure, not per frame.
    last_note: String,
    /// The real library: bo-controller's SQLite catalog, read directly
    /// (WAL-safe, see real_catalog.rs) with writes routed to the worker
    /// thread spawned below.
    catalog: bo_suite_egui::real_catalog::RealCatalog,
    /// Confirmations and progress from the media worker (rating echoes,
    /// scan progress, errors). Read non-blocking each frame in
    /// `pump_media_results`.
    media_res_rx: std::sync::mpsc::Receiver<bo_media::MediaResult>,
    /// Casting to a networked B&O speaker — see cast.rs. Commands go out
    /// on `cast_cmd_tx`; results come back non-blocking on `cast_res_rx`.
    cast_cmd_tx: std::sync::mpsc::Sender<bo_suite_egui::cast::CastCmd>,
    cast_res_rx: std::sync::mpsc::Receiver<bo_suite_egui::cast::CastResult>,
    /// SSDP discovery results, refreshed periodically by a background
    /// thread and read non-blocking each frame — see cast.rs.
    discovery_rx: std::sync::mpsc::Receiver<Vec<bo_suite_egui::outputs::Device>>,
    /// The recogniser, when one is configured and voice is on.
    voice: bo_suite_egui::voice_source::VoiceSource,
    /// What the recogniser was last started with, so a settings change
    /// restarts it and an unchanged one does not — respawning a process
    /// every frame is the shape of bug that empties a battery.
    voice_cmd: String,
    /// Where mpv serves its IPC socket for this process. Per-pid so two
    /// instances do not steer each other's player.
    ipc_path: String,
    /// The URL the player was last started with and the sink, so a
    /// resume can tell "same track, still alive" from "start again".
    playing_url: String,
    /// When the session file was last written; it is written on change
    /// but no more than once a second — the position moves every frame.
    session_saved: std::time::Instant,
    session_text: String,
    /// A restored session names a track to load paused; resolved once
    /// the catalogue answers.
    restore_track: Option<(i64, f64)>,
    /// The track whose play is recorded once the player says it started.
    record_on_start: Option<i64>,
    /// When to re-read the Home lists after a play was recorded.
    home_refresh_at: Option<std::time::Instant>,
}

/// The session file: queue, modes, what was loaded. Under the same app
/// name as the room and the voice prefs.
const SESSION_APP: &str = "bo-suite";
const SESSION_FILE: &str = "session.conf";

/// The device name a local play is recorded under in the history table.
const LOCAL_DEVICE: &str = "local";

/// Fill `track_paths` for every row currently loaded into the all-tracks
/// page or the browse grids — the set of ids `request_play` can actually
/// resolve. Called after any fetch from `RealCatalog`, since `CatalogSource`
/// itself doesn't carry paths (see `real_catalog.rs`'s note on why).
fn sync_track_paths(controller: &mut bo_suite_egui::Controller, catalog: &bo_suite_egui::real_catalog::RealCatalog) {
    let ids: Vec<i64> = controller.all_tracks.tracks().iter().map(|t| t.id)
        .chain(controller.browse_tracks.iter().map(|t| t.id))
        .collect();
    // One query for the set — this ran a statement per row and an O(n²)
    // merge on every page load.
    let paths = catalog.paths_for(&ids);
    controller.merge_track_paths(paths);
}

/// Paths for one freshly fetched list, merged in.
fn paths_for_rows(controller: &mut bo_suite_egui::Controller,
                  catalog: &bo_suite_egui::real_catalog::RealCatalog,
                  rows: &[bo_suite_egui::browse::BrowseTrack]) {
    let ids: Vec<i64> = rows.iter().map(|t| t.id).collect();
    let paths = catalog.paths_for(&ids);
    controller.merge_track_paths(paths);
}

impl App {
    fn new() -> Self {
        let devices = vec![
            Device { name: "Living Room".into(), volume: 62, playing: true },
            Device { name: "Kitchen".into(), volume: 40, playing: false },
            Device { name: "Studio".into(), volume: 78, playing: true },
        ];
        let scenes = vec![
            Scene { name: "Morning".into() },
            Scene { name: "Focus".into() },
            Scene { name: "Evening Wind-down".into() },
        ];
        // THE LIBRARY, from bo-controller's own database. `CatalogSource`
        // had two implementations and both were test fakes (a third,
        // `FsCatalog`, guessed metadata from filenames because opening the
        // real catalog from a second process was assumed unsafe — it
        // isn't: the DB is WAL-mode specifically so a second reader
        // doesn't block the writer). Ratings, tags, and everything the Qt
        // app ever scanned now show up here unchanged.
        let db_path = bo_suite_egui::real_catalog::db_path();
        let music_roots = bo_suite_egui::real_catalog::music_roots();
        let (media_cmd_tx, media_cmd_rx) = std::sync::mpsc::channel();
        let (media_res_tx, media_res_rx) = std::sync::mpsc::channel();
        eprintln!("bo-suite: media db at {}, roots: {:?}", db_path.display(), music_roots);
        // FIRST LAUNCH. Nothing created the data directory, so on a fresh
        // machine the worker's `catalog::open` failed, the worker exited,
        // and every scan request went to a dropped channel — the library
        // could never be built from this app. And the two opens raced:
        // both connections read user_version 0 and both ran CREATE TABLE,
        // so even with the directory the second one failed with "table
        // tracks already exists". The directory is made here and the
        // read handle is opened BEFORE the worker starts, so the schema
        // exists by the time the worker looks; `catalog::migrate` is now
        // also atomic, for the day the Qt app and this one start at once.
        if let Some(dir) = db_path.parent() {
            if let Err(e) = std::fs::create_dir_all(dir) {
                eprintln!("bo-suite: could not create {}: {e}", dir.display());
            }
        }
        let catalog_first = bo_suite_egui::real_catalog::RealCatalog::open(&db_path, media_cmd_tx.clone());
        {
            let db_path = db_path.clone();
            std::thread::spawn(move || {
                bo_media::run_media_worker(media_cmd_rx, media_res_tx, db_path, music_roots, Vec::new());
            });
        }
        // Casting to a networked B&O speaker. Port 0 would be simpler but
        // the URL handed to the device has to be stable across calls
        // within one run, and TrackServer::bind picks whatever the OS
        // gives it — 51423 is arbitrary but fixed, matching bo-controller's
        // own practice of a fixed, documented port rather than one that
        // changes every launch and complicates firewall rules.
        let (cast_cmd_tx, cast_cmd_rx) = std::sync::mpsc::channel();
        let (cast_res_tx, cast_res_rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            bo_suite_egui::cast::run_cast_worker(cast_cmd_rx, cast_res_tx, 51423);
        });
        // SSDP discovery for the "cast to" device picker. Every 20s: rare
        // enough not to spam the LAN or wake sleeping devices repeatedly,
        // often enough that a speaker that just powered on shows up
        // without restarting the app.
        let (discovery_tx, discovery_rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            bo_suite_egui::cast::run_discovery_worker(discovery_tx, std::time::Duration::from_secs(20));
        });
        let catalog = match catalog_first {
            Ok(c) => c,
            Err(e) => {
                // The worker thread above still runs and will create the
                // database on its first write (a scan); this read handle
                // just couldn't open it yet (e.g. parent dir missing on
                // first launch). Retrying next frame is not implemented —
                // like FsCatalog before it, an empty library here is
                // reported, not silently swallowed.
                eprintln!("bo-suite: catalog open failed: {e} — library will appear empty \
                           until a rescan creates it");
                // A connection to an in-memory DB always succeeds, so this
                // never has to be an Option threaded through every view.
                let (fallback_tx, _fallback_rx) = std::sync::mpsc::channel();
                bo_suite_egui::real_catalog::RealCatalog::open(
                    std::path::Path::new(":memory:"), fallback_tx)
                    .expect("in-memory catalog always opens")
            }
        };
        let mut controller = Controller::new(devices, scenes);
        if catalog.total() == 0 {
            eprintln!("bo-suite: no music cataloged yet — rescan from Settings to scan \
                       $BO_MUSIC_DIR or ~/Music");
            // An empty catalogue on a machine with a music folder: scan it
            // without being asked, once. The alternative is an app that
            // opens on "no music" until the user finds the button.
            if bo_suite_egui::real_catalog::music_roots().iter().any(|r| r.is_dir()) {
                eprintln!("bo-suite: scanning the music folder(s) for the first time");
                controller.settings.scan = bo_suite_egui::settings_page::Scan::Running {
                    seen: 0, total: None, path: String::new() };
                catalog.rescan();
            }
        } else {
            eprintln!("bo-suite: {} track(s) in the catalog", catalog.total());
            // The browse aggregates from SQL, with real play counts, BEFORE
            // the page answer — `answer_tracks_from` fills them from the
            // page source when they are empty, and that source has no
            // counts.
            controller.supply_browse_tracks(catalog.browse_tracks());
            // The first page, so the list is not empty before anyone
            // scrolls. `answer_tracks_from` needs a pending request; asking
            // for offset 0 is that request.
            controller.pending_tracks =
                Some(bo_suite_egui::all_tracks::TracksAction::Load { offset: 0 });
            controller.answer_tracks_from(&catalog);
            sync_track_paths(&mut controller, &catalog);
        }
        // Whatever playlists already exist, so the page isn't empty until
        // the user creates one to see it work.
        controller.playlists_page.set(catalog.playlists());
        // The last session: queue, modes, and the track that was loaded —
        // paused where it was, not started.
        let mut restore_track = None;
        if let Some(text) = config_kit::load(SESSION_APP, SESSION_FILE) {
            restore_track = controller.restore_session(&text).map(|id| (id, controller.transport.position()));
            if let Some((id, at)) = restore_track {
                if let Some(t) = controller.browse_tracks.iter().find(|t| t.id == id).cloned() {
                    if let Some(path) = catalog.path_of(id) {
                        controller.load_paused(id, &t.title, &t.primary_artist, &path, at);
                        controller.merge_track_paths(vec![(id, path)]);
                        // Its length, for the bar: no player is running
                        // to report one.
                        catalog.resolve_for_play(id);
                        restore_track = None;
                    }
                }
            }
        }
        let ipc_path = std::env::var("XDG_RUNTIME_DIR").ok()
            .filter(|d| !d.is_empty())
            .unwrap_or_else(|| std::env::temp_dir().display().to_string());
        let ipc_path = format!("{ipc_path}/bo-suite-mpv-{}.sock", std::process::id());
        Self {
            backdrop: uikit_egui::Backdrop::for_app("bo-suite"),
            controller,
            player: bo_suite_egui::playback::Player::new(),
            last_note: String::new(),
            catalog,
            media_res_rx,
            cast_cmd_tx,
            cast_res_rx,
            discovery_rx,
            voice: bo_suite_egui::voice_source::VoiceSource::new(),
            voice_cmd: String::new(),
            ipc_path,
            playing_url: String::new(),
            session_saved: std::time::Instant::now(),
            session_text: String::new(),
            restore_track,
            record_on_start: None,
            home_refresh_at: None,
        }
    }

    /// Write the session file when it changed, at most once a second.
    fn save_session(&mut self) {
        if self.session_saved.elapsed() < std::time::Duration::from_secs(1) { return; }
        self.session_saved = std::time::Instant::now();
        let text = self.controller.render_session();
        if text == self.session_text { return; }
        match config_kit::save(SESSION_APP, SESSION_FILE, &text) {
            Ok(()) => self.session_text = text,
            Err(e) => eprintln!("bo-suite: could not save session: {e}"),
        }
    }

    /// Answer the library's outstanding questions: the browse
    /// aggregates, a search, the Home lists, the open playlist's rows.
    fn pump_library(&mut self) {
        if self.controller.browse_stale && self.catalog.total() > 0 {
            let rows = self.catalog.browse_tracks();
            paths_for_rows(&mut self.controller, &self.catalog, &rows);
            self.controller.supply_browse_tracks(rows);
            // A restored session could not resolve its track before the
            // library was read; it can now.
            if let Some((id, at)) = self.restore_track.take() {
                if let Some(t) = self.controller.browse_tracks.iter().find(|t| t.id == id).cloned() {
                    if let Some(path) = self.catalog.path_of(id) {
                        self.controller.load_paused(id, &t.title, &t.primary_artist, &path, at);
                        self.controller.merge_track_paths(vec![(id, path)]);
                        self.catalog.resolve_for_play(id);
                    }
                }
            }
        }
        if let Some(q) = self.controller.pending_search.take() {
            let rows = self.catalog.search(&q, 200);
            paths_for_rows(&mut self.controller, &self.catalog, &rows);
            self.controller.supply_search(&q, rows);
        }
        if self.home_refresh_at.is_some_and(|t| std::time::Instant::now() >= t) {
            self.home_refresh_at = None;
            self.controller.pending_home = true;
        }
        if self.controller.pending_home {
            let home = self.catalog.home(bo_suite_egui::HOME_ROWS);
            for list in [&home.recently_played, &home.recently_added, &home.most_played, &home.never_played] {
                paths_for_rows(&mut self.controller, &self.catalog, list);
            }
            self.controller.supply_home(home);
        }
        if let Some(id) = self.controller.pending_playlist_open.take() {
            let tracks = self.catalog.playlist_tracks(id);
            paths_for_rows(&mut self.controller, &self.catalog, &tracks);
            self.controller.playlist_tracks = tracks;
        }
        if let Some((pid, tid)) = self.controller.pending_playlist_remove.take() {
            if self.catalog.remove_from_playlist(pid, tid) {
                let tracks = self.catalog.playlist_tracks(pid);
                self.controller.playlist_tracks = tracks;
                self.controller.playlists_page.set(self.catalog.playlists());
            } else {
                self.controller.toast.show("could not remove that track");
            }
        }
    }
}

impl App {
    /// Keep the recogniser in step with the settings, and act on what it
    /// heard.
    ///
    /// `Controller::heard` had no caller until now: voice control was a
    /// complete, tested parser with no source, which is a switch that
    /// does nothing when you turn it on — worse than a missing feature,
    /// because the switch implies otherwise.
    fn pump_voice(&mut self) {
        // WANTED is both halves. Turning voice off must stop the process,
        // not merely ignore what it says: a recogniser still running after
        // the switch is off is a microphone nobody thinks is on.
        let prefs = &self.controller.voice;
        let wanted = if prefs.enabled { prefs.recognizer.trim().to_string() }
                     else { String::new() };
        if wanted != self.voice_cmd {
            self.voice_cmd = wanted.clone();
            self.voice.stop_now();
            if !wanted.is_empty() {
                eprintln!("bo-suite: listening with: {wanted}");
                self.voice.start(&wanted);
            }
        }
        if let Some(why) = self.voice.heard.take_failure() {
            // Said once. A recogniser that quits because its model is
            // missing otherwise leaves a switch reading "on" and a room
            // where nothing happens.
            self.last_note = why;
        }
        for phrase in self.voice.heard.drain() {
            // Logged before acting, so a phrase that did nothing can be
            // told apart from one that was never heard — the two look
            // identical from the room.
            eprintln!("bo-suite: heard {phrase:?}");
            let _ = self.controller.heard(&phrase);
        }
    }

    /// Carry out whatever the view asked for, and report back what the
    /// player did.
    fn pump_playback(&mut self) {
        use bo_suite_egui::playback::{Request, State};
        if self.controller.pending_rescan {
            self.controller.pending_rescan = false;
            self.catalog.rescan();
        }
        if let Some((id, stars)) = self.controller.pending_rating.take() {
            // Optimistic star already filled by `Controller::rate`; this
            // persists it. `pump_media_results` reconciles the pending dot
            // once the worker echoes it back as `MediaResult::TrackRating`.
            self.catalog.set_rating(id, stars as i64);
        }
        if let Some(device_base) = self.controller.pending_cast.take() {
            // Distributed daemon playback: hand the file to bo-cast's
            // worker rather than local mpv. Resolved the same way
            // request_play resolves a path — through RealCatalog, not a
            // second lookup mechanism.
            if let Some(id) = self.controller.now_playing_id {
                match self.catalog.path_of(id) {
                    Some(path) => {
                        let title = self.controller.all_tracks.tracks().iter()
                            .find(|t| t.id == id)
                            .map(|t| t.title_text())
                            .unwrap_or_default();
                        let _ = self.cast_cmd_tx.send(bo_suite_egui::cast::CastCmd::PlayFile {
                            device_base, local_path: std::path::PathBuf::from(path), title,
                        });
                    }
                    None => self.last_note = "cast: no file for that track".into(),
                }
            }
        }
        if let Some((name, description)) = self.controller.pending_playlist_create.take() {
            // A local SQLite insert, not a network/scan operation — done
            // directly here rather than round-tripped through the media
            // worker, same reasoning as RealCatalog's other direct writes.
            match self.catalog.create_playlist(&name, &description) {
                Some(_id) => {
                    // Replaces the optimistic fake-id row from
                    // apply_playlist with the real list rather than trying
                    // to patch just that one row in place.
                    self.controller.playlists_page.set(self.catalog.playlists());
                }
                None => self.last_note = format!("could not create playlist '{name}'"),
            }
        }
        if let Some(id) = self.controller.pending_playlist_delete.take() {
            if self.catalog.delete_playlist(id) {
                self.controller.playlists_page.set(self.catalog.playlists());
            } else {
                self.last_note = "could not delete playlist".into();
            }
        }
        if let Some((track_id, playlist_id)) = self.controller.pending_add_to_playlist.take() {
            if self.catalog.add_to_playlist(playlist_id, track_id) {
                // track_count on the index page is now stale.
                self.controller.playlists_page.set(self.catalog.playlists());
            } else {
                self.last_note = "could not add track to playlist".into();
            }
        }
        if let Some(id) = self.controller.pending_track_edit_open.take() {
            match self.catalog.track_fields(id) {
                Some(fields) => self.controller.edit_track.open(&id.to_string(), fields),
                None => self.last_note = "could not load track for editing".into(),
            }
        }
        if let Some((id, title, artist, album, year)) = self.controller.pending_track_edit_save.take() {
            self.catalog.save_track_edit(id, title, artist, album, year);
            // The override applies immediately in the catalog (see
            // real_catalog.rs), so reload whatever's currently showing
            // this track rather than waiting for the next scan.
            if let Some(pid) = self.controller.open_playlist {
                self.controller.playlist_tracks = self.catalog.playlist_tracks(pid);
            }
            self.controller.pending_tracks =
                Some(bo_suite_egui::all_tracks::TracksAction::Load { offset: 0 });
            self.controller.browse_stale = true;
            self.controller.pending_home = true;
        }
        if self.controller.pending_scan_picker_open {
            self.controller.pending_scan_picker_open = false;
            // The picker's own doc: suggestions are the host's job, since
            // it (not the Controller) knows $HOME/$BO_MUSIC_DIR. Offer the
            // configured default roots plus common removable-media mount
            // points, when they exist — a folder that isn't there yet
            // isn't worth ticking.
            let mut suggestions: Vec<String> = bo_suite_egui::real_catalog::music_roots()
                .into_iter().map(|p| p.display().to_string()).collect();
            for base in ["/media", "/run/media"] {
                if let Ok(entries) = std::fs::read_dir(base) {
                    for entry in entries.flatten() {
                        suggestions.push(entry.path().display().to_string());
                    }
                }
            }
            self.controller.scan_picker.open(suggestions);
        }
        if let Some(roots) = self.controller.pending_scan_with_roots.take() {
            self.catalog.scan_with_roots(roots);
        }
        if let Some(url) = self.controller.pending_remote_queue_load.take() {
            self.controller.remote_queue = self.catalog.queue_for_device(&url).into_iter()
                .map(|e| (e.track_id, e.title, e.primary_artist.unwrap_or_default()))
                .collect();
        }
        if let Some((url, track_id)) = self.controller.pending_remote_queue_push.take() {
            if self.catalog.queue_push(track_id, &url) {
                // Reflect it immediately rather than waiting for the next
                // time the device is reselected.
                self.controller.remote_queue = self.catalog.queue_for_device(&url).into_iter()
                    .map(|e| (e.track_id, e.title, e.primary_artist.unwrap_or_default()))
                    .collect();
            } else {
                self.last_note = "could not queue track on that speaker".into();
            }
        }
        if let Some(id) = self.controller.last_played_track.take() {
            // Not recorded yet: a play is counted when the player reports
            // it STARTED, not when it was asked for — a track that failed
            // to open was showing up as "2 plays" in Most played.
            self.record_on_start = Some(id);
            // The tag reader's duration, for players that report none.
            self.catalog.resolve_for_play(id);
        }
        if let Some(v) = self.controller.pending_volume.take() {
            self.controller.volume = v.clamp(0.0, 100.0);
            self.controller.dial.set_external(self.controller.volume);
            let sink = self.controller.audio_sink.clone();
            if !self.player.set_volume_with(&sink, v, "sh") {
                self.last_note = "volume: pactl is not available on this machine".into();
            }
        }
        if let Some(req) = self.controller.pending_playback.take() {
            use bo_suite_egui::playback::Control;
            match req {
                Request::Play { url, sink, start_secs } => self.start_player(&url, &sink, start_secs),
                Request::Pause => {
                    // In place when the player can; a kill otherwise. The
                    // transport remembered the position either way.
                    if !self.player.control(Control::Pause) { self.player.stop_now(); }
                }
                Request::Resume { url, sink, at } => {
                    let same = self.playing_url == url && self.player.is_running();
                    if !(same && self.player.control(Control::Resume)) {
                        self.start_player(&url, &sink, at);
                    }
                }
                Request::Seek { url, sink, at } => {
                    let same = self.playing_url == url && self.player.is_running();
                    if !(same && self.player.control(Control::Seek(at))) {
                        self.start_player(&url, &sink, at);
                    }
                }
                Request::Stop => { self.player.stop_now(); self.playing_url.clear(); }
                Request::ReleaseRoute => { self.player.release_route("sh"); self.playing_url.clear(); }
                Request::Volume(v) => { self.controller.pending_volume = Some(v); }
            }
        }
        // THE PLAYER'S CLOCK INTO THE BAR. `Transport::report` is the sync
        // point between them and had no caller, so the bar extrapolated
        // from zero forever and `duration` was never set — `fraction()`
        // was permanently 0 and the handle never moved.
        //
        // The local `tick` still runs between reports: mpv prints about
        // once a second, and a bar that only moved then would stutter.
        if let Some((pos, dur)) = self.player.shared.take_progress() {
            self.controller.transport.duration = dur;
            self.controller.transport.report(pos);
        }
        if let Some(state) = self.player.shared.take_state() {
            if let State::Playing { .. } = &state {
                if let Some(id) = self.record_on_start.take() {
                    // Persisted through the worker (play count + history),
                    // not written on this connection — see real_catalog.rs.
                    // Both tables: `RecordPlay` bumps the count, and only
                    // the history row makes "recently played" true.
                    self.catalog.record_play(id);
                    self.catalog.record_history(id, LOCAL_DEVICE, None);
                    // Home's "recently played" is stale the moment this
                    // plays. Asked for a moment later, so the worker has
                    // written the row by the time the read runs.
                    self.home_refresh_at = Some(std::time::Instant::now() + std::time::Duration::from_millis(400));
                }
            }
            if let State::Failed(why) = state {
                self.record_on_start = None;
                // The player's own words. "Playback failed" tells nobody
                // anything; "Failed to open audio device" tells them
                // everything.
                self.last_note = format!("player: {why}");
                self.controller.transport.pause();
                self.controller.now_playing.playing = false;
                self.playing_url.clear();
            }
        }
        if self.player.shared.take_ended() {
            // A track that finished is not a track that was stopped — the
            // player distinguishes them precisely so this can advance
            // without pausing skipping a track. The view decides WHAT is
            // next (repeat mode, queue head); this only reports that the
            // last one is over.
            self.controller.on_track_ended();
        }
        if !self.last_note.is_empty() {
            // Logged AND shown: a failure only on stderr is invisible on
            // a phone, where there is no terminal behind the window.
            let note = std::mem::take(&mut self.last_note);
            eprintln!("bo-suite: {note}");
            self.controller.toast.show(&note);
        }
    }

    /// Spawn the player for a track at an offset.
    fn start_player(&mut self, url: &str, sink: &bo_suite_egui::playback::Sink, start_secs: f64) {
        use bo_suite_egui::playback::{plan_with, PlayOpts};
        if url.trim().is_empty() {
            // NOT attempted. Handing an empty path to a player gets "no
            // such file", which reads as a broken player rather than as
            // the missing catalogue it actually is.
            self.last_note = "no track resolved — the library has titles, not files yet".into();
            self.controller.transport.pause();
            return;
        }
        // Whatever is installed, not whatever I assumed. mpv was
        // hardcoded and is not always present.
        let Some(player) = bo_suite_egui::playback::discover_player() else {
            self.last_note = "no player found — install mpv, ffplay or paplay".into();
            self.controller.transport.pause();
            return;
        };
        let opts = PlayOpts { start_secs, ipc: Some(self.ipc_path.clone()) };
        let p = plan_with(player, url, sink, &opts);
        for c in &p.setup { eprintln!("bo-suite: {c}"); }
        eprintln!("bo-suite: {}", p.play);
        self.player.start(&p);
        self.playing_url = url.to_string();
    }

    /// Drain whatever the media worker has sent back since last frame.
    /// Non-blocking: a worker mid-scan or mid-write just means nothing is
    /// ready yet, not that this should wait for it.
    fn pump_media_results(&mut self) {
        use bo_media::MediaResult;
        while let Ok(msg) = self.media_res_rx.try_recv() {
            match msg {
                // Closes the gap `confirm_rating`'s own doc comment named:
                // a rating tapped in the UI stayed "pending" forever
                // because nothing ever reported the stored value back.
                MediaResult::TrackRating { track_id, rating } => {
                    self.controller.confirm_rating(track_id, rating as u8);
                }
                MediaResult::ScanProgress(p) => {
                    use bo_media::scanner::ScanPhase;
                    use bo_suite_egui::settings_page::Scan;
                    eprintln!("bo-suite: scan {:?} {}/{}", p.phase, p.done, p.total);
                    self.controller.settings.scan = match p.phase {
                        ScanPhase::Idle => Scan::Idle,
                        ScanPhase::Done => Scan::Done { added: p.done as usize },
                        ScanPhase::Walking | ScanPhase::Parsing | ScanPhase::Analyzing =>
                            Scan::Running {
                                seen: p.done as usize,
                                total: if p.total > 0 { Some(p.total as usize) } else { None },
                                path: String::new(),
                            },
                    };
                    if p.phase == ScanPhase::Done {
                        // The list showing what was already on disk before
                        // the scan started is now stale — reload the page
                        // and paths in view so new/changed tracks appear
                        // without the user having to leave and re-enter
                        // the library.
                        self.controller.pending_tracks =
                            Some(bo_suite_egui::all_tracks::TracksAction::Load { offset: 0 });
                        self.controller.browse_stale = true;
                        self.controller.pending_home = true;
                        self.controller.toast.show(&format!("library: {} tracks", self.catalog.total()));
                    }
                }
                MediaResult::TrackForPlay { track_id, duration_secs, .. } => {
                    // The tag reader's length, for the bar when the player
                    // reports none (ffplay, paplay). Only for the track
                    // still loaded: the answer can arrive after a skip.
                    if self.controller.now_playing_id == Some(track_id) {
                        self.controller.set_duration_hint(duration_secs as f64);
                    }
                }
                MediaResult::Error(e) => self.last_note = format!("library: {e}"),
                // Everything else (playlists, queue, history, soundscape,
                // mixer-offline, tags) has no egui surface calling the
                // matching MediaCmd yet, so there is nothing to reconcile
                // here either — see real_catalog.rs's follow-up note.
                _ => {}
            }
        }
        // Cast attempts (bo-cast), separate from the media worker above —
        // different channel, same drain-non-blocking shape.
        while let Ok(r) = self.cast_res_rx.try_recv() {
            self.controller.cast_result(r.ok, &r.detail);
        }
        // Only the LAST round matters — a discovery result is a full
        // snapshot, not a delta, so draining to the newest and discarding
        // any stale ones queued behind it is correct, not a shortcut.
        let mut latest_devices = None;
        while let Ok(d) = self.discovery_rx.try_recv() { latest_devices = Some(d); }
        if let Some(devices) = latest_devices {
            self.controller.device_selector.set_devices(devices);
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, _ui: &mut egui::Ui, _frame: &mut eframe::Frame) {}
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let colors = uikit_egui::apply_palette(ctx, snapclone_theme::palette_for(snapclone_theme::PaletteId::Amber));
        self.backdrop.paint(ctx, &colors);
        let mut prefs = settings_kit::Prefs::default();
        prefs.palette = "amber".to_string();
        self.controller.apply_settings(ctx, &prefs);
        egui::CentralPanel::default().show(ctx, |ui| {
            let adaptive = Adaptive::detect(ctx);
            self.controller.ui(ui, &adaptive);
        });
        // Pages the list asks for, answered from the scan.
        if self.controller.pending_tracks.is_some() {
            self.controller.answer_tracks_from(&self.catalog);
            sync_track_paths(&mut self.controller, &self.catalog);
        }
        self.pump_library();
        self.pump_voice();
        self.pump_playback();
        self.pump_media_results();
        self.save_session();
    }
}

/// Must match packaging/org.bosuite.BoSuite.desktop exactly. A mismatch fails
/// SILENTLY: the app runs and has no icon, no dock grouping and the wrong
/// name in the switcher.
const APP_ID: &str = "org.bosuite.BoSuite";

fn main() -> eframe::Result<()> {
    // Why the window will not open, said before the panic in a dependency does.
    // `uikit_egui::backend_check`: the usual cause is a static musl build, which cannot
    // dlopen libwayland-client, and wayland-sys's own message points at a package that is
    // already installed. Silent when the setup is fine.
    if let Some(why) = uikit_egui::backend_check::preflight() {
        eprintln!("{why}");
    }
    eframe::run_native(
        "bo-suite (egui)",
        eframe::NativeOptions {
        // Glow SAID, not left to eframe: its 0.34 defaults include
        // wgpu but not glow, and wgpu's default device descriptor
        // asks for 8 colour attachments — which dies at startup on
        // a phone GPU allowing 5. A glow-only feature list is not
        // enough; gitmind-egui declared one and still got wgpu
        // through feature unification.
        renderer: eframe::Renderer::Glow,
        viewport: uikit_egui::desktop::viewport(APP_ID, "Bo Suite", [900.0, 700.0]),
        ..Default::default()
    },
        Box::new(|_cc| Ok(Box::new(App::new()))),
    )
}
