//! More search in the same minutes.
//!
//! The searches in this crate -- the greeble evolution, the spec
//! tuner, the plant breeder, and now the player simulation -- all have
//! the same shape: a population of candidates, each scored
//! independently against an unchanging world. That is the easiest
//! thing there is to parallelise, and it is the thing that decides how
//! good the answer is, because a fixed time budget spent four ways is
//! four times the candidates.
//!
//! WHY THIS AND NOT RAYON
//!
//! The crate has no dependencies, which is what lets it build on a
//! fresh machine with no network and lets the harness compile in a
//! second. A work-stealing pool would be better under load; a scoped
//! `std::thread` fan-out is enough for a fan-out of eight over work
//! items that take milliseconds each, and it is forty lines.
//!
//! WHAT IT DOES NOT DO
//!
//! It does not make the world thread-safe. Every candidate reads a
//! SHARED IMMUTABLE world and writes only its own result; anything
//! that mutates the world -- applying a carve, running a plan --
//! stays on one thread. That is the whole discipline, and it is why
//! `eval` takes `&W` and returns a value rather than taking `&mut`.
//!
//! DETERMINISM SURVIVES. Results come back in the order the inputs
//! went in, never in completion order, so a seeded search gives the
//! same answer at one thread or at eight. A test asserts exactly that,
//! because a parallel search that is not reproducible is not a search,
//! it is a slot machine.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

/// How many threads to use. Respects the platform, and caps: past a
/// handful the frame thread starts to notice, and this runs beside a
/// game.
pub fn threads(cap: usize) -> usize {
    std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1)
        .saturating_sub(1)          // leave one for the frame
        .clamp(1, cap.max(1))
}

/// Map `f` over `items` on `n` threads, in order. `f` must be `Sync`
/// and the items `Sync`: they are shared, not moved.
pub fn map<T: Sync, R: Send>(items: &[T], n: usize, f: impl Fn(&T) -> R + Sync) -> Vec<R> {
    let n = n.clamp(1, 64);
    if n == 1 || items.len() < 2 {
        return items.iter().map(&f).collect();
    }
    let next = AtomicUsize::new(0);
    let slots: Vec<Mutexish<R>> = (0..items.len()).map(|_| Mutexish::new()).collect();
    std::thread::scope(|s| {
        for _ in 0..n.min(items.len()) {
            s.spawn(|| {
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    if i >= items.len() { break; }
                    slots[i].set(f(&items[i]));
                }
            });
        }
    });
    slots.into_iter().map(|s| s.take()).collect()
}

/// Map with a DEADLINE: items not started by the deadline are not
/// started at all, and the caller is told how many were done. A search
/// with a wall-clock budget wants this -- it would rather have sixty
/// candidates scored in the two minutes it has than four hundred in
/// eight minutes it does not.
pub fn map_until<T: Sync, R: Send>(items: &[T], n: usize, deadline: Instant,
                                   f: impl Fn(&T) -> R + Sync) -> (Vec<Option<R>>, usize) {
    let n = n.clamp(1, 64);
    let next = AtomicUsize::new(0);
    let done = AtomicUsize::new(0);
    let slots: Vec<Mutexish<R>> = (0..items.len()).map(|_| Mutexish::new()).collect();
    std::thread::scope(|s| {
        for _ in 0..n.min(items.len().max(1)) {
            s.spawn(|| {
                loop {
                    if Instant::now() >= deadline { break; }
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    if i >= items.len() { break; }
                    slots[i].set(f(&items[i]));
                    done.fetch_add(1, Ordering::Relaxed);
                }
            });
        }
    });
    (slots.into_iter().map(|s| s.take_opt()).collect(), done.load(Ordering::Relaxed))
}

/// How long the work took and how much of the budget it used, for the
/// log a player sees.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Timing {
    pub wall: Duration,
    pub items: usize,
    pub threads: usize,
}

impl Timing {
    pub fn summary(&self) -> String {
        let per = if self.items == 0 { 0.0 } else { self.wall.as_secs_f32() * 1000.0 / self.items as f32 };
        format!("{} item(s) on {} thread(s) in {:.1}s ({:.1} ms each)",
                self.items, self.threads, self.wall.as_secs_f32(), per)
    }
}

/// Map and time it.
pub fn map_timed<T: Sync, R: Send>(items: &[T], n: usize, f: impl Fn(&T) -> R + Sync)
    -> (Vec<R>, Timing) {
    let t0 = Instant::now();
    let out = map(items, n, f);
    (out, Timing { wall: t0.elapsed(), items: items.len(), threads: n })
}

/// A one-shot cell that a scoped thread fills and the owner takes.
///
/// `Mutex<Option<R>>` would do this, and did: the lock is uncontended
/// because exactly one thread ever writes each slot, but every write
/// still pays for the atomic and every read for a `.unwrap()` on a
/// poison that cannot happen. This is the same thing with the
/// synchronisation that the scope already provides -- the scope joins
/// every thread before the owner reads, which is the ordering the
/// unsafe below relies on and the reason it is sound.
struct Mutexish<R> {
    cell: std::cell::UnsafeCell<Option<R>>,
}

// SAFETY: each slot is written by exactly one thread (the one that
// claimed index `i` from the atomic counter, and indices are handed out
// once), and read only after `scope` has joined every thread. There is
// no aliasing and no read-write overlap.
unsafe impl<R: Send> Sync for Mutexish<R> {}

impl<R> Mutexish<R> {
    fn new() -> Self { Self { cell: std::cell::UnsafeCell::new(None) } }
    fn set(&self, v: R) { unsafe { *self.cell.get() = Some(v); } }
    fn take(self) -> R { self.cell.into_inner().expect("every slot is filled by `map`") }
    fn take_opt(self) -> Option<R> { self.cell.into_inner() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn results_come_back_in_order_whatever_the_thread_count() {
        let items: Vec<u64> = (0..200).collect();
        let serial = map(&items, 1, |i| i * 2);
        for n in [2, 4, 8] {
            let parallel = map(&items, n, |i| {
                // uneven work, so completion order differs from input order
                if i % 7 == 0 { std::thread::sleep(Duration::from_micros(200)); }
                i * 2
            });
            assert_eq!(parallel, serial, "{n} threads must agree with one");
        }
    }

    #[test]
    fn a_seeded_search_gives_the_same_answer_at_one_thread_or_eight() {
        // the property that matters: a parallel search that is not
        // reproducible is a slot machine
        let hash = |x: u64| { let mut h = x.wrapping_mul(0x9E3779B97F4A7C15); h ^= h >> 31; h };
        let items: Vec<u64> = (0..500).collect();
        let a = map(&items, 1, |i| hash(*i) % 1000);
        let b = map(&items, 8, |i| hash(*i) % 1000);
        assert_eq!(a, b);
        let best_a = a.iter().enumerate().min_by_key(|(_, v)| **v);
        let best_b = b.iter().enumerate().min_by_key(|(_, v)| **v);
        assert_eq!(best_a, best_b, "the same winner");
    }

    #[test]
    fn work_is_actually_shared_out() {
        // measurable only if the work is big enough to dwarf the spawn
        let items: Vec<u64> = (0..16).collect();
        let heavy = |_: &u64| { std::thread::sleep(Duration::from_millis(20)); 1u8 };
        let t0 = Instant::now();
        let _ = map(&items, 1, heavy);
        let serial = t0.elapsed();
        let t0 = Instant::now();
        let _ = map(&items, 4, heavy);
        let parallel = t0.elapsed();
        assert!(parallel < serial * 3 / 4, "four threads beat one: {parallel:?} vs {serial:?}");
    }

    #[test]
    fn a_deadline_stops_starting_new_work_and_says_how_much_was_done() {
        let items: Vec<u64> = (0..500).collect();
        let t0 = Instant::now();
        let (out, done) = map_until(&items, 4, t0 + Duration::from_millis(60), |_| {
            std::thread::sleep(Duration::from_millis(5)); 1u8
        });
        assert!(t0.elapsed() < Duration::from_millis(300), "it stopped: {:?}", t0.elapsed());
        assert!(done > 0 && done < 500, "some but not all: {done}");
        assert_eq!(out.iter().filter(|o| o.is_some()).count(), done);
        // the ones that ran are still in their input positions
        assert!(out[0].is_some(), "it started at the beginning");
    }

    #[test]
    fn the_thread_count_leaves_one_for_the_frame_and_is_capped() {
        let n = threads(8);
        assert!(n >= 1 && n <= 8);
        assert_eq!(threads(1), 1, "a cap of one is honoured");
        let all = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1);
        if all > 1 { assert!(threads(64) < all, "one core is left for the frame"); }
    }

    #[test]
    fn empty_and_single_inputs_do_not_spawn() {
        let empty: Vec<u64> = Vec::new();
        assert!(map(&empty, 8, |i| *i).is_empty());
        assert_eq!(map(&[7u64], 8, |i| *i * 3), vec![21]);
        let (_, t) = map_timed(&[1u64, 2, 3], 4, |i| *i);
        assert_eq!(t.items, 3);
        assert!(t.summary().contains("3 item(s)"));
    }
}
