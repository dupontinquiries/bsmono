//! The height cache: unendlich's surface, one column at a time, computed
//! once and reused every frame.
//!
//! A frame never waits for world generation. A lookup that misses answers
//! from a coarser level (or not at all, and the renderer draws fog there),
//! and queues the cell. Worker threads fill the queue in the order the
//! renderer asked, which is front to back, so the nearest ground fills
//! first. The app re-renders as `filled()` advances: the picture sharpens
//! while you look at it. That is deliberately a stress test for eink-kit:
//! arbitrary pixels changing frame after frame, not fixed widgets.
//!
//! Levels: level `l` samples one column per `1 << l` blocks (at the cell's
//! centre). Near ground uses level 0; distance picks coarser levels.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, RwLock};

pub const LEVELS: u8 = 5;

/// Where heights come from. The real one is unendlich's `WorldGenerator`;
/// tests use synthetic terrain.
pub trait HeightSource: Send + Sync + 'static {
    /// Surface height (world y of the topmost standable block) at a column.
    fn height(&self, x: i32, z: i32) -> i16;
}

/// unendlich's generator as a height source.
pub struct World(pub unendlich_engine::world::cpu_generator::WorldGenerator);

impl HeightSource for World {
    fn height(&self, x: i32, z: i32) -> i16 {
        self.0.surface_y_at(x, z, 0, 320).unwrap_or(0) as i16
    }
}

type Key = (u8, i32, i32);

struct Shared {
    cells: RwLock<HashMap<Key, i16>>,
    queue: Mutex<VecDeque<Key>>,
    queued: Mutex<HashSet<Key>>,
    wake: Condvar,
    filled: AtomicU64,
    stop: AtomicBool,
}

pub struct HeightCache {
    shared: Arc<Shared>,
    workers: Vec<std::thread::JoinHandle<()>>,
    /// Upper bound on cached cells before far ones are evicted.
    pub max_cells: usize,
}

impl HeightCache {
    pub fn new(source: Arc<dyn HeightSource>, workers: usize) -> HeightCache {
        let shared = Arc::new(Shared {
            cells: RwLock::new(HashMap::new()),
            queue: Mutex::new(VecDeque::new()),
            queued: Mutex::new(HashSet::new()),
            wake: Condvar::new(),
            filled: AtomicU64::new(0),
            stop: AtomicBool::new(false),
        });
        let handles = (0..workers.max(1))
            .map(|i| {
                let (sh, src) = (shared.clone(), source.clone());
                std::thread::Builder::new()
                    .name(format!("heights-{i}"))
                    .spawn(move || worker(sh, src))
                    .expect("spawn height worker")
            })
            .collect();
        HeightCache { shared, workers: handles, max_cells: 600_000 }
    }

    /// Height of cell `(level, cx, cz)` if known; otherwise queue it and
    /// answer from the nearest coarser level that is known.
    pub fn lookup(&self, level: u8, cx: i32, cz: i32) -> Option<i16> {
        let cells = self.shared.cells.read().unwrap();
        if let Some(h) = cells.get(&(level, cx, cz)) {
            return Some(*h);
        }
        drop(cells);
        self.request((level, cx, cz));
        let cells = self.shared.cells.read().unwrap();
        (level + 1..LEVELS).find_map(|l| {
            let shift = (l - level) as i32;
            cells.get(&(l, cx >> shift, cz >> shift)).copied()
        })
    }

    fn request(&self, k: Key) {
        let mut queued = self.shared.queued.lock().unwrap();
        if queued.insert(k) {
            self.shared.queue.lock().unwrap().push_back(k);
            self.shared.wake.notify_one();
        }
        drop(queued);
    }

    /// Drop queued work (the camera moved on): what the next frame asks for
    /// is queued again, in its own near-first order.
    pub fn clear_queue(&self) {
        self.shared.queue.lock().unwrap().clear();
        self.shared.queued.lock().unwrap().clear();
    }

    /// Cells computed so far (monotonic): the app re-renders when it moves.
    pub fn filled(&self) -> u64 {
        self.shared.filled.load(Ordering::Relaxed)
    }

    pub fn pending(&self) -> usize {
        self.shared.queue.lock().unwrap().len()
    }

    pub fn len(&self) -> usize {
        self.shared.cells.read().unwrap().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Keep memory bounded: past `max_cells`, drop cells far from `(x, z)`.
    pub fn evict_far(&self, x: f32, z: f32) {
        let mut cells = self.shared.cells.write().unwrap();
        if cells.len() <= self.max_cells {
            return;
        }
        let keep_r2 = |l: u8| {
            let r = 96.0 * (1u32 << l) as f32;
            r * r
        };
        cells.retain(|&(l, cx, cz), _| {
            let s = (1i32 << l) as f32;
            let (dx, dz) = ((cx as f32 + 0.5) * s - x, (cz as f32 + 0.5) * s - z);
            dx * dx + dz * dz <= keep_r2(l)
        });
    }
}

impl Drop for HeightCache {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Relaxed);
        self.shared.wake.notify_all();
        for h in self.workers.drain(..) {
            let _ = h.join();
        }
    }
}

fn worker(sh: Arc<Shared>, src: Arc<dyn HeightSource>) {
    loop {
        let k = {
            let mut q = sh.queue.lock().unwrap();
            loop {
                if sh.stop.load(Ordering::Relaxed) {
                    return;
                }
                if let Some(k) = q.pop_front() {
                    break k;
                }
                q = sh.wake.wait(q).unwrap();
            }
        };
        let (l, cx, cz) = k;
        let s = 1i32 << l;
        let h = src.height(cx * s + s / 2, cz * s + s / 2);
        sh.cells.write().unwrap().insert(k, h);
        sh.queued.lock().unwrap().remove(&k);
        sh.filled.fetch_add(1, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    struct Ramp;
    impl HeightSource for Ramp {
        fn height(&self, x: i32, _z: i32) -> i16 {
            (x / 4) as i16
        }
    }

    fn wait_until(mut f: impl FnMut() -> bool) {
        let t = Instant::now();
        while !f() {
            assert!(t.elapsed() < Duration::from_secs(5), "timed out");
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    #[test]
    fn a_miss_queues_and_a_worker_fills_it() {
        let c = HeightCache::new(Arc::new(Ramp), 2);
        assert_eq!(c.lookup(0, 40, 0), None, "never blocks on generation");
        wait_until(|| c.lookup(0, 40, 0).is_some());
        assert_eq!(c.lookup(0, 40, 0), Some(10)); // x = 40 (+0 centre offset at level 0) / 4
        assert!(c.filled() >= 1);
    }

    #[test]
    fn a_miss_answers_from_a_coarser_level_when_it_can() {
        let c = HeightCache::new(Arc::new(Ramp), 1);
        let _ = c.lookup(2, 10, 0); // level 2 cell 10 covers x 40..44, centre 42
        wait_until(|| c.lookup(2, 10, 0).is_some());
        // a level-0 cell inside it: not cached yet, but the coarse answer is
        assert_eq!(c.lookup(0, 41, 0), Some(10));
    }

    #[test]
    fn eviction_keeps_near_cells() {
        let mut c = HeightCache::new(Arc::new(Ramp), 2);
        c.max_cells = 10;
        for i in 0..40 {
            let _ = c.lookup(0, i * 50, 0);
        }
        wait_until(|| c.pending() == 0 && c.len() >= 40);
        c.evict_far(0.0, 0.0);
        assert!(c.len() < 40);
        assert!(c.lookup(0, 0, 0).is_some(), "the cell at the camera survives");
    }

    #[test]
    fn the_real_world_is_a_height_source() {
        let w = World(unendlich_engine::world::cpu_generator::WorldGenerator::new(1337));
        let h = w.height(0, 0);
        assert!((1..320).contains(&h), "surface at origin: {h}");
    }
}
