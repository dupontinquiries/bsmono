//! End-to-end answer to the compatibility question: one live pointer set per
//! frame goes into `PointerRouter::update`; the nav subset it returns goes
//! verbatim into the *real* `touchkit::GestureRecognizer::update`. A drawing
//! finger produces ink and touchkit stays silent; a two-finger pinch produces
//! no ink and touchkit emits `Pinch` with a sensible cumulative scale.
//!
//! This is the exact loop the egui adapter sketch runs, minus egui.

use ink_kit::route::{InkEvent, PointerKind, PointerRouter, RawPointer};
use ink_kit::Vec2f;
use touchkit::{Gesture, GestureRecognizer, Touch};

fn finger(id: u64, x: f32, y: f32) -> RawPointer {
    RawPointer {
        id,
        pos: Vec2f::new(x, y),
        pressure: None,
        kind: PointerKind::Touch,
    }
}

/// One frame of the combined loop.
fn step(
    router: &mut PointerRouter,
    rec: &mut GestureRecognizer,
    live: &[RawPointer],
    t: f64,
) -> (Vec<InkEvent>, Vec<Gesture>) {
    let frame = router.update(live, t);
    let nav: Vec<Touch> = frame
        .nav
        .iter()
        .map(|(id, p)| Touch::new(*id, touchkit::Vec2::new(p.x, p.y)))
        .collect();
    let gestures = rec.update(&nav, t);
    (frame.ink, gestures)
}

#[test]
fn drawing_finger_inks_and_touchkit_stays_silent() {
    let mut router = PointerRouter::default();
    let mut rec = GestureRecognizer::default();

    let mut ink_events = 0;
    let mut gestures = 0;
    for i in 0..20 {
        let t = i as f64 * 16.0;
        let (ink, g) = step(
            &mut router,
            &mut rec,
            &[finger(1, i as f32 * 10.0, 40.0)],
            t,
        );
        ink_events += ink.len();
        gestures += g.len();
    }
    let (ink, g) = step(&mut router, &mut rec, &[], 20.0 * 16.0);
    ink_events += ink.len();
    gestures += g.len();

    assert!(ink_events > 10, "stroke captured ({} events)", ink_events);
    assert!(ink.contains(&InkEvent::End));
    assert_eq!(gestures, 0, "touchkit never saw the drawing finger");
}

#[test]
fn two_finger_pinch_reaches_touchkit_with_no_ink() {
    let mut router = PointerRouter::default();
    let mut rec = GestureRecognizer::default();

    let mut any_ink = false;
    let mut last_scale = 1.0f32;
    let mut saw_pinch = false;

    // Both fingers land within the grace window, then spread apart 2x.
    for i in 0..30 {
        let t = i as f64 * 16.0;
        let spread = 50.0 + i as f32 * 5.0; // 50 → 195 half-distance
        let live = [
            finger(1, 200.0 - spread, 300.0),
            finger(2, 200.0 + spread, 300.0),
        ];
        let (ink, gestures) = step(&mut router, &mut rec, &live, t);
        any_ink |= !ink.is_empty();
        for g in gestures {
            if let Gesture::Pinch { scale, .. } = g {
                saw_pinch = true;
                last_scale = scale;
            }
        }
    }

    assert!(!any_ink, "pinch produced no ink");
    assert!(saw_pinch, "touchkit recognized the pinch through the router");
    assert!(
        last_scale > 2.0,
        "cumulative scale grew (got {})",
        last_scale
    );
}

#[test]
fn one_finger_then_second_switches_streams_exactly_once() {
    let mut router = PointerRouter::default();
    let mut rec = GestureRecognizer::default();

    // Committed finger stroke…
    let _ = step(&mut router, &mut rec, &[finger(1, 0.0, 0.0)], 0.0);
    let (ink, _) = step(&mut router, &mut rec, &[finger(1, 30.0, 0.0)], 16.0);
    assert!(ink
        .iter()
        .any(|e| matches!(e, InkEvent::Begin { .. })));

    // …second finger: default policy cancels to nav; touchkit takes over.
    let mut cancels = 0;
    let mut nav_frames = 0;
    for i in 2..12 {
        let t = i as f64 * 16.0;
        let d = i as f32 * 8.0;
        let live = [finger(1, 30.0 + d, 0.0), finger(2, 300.0 - d, 0.0)];
        let (ink, _g) = step(&mut router, &mut rec, &live, t);
        cancels += ink.iter().filter(|e| **e == InkEvent::Cancel).count();
        let frame_had_nav = !router_probe_nav_empty(&mut router, &live, t);
        if frame_had_nav {
            nav_frames += 1;
        }
    }
    assert_eq!(cancels, 1, "exactly one Cancel at the handoff");
    assert!(nav_frames > 0);
}

/// Peek helper: re-running update would double-feed, so probe via a clone of
/// the observable effect instead — the router reports nav in the same frame,
/// which the loop above already consumed. This just asserts the *next* frame
/// with the same contacts still routes to nav (mode is sticky until all lift).
fn router_probe_nav_empty(router: &mut PointerRouter, live: &[RawPointer], t: f64) -> bool {
    router.update(live, t + 0.001).nav.is_empty()
}
