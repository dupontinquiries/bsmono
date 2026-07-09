//! tilt-source — sensor-kit's reference driver and the shared bridge for games that aren't written
//! in Rust. It reads iio-sensor-proxy's `monitor-sensor` stream and emits a 2-D gravity vector, so
//! the tilt parsing + control model live in one tested crate instead of being re-derived per game.
//!
//! Pipe it between the sensor and the game:
//!
//!     monitor-sensor --accel 2>&1 | tilt-source | python3 ball_maze.py
//!
//! Output: one `"<gx> <gy>\n"` line per sensor change, where (gx, gy) is the sustained gravity
//! **direction** in screen coordinates (+y = down), unit-scaled — the consumer multiplies by its own
//! gravity constant. `monitor-sensor` is a *coarse change-event* source, so this uses sensor-kit's
//! sustained-hold model ([`TiltControl::hold_gravity`]); a continuous sample source would instead
//! drive the impulse model (`tick`). Emitting per change (not on a timer) matches the source's
//! cadence and keeps the process idle between events.

use sensor_kit::TiltControl;
use std::io::{BufRead, Write};

fn main() {
    let mut ctl = TiltControl::new();
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();

    for line in stdin.lock().lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => break, // sensor pipe error
        };
        // Only emit when the line actually changed tilt/orientation (feed_line returns true then).
        if ctl.feed_line(&line) {
            let g = ctl.hold_gravity();
            let mut out = stdout.lock();
            if writeln!(out, "{:.4} {:.4}", g.x, g.y).is_err() {
                break; // the game closed the pipe
            }
            let _ = out.flush(); // flush so the game sees each change immediately
        }
    }
}
