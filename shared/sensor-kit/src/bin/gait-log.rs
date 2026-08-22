//! gait-log — sensor-kit's second reference driver, for the *sample-stream* layer that
//! tilt-source's change-event layer can't reach. Pipe raw accelerometer samples in (one
//! `"<x> <y> <z>"` line per sample, m/s², post-scale), get a gait log out:
//!
//!     iio-accel-cat 50 | gait-log 50
//!
//! Output lines, tab-separated, flushed per event so it's watchable live:
//!
//!     step\t<total>\t<carry-state>
//!     carry\t<from>\t<to>\t<sample-index>
//!
//! Every carry transition retunes the step detector via `step_tuning_for` — this bin IS the
//! wiring diagram for the stepd daemon, kept runnable so thresholds can be tuned against a
//! recorded walk file long before a daemon exists:
//!
//!     gait-log 50 < fp4_pocket_walk.xyz
//!
//! The argument is the sample rate in Hz (default 50).

use sensor_kit::carry::{step_tuning_for, CarryFsm, CarryGuards};
use sensor_kit::gait::{Sample, StepDetector, StepTuning, Windower};
use std::io::{BufRead, Write};

fn main() {
    let hz: f64 = std::env::args().nth(1).and_then(|a| a.parse().ok()).unwrap_or(50.0);
    let mut windower = Windower::new(hz, 2.0, 0.5);
    let mut fsm = CarryFsm::new(CarryGuards::default(), 3);
    let mut steps = StepDetector::new(hz, StepTuning::default());

    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let mut i: u64 = 0;

    for line in stdin.lock().lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => break,
        };
        let mut it = line.split_whitespace().filter_map(|t| t.parse::<f64>().ok());
        let s = match (it.next(), it.next(), it.next()) {
            (Some(x), Some(y), Some(z)) => Sample { x, y, z },
            _ => continue, // headers, blanks, partial lines
        };
        i += 1;

        if steps.feed(s) {
            let mut out = stdout.lock();
            if writeln!(out, "step\t{}\t{}", steps.steps(), fsm.state().as_str()).is_err() {
                break;
            }
            let _ = out.flush();
        }
        if let Some(f) = windower.push(s) {
            if let Some(t) = fsm.step(&f) {
                steps.retune(step_tuning_for(t.to));
                let mut out = stdout.lock();
                if writeln!(out, "carry\t{}\t{}\t{}", t.from.as_str(), t.to.as_str(), i)
                    .is_err()
                {
                    break;
                }
                let _ = out.flush();
            }
        }
    }
}
