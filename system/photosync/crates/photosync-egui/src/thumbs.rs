//! thumbs — real thumbnails for the grid, decoded off the UI thread.
//!
//! The grid painted procedural swatches because no decoder was wired in.
//! This is the headless half: a path goes in, a small RGBA image comes out;
//! the UI (`ui.rs`) only uploads finished images as textures and draws them.
//!
//! - Decoding runs on worker threads, behind the `Decode` trait (the real one
//!   is `ImageDecode`, on the `image` crate photosync already builds; tests
//!   use a fake).
//! - NEWEST REQUEST FIRST. The grid asks for what it paints, which is only the
//!   visible band (`gallery_grid::visible_range`), so the most recent requests
//!   are what is on screen now; while scrolling, older requests are for rows
//!   already gone. Pending requests are a stack, capped (`max_pending`): the
//!   oldest are dropped, not decoded late.
//! - MEMORY ONLY, bounded in bytes, least-recently-used out. No cache on disk
//!   to grow (the gallery's standing rule, ARCHITECTURE.md). Evicted paths are
//!   reported so the UI can free their textures too.
//! - A failure is a state, not a retry loop: a file that does not decode is
//!   asked about every frame while visible, and must not be decoded every frame.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;

/// A decoded thumbnail: RGBA8, `w * h * 4` bytes.
#[derive(Debug, Clone, PartialEq)]
pub struct Thumb {
    pub w: u32,
    pub h: u32,
    pub rgba: Vec<u8>,
}

impl Thumb {
    pub fn bytes(&self) -> usize { self.rgba.len() }
}

/// Turns a file into a thumbnail no larger than `max_side` on its long side.
pub trait Decode: Send + Sync + 'static {
    fn decode(&self, path: &str, max_side: u32) -> Result<Thumb, String>;
}

/// The real decoder: JPEG, PNG, WebP, ... via the `image` crate, scaled with
/// its fast thumbnail filter, aspect kept.
pub struct ImageDecode;

impl Decode for ImageDecode {
    fn decode(&self, path: &str, max_side: u32) -> Result<Thumb, String> {
        let bytes = std::fs::read(path).map_err(|e| format!("{path}: {e}"))?;
        let img = image::load_from_memory(&bytes).map_err(|e| format!("{path}: {e}"))?;
        // Upright: phones store the pixels as the sensor saw them and the turn in
        // EXIF, which image 0.24 does not apply -- so portrait shots came out
        // sideways. Applied to the small image, where turning is cheap.
        // Shrink only: image 0.24's thumbnail() also ENLARGES a smaller picture to
        // fill the box (a 600x400 photo came back 2048x1365 for the viewer) --
        // memory spent on no extra detail.
        let fitted = if img.width().max(img.height()) > max_side { img.thumbnail(max_side, max_side) } else { img };
        let small = orient(fitted, exif_orientation(&bytes)).to_rgba8();
        let (w, h) = small.dimensions();
        Ok(Thumb { w, h, rgba: small.into_raw() })
    }
}

/// Thumbnails the photosync service already made, from its database, instead
/// of decoding every original (a 12 MP JPEG per tile, on a phone).
///
/// The service stores `thumbnails(hash, data)` as JPEG made with image's
/// `thumbnail()`: it does not apply EXIF orientation, and re-encoding drops the
/// EXIF -- so each stored thumbnail keeps the sensor's orientation with nothing
/// saying so. The turn is read from the ORIGINAL file's header (its first
/// 128 KB) and applied here, which is right for every thumbnail already
/// stored. No stored thumbnail (not generated yet): the original is decoded.
pub struct StoreDecode {
    conn: Mutex<rusqlite::Connection>,
    hashes: HashMap<String, String>,
}

impl StoreDecode {
    /// Read-only on the service's database; `hashes` maps a file's path to its
    /// thumbnail hash (from the library's items).
    pub fn open(db: &std::path::Path, hashes: HashMap<String, String>) -> Result<StoreDecode, String> {
        let conn = rusqlite::Connection::open_with_flags(db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(|e| format!("{}: {e}", db.display()))?;
        Ok(StoreDecode { conn: Mutex::new(conn), hashes })
    }

    fn stored(&self, hash: &str) -> Option<Vec<u8>> {
        let conn = self.conn.lock().ok()?;
        conn.query_row("SELECT data FROM thumbnails WHERE hash = ?1", [hash], |r| r.get::<_, Vec<u8>>(0)).ok()
    }
}

/// The first bytes of a file -- enough for its EXIF header.
fn head(path: &str, n: u64) -> Vec<u8> {
    use std::io::Read;
    let mut buf = Vec::new();
    if let Ok(f) = std::fs::File::open(path) {
        let _ = f.take(n).read_to_end(&mut buf);
    }
    buf
}

impl Decode for StoreDecode {
    fn decode(&self, path: &str, max_side: u32) -> Result<Thumb, String> {
        if let Some(bytes) = self.hashes.get(path).and_then(|h| self.stored(h)) {
            if let Ok(img) = image::load_from_memory(&bytes) {
                let fitted = if img.width().max(img.height()) > max_side { img.thumbnail(max_side, max_side) } else { img };
                let small = orient(fitted, exif_orientation(&head(path, 128 * 1024))).to_rgba8();
                let (w, h) = small.dimensions();
                return Ok(Thumb { w, h, rgba: small.into_raw() });
            }
        }
        ImageDecode.decode(path, max_side)
    }
}

/// EXIF orientation (1..=8) of a JPEG, or 1 when there is none or it cannot be
/// read: APP1 "Exif\0\0", a TIFF header (II / MM), IFD0's tag 0x0112. Reads
/// only the headers; anything malformed is "no rotation", never an error.
pub fn exif_orientation(bytes: &[u8]) -> u16 {
    fn u16_at(b: &[u8], i: usize, le: bool) -> Option<u16> {
        let v = b.get(i..i + 2)?;
        Some(if le { u16::from_le_bytes([v[0], v[1]]) } else { u16::from_be_bytes([v[0], v[1]]) })
    }
    fn u32_at(b: &[u8], i: usize, le: bool) -> Option<u32> {
        let v = b.get(i..i + 4)?;
        Some(if le { u32::from_le_bytes([v[0], v[1], v[2], v[3]]) } else { u32::from_be_bytes([v[0], v[1], v[2], v[3]]) })
    }
    fn find(b: &[u8]) -> Option<u16> {
        if b.get(0..2)? != [0xFF, 0xD8] { return None; } // not a JPEG
        let mut i = 2;
        while i + 4 <= b.len() && b[i] == 0xFF {
            let marker = b[i + 1];
            let len = u16::from_be_bytes([b[i + 2], b[i + 3]]) as usize;
            if marker == 0xDA || len < 2 { return None; } // image data: no EXIF before it
            if marker == 0xE1 && b.get(i + 4..i + 10)? == b"Exif\0\0" {
                let t = i + 10; // the TIFF header
                let le = match b.get(t..t + 2)? { b"II" => true, b"MM" => false, _ => return None };
                let ifd = t + u32_at(b, t + 4, le)? as usize;
                let n = u16_at(b, ifd, le)? as usize;
                for k in 0..n.min(512) {
                    let e = ifd + 2 + k * 12;
                    if u16_at(b, e, le)? == 0x0112 {
                        let v = u16_at(b, e + 8, le)?;
                        return (1..=8).contains(&v).then_some(v);
                    }
                }
                return None;
            }
            i += 2 + len;
        }
        None
    }
    find(bytes).unwrap_or(1)
}

/// Apply an EXIF orientation so the picture is upright.
pub fn orient(img: image::DynamicImage, orientation: u16) -> image::DynamicImage {
    match orientation {
        2 => img.fliph(),
        3 => img.rotate180(),
        4 => img.flipv(),
        5 => img.rotate90().fliph(),
        6 => img.rotate90(),
        7 => img.rotate270().fliph(),
        8 => img.rotate270(),
        _ => img,
    }
}

/// What the grid gets for a path this frame.
#[derive(Debug, Clone, PartialEq)]
pub enum State {
    Loading,
    Ready(Arc<Thumb>),
    Failed(String),
}

struct Entry {
    state: State,
    bytes: usize,
    last_use: u64,
}

struct Inner {
    /// Pending requests, newest at the back.
    wanted: VecDeque<String>,
    /// Being decoded right now (so a repeat request is not queued twice).
    in_flight: Vec<String>,
    done: HashMap<String, Entry>,
    clock: u64,
    used: usize,
    budget: usize,
    max_pending: usize,
    fresh: Vec<(String, Arc<Thumb>)>,
    evicted: Vec<String>,
    closing: bool,
}

type Wake = Arc<dyn Fn() + Send + Sync>;

pub struct Thumbs {
    shared: Arc<(Mutex<Inner>, Condvar)>,
    workers: Vec<JoinHandle<()>>,
}

impl Thumbs {
    /// `threads` decoders, thumbnails up to `max_side` px, at most
    /// `budget_bytes` resident, at most `max_pending` requests waiting.
    /// `wake` runs after each finished decode (the UI passes its repaint).
    pub fn new(decoder: Arc<dyn Decode>, threads: usize, max_side: u32, budget_bytes: usize, max_pending: usize, wake: Option<Wake>) -> Thumbs {
        let shared = Arc::new((Mutex::new(Inner {
            wanted: VecDeque::new(), in_flight: Vec::new(), done: HashMap::new(), clock: 0, used: 0,
            budget: budget_bytes, max_pending: max_pending.max(1), fresh: Vec::new(), evicted: Vec::new(), closing: false,
        }), Condvar::new()));
        let workers = (0..threads.max(1)).map(|_| {
            let shared = shared.clone();
            let decoder = decoder.clone();
            let wake = wake.clone();
            std::thread::spawn(move || worker(shared, decoder, max_side, wake))
        }).collect();
        Thumbs { shared, workers }
    }

    /// The thumbnail for `path`: Ready, Failed, or Loading (and now requested).
    pub fn get(&self, path: &str) -> State {
        let (lock, cv) = &*self.shared;
        let mut g = lock.lock().unwrap();
        g.clock += 1;
        let now = g.clock;
        if let Some(e) = g.done.get_mut(path) {
            e.last_use = now;
            return e.state.clone();
        }
        if !g.in_flight.iter().any(|p| p == path) {
            if let Some(i) = g.wanted.iter().position(|p| p == path) {
                // Asked again: it is still on screen -- move it to the front.
                let p = g.wanted.remove(i).unwrap();
                g.wanted.push_back(p);
            } else {
                g.wanted.push_back(path.to_string());
                while g.wanted.len() > g.max_pending {
                    g.wanted.pop_front(); // scrolled past: not worth decoding late
                }
                cv.notify_one();
            }
        }
        State::Loading
    }

    /// Thumbnails finished since the last call, each once: upload these.
    pub fn take_fresh(&self) -> Vec<(String, Arc<Thumb>)> {
        std::mem::take(&mut self.shared.0.lock().unwrap().fresh)
    }

    /// Paths evicted since the last call: free their textures.
    pub fn take_evicted(&self) -> Vec<String> {
        std::mem::take(&mut self.shared.0.lock().unwrap().evicted)
    }

    /// Bytes resident and requests waiting (for tests and a status line).
    pub fn stats(&self) -> (usize, usize) {
        let g = self.shared.0.lock().unwrap();
        (g.used, g.wanted.len())
    }
}

impl Drop for Thumbs {
    fn drop(&mut self) {
        {
            let (lock, cv) = &*self.shared;
            lock.lock().unwrap().closing = true;
            cv.notify_all();
        }
        for w in self.workers.drain(..) {
            let _ = w.join();
        }
    }
}

fn worker(shared: Arc<(Mutex<Inner>, Condvar)>, decoder: Arc<dyn Decode>, max_side: u32, wake: Option<Wake>) {
    let (lock, cv) = &*shared;
    loop {
        let path = {
            let mut g = lock.lock().unwrap();
            loop {
                if g.closing { return; }
                if let Some(p) = g.wanted.pop_back() { // newest first
                    g.in_flight.push(p.clone());
                    break p;
                }
                g = cv.wait(g).unwrap();
            }
        };
        let result = decoder.decode(&path, max_side);
        {
            let mut g = lock.lock().unwrap();
            g.in_flight.retain(|p| p != &path);
            g.clock += 1;
            let now = g.clock;
            let (state, bytes) = match result {
                Ok(t) => {
                    let t = Arc::new(t);
                    g.fresh.push((path.clone(), t.clone()));
                    let b = t.bytes();
                    (State::Ready(t), b)
                }
                Err(e) => (State::Failed(e), 0),
            };
            g.used += bytes;
            g.done.insert(path.clone(), Entry { state, bytes, last_use: now });
            // Least recently used out until within budget -- never the one
            // just decoded (it is on screen).
            while g.used > g.budget {
                let victim = g.done.iter()
                    .filter(|(p, e)| *p != &path && e.bytes > 0)
                    .min_by_key(|(_, e)| e.last_use)
                    .map(|(p, _)| p.clone());
                let Some(v) = victim else { break };
                if let Some(e) = g.done.remove(&v) {
                    g.used -= e.bytes;
                    g.fresh.retain(|(p, _)| p != &v);
                    g.evicted.push(v);
                }
            }
        }
        if let Some(w) = &wake { w(); }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{Duration, Instant};

    /// A decoder that records the order it was asked in, can be held shut,
    /// and fails on paths containing "bad". Every thumbnail is 100 bytes.
    #[derive(Default)]
    struct Fake {
        order: Mutex<Vec<String>>,
        calls: AtomicUsize,
        gate: Mutex<bool>, // true = held
        gate_cv: Condvar,
    }
    impl Fake {
        fn hold(&self, on: bool) { *self.gate.lock().unwrap() = on; self.gate_cv.notify_all(); }
    }
    impl Decode for Fake {
        fn decode(&self, path: &str, _max: u32) -> Result<Thumb, String> {
            let mut g = self.gate.lock().unwrap();
            while *g { g = self.gate_cv.wait(g).unwrap(); }
            drop(g);
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.order.lock().unwrap().push(path.to_string());
            if path.contains("bad") { return Err("not an image".into()); }
            Ok(Thumb { w: 5, h: 5, rgba: vec![0; 100] })
        }
    }

    fn until(mut f: impl FnMut() -> bool) {
        let t = Instant::now();
        while !f() {
            assert!(t.elapsed() < Duration::from_secs(5), "timed out");
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    #[test]
    fn a_requested_thumbnail_arrives_once_and_is_then_ready() {
        let fake = Arc::new(Fake::default());
        let t = Thumbs::new(fake.clone(), 1, 64, 10_000, 64, None);
        assert_eq!(t.get("a.jpg"), State::Loading);
        until(|| matches!(t.get("a.jpg"), State::Ready(_)));
        let fresh = t.take_fresh();
        assert_eq!(fresh.len(), 1);
        assert_eq!(fresh[0].0, "a.jpg");
        assert!(t.take_fresh().is_empty(), "reported once");
        for _ in 0..10 { t.get("a.jpg"); }
        assert_eq!(fake.calls.load(Ordering::SeqCst), 1, "decoded once, however often asked");
    }

    #[test]
    fn newest_requests_are_decoded_first_and_stale_ones_dropped() {
        let fake = Arc::new(Fake::default());
        fake.hold(true);
        let t = Thumbs::new(fake.clone(), 1, 64, 1_000_000, 4, None);
        t.get("p0"); // the worker takes this one and blocks on the gate
        until(|| t.stats().1 == 0);
        for i in 1..10 { t.get(&format!("p{i}")); }
        assert_eq!(t.stats().1, 4, "only the newest 4 wait");
        fake.hold(false);
        until(|| fake.calls.load(Ordering::SeqCst) == 5);
        std::thread::sleep(Duration::from_millis(30));
        let order = fake.order.lock().unwrap().clone();
        assert_eq!(order, vec!["p0", "p9", "p8", "p7", "p6"], "in flight, then newest first; p1..p5 scrolled past");
    }

    #[test]
    fn asking_again_moves_a_waiting_request_to_the_front() {
        let fake = Arc::new(Fake::default());
        fake.hold(true);
        let t = Thumbs::new(fake.clone(), 1, 64, 1_000_000, 16, None);
        t.get("busy");
        until(|| t.stats().1 == 0);
        t.get("x"); t.get("y"); t.get("x"); // x is still on screen
        fake.hold(false);
        until(|| fake.calls.load(Ordering::SeqCst) == 3);
        assert_eq!(fake.order.lock().unwrap()[1], "x");
    }

    #[test]
    fn memory_is_bounded_least_recently_used_first_and_evictions_reported() {
        let fake = Arc::new(Fake::default());
        let t = Thumbs::new(fake.clone(), 1, 64, 250, 64, None); // room for two
        for p in ["a", "b"] { t.get(p); until(|| matches!(t.get(p), State::Ready(_))); }
        t.get("a"); // a used recently; b is the oldest
        t.get("c");
        until(|| matches!(t.get("c"), State::Ready(_)));
        assert_eq!(t.take_evicted(), vec!["b".to_string()]);
        assert!(t.stats().0 <= 250, "resident {} bytes", t.stats().0);
        assert!(matches!(t.get("a"), State::Ready(_)), "a survived");
        assert_eq!(t.get("b"), State::Loading, "b must be decoded again");
    }

    #[test]
    fn a_file_that_does_not_decode_fails_once_and_is_not_retried() {
        let fake = Arc::new(Fake::default());
        let t = Thumbs::new(fake.clone(), 1, 64, 10_000, 64, None);
        t.get("bad.jpg");
        until(|| matches!(t.get("bad.jpg"), State::Failed(_)));
        for _ in 0..20 { t.get("bad.jpg"); }
        std::thread::sleep(Duration::from_millis(20));
        assert_eq!(fake.calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn the_wake_callback_fires_and_dropping_joins_the_workers() {
        let woke = Arc::new(AtomicUsize::new(0));
        let w = woke.clone();
        let t = Thumbs::new(Arc::new(Fake::default()), 2, 64, 10_000, 64, Some(Arc::new(move || { w.fetch_add(1, Ordering::SeqCst); })));
        t.get("a"); t.get("b");
        until(|| woke.load(Ordering::SeqCst) == 2);
        let start = Instant::now();
        drop(t);
        assert!(start.elapsed() < Duration::from_secs(1), "workers exit promptly");
    }

    /// A minimal JPEG header carrying only an EXIF orientation, in either byte order.
    fn exif_app1(orientation: u16, le: bool) -> Vec<u8> {
        let w16 = |v: u16| if le { v.to_le_bytes() } else { v.to_be_bytes() };
        let w32 = |v: u32| if le { v.to_le_bytes() } else { v.to_be_bytes() };
        let mut tiff = Vec::new();
        tiff.extend_from_slice(if le { b"II" } else { b"MM" });
        tiff.extend_from_slice(&w16(42));
        tiff.extend_from_slice(&w32(8)); // IFD0 right after the header
        tiff.extend_from_slice(&w16(1)); // one entry
        tiff.extend_from_slice(&w16(0x0112)); tiff.extend_from_slice(&w16(3)); tiff.extend_from_slice(&w32(1));
        tiff.extend_from_slice(&w16(orientation)); tiff.extend_from_slice(&[0, 0]);
        tiff.extend_from_slice(&w32(0));
        let mut seg = vec![0xFF, 0xE1];
        seg.extend_from_slice(&((2 + 6 + tiff.len()) as u16).to_be_bytes());
        seg.extend_from_slice(b"Exif\0\0");
        seg.extend_from_slice(&tiff);
        seg
    }

    #[test]
    fn the_exif_orientation_is_read_in_both_byte_orders_and_malformed_is_upright() {
        for le in [true, false] {
            let mut j = vec![0xFF, 0xD8];
            j.extend(exif_app1(6, le));
            j.extend_from_slice(&[0xFF, 0xDA, 0, 2]);
            assert_eq!(exif_orientation(&j), 6, "le={le}");
        }
        assert_eq!(exif_orientation(&[0xFF, 0xD8, 0xFF, 0xDA, 0, 2]), 1, "no EXIF");
        assert_eq!(exif_orientation(b"\x89PNG\r\n"), 1, "not a JPEG");
        let mut cut = vec![0xFF, 0xD8];
        cut.extend(&exif_app1(6, true)[..14]);
        assert_eq!(exif_orientation(&cut), 1, "truncated");
        let mut odd = vec![0xFF, 0xD8];
        odd.extend(exif_app1(9, true));
        assert_eq!(exif_orientation(&odd), 1, "out of range");
    }

    #[test]
    fn a_portrait_phone_photo_is_decoded_upright() {
        // A 400x200 JPEG whose left half is red, stored with orientation 6 (turn 90
        // degrees clockwise): upright it is 200 wide and 400 tall, red on TOP.
        let mut img = image::RgbImage::new(400, 200);
        for (x, _, p) in img.enumerate_pixels_mut() { *p = if x < 200 { image::Rgb([220, 20, 20]) } else { image::Rgb([20, 20, 220]) }; }
        let mut jpeg = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 95).encode_image(&img).unwrap();
        let mut with_exif = jpeg[..2].to_vec();
        with_exif.extend(exif_app1(6, false));
        with_exif.extend_from_slice(&jpeg[2..]);
        let dir = std::env::temp_dir().join(format!("photosync-orient-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("portrait.jpg");
        std::fs::write(&p, &with_exif).unwrap();
        let t = ImageDecode.decode(p.to_str().unwrap(), 128).unwrap();
        assert_eq!((t.w, t.h), (64, 128), "upright: portrait");
        let top = &t.rgba[(8 * t.w as usize + 32) * 4..][..3];
        let bottom = &t.rgba[(120 * t.w as usize + 32) * 4..][..3];
        assert!(top[0] > 150 && top[2] < 100, "red on top: {top:?}");
        assert!(bottom[2] > 150 && bottom[0] < 100, "blue below: {bottom:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The service's stored thumbnail is used (green: the original is red and
    /// blue), turned by the ORIGINAL's EXIF orientation (the stored JPEG has
    /// none); a file without one falls back to decoding the original.
    #[test]
    fn stored_thumbnails_are_used_and_turned_by_the_originals_orientation() {
        let dir = std::env::temp_dir().join(format!("photosync-store-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("blocks")).unwrap();
        let db = dir.join("photosync.db");
        let mut service = photosync::PhotoSyncDB::new(&db, &dir.join("blocks")).unwrap();
        // The original: 400x200 sensor pixels, left half red, EXIF orientation 6.
        let mut img = image::RgbImage::new(400, 200);
        for (x, _, px) in img.enumerate_pixels_mut() { *px = if x < 200 { image::Rgb([220, 20, 20]) } else { image::Rgb([20, 20, 220]) }; }
        let mut jpeg = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 95).encode_image(&img).unwrap();
        let mut original = jpeg[..2].to_vec();
        original.extend(exif_app1(6, true));
        original.extend_from_slice(&jpeg[2..]);
        let photo = dir.join("IMG_0001.jpg");
        std::fs::write(&photo, &original).unwrap();
        // The service's thumbnail of it: sensor orientation (landscape), no EXIF.
        let mut thumb = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut thumb, 95)
            .encode_image(&image::RgbImage::from_pixel(200, 100, image::Rgb([20, 200, 20]))).unwrap();
        service.store_thumbnail("h1", &thumb, 400, 200).unwrap();
        drop(service);
        let key = photo.to_string_lossy().into_owned();
        let d = StoreDecode::open(&db, [(key.clone(), "h1".to_string())].into_iter().collect()).unwrap();
        let t = d.decode(&key, 256).unwrap();
        assert_eq!((t.w, t.h), (100, 200), "the stored thumbnail, turned upright by the original's EXIF");
        assert!(t.rgba[1] > 150 && t.rgba[0] < 100, "green: it came from the store, not the original");
        let none = StoreDecode::open(&db, HashMap::new()).unwrap().decode(&key, 128).unwrap();
        assert_eq!((none.w, none.h), (64, 128), "no stored thumbnail: the original, upright");
        assert!(none.rgba[0] > 150, "red on top: decoded from the original");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_real_decoder_scales_a_picture_keeping_its_aspect() {
        let dir = std::env::temp_dir().join(format!("photosync-thumbs-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("wide.png");
        image::RgbaImage::from_pixel(400, 200, image::Rgba([200, 30, 30, 255])).save(&p).unwrap();
        let t = ImageDecode.decode(p.to_str().unwrap(), 128).unwrap();
        assert_eq!((t.w, t.h), (128, 64));
        assert_eq!(t.rgba.len(), 128 * 64 * 4);
        assert_eq!(&t.rgba[..4], &[200, 30, 30, 255]);
        assert!(ImageDecode.decode(dir.join("missing.jpg").to_str().unwrap(), 128).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
