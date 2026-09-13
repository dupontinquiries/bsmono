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
pub mod settings;

use photosync::album::{Item, Library};
use std::collections::BTreeSet;

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
    /// Selected paths, for "add to album" / share. A set: the grid toggles
    /// membership on click, so a path can only ever be selected once.
    pub selected: BTreeSet<String>,
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
}

impl App {
    pub fn new(library: Library) -> App {
        App {
            backdrop: uikit_egui::Backdrop::for_app("photosync"),
            library,
            view: View::Library,
            selected: BTreeSet::new(),
            new_album_name: String::new(),
            new_album_open: false,
            tile_target: 168.0,
            rail: uikit_egui::sidebar::Sidebar::new(uikit_egui::sidebar::Side::Left, 200.0)
                .min_width(160.0)
                .rail_width(56.0)
                .reserve(360.0),
            status: String::new(),
            settings: settings::Settings::default(),
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
