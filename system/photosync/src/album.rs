//! Albums and the library view — the model, with no GUI in it.
//!
//! # What this is for
//!
//! photosync stores files: `files` has a path, timestamps, a thumbnail hash and
//! a metadata blob, and `blocks` deduplicates the content underneath. What it
//! has never had is a way to LOOK at any of that. The Qt/QML surface under
//! `gui/` is a camera, not a gallery.
//!
//! The ask is a Fotema-shaped one: a camera roll grouped by date, albums you
//! can make, metadata you can edit, and a way to share a selection. This is the
//! half of that which does not need a window — grouping, membership, selection
//! and the rules about what a share means — so it can be tested without one,
//! and so the eventual egui shell is a renderer rather than the whole feature.
//!
//! It is the same split every other port in this tree uses, and for the same
//! reason: `switcheroo-core` is headless with a demo backend and twelve tests
//! while `switcheroo-egui` owns the frame loop.
//!
//! # The rule that shapes the whole module
//!
//! **An album is a SET OF PATHS, not a copy and not a move.** photosync's whole
//! premise is that the bytes live once in a content-addressed store; an album
//! that copied files would defeat the deduplication the daemon exists to do,
//! and an album that moved them would make "remove from album" a destructive
//! act. So membership is a reference, removal is always safe, and the same
//! photo can be in any number of albums for free.
//!
//! That also decides what deleting an album means: it removes the album, never
//! the photos. A gallery that can lose your pictures by tidying is not one
//! anybody should keep photos in.

use std::collections::{BTreeMap, BTreeSet};

/// A photo or video in the library, as the view needs it.
///
/// Deliberately not the `files` row: the view wants a display name and a date
/// it can group by, and does not want chunk lists or hash algorithms. Keeping
/// them apart means a schema change does not reach the UI, and a UI change
/// does not reach the store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    pub path: String,
    /// Seconds since the epoch. `created_at` from the row — when the picture
    /// was TAKEN where that is known, not when it was copied in.
    pub taken_at: i64,
    /// Content hash of the thumbnail, if one has been generated.
    pub thumbnail: Option<String>,
    pub bytes: u64,
    /// Free-form metadata as stored, parsed by the caller when it cares.
    pub metadata_json: Option<String>,
}

impl Item {
    /// `YYYY-MM-DD` in UTC, the key the camera roll groups by.
    ///
    /// UTC, and that is a choice with a cost worth stating: a photo taken at
    /// 23:00 local in a UTC+2 zone files under the next day. The alternative —
    /// local time — makes the grouping depend on where the VIEWER is, so the
    /// same library reshuffles when you travel and a shared album disagrees
    /// with itself between two people. A fixed frame that is sometimes off by
    /// a day beats a moving one that is always plausible.
    ///
    /// Doing it by hand rather than pulling a date crate: this is a civil
    /// calendar conversion of a Unix timestamp, it is thirty lines, and
    /// photosync's dependency list is currently rusqlite and not much else.
    pub fn day(&self) -> String {
        let (y, m, d) = civil_from_unix(self.taken_at);
        format!("{y:04}-{m:02}-{d:02}")
    }
}

/// Days since the epoch → (year, month, day), proleptic Gregorian.
///
/// Howard Hinnant's `civil_from_days`, which is the standard way to do this
/// without a calendar library. Negative timestamps (photos dated before 1970 —
/// a scan of an old print, or a camera with a dead clock) floor correctly
/// rather than truncating toward zero, which is the bug every naive version
/// of this has.
fn civil_from_unix(secs: i64) -> (i64, u32, u32) {
    let days = secs.div_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// A named set of items. See the module note: a reference, never a copy.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Album {
    pub name: String,
    /// Paths, ordered and deduplicated. A `BTreeSet` because adding the same
    /// photo twice is a thing a user does by accident — dragging a selection
    /// that overlaps one they already added — and it should be a no-op rather
    /// than a duplicate tile.
    pub items: BTreeSet<String>,
}

/// The library: everything known, plus the albums over it.
#[derive(Debug, Clone, Default)]
pub struct Library {
    items: Vec<Item>,
    pub(crate) albums: BTreeMap<String, Album>,
}

impl Library {
    pub fn new(items: Vec<Item>) -> Library {
        Library { items, albums: BTreeMap::new() }
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// The camera roll: newest day first, and newest within each day.
    ///
    /// Both orders matter and they are not the same choice. Days descend
    /// because a gallery opens on what you just took. Within a day the newest
    /// is also first, so scrolling into a day reads as continuing backwards in
    /// time rather than reversing direction at every heading.
    pub fn by_day(&self) -> Vec<(String, Vec<&Item>)> {
        let mut groups: BTreeMap<String, Vec<&Item>> = BTreeMap::new();
        for i in &self.items {
            groups.entry(i.day()).or_default().push(i);
        }
        let mut out: Vec<(String, Vec<&Item>)> = groups.into_iter().collect();
        out.reverse();
        for (_, v) in out.iter_mut() {
            v.sort_by(|a, b| b.taken_at.cmp(&a.taken_at).then(a.path.cmp(&b.path)));
        }
        out
    }

    pub fn album_names(&self) -> Vec<&str> {
        self.albums.keys().map(|s| s.as_str()).collect()
    }

    pub fn album(&self, name: &str) -> Option<&Album> {
        self.albums.get(name)
    }

    /// Make an album. Returns false when the name is already taken or blank.
    ///
    /// Refusing a duplicate name rather than silently merging: two albums
    /// called "Holiday" is confusing, and quietly folding a new one into an
    /// existing one loses the user's intent without telling them.
    pub fn create_album(&mut self, name: &str) -> bool {
        let name = name.trim();
        if name.is_empty() || self.albums.contains_key(name) {
            return false;
        }
        self.albums.insert(name.to_string(), Album { name: name.to_string(), ..Default::default() });
        true
    }

    /// Delete an album. The PHOTOS ARE NOT TOUCHED — see the module note.
    pub fn delete_album(&mut self, name: &str) -> bool {
        self.albums.remove(name).is_some()
    }

    /// Add paths to an album, ignoring any that are not in the library.
    ///
    /// Returns how many were added. Silently skipping unknown paths rather
    /// than failing the whole call: a selection can go stale between the click
    /// and the commit if the daemon removed a file underneath, and losing the
    /// other nineteen photos to that is worse than losing the one.
    pub fn add_to_album(&mut self, name: &str, paths: &[String]) -> usize {
        let known: BTreeSet<&str> = self.items.iter().map(|i| i.path.as_str()).collect();
        let Some(a) = self.albums.get_mut(name) else { return 0 };
        let mut n = 0;
        for p in paths {
            if known.contains(p.as_str()) && a.items.insert(p.clone()) {
                n += 1;
            }
        }
        n
    }

    pub fn remove_from_album(&mut self, name: &str, path: &str) -> bool {
        self.albums.get_mut(name).is_some_and(|a| a.items.remove(path))
    }

    /// The items of an album, in camera-roll order.
    ///
    /// Resolved against the library every time rather than stored: a path in
    /// an album whose file has gone is simply absent from the view, and no
    /// cleanup pass is needed to make that true.
    pub fn album_items(&self, name: &str) -> Vec<&Item> {
        let Some(a) = self.albums.get(name) else { return Vec::new() };
        let mut v: Vec<&Item> = self.items.iter().filter(|i| a.items.contains(&i.path)).collect();
        v.sort_by(|x, y| y.taken_at.cmp(&x.taken_at).then(x.path.cmp(&y.path)));
        v
    }

    /// Which albums contain this path.
    pub fn albums_containing(&self, path: &str) -> Vec<&str> {
        self.albums.values().filter(|a| a.items.contains(path)).map(|a| a.name.as_str()).collect()
    }
}

/// What a share hands over.
///
/// A list of PATHS and an explicit count, not the bytes: photosync's transport
/// already knows how to move content-addressed blocks, and re-packing the
/// photos here would be a second copy of that with its own bugs.
///
/// The count is carried separately from the list on purpose. A share dialog
/// that says "share 12 photos" and hands over 11 because one went missing
/// between the click and the send is the kind of quiet discrepancy nobody
/// notices until the other person asks where the last one is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Share {
    pub paths: Vec<String>,
    pub requested: usize,
    /// Paths asked for that the library no longer has.
    pub missing: Vec<String>,
}

impl Share {
    pub fn is_complete(&self) -> bool {
        self.missing.is_empty()
    }
}

/// Prepare a share of `paths` from `lib`.
pub fn prepare_share(lib: &Library, paths: &[String]) -> Share {
    let known: BTreeSet<&str> = lib.items.iter().map(|i| i.path.as_str()).collect();
    let mut ok = Vec::new();
    let mut missing = Vec::new();
    for p in paths {
        if known.contains(p.as_str()) {
            ok.push(p.clone());
        } else {
            missing.push(p.clone());
        }
    }
    Share { paths: ok, requested: paths.len(), missing }
}

// ── persistence ─────────────────────────────────────────────────────────────
//
// The model above is pure; this is the only part that touches SQLite, and it is
// small on purpose. Everything interesting — grouping, membership, the rules
// about deletion and sharing — is testable without a database, and this layer
// is a query and two tables.

/// Create the album tables if they are not there.
///
/// Separate from `create_schema` in lib.rs rather than folded into it: albums
/// are a VIEW concern and the dedup store predates them by a long way. A
/// daemon that never opens a gallery should not have to carry the migration,
/// and `IF NOT EXISTS` makes calling this from either side harmless.
pub fn create_album_schema(conn: &rusqlite::Connection) -> rusqlite::Result<()> {
    conn.execute(
        "CREATE TABLE IF NOT EXISTS albums (
            name TEXT PRIMARY KEY,
            created_at INTEGER NOT NULL
        )",
        [],
    )?;
    // ON DELETE CASCADE on the album, and deliberately NO foreign key to
    // `files`. The module's rule is that membership is a reference: a photo
    // whose row goes away must leave the membership record intact, so that
    // restoring the file restores the album entry. A foreign key here would
    // silently delete memberships during a garbage collect.
    conn.execute(
        "CREATE TABLE IF NOT EXISTS album_items (
            album TEXT NOT NULL,
            path TEXT NOT NULL,
            PRIMARY KEY (album, path),
            FOREIGN KEY (album) REFERENCES albums(name) ON DELETE CASCADE
        )",
        [],
    )?;
    Ok(())
}

/// Read the library and its albums out of an open database.
///
/// `created_at` is the taken-at date. The `files` row also has `modified_at`,
/// which is when the file last CHANGED on disk — copying a photo between
/// devices updates it, so grouping by it would file every photo under the day
/// you synced rather than the day you took it. That is the single most
/// wrong-looking thing a gallery can do, so the choice is worth naming here
/// rather than being a column name someone picks later.
pub fn load(conn: &rusqlite::Connection) -> rusqlite::Result<Library> {
    let mut st = conn.prepare(
        "SELECT path, created_at, thumbnail_hash, size, metadata_json FROM files",
    )?;
    let items = st
        .query_map([], |r| {
            Ok(Item {
                path: r.get(0)?,
                taken_at: r.get(1)?,
                thumbnail: r.get(2)?,
                bytes: r.get::<_, i64>(3)?.max(0) as u64,
                metadata_json: r.get(4)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    let mut lib = Library::new(items);
    load_albums(conn, &mut lib)?;
    Ok(lib)
}

/// Replace `lib`'s albums with the ones stored in `conn`. Returns whether the
/// database had album tables at all.
///
/// Separate from `load` because the albums need not live in the same database
/// as the files: the daemon's store is typically the service's (root-owned,
/// opened read-only by a viewer), while albums are the viewing user's, kept
/// wherever that user can write (the gallery's own albums file).
///
/// Albums may not exist yet on a database written before this feature; an
/// absent table is no albums, not an error — and `lib` is left as it was, so a
/// caller can tell "no albums file yet" from "an albums file with none".
pub fn load_albums(conn: &rusqlite::Connection, lib: &mut Library) -> rusqlite::Result<bool> {
    let names = match conn.prepare("SELECT name FROM albums ORDER BY name").and_then(|mut s| {
        s.query_map([], |r| r.get::<_, String>(0))?.collect::<rusqlite::Result<Vec<_>>>()
    }) {
        Ok(n) => n,
        Err(_) => return Ok(false),
    };
    lib.albums.clear();
    for n in names {
        lib.albums.insert(n.clone(), Album { name: n, ..Default::default() });
    }
    let mut st = conn.prepare("SELECT album, path FROM album_items")?;
    let rows = st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
    for row in rows {
        let (album, path) = row?;
        if let Some(a) = lib.albums.get_mut(&album) {
            a.items.insert(path);
        }
    }
    Ok(true)
}

/// Write the albums back. The `files` table is NOT touched.
///
/// Replace-in-place rather than diffing: the album set is tens of rows, a diff
/// is more code than the write, and the failure mode of a wrong diff (a
/// membership silently dropped) is much worse than the cost of rewriting.
pub fn save_albums(conn: &rusqlite::Connection, lib: &Library) -> rusqlite::Result<()> {
    // One transaction: the delete-then-reinsert below must never be seen, or
    // survive a crash, half done — that would be every album emptied.
    let tx = conn.unchecked_transaction()?;
    save_albums_in(&tx, lib)?;
    tx.commit()
}

fn save_albums_in(conn: &rusqlite::Connection, lib: &Library) -> rusqlite::Result<()> {
    create_album_schema(conn)?;
    // REDUNDANT while foreign keys are on — `DELETE FROM albums` below already
    // cascades, which is why breaking this line fails no test. Kept, and
    // `foreign_keys_are_on_and_the_cascade_is_what_clears_memberships` pins the
    // assumption: if a future rusqlite stops enabling them, that test fails
    // first and this line becomes the only thing keeping stale memberships out.
    conn.execute("DELETE FROM album_items", [])?;
    conn.execute("DELETE FROM albums", [])?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    for a in lib.albums.values() {
        conn.execute("INSERT INTO albums (name, created_at) VALUES (?1, ?2)", rusqlite::params![a.name, now])?;
        for p in &a.items {
            conn.execute("INSERT INTO album_items (album, path) VALUES (?1, ?2)", rusqlite::params![a.name, p])?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(path: &str, taken_at: i64) -> Item {
        Item { path: path.into(), taken_at, thumbnail: None, bytes: 100, metadata_json: None }
    }

    // 2026-09-03 00:00:00 UTC
    const SEP3: i64 = 1_788_393_600;
    const DAY: i64 = 86_400;

    fn lib() -> Library {
        Library::new(vec![
            item("/p/a.jpg", SEP3 + 3600),
            item("/p/b.jpg", SEP3 + 7200),
            item("/p/c.jpg", SEP3 - DAY),
            item("/p/d.jpg", SEP3 - 2 * DAY),
        ])
    }

    fn memdb() -> rusqlite::Connection {
        let c = rusqlite::Connection::open_in_memory().unwrap();
        c.execute(
            "CREATE TABLE files (path TEXT PRIMARY KEY, size INTEGER NOT NULL,
             created_at INTEGER NOT NULL, modified_at INTEGER NOT NULL,
             thumbnail_hash TEXT, metadata_json TEXT)",
            [],
        ).unwrap();
        for (p, t) in [("/p/a.jpg", SEP3 + 3600), ("/p/b.jpg", SEP3 - DAY)] {
            c.execute(
                "INSERT INTO files (path, size, created_at, modified_at, thumbnail_hash, metadata_json)
                 VALUES (?1, 100, ?2, ?3, 'thumb', NULL)",
                rusqlite::params![p, t, SEP3 + 99_999],
            ).unwrap();
        }
        c
    }

    #[test]
    fn a_library_loads_from_the_files_table_and_dates_by_created_at() {
        // NOT modified_at. Copying a photo between devices bumps it, so
        // grouping by it files every photo under the day you synced — the most
        // wrong-looking thing a gallery can do. The fixture sets modified_at
        // far away from created_at precisely so a swap fails here.
        let c = memdb();
        let lib = load(&c).unwrap();
        assert_eq!(lib.len(), 2);
        let grouped = lib.by_day();
        let days: Vec<&str> = grouped.iter().map(|(d, _)| d.as_str()).collect();
        assert_eq!(days, vec!["2026-09-03", "2026-09-02"], "dated by the wrong column");
        assert_eq!(grouped[0].1[0].thumbnail.as_deref(), Some("thumb"));
    }

    #[test]
    fn albums_survive_a_save_and_load() {
        let c = memdb();
        let mut lib = load(&c).unwrap();
        lib.create_album("Trip");
        lib.add_to_album("Trip", &["/p/a.jpg".into()]);
        save_albums(&c, &lib).unwrap();

        let back = load(&c).unwrap();
        assert_eq!(back.album_names(), vec!["Trip"]);
        assert_eq!(back.album_items("Trip").len(), 1);
        // And an album emptied in memory really empties on disk — the failure
        // a replace-in-place write exists to avoid getting wrong.
        let mut back = back;
        back.remove_from_album("Trip", "/p/a.jpg");
        save_albums(&c, &back).unwrap();
        assert_eq!(load(&c).unwrap().album_items("Trip").len(), 0);
        // And the MEMBERSHIP ROW is gone, not merely the view of it. Asserting
        // through album_items alone passes when the stale row survives and the
        // file happens to be absent, which is not the same thing — found by
        // breaking the DELETE and watching nothing fail.
        assert!(
            load(&c).unwrap().album("Trip").unwrap().items.is_empty(),
            "a removed membership was left behind in album_items"
        );
    }

    #[test]
    fn albums_kept_in_another_database_load_onto_the_library() {
        // The gallery keeps albums in the viewing user's own file, apart from
        // the service's (read-only) store: the two must combine.
        let files = memdb();
        let mut lib = load(&files).unwrap();
        let albums = rusqlite::Connection::open_in_memory().unwrap();
        assert!(!load_albums(&albums, &mut lib).unwrap(), "no tables yet: says so");
        let mut other = lib.clone();
        other.create_album("Trip");
        other.add_to_album("Trip", &["/p/a.jpg".to_string()]);
        save_albums(&albums, &other).unwrap();
        assert!(load_albums(&albums, &mut lib).unwrap());
        assert_eq!(lib.album_names(), vec!["Trip"]);
        assert_eq!(lib.album_items("Trip").len(), 1);
        // An album file with NO albums replaces, rather than merges into, what
        // was loaded: deleting the last album must stick.
        save_albums(&albums, &Library::new(vec![])).unwrap();
        assert!(load_albums(&albums, &mut lib).unwrap());
        assert!(lib.album_names().is_empty());
    }

    #[test]
    fn a_database_with_no_album_tables_loads_as_an_empty_gallery() {
        // Every existing photosync database is one of these. Treating a
        // missing table as an error would make the gallery refuse to open on
        // exactly the libraries it is being added for.
        let c = memdb();
        let lib = load(&c).unwrap();
        assert_eq!(lib.len(), 2, "the photos did not load");
        assert!(lib.album_names().is_empty());
    }

    #[test]
    fn a_membership_outlives_the_file_row() {
        // No foreign key to `files`, on purpose: the module's rule is that
        // membership is a reference. A garbage collect that removes a file row
        // must not silently delete the album entry, so that restoring the file
        // restores the entry.
        let c = memdb();
        let mut lib = load(&c).unwrap();
        lib.create_album("X");
        lib.add_to_album("X", &["/p/a.jpg".into()]);
        save_albums(&c, &lib).unwrap();
        c.execute("DELETE FROM files WHERE path = '/p/a.jpg'", []).unwrap();

        let back = load(&c).unwrap();
        assert_eq!(back.album_items("X").len(), 0, "a vanished file still shows in the view");
        assert!(back.album("X").unwrap().items.contains("/p/a.jpg"),
                "the membership record was destroyed with the file");
    }

#[test]
fn foreign_keys_are_on_and_the_cascade_is_what_clears_memberships() {
    // Written because breaking `DELETE FROM album_items` in save_albums
    // failed NO test twice, even after sharpening the assertion. The reason
    // is that rusqlite enables foreign keys, so `DELETE FROM albums` already
    // cascades — the explicit delete is redundant, not load-bearing.
    //
    // That is worth pinning rather than deleting the line: if a future
    // rusqlite (or an explicit `PRAGMA foreign_keys = OFF`) changes the
    // default, the cascade stops and the redundant delete becomes the only
    // thing keeping stale memberships out. This test fails first and says why.
    let c = rusqlite::Connection::open_in_memory().unwrap();
    create_album_schema(&c).unwrap();
    c.execute("INSERT INTO albums (name, created_at) VALUES ('A', 0)", []).unwrap();
    c.execute("INSERT INTO album_items (album, path) VALUES ('A', '/p/x.jpg')", []).unwrap();
    c.execute("DELETE FROM albums", []).unwrap();
    let left: i64 = c.query_row("SELECT COUNT(*) FROM album_items", [], |r| r.get(0)).unwrap();
    let fk: i64 = c.query_row("PRAGMA foreign_keys", [], |r| r.get(0)).unwrap();
    assert_eq!(fk, 1, "foreign keys are OFF — the explicit DELETE in save_albums is now the only guard");
    assert_eq!(left, 0, "the cascade did not fire");
}

    #[test]
    fn the_civil_date_is_right_including_before_1970_and_on_leap_days() {
        // The naive version of this truncates toward zero and puts a 1969 photo
        // on the wrong day. A scan of an old print or a camera with a dead
        // clock produces exactly those timestamps.
        assert_eq!(civil_from_unix(0), (1970, 1, 1));
        assert_eq!(civil_from_unix(-1), (1969, 12, 31));
        assert_eq!(civil_from_unix(-86_400), (1969, 12, 31));
        assert_eq!(civil_from_unix(-86_401), (1969, 12, 30));
        // 2024-02-29, a leap day in a leap century-rule year.
        assert_eq!(civil_from_unix(1_709_164_800), (2024, 2, 29));
        // 2000-02-29 — divisible by 400, so a leap year where the naive
        // "divisible by 100 is not" rule gets it wrong.
        assert_eq!(civil_from_unix(951_782_400), (2000, 2, 29));
        // 1900 was NOT a leap year; 1900-03-01.
        assert_eq!(civil_from_unix(-2_203_891_200), (1900, 3, 1));
    }

    #[test]
    fn the_camera_roll_is_newest_first_within_newest_day_first() {
        // Two orders, two reasons. Asserting only the day order would pass on
        // a within-day order that reverses direction at every heading.
        let l = lib();
        let days = l.by_day();
        let names: Vec<&str> = days.iter().map(|(d, _)| d.as_str()).collect();
        assert_eq!(names, vec!["2026-09-03", "2026-09-02", "2026-09-01"]);
        let first_day: Vec<&str> = days[0].1.iter().map(|i| i.path.as_str()).collect();
        assert_eq!(first_day, vec!["/p/b.jpg", "/p/a.jpg"], "within a day, newest is not first");
    }

    #[test]
    fn an_album_is_a_reference_and_deleting_it_keeps_the_photos() {
        // The rule the whole module is shaped by. A gallery that can lose
        // pictures by tidying is not one anyone should keep photos in.
        let mut l = lib();
        assert!(l.create_album("Trip"));
        assert_eq!(l.add_to_album("Trip", &["/p/a.jpg".into(), "/p/c.jpg".into()]), 2);
        assert_eq!(l.album_items("Trip").len(), 2);
        assert!(l.delete_album("Trip"));
        assert_eq!(l.len(), 4, "deleting an album removed photos");
        assert!(l.album("Trip").is_none());
    }

    #[test]
    fn one_photo_can_be_in_many_albums_and_adding_twice_is_a_no_op() {
        // Both follow from membership being a reference: overlap is free, and
        // re-adding a photo from an overlapping selection must not produce a
        // duplicate tile.
        let mut l = lib();
        l.create_album("A");
        l.create_album("B");
        l.add_to_album("A", &["/p/a.jpg".into()]);
        l.add_to_album("B", &["/p/a.jpg".into()]);
        assert_eq!(l.albums_containing("/p/a.jpg"), vec!["A", "B"]);
        assert_eq!(l.add_to_album("A", &["/p/a.jpg".into()]), 0, "a duplicate was added");
        assert_eq!(l.album_items("A").len(), 1);
    }

    #[test]
    fn a_duplicate_album_name_is_refused_rather_than_merged() {
        let mut l = lib();
        assert!(l.create_album("Holiday"));
        assert!(!l.create_album("Holiday"), "a second Holiday was created");
        assert!(!l.create_album("  Holiday  "), "whitespace made a second one");
        assert!(!l.create_album("   "), "a blank name was accepted");
        assert_eq!(l.album_names(), vec!["Holiday"]);
    }

    #[test]
    fn adding_a_stale_selection_keeps_the_photos_that_still_exist() {
        // A selection can go stale between the click and the commit. Losing
        // the other nineteen to one missing file is the wrong trade.
        let mut l = lib();
        l.create_album("Mixed");
        let n = l.add_to_album("Mixed", &["/p/a.jpg".into(), "/p/gone.jpg".into(), "/p/b.jpg".into()]);
        assert_eq!(n, 2);
        assert_eq!(l.album_items("Mixed").len(), 2);
    }

    #[test]
    fn a_share_reports_what_it_could_not_include() {
        // "share 12 photos" that hands over 11 is a discrepancy nobody notices
        // until the other person asks where the last one is. Both directions:
        // a complete share must say so.
        let l = lib();
        let s = prepare_share(&l, &["/p/a.jpg".into(), "/p/nope.jpg".into()]);
        assert_eq!(s.requested, 2);
        assert_eq!(s.paths, vec!["/p/a.jpg".to_string()]);
        assert_eq!(s.missing, vec!["/p/nope.jpg".to_string()]);
        assert!(!s.is_complete());

        let good = prepare_share(&l, &["/p/a.jpg".into(), "/p/b.jpg".into()]);
        assert!(good.is_complete());
        assert_eq!(good.paths.len(), good.requested);
    }

    #[test]
    fn an_album_item_whose_file_vanished_is_simply_absent() {
        // Membership resolves against the library every time, so no cleanup
        // pass is needed to keep the view honest.
        let mut l = lib();
        l.create_album("X");
        l.add_to_album("X", &["/p/a.jpg".into(), "/p/b.jpg".into()]);
        l.items.retain(|i| i.path != "/p/a.jpg");
        assert_eq!(l.album_items("X").len(), 1);
        // And the membership record is untouched, so restoring the file
        // restores the album entry rather than needing it re-added.
        assert!(l.album("X").unwrap().items.contains("/p/a.jpg"));
    }
}
