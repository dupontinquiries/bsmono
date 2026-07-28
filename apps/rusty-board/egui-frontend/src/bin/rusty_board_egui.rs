//! rusty_board_egui.rs — the deployable egui rusty-board app.
//!
//! The screen itself is `rusty_board_egui::demo_app` so this binary and `examples/feel.rs`
//! cannot drift apart (they did, when each carried its own copy).
//!
//! Run:  cargo run --bin rusty-board-egui --features app   (from apps/rusty-board/egui-frontend)

fn main() -> eframe::Result<()> {
    rusty_board_egui::demo_app::run("rusty-board")
}
