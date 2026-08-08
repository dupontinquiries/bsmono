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

    s.handle(Ev::Key { key: K::Tab, ctrl: false, shift: false, alt: false }, t + 400);
    let text = s.editor.buffer.to_string();
    assert!(
        text.starts_with("fn main() {") && text.contains("hello from aperture"),
        "accepted completion landed in the buffer: {text:?}"
    );

    // And the acceptance is one undo gesture.
    s.handle(Ev::Key { key: K::Z, ctrl: true, shift: false, alt: false }, t + 500);
    assert_eq!(s.editor.buffer.to_string(), "fn main");
}
