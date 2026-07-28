//! feel — the dev-iteration harness for the keyboard screen. Identical to the deploy binary
//! by construction: both are one call into `demo_app`.
//!
//! Run:  cargo run --example feel

fn main() -> eframe::Result<()> {
    rusty_board_egui::demo_app::run("rusty-board · feel")
}
