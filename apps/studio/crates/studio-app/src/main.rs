//! The Studio binary.
//!
//! One line, like every other frontend in the fleet: the screen lives in the
//! library so the binary and any example cannot drift apart — they did in
//! rusty-board when each carried its own copy.

fn main() -> eframe::Result<()> {
    studio_app::run("Studio")
}
