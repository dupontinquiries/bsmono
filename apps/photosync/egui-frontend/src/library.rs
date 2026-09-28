//! library — what the app knows about the photos on disk.
//!
//! One [`Photo`] per file: where it is, when it was taken, how big it is,
//! the core's [`photosync::Entry`] (hash, ascii, tags, fingerprint), and
//! the thumbnail once the scanner has decoded it. The [`Library`] owns the
//! list and answers the questions the pages ask — group by day, filter by
//! a search, find duplicates, find photos like this one — as pure
//! functions over that list, so every one of them is tested without a
//! decoder, a disk or a window.
//!
//! ## The index
//!
//! Hashing every photo on every launch is the wrong cost model for a
//! phone: a 2,000-photo library is a minute of decoding for numbers that
//! did not change. [`Index`] persists what the scanner learned, keyed by
//! path with the file's size and modification time, and the scanner
//! rescans only files that are new, changed, or were scanned by an older
//! `SCAN_VERSION` (the fotema idea the core adopted). Thumbnails are not
//! persisted: they are cheap to make from the file and expensive to keep.
//!
//! Written tags live in the index too. They are the user's work and the
//! only part of an entry that cannot be recomputed.

use photosync::{fingerprint::Fingerprint, Entry};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// A thumbnail's longest side, in pixels. 256 fits three across a phone
/// at 2× and still reads on a desktop grid.
pub const THUMB_PX: u32 = 256;

/// How much of the ascii sketch two duplicates must share (cells within
/// one ramp step). A re-encode or a resize scores near 1.0; a gradient
/// against a flat colour, both with a zero dHash, scores well under.
pub const DUPLICATE_TONE: f32 = 0.8;

/// One photo on disk.
#[derive(Debug, Clone, PartialEq)]
pub struct Photo {
    pub path: String,
    /// Unix seconds, from the file's modification time — the closest thing
    /// to "taken" without EXIF, which the decoder feature set here does
    /// not read. Stated as such in the UI ("modified"), not as "taken".
    pub at: i64,
    pub size: u64,
    pub w: u32,
    pub h: u32,
    pub entry: Entry,
    /// Decoded thumbnail, RGBA, `tw * th * 4` bytes. Absent until the
    /// scanner delivers it; the grid draws a placeholder.
    pub thumb: Option<Thumb>,
}

/// RGBA pixels for a thumbnail.
#[derive(Debug, Clone, PartialEq)]
pub struct Thumb {
    pub w: usize,
    pub h: usize,
    pub rgba: Vec<u8>,
}

impl Photo {
    /// The file name without its directory.
    pub fn name(&self) -> &str {
        self.path.rsplit('/').next().unwrap_or(&self.path)
    }

    /// The written (not derived) tags.
    pub fn written_tags(&self) -> Vec<&str> {
        self.entry.tags.iter().filter(|t| !photosync::cv::is_auto(t)).map(String::as_str).collect()
    }

    /// The derived tags without their prefix, for display.
    pub fn auto_tags(&self) -> Vec<&str> {
        self.entry.tags.iter().filter(|t| photosync::cv::is_auto(t))
            .map(|t| t.trim_start_matches(photosync::cv::AUTO_PREFIX)).collect()
    }

    /// The day this photo belongs to, as days since the epoch in local
    /// time is not knowable without a zone; UTC days keep the grouping
    /// stable across devices, which matters more than the midnight line.
    pub fn day(&self) -> i64 { self.at.div_euclid(86_400) }

    /// "2026-09-27", from the UTC day.
    pub fn day_label(&self) -> String { day_label(self.day()) }

    /// Does a search hit this photo? Case-insensitive, over the file name
    /// and every tag (written or derived, with or without the prefix).
    pub fn matches(&self, query: &str) -> bool {
        let q = query.trim().to_lowercase();
        if q.is_empty() { return true; }
        q.split_whitespace().all(|word| {
            self.name().to_lowercase().contains(word)
                || self.entry.tags.iter().any(|t| t.contains(word))
        })
    }
}

/// "YYYY-MM-DD" for a UTC day number. Civil-from-days, so no chrono here.
pub fn day_label(day: i64) -> String {
    let z = day + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}")
}

/// A day's worth of photos, newest first within the day.
#[derive(Debug, Clone, PartialEq)]
pub struct DayGroup {
    pub day: i64,
    pub label: String,
    /// Indices into `Library::photos`.
    pub photos: Vec<usize>,
}

/// The photos, and the questions the pages ask of them.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Library {
    pub photos: Vec<Photo>,
    /// Where the photos came from, for the settings page and the index.
    pub root: String,
}

impl Library {
    pub fn len(&self) -> usize { self.photos.len() }
    pub fn is_empty(&self) -> bool { self.photos.is_empty() }

    pub fn find(&self, path: &str) -> Option<usize> {
        self.photos.iter().position(|p| p.path == path)
    }

    /// Add or replace a photo by path. Returns its index.
    pub fn upsert(&mut self, photo: Photo) -> usize {
        match self.find(&photo.path) {
            Some(i) => { self.photos[i] = photo; i }
            None => { self.photos.push(photo); self.photos.len() - 1 }
        }
    }

    /// Forget a photo (deleted from disk, or by the dedup page).
    pub fn remove(&mut self, path: &str) -> Option<Photo> {
        let i = self.find(path)?;
        Some(self.photos.remove(i))
    }

    /// Newest first, then by path so ties are stable.
    pub fn sorted(&self) -> Vec<usize> {
        let mut ix: Vec<usize> = (0..self.photos.len()).collect();
        ix.sort_by(|a, b| {
            let (pa, pb) = (&self.photos[*a], &self.photos[*b]);
            pb.at.cmp(&pa.at).then_with(|| pa.path.cmp(&pb.path))
        });
        ix
    }

    /// The grid's rows: days, newest first, each holding the photos that
    /// match `query`. Days with no match are left out rather than shown
    /// empty, so a search for "beach" is a list of beach days.
    pub fn days(&self, query: &str) -> Vec<DayGroup> {
        let mut out: Vec<DayGroup> = Vec::new();
        for i in self.sorted() {
            let p = &self.photos[i];
            if !p.matches(query) { continue; }
            match out.last_mut() {
                Some(g) if g.day == p.day() => g.photos.push(i),
                _ => out.push(DayGroup { day: p.day(), label: p.day_label(), photos: vec![i] }),
            }
        }
        out
    }

    /// Every written tag in the library with its count, most used first.
    pub fn tag_counts(&self) -> Vec<(String, usize)> {
        let mut m: HashMap<&str, usize> = HashMap::new();
        for p in &self.photos {
            for t in p.written_tags() { *m.entry(t).or_default() += 1; }
        }
        let mut v: Vec<(String, usize)> = m.into_iter().map(|(t, n)| (t.to_string(), n)).collect();
        v.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        v
    }

    /// Near-duplicate groups, as indices — largest file first within a
    /// group, because "keep the biggest" is the usual answer and the page
    /// offers it as the default.
    ///
    /// The core's hash-only rule, then one more check: members must also
    /// agree in tone (`ascii_similarity` ≥ [`DUPLICATE_TONE`]). dHash
    /// encodes only the SIGN of each neighbour difference, so a smooth
    /// gradient and a flat colour both hash to zero and the core groups
    /// them — seen on the first real library, where a sky gradient was
    /// offered as a duplicate of a dark wall. Tone agreement is cheap,
    /// keeps every re-encode and resize together (their sketches match
    /// cell for cell), and errs toward a missed duplicate, which is the
    /// recoverable mistake.
    pub fn duplicate_groups(&self) -> Vec<Vec<usize>> {
        let entries: Vec<Entry> = self.photos.iter().map(|p| p.entry.clone()).collect();
        let mut out = Vec::new();
        for paths in photosync::duplicate_groups(&entries, photosync::DUPLICATE_BITS) {
            let mut members: Vec<usize> = paths.iter().filter_map(|p| self.find(p)).collect();
            // Split by tone: each member joins the first sub-group whose
            // leader it agrees with, else starts its own.
            let mut subs: Vec<Vec<usize>> = Vec::new();
            members.sort_by(|a, b| self.photos[*b].size.cmp(&self.photos[*a].size)
                .then_with(|| self.photos[*a].path.cmp(&self.photos[*b].path)));
            for i in members {
                let place = subs.iter().position(|g| photosync::ascii_similarity(
                    &self.photos[g[0]].entry.ascii, &self.photos[i].entry.ascii) >= DUPLICATE_TONE);
                match place {
                    Some(k) => subs[k].push(i),
                    None => subs.push(vec![i]),
                }
            }
            out.extend(subs.into_iter().filter(|g| g.len() > 1));
        }
        out.sort();
        out
    }

    /// Photos like this one, best first, with their scores.
    pub fn like(&self, i: usize, limit: usize) -> Vec<(usize, f32)> {
        let Some(q) = self.photos.get(i) else { return Vec::new() };
        let entries: Vec<Entry> = self.photos.iter().map(|p| p.entry.clone()).collect();
        photosync::best_matches(&q.entry, &entries, photosync::Weights::default(), limit)
            .into_iter()
            .filter_map(|(e, s)| self.find(&e.path).map(|i| (i, s)))
            .collect()
    }

    /// Add a written tag to a photo. Returns false when it was there.
    pub fn add_tag(&mut self, i: usize, tag: &str) -> bool {
        let t = tag.trim().to_lowercase();
        if t.is_empty() || t.starts_with(photosync::cv::AUTO_PREFIX) { return false; }
        let Some(p) = self.photos.get_mut(i) else { return false };
        if p.entry.tags.contains(&t) { return false; }
        p.entry.tags.push(t);
        p.entry.tags.sort();
        true
    }

    /// Remove a written tag. Derived tags cannot be removed: they are
    /// facts about the pixels, and the next scan would put them back.
    pub fn remove_tag(&mut self, i: usize, tag: &str) -> bool {
        if photosync::cv::is_auto(tag) { return false; }
        let Some(p) = self.photos.get_mut(i) else { return false };
        let before = p.entry.tags.len();
        p.entry.tags.retain(|t| t != tag);
        p.entry.tags.len() != before
    }

    /// Bytes across the library, for the settings page.
    pub fn total_bytes(&self) -> u64 { self.photos.iter().map(|p| p.size).sum() }
}

/// What the index file keeps per photo.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IndexEntry {
    pub path: String,
    pub at: i64,
    pub size: u64,
    pub w: u32,
    pub h: u32,
    pub dhash: u64,
    pub ascii: String,
    /// Every tag, written and derived, as the core keeps them.
    pub tags: Vec<String>,
    pub blur: Vec<u8>,
    pub edge: Vec<u8>,
    pub scan_version: u32,
}

/// The persisted index.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Index {
    pub root: String,
    pub entries: Vec<IndexEntry>,
}

impl Index {
    pub fn from_library(lib: &Library) -> Index {
        Index {
            root: lib.root.clone(),
            entries: lib.photos.iter().map(|p| IndexEntry {
                path: p.path.clone(), at: p.at, size: p.size, w: p.w, h: p.h,
                dhash: p.entry.dhash, ascii: p.entry.ascii.clone(), tags: p.entry.tags.clone(),
                blur: p.entry.fingerprint.blur.to_vec(), edge: p.entry.fingerprint.edge.to_vec(),
                scan_version: p.entry.scan_version,
            }).collect(),
        }
    }

    /// A library from the index, thumbnails absent.
    pub fn to_library(&self) -> Library {
        Library {
            root: self.root.clone(),
            photos: self.entries.iter().filter_map(|e| {
                let mut blur = [0u8; photosync::fingerprint::BLUR_HASH_LEN];
                let mut edge = [0u8; photosync::fingerprint::EDGE_HASH_LEN];
                if e.blur.len() != blur.len() || e.edge.len() != edge.len() { return None; }
                blur.copy_from_slice(&e.blur);
                edge.copy_from_slice(&e.edge);
                Some(Photo {
                    path: e.path.clone(), at: e.at, size: e.size, w: e.w, h: e.h,
                    entry: Entry {
                        path: e.path.clone(), dhash: e.dhash, ascii: e.ascii.clone(),
                        tags: e.tags.clone(), fingerprint: Fingerprint { blur, edge },
                        scan_version: e.scan_version,
                    },
                    thumb: None,
                })
            }).collect(),
        }
    }

    /// Is the indexed entry for `path` still good for a file of this size
    /// and mtime? Stale scan versions are not.
    pub fn is_current(&self, path: &str, size: u64, at: i64) -> bool {
        self.entries.iter().any(|e| e.path == path && e.size == size && e.at == at
            && e.scan_version == photosync::SCAN_VERSION)
    }

    pub fn encode(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_default()
    }

    pub fn decode(s: &str) -> Option<Index> {
        serde_json::from_str(s).ok()
    }

    /// The index file for a library root, under the app's XDG data dir.
    /// Named by a hash of the root so two libraries do not share one.
    pub fn path_for(root: &str) -> std::path::PathBuf {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for b in root.bytes() { h ^= b as u64; h = h.wrapping_mul(0x0100_0000_01b3); }
        config_kit::data_dir("photosync").join(format!("index-{h:016x}.json"))
    }

    pub fn load(root: &str) -> Option<Index> {
        let text = std::fs::read_to_string(Self::path_for(root)).ok()?;
        let idx = Self::decode(&text)?;
        if idx.root == root { Some(idx) } else { None }
    }

    pub fn save(&self) -> std::io::Result<()> {
        let p = Self::path_for(&self.root);
        if let Some(dir) = p.parent() { std::fs::create_dir_all(dir)?; }
        std::fs::write(p, self.encode())
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use photosync::Gray;

    /// A photo from a synthetic image, so the entry is real.
    pub(crate) fn photo(path: &str, at: i64, px: &[u8], w: usize, h: usize, tags: &[&str]) -> Photo {
        let g = Gray::new(w, h, px).unwrap();
        Photo {
            path: path.into(), at, size: px.len() as u64, w: w as u32, h: h as u32,
            entry: Entry::scan(path, &g, tags), thumb: None,
        }
    }

    /// A textured synthetic picture. Gradients in both directions with a
    /// seeded twist, so different seeds hash differently — a plain ramp
    /// hashes to zero, and every plain ramp is then a "duplicate" of
    /// every other, which is not what a test of duplicates wants.
    pub(crate) fn ramp(w: usize, h: usize, seed: u8) -> Vec<u8> {
        (0..w * h).map(|i| {
            let (x, y) = ((i % w) as u32, (i / w) as u32);
            let s = seed as u32;
            (((x * (7 + s)) ^ (y * (13 + 3 * s)) ^ (x * y / (1 + s))) % 251) as u8
        }).collect()
    }

    pub(crate) fn sample() -> Library {
        let mut lib = Library { root: "/pics".into(), ..Default::default() };
        let day = 86_400;
        lib.upsert(photo("/pics/beach.jpg", 20 * day + 100, &ramp(32, 32, 0), 32, 32, &["beach", "family"]));
        lib.upsert(photo("/pics/beach-copy.jpg", 20 * day + 200, &ramp(32, 32, 0), 32, 32, &[]));
        // Dark and bright textures with different structure, so neither is
        // a hash-duplicate of anything.
        let dark: Vec<u8> = ramp(32, 32, 3).iter().map(|v| v % 24).collect();
        let bright: Vec<u8> = ramp(32, 32, 5).iter().map(|v| 225 + v % 30).collect();
        lib.upsert(photo("/pics/dark.jpg", 19 * day + 50, &dark, 32, 32, &["night"]));
        lib.upsert(photo("/pics/bright.png", 18 * day + 50, &bright, 32, 32, &[]));
        lib
    }

    #[test]
    fn days_are_newest_first_and_hold_their_photos_newest_first() {
        let lib = sample();
        let days = lib.days("");
        assert_eq!(days.len(), 3);
        assert_eq!(days[0].label, "1970-01-21");
        assert_eq!(days[0].photos.len(), 2);
        assert_eq!(lib.photos[days[0].photos[0]].name(), "beach-copy.jpg", "newest in the day first");
        assert!(days[0].day > days[1].day && days[1].day > days[2].day);
    }

    #[test]
    fn a_search_narrows_to_the_days_that_match() {
        let lib = sample();
        let days = lib.days("night");
        assert_eq!(days.len(), 1);
        assert_eq!(lib.photos[days[0].photos[0]].name(), "dark.jpg");
        // by file name, case-insensitively, and by a derived tag
        assert_eq!(lib.days("BEACH").iter().map(|d| d.photos.len()).sum::<usize>(), 2);
        assert!(!lib.days("dark").is_empty(), "the derived tag auto:dark should match 'dark'");
        assert!(lib.days("nothing-like-this").is_empty());
    }

    #[test]
    fn the_civil_date_is_right_at_the_epoch_and_around_leap_years() {
        assert_eq!(day_label(0), "1970-01-01");
        assert_eq!(day_label(19_723), "2024-01-01");
        assert_eq!(day_label(19_782), "2024-02-29");
        assert_eq!(day_label(20_723), "2026-09-27");
        assert_eq!(day_label(-1), "1969-12-31");
    }

    #[test]
    fn duplicates_group_the_copy_with_the_original_biggest_first() {
        let mut lib = sample();
        lib.photos.iter_mut().find(|p| p.name() == "beach-copy.jpg").unwrap().size = 10;
        let groups = lib.duplicate_groups();
        assert_eq!(groups.len(), 1, "{groups:?}");
        assert_eq!(lib.photos[groups[0][0]].name(), "beach.jpg", "the bigger file leads");
        assert_eq!(groups[0].len(), 2);
    }

    #[test]
    fn a_gradient_and_a_flat_colour_are_not_duplicates_though_their_hashes_agree() {
        // Both dHash to zero — every neighbour comparison is "not greater".
        // The core groups them; the tone check must not.
        let (w, h) = (32, 32);
        let gradient: Vec<u8> = (0..w * h).map(|i| ((i / w) * 255 / h) as u8).collect();
        let flat = vec![32u8; w * h];
        let mut lib = Library::default();
        lib.upsert(photo("/p/sky.png", 1, &gradient, w, h, &[]));
        lib.upsert(photo("/p/wall.jpg", 2, &flat, w, h, &[]));
        assert_eq!(photosync::hamming(lib.photos[0].entry.dhash, lib.photos[1].entry.dhash), 0,
                   "the premise: the core cannot tell them apart");
        assert!(lib.duplicate_groups().is_empty(), "grouped by a zero hash alone");
        // A re-encode of the gradient (a little noise) still pairs with it.
        let noisy: Vec<u8> = gradient.iter().enumerate().map(|(i, v)| if i % 5 == 0 { v.saturating_add(3) } else { *v }).collect();
        lib.upsert(photo("/p/sky-copy.png", 3, &noisy, w, h, &[]));
        let groups = lib.duplicate_groups();
        assert_eq!(groups.len(), 1, "{groups:?}");
        assert_eq!(groups[0].len(), 2);
    }

    #[test]
    fn like_this_excludes_the_photo_itself_and_ranks_its_copy_first() {
        let lib = sample();
        let i = lib.find("/pics/beach.jpg").unwrap();
        let like = lib.like(i, 3);
        assert!(like.iter().all(|(j, _)| *j != i));
        assert_eq!(lib.photos[like[0].0].name(), "beach-copy.jpg");
        assert!(like[0].1 > like[1].1);
    }

    #[test]
    fn tags_can_be_written_and_removed_but_derived_ones_cannot() {
        let mut lib = sample();
        let i = lib.find("/pics/bright.png").unwrap();
        assert!(lib.add_tag(i, " Sunset "));
        assert!(!lib.add_tag(i, "sunset"), "already there");
        assert!(!lib.add_tag(i, "auto:fake"), "the prefix is the scanner's");
        assert_eq!(lib.photos[i].written_tags(), vec!["sunset"]);
        let auto = lib.photos[i].auto_tags();
        assert!(auto.contains(&"bright"), "{auto:?}");
        assert!(!lib.remove_tag(i, "auto:bright"));
        assert!(lib.remove_tag(i, "sunset"));
        assert!(lib.photos[i].written_tags().is_empty());
    }

    #[test]
    fn the_index_round_trips_the_library_without_thumbnails() {
        let lib = sample();
        let idx = Index::from_library(&lib);
        let back = Index::decode(&idx.encode()).unwrap().to_library();
        assert_eq!(back.len(), lib.len());
        for (a, b) in lib.photos.iter().zip(&back.photos) {
            assert_eq!(a.entry, b.entry);
            assert_eq!((a.at, a.size, a.w, a.h), (b.at, b.size, b.w, b.h));
            assert!(b.thumb.is_none());
        }
        assert!(idx.is_current("/pics/beach.jpg", lib.photos[0].size, lib.photos[0].at));
        assert!(!idx.is_current("/pics/beach.jpg", lib.photos[0].size + 1, lib.photos[0].at), "a changed file");
        assert!(!idx.is_current("/pics/new.jpg", 1, 1));
    }

    #[test]
    fn tag_counts_are_most_used_first() {
        let mut lib = sample();
        let i = lib.find("/pics/dark.jpg").unwrap();
        lib.add_tag(i, "family");
        let counts = lib.tag_counts();
        assert_eq!(counts[0], ("family".into(), 2));
    }
}
