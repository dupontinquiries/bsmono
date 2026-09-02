//! rusty_board_egui.rs — the deployable egui rusty-board app.
//!
//! The screen itself is `rusty_board_egui::demo_app` so this binary and `examples/feel.rs`
//! cannot drift apart (they did, when each carried its own copy).
//!
//! Run:  cargo run --bin rusty-board-egui --features app   (from apps/rusty-board/egui-frontend)

fn main() -> eframe::Result<()> {
    // FIRST, before eframe/winit touch Wayland: KWin hands the input-method socket over as
    // WAYLAND_SOCKET, and a toolkit that sees it connects to the wrong socket for rendering.
    rusty_board_egui::platform::capture_inherited_socket();
    rusty_board_egui::demo_app::run("rusty-board")
}
