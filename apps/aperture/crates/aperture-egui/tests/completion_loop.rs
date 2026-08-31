//! End-to-end through real threads: AppState → Engine (worker) → provider →
//! outcome → ghost → Tab. The unit tests inject outcomes; this one earns them
//! through the actual channels, so the wiring between the three crates is
//! covered, not just each crate alone. No display needed — this is exactly
//! what the state/view split buys.

use aperture_egui::state::{AppState, Ev, K, ProviderKind};
use std::time::{Duration, Instant};

#[test]
fn demo_provider_completes_fn_main_through_the_real_engine() {
    let mut s = AppState::new(None, false);
    s.provider_kind = ProviderKind::Demo;
    s.engine = Some(aperture_complete::engine::Engine::new(s.build_provider()));

    let mut t = 0u64;
    for ch in "fn main".chars() {
        s.handle(Ev::Text(ch.to_string()), t);
        t += 40;
    }
    s.tick_completion(t + 300); // debounce (280ms) elapsed → request goes out
    assert!(s.ghost.is_pending());

    // The worker thread answers on its own schedule; poll like the view does.
    let deadline = Instant::now() + Duration::from_secs(5);
    while !s.ghost.is_shown() && Instant::now() < deadline {
        s.poll_completions(t);
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(s.ghost.is_shown(), "demo provider should answer `fn main`");
    assert_eq!(s.loop_log.len(), 1, "the loop pane saw the round trip");
    assert!(s.loop_log[0].ok);

    // Tab types ONE BIT. Pressed until the typeout is exhausted, because
    // this test is about the completion LOOP — provider to buffer — not
    // about the granularity of acceptance, which state.rs tests
    // directly. Bounded rather than `while`, so a typeout that never
    // finishes fails here instead of hanging CI with no message.
    for i in 0..64 {
        s.handle(Ev::Key { key: K::Tab, ctrl: false, shift: false, alt: false },
                 t + 400 + i * 10);
        if s.typeout.is_none() {
            break;
        }
    }
    let text = s.editor.buffer.to_string();
    assert!(
        text.starts_with("fn main() {") && text.contains("hello from aperture"),
        "accepted completion landed in the buffer: {text:?}"
    );

    // Undo is one gesture PER BIT, not one for the whole completion.
    // That is the typeout design and it is deliberate: taking a
    // suggestion a piece at a time is only useful if you can also give
    // back a piece at a time.
    //
    // Bounded, and asserted to have actually got there — a loop that
    // ran out would otherwise pass silently as "well, it stopped".
    for i in 0..64 {
        if s.editor.buffer.to_string() == "fn main" {
            break;
        }
        s.handle(Ev::Key { key: K::Z, ctrl: true, shift: false, alt: false },
                 t + 500 + i * 10);
    }
    assert_eq!(s.editor.buffer.to_string(), "fn main");
}
