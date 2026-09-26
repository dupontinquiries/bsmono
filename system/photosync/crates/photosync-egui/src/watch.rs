//! New photos while the gallery is open — headless.
//!
//! The service keeps importing (a phone syncing, a camera card copied in)
//! while someone is looking at the library, and the gallery loaded it once at
//! start: nothing new appeared until a restart.
//!
//! A thread polls the service's database, read-only, with
//! `album::files_stamp` (one cheap scan) and reloads the library only when the
//! stamp moves. Both happen off the UI thread: a reload of a large library is
//! tens of milliseconds, which would be a dropped frame on a phone. The frame
//! loop takes the new library with `take` and swaps it in.

use photosync::album::Library;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver};
use std::sync::Arc;
use std::time::Duration;

pub type Wake = Arc<dyn Fn() + Send + Sync>;

pub struct Watch {
    rx: Receiver<Library>,
    stop: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl Watch {
    /// Poll `db` every `every`. The stamp is taken once at start, so a
    /// library that has not changed since the caller loaded it is never
    /// reloaded. `wake` is called after a new library is sent (a repaint).
    pub fn start(db: std::path::PathBuf, every: Duration, wake: Option<Wake>) -> Result<Watch, String> {
        let conn = rusqlite::Connection::open_with_flags(&db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(|e| format!("{}: {e}", db.display()))?;
        let mut last = photosync::album::files_stamp(&conn).map_err(|e| format!("{}: {e}", db.display()))?;
        let (tx, rx) = channel();
        let stop = Arc::new(AtomicBool::new(false));
        let stop_t = stop.clone();
        let handle = std::thread::Builder::new().name("photosync-watch".into()).spawn(move || {
            // Sleep in short slices so dropping the Watch joins promptly.
            let slice = Duration::from_millis(20).min(every);
            loop {
                let mut slept = Duration::ZERO;
                while slept < every {
                    if stop_t.load(Ordering::Relaxed) { return; }
                    std::thread::sleep(slice);
                    slept += slice;
                }
                // A busy or locked database (the service mid-write) is simply
                // tried again next time.
                let Ok(now) = photosync::album::files_stamp(&conn) else { continue };
                if now == last { continue; }
                let Ok(lib) = photosync::album::load(&conn) else { continue };
                last = now;
                if tx.send(lib).is_err() { return; }
                if let Some(w) = &wake { w(); }
            }
        }).map_err(|e| e.to_string())?;
        Ok(Watch { rx, stop, handle: Some(handle) })
    }

    /// The newest library loaded since the last call, if any (older ones in
    /// between are dropped: only the latest matters).
    pub fn take(&self) -> Option<Library> {
        let mut newest = None;
        while let Ok(lib) = self.rx.try_recv() {
            newest = Some(lib);
        }
        newest
    }
}

impl Drop for Watch {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    #[test]
    fn a_new_photo_is_noticed_and_nothing_is_reloaded_while_nothing_changes() {
        let dir = std::env::temp_dir().join(format!("photosync-watch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("blocks")).unwrap();
        let db = dir.join("p.db");
        let mut service = photosync::PhotoSyncDB::new(&db, &dir.join("blocks")).unwrap();
        let photo = |i: u32| {
            let p = dir.join(format!("w{i}.png"));
            image::RgbImage::from_pixel(40, 30, image::Rgb([i as u8 * 40, 90, 90])).save(&p).unwrap();
            p
        };
        service.import_photo(&photo(0), "t", 32).unwrap();

        let woke = Arc::new(AtomicUsize::new(0));
        let w = woke.clone();
        let watch = Watch::start(db.clone(), Duration::from_millis(30), Some(Arc::new(move || { w.fetch_add(1, Ordering::SeqCst); }))).unwrap();
        std::thread::sleep(Duration::from_millis(200));
        assert!(watch.take().is_none(), "unchanged: no reload");
        assert_eq!(woke.load(Ordering::SeqCst), 0);

        service.import_photo(&photo(1), "t", 32).unwrap();
        let start = std::time::Instant::now();
        let lib = loop {
            if let Some(l) = watch.take() { break l; }
            assert!(start.elapsed() < Duration::from_secs(5), "the new photo was never noticed");
            std::thread::sleep(Duration::from_millis(10));
        };
        assert_eq!(lib.len(), 2);
        assert!(woke.load(Ordering::SeqCst) >= 1, "and the frame loop was woken");
        let t = std::time::Instant::now();
        drop(watch);
        assert!(t.elapsed() < Duration::from_secs(1), "dropping joins promptly");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
