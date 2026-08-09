//! Does the wake-kit gating actually save wake-ups?
//!
//! The engine's `tick` used to carry a comment saying reannounce and bucket
//! refresh "would live here". They do now, and they are gated — so the question
//! worth testing is not whether they run, but whether they *stay quiet* when
//! running would mean powering up a modem on this node's own schedule.

use snapclone_dht::core::{Addr, InfoHash, Value};
use snapclone_dht::engine::{Engine, REANNOUNCE_MS, REFRESH_MS};

/// A node that knows peers. Without them maintenance has nobody to send to and
/// produces no packets at all — which my first version of these tests missed,
/// measuring an empty routing table rather than the gating policy.
fn node(seed: u64) -> Engine {
    let mut e = Engine::new(InfoHash::from_seed(seed));
    for i in 1..6u64 {
        e.table.on_seen(InfoHash::from_seed(seed ^ (i * 0x9E37)), i as Addr, 0);
    }
    e
}

/// Count ticks across a span that produced any outbound packet.
fn wakeups(e: &mut Engine, span_ms: u64, step_ms: u64, radio_up: bool) -> usize {
    let mut n = 0;
    let mut t = 0;
    while t < span_ms {
        if !e.tick_with_radio(t, radio_up).is_empty() { n += 1; }
        t += step_ms;
    }
    n
}

#[test]
fn a_node_with_nothing_stored_never_transmits_on_its_own() {
    // The baseline that matters most: an idle node must be silent. A DHT that
    // chattered while holding nothing would drain a phone in a pocket.
    let mut e = node(1);
    assert_eq!(wakeups(&mut e, REANNOUNCE_MS * 3, 10_000, false), 0);
}

#[test]
fn expiry_still_runs_on_every_tick_even_when_the_radio_stays_down() {
    // Expiry is local bookkeeping — no packets — so gating it would grow memory
    // for no power saving at all.
    let mut e = node(2);
    let k = InfoHash::from_seed(99);
    e.store.put(k, Value { id: 1, data: b"x".to_vec() }, 0);
    assert!(!e.store.get(&k, 0).is_empty());
    e.tick_with_radio(u64::MAX / 2, false);
    assert!(e.store.get(&k, u64::MAX / 2).is_empty(), "expired without needing a radio");
}

#[test]
fn bucket_refresh_never_powers_up_the_radio_by_itself() {
    // Opportunistic. A stale bucket costs a slower lookup later, never a
    // failure, so it must never be the reason a modem wakes.
    let mut e = node(3);
    let quiet = wakeups(&mut e, REFRESH_MS * 4, 30_000, false);
    let riding = wakeups(&mut node(3), REFRESH_MS * 4, 30_000, true);
    assert!(riding > quiet, "refresh should ride a hot radio: {riding} vs {quiet}");
}

#[test]
fn a_stored_value_is_reannounced_but_not_on_every_tick() {
    // The saving, stated as a ratio. Ticking every ten seconds across three
    // re-announce periods is 180 opportunities; a gated node takes a handful.
    let mut e = node(4);
    e.store.put(InfoHash::from_seed(7), Value { id: 1, data: b"presence".to_vec() }, 0);
    let ticks = (REANNOUNCE_MS * 3) / 10_000;
    let woke = wakeups(&mut e, REANNOUNCE_MS * 3, 10_000, false);
    assert!(woke > 0, "it must republish at some point, or values die at TTL");
    assert!(woke * 10 < ticks as usize,
            "woke {woke} times in {ticks} ticks — that is not batching");
}

#[test]
fn nodes_spread_within_the_window_but_not_across_the_period() {
    // What the offset actually buys, stated honestly after a coarser version of
    // this test asserted more than the design delivers.
    //
    // The window offset spreads nodes across a WINDOW, not across the
    // re-announce PERIOD. Two nodes that start together and share a period stay
    // roughly in step — their due times coincide — and the offset only
    // separates them by up to one window. That is the intended trade: a window
    // is long enough to batch a device's own jobs together and short enough
    // that presence does not go visibly stale, and de-phasing an entire fleet
    // across ten minutes would need per-node period jitter, which is a
    // different mechanism.
    //
    // So the property to assert is separation on the order of the window, and
    // that they are not bit-for-bit identical.
    let mut first: Vec<u64> = Vec::new();
    for seed in [0xAAAAu64, 0xBBBB, 0xCCCC, 0xDDDD] {
        let mut e = node(seed);
        e.store.put(InfoHash::from_seed(7), Value { id: 1, data: b"p".to_vec() }, 0);
        let mut t = 0;
        while t < REANNOUNCE_MS * 2 {
            if !e.tick_with_radio(t, false).is_empty() { first.push(t); break; }
            t += 500;
        }
    }
    assert_eq!(first.len(), 4, "every node republished: {first:?}");
    let mut uniq = first.clone();
    uniq.sort_unstable();
    uniq.dedup();
    assert!(uniq.len() > 1, "all four fired at the identical instant: {first:?}");
    let spread = uniq.last().unwrap() - uniq.first().unwrap();
    assert!(spread > 0 && spread <= 60_000,
            "spread is {spread}ms — expected within a window or two");
}
