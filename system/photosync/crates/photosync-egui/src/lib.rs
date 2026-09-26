//! photosync-egui — the shell. All grouping, membership and layout math live
//! in `photosync::album` and `photosync::gallery_grid`; this owns the frame
//! loop, the palette, and nothing else.
//!
//! The QML surface under `system/photosync/gui` is a camera, not a gallery —
//! see the module note on `album`. This is the egui renderer for the
//! Fotema-shaped view that module was built for: a camera roll grouped by
//! date, on the left a rail for Library / Albums, and albums you can make
//! from a selection.
//!
//! It follows the same split as every other port in this tree
//! (`switcheroo-core`/`switcheroo-egui`, `aperture-egui`, …): the core is
//! headless and tested without a window, and this crate is a thin renderer
//! over it.

pub mod ui;
/// Real thumbnails, decoded off the UI thread (headless; ui.rs uploads and draws).
pub mod thumbs;
pub mod settings;
/// What a press on a tile means (tap opens, long-press selects) — headless.
pub mod selection;

use photosync::album::{Item, Library};

/// Which rail entry is selected. `Settings` sits in this same list, as a
/// normal destination — not a gear icon that opens a separate preferences
/// window the way GTK/libadwaita apps (Fotema included) tuck it away. The
/// content differs, the navigation model doesn't.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum View {
    Library,
    Album(String),
    Settings,
}

pub struct App {
    /// The live backdrop behind the panels — same finish every -egui app in
    /// the fleet uses, through the kit.
    pub backdrop: uikit_egui::Backdrop,
    pub library: Library,
    pub view: View,
    /// Selected photos, for "add to album" / share. See `selection` for what
    /// a tap, a long-press and the modifiers do.
    pub selected: selection::Selection,
    /// New-album name field, shown while `new_album_open` is true.
    pub new_album_name: String,
    pub new_album_open: bool,
    /// Target cell size fed to `gallery_grid::grid_for`; the "zoom" slider.
    pub tile_target: f32,
    /// Left rail. Collapses to a rail of icons below its minimum width —
    /// see `uikit_egui::sidebar` for why that's a field here rather than
    /// something read back from egui's own panel layout.
    pub rail: uikit_egui::sidebar::Sidebar,
    pub status: String,
    pub settings: settings::Settings,
    /// Background thumbnail decoding; started by the first frame (`ui::draw`),
    /// which has the repaint callback to give it.
    pub thumbs: Option<thumbs::Thumbs>,
    /// Uploaded thumbnails by path; entries leave when `thumbs` evicts them.
    pub textures: std::collections::HashMap<String, egui::TextureHandle>,
    /// The photo open in the full-size viewer (double-click a tile).
    pub viewing: Option<String>,
    /// Large decodes for the viewer: the same engine as `thumbs`, its own small
    /// budget, so it never competes with the grid's cache.
    pub full: Option<thumbs::Thumbs>,
    /// The viewer's current large picture, by path.
    pub full_tex: Option<(String, egui::TextureHandle)>,
    /// The service's database this library came from (`from_path`), whose
    /// stored thumbnails the grid uses; None for the demo.
    pub db_path: Option<std::path::PathBuf>,
    /// Where albums are kept (`open_albums`): the viewing user's own file,
    /// since the service's store is opened read-only. None: albums live for
    /// this run only (the demo), and the status line says so on a change.
    pub albums_db: Option<std::path::PathBuf>,
    /// "Delete album" asks once: the album awaiting the second press.
    pub confirm_delete: Option<String>,    /// The tiles drawn last frame and where: what a press can land on. Read
    /// by tests driving real input, and the basis for keyboard focus later.
    pub last_tiles: Vec<(String, egui::Rect)>,
    /// This frame's shape (`is_compact` of the window width), set by the
    /// frame loop: a phone gets a drawer and a condensed header.
    pub compact: bool,
    /// The drawer (the rail's contents, on a phone) is open.
    pub drawer_open: bool,    /// The viewer's photo, dragged sideways this far (px) by a swipe in progress.
    pub viewer_dx: f32,
}

/// Below this window width the rail becomes a drawer, the header condenses
/// and tiles size themselves to the screen. The rail (200) beside two
/// comfortable columns needs about this much; under it the rail took 160 px
/// of a 380 px phone and left one column of small tiles.
pub const COMPACT_BELOW: f32 = 600.0;

pub fn is_compact(window_width: f32) -> bool {
    window_width < COMPACT_BELOW
}

/// On a phone the tile size follows the screen rather than the (desktop)
/// slider: three across in portrait, four once there is room, the way a
/// camera roll looks. Returns a target for `gallery_grid::grid_for` that
/// lands on exactly that many columns for `width` and `gap`.
pub fn compact_tile_target(width: f32, gap: f32) -> f32 {
    let cols = if width >= 480.0 { 4.0 } else { 3.0 };
    (((width - gap) / cols) - gap).floor().max(photosync::gallery_grid::MIN_CELL)
}

/// A sideways swipe in the viewer, decided at release: 1 for the next photo
/// (dragged left), -1 for the previous, 0 to spring back. Far enough (a fifth
/// of the width) or a flick (fast, in the same direction, past a small
/// distance so a tap's jitter is not one) both count.
pub fn swipe_step(dx: f32, width: f32, velocity_x: f32) -> i32 {
    let far = dx.abs() > 0.2 * width;
    let flick = velocity_x.abs() > 500.0 && dx.abs() > 24.0 && dx.signum() == velocity_x.signum();
    if far || flick { if dx < 0.0 { 1 } else { -1 } } else { 0 }
}

fn plural(n: usize) -> &'static str { if n == 1 { "" } else { "s" } }

/// Where the gallery keeps albums by default: `$XDG_DATA_HOME/photosync/albums.db`,
/// else `~/.local/share/photosync/albums.db`.
pub fn default_albums_path() -> Option<std::path::PathBuf> {
    let base = std::env::var_os("XDG_DATA_HOME").filter(|v| !v.is_empty()).map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".local/share")))?;
    Some(base.join("photosync").join("albums.db"))
}

/// The photo `delta` steps from `current` in `order`, clamped at the ends;
/// None if `current` is not in it.
pub fn step(order: &[String], current: &str, delta: i32) -> Option<String> {
    let i = order.iter().position(|p| p == current)? as i64;
    let j = (i + delta as i64).clamp(0, order.len() as i64 - 1) as usize;
    Some(order[j].clone())
}

impl App {
    pub fn new(library: Library) -> App {
        App {
            backdrop: uikit_egui::Backdrop::for_app("photosync"),
            library,
            view: View::Library,
            selected: selection::Selection::default(),
            new_album_name: String::new(),
            new_album_open: false,
            tile_target: 168.0,
            rail: uikit_egui::sidebar::Sidebar::new(uikit_egui::sidebar::Side::Left, 200.0)
                .min_width(160.0)
                .rail_width(56.0)
                .reserve(360.0),
            status: String::new(),
            settings: settings::Settings::default(),
            thumbs: None,
            textures: std::collections::HashMap::new(),
            viewing: None,
            full: None,
            full_tex: None,
            db_path: None,
            albums_db: None,
            confirm_delete: None,
            last_tiles: Vec::new(),
            compact: false,
            drawer_open: false,
            viewer_dx: 0.0,
        }
    }

    /// A library populated from an actual `PhotoSyncDB` — `album::load`
    /// over `PhotoSyncDB::connection()`, now that the connection is
    /// reachable (see that method's doc for why it wasn't before). This is
    /// the real constructor; `demo()` below is only for looking at the
    /// shell before a store exists to point it at.
    pub fn from_db(db: &photosync::PhotoSyncDB) -> Result<App, rusqlite::Error> {
        Ok(App::new(photosync::album::load(db.connection())?))
    }

    /// The daemon's library, opened READ-ONLY: a viewer never creates or
    /// changes the daemon's store (`PhotoSyncDB::new` would create it).
    pub fn from_path(db: &std::path::Path) -> Result<App, String> {
        let conn = rusqlite::Connection::open_with_flags(db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(|e| format!("{}: {e}", db.display()))?;
        let lib = photosync::album::load(&conn).map_err(|e| format!("{}: {e}", db.display()))?;
        let mut app = App::new(lib);
        app.db_path = Some(db.to_path_buf());
        Ok(app)
    }

    /// Keep albums in `path` (created if missing — it is the user's file, not
    /// the service's). Albums already in it replace the ones the library
    /// loaded; a file with no album tables yet keeps those (albums from an
    /// older gallery that wrote into the service's database) and they are
    /// written here on the first change.
    pub fn open_albums(&mut self, path: &std::path::Path) -> Result<(), String> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
        let conn = rusqlite::Connection::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
        photosync::album::load_albums(&conn, &mut self.library).map_err(|e| format!("{}: {e}", path.display()))?;
        self.albums_db = Some(path.to_path_buf());
        Ok(())
    }

    /// Write the albums to `albums_db`. A failure is said on the status line:
    /// the change stays on screen, and the user is told it will not survive.
    fn persist_albums(&mut self) {
        let Some(path) = &self.albums_db else {
            self.status = format!("{} (albums are not saved in the demo)", self.status);
            return;
        };
        let r = rusqlite::Connection::open(path)
            .and_then(|c| photosync::album::save_albums(&c, &self.library));
        if let Err(e) = r {
            self.status = format!("Albums not saved: {}: {e}", path.display());
        }
    }

    /// The photos the grid shows now, in its order: the library by day, or
    /// the open album. Shift-selection ranges and the viewer's arrows walk
    /// this — so stepping through an album stays in the album.
    pub fn visible_order(&self) -> Vec<String> {
        match &self.view {
            View::Album(name) => self.library.album_items(name).into_iter().map(|i| i.path.clone()).collect(),
            _ => self.library.by_day().into_iter().flat_map(|(_, v)| v.into_iter().map(|i| i.path.clone())).collect(),
        }
    }

    /// Make an album, with the selection in it if there is one, and show it.
    pub fn new_album(&mut self, name: &str) -> bool {
        let name = name.trim().to_string();
        if !self.library.create_album(&name) {
            self.status = "Album name already taken, or blank".into();
            return false;
        }
        let n = self.library.add_to_album(&name, &self.selected.paths());
        self.selected.clear();
        self.status = if n > 0 { format!("Created album \"{name}\" with {n} photo{}", plural(n)) }
                      else { format!("Created album \"{name}\"") };
        self.view = View::Album(name);
        self.persist_albums();
        true
    }

    /// Add the selection to an existing album. Returns how many were new to it.
    pub fn add_selection_to(&mut self, album: &str) -> usize {
        let asked = self.selected.len();
        let n = self.library.add_to_album(album, &self.selected.paths());
        self.selected.clear();
        self.status = match (n, asked - n) {
            (n, 0) => format!("Added {n} photo{} to \"{album}\"", plural(n)),
            (n, already) => format!("Added {n} to \"{album}\" ({already} already in it)"),
        };
        self.persist_albums();
        n
    }

    /// Take the selection out of an album. The photos are not touched —
    /// membership is a reference (see `photosync::album`).
    pub fn remove_selection_from(&mut self, album: &str) -> usize {
        let n = self.selected.paths().iter().filter(|p| self.library.remove_from_album(album, p)).count();
        self.selected.clear();
        self.status = format!("Removed {n} photo{} from \"{album}\" (the photos are kept)", plural(n));
        self.persist_albums();
        n
    }

    /// Delete an album — never its photos — and go back to the library.
    pub fn delete_album(&mut self, album: &str) -> bool {
        if !self.library.delete_album(album) { return false; }
        self.confirm_delete = None;
        self.selected.clear();
        if self.view == View::Album(album.to_string()) { self.view = View::Library; }
        self.status = format!("Deleted album \"{album}\" (its photos are still in the library)");
        self.persist_albums();
        true
    }

    /// A library with no real store behind it — see `from_db` for the real
    /// path. This exists so the shell has something to paint and can be
    /// looked at before a database is wired up, the same way `switcheroo`
    /// ships a `DemoBackend`.
    pub fn demo() -> App {
        let mut items = Vec::new();
        let base = 1_726_000_000_i64; // an arbitrary anchor; only relative spacing matters
        for day in 0..14 {
            let shots_today = if day % 5 == 0 { 1 } else { 3 + (day % 4) };
            for shot in 0..shots_today {
                let taken_at = base - day * 86_400 + shot * 300;
                items.push(Item {
                    path: format!("/photos/2026/img_{day:02}_{shot:02}.jpg"),
                    taken_at,
                    thumbnail: None,
                    bytes: (1.5 + shot as f64 * 0.3) as u64 * 1024 * 1024,
                    metadata_json: None,
                });
            }
        }
        let mut library = Library::new(items);
        library.create_album("Favourites");
        let first_three: Vec<String> = library.by_day().into_iter().flat_map(|(_, v)| v)
            .take(3).map(|i| i.path.clone()).collect();
        library.add_to_album("Favourites", &first_three);
        App::new(library)
    }
}

#[cfg(test)]
mod from_path_tests {
    use super::*;

    #[test]
    fn a_missing_library_is_an_error_and_is_not_created() {
        let dir = std::env::temp_dir().join(format!("photosync-egui-nodb-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let db = dir.join("photosync.db");
        assert!(App::from_path(&db).is_err());
        assert!(!db.exists(), "a viewer must never create the daemon's store");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_daemons_library_opens_read_only() {
        let dir = std::env::temp_dir().join(format!("photosync-egui-db-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("blocks")).unwrap();
        let db = dir.join("photosync.db");
        drop(photosync::PhotoSyncDB::new(&db, &dir.join("blocks")).expect("the daemon's schema"));
        let before = std::fs::metadata(&db).unwrap().modified().unwrap();
        let app = App::from_path(&db).expect("opens");
        assert!(app.library.is_empty());
        assert_eq!(std::fs::metadata(&db).unwrap().modified().unwrap(), before, "not written");
        let _ = std::fs::remove_dir_all(&dir);
    }
}


#[cfg(test)]
mod layout_tests {
    use super::*;

    #[test]
    fn a_swipe_steps_when_far_or_flicked_and_springs_back_otherwise() {
        assert_eq!(swipe_step(-120.0, 400.0, 0.0), 1, "dragged left a fifth: next");
        assert_eq!(swipe_step(120.0, 400.0, 0.0), -1, "right: previous");
        assert_eq!(swipe_step(-60.0, 400.0, 0.0), 0, "short and slow: back");
        assert_eq!(swipe_step(-40.0, 400.0, -900.0), 1, "a flick");
        assert_eq!(swipe_step(-40.0, 400.0, 900.0), 0, "moving back the other way is not a flick");
        assert_eq!(swipe_step(-10.0, 400.0, -2000.0), 0, "jitter is not a flick");
    }

    #[test]
    fn a_phone_is_compact_and_gets_three_or_four_columns() {
        assert!(is_compact(360.0) && is_compact(599.0) && !is_compact(600.0) && !is_compact(1040.0));
        for (w, cols) in [(352.0, 3), (400.0, 3), (479.0, 3), (480.0, 4), (592.0, 4)] {
            let g = photosync::gallery_grid::grid_for(w, compact_tile_target(w, 3.0), 3.0);
            assert_eq!(g.columns, cols, "width {w}");
            assert!(g.cell * cols as f32 + 3.0 * (cols + 1) as f32 <= w + 0.01, "fits: {w} {g:?}");
        }
    }
}
