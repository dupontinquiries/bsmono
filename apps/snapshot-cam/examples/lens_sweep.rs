//! lens_sweep -- drive the real lens motor the way the app does, to listen for the click.
//!
//! `lens_sweep <subdev> glide|jump` makes three round trips 0 <-> 1000 with a pause between moves, printing the
//! milliseconds since start at each move so a simultaneous microphone recording can be lined up. `glide` is
//! `tapfocus::IoctlFocus` through `lensmove::Mover` (what V4l2Lens now does); `jump` is one write per move (what it did).
//! Mirrors `tapfocus::V4l2Lens::new`'s worker. Opens only the motor's sub-device, never libcamera.

use snapshot_cam::lensmove::{self, FocusWrite, Mover};
use snapshot_cam::tapfocus::IoctlFocus;
use std::time::{Duration, Instant};

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let (dev, mode) = (a.get(1).expect("subdev"), a.get(2).map(String::as_str).unwrap_or("glide"));
    let mut f = IoctlFocus::open(dev).expect("open");
    let start = Instant::now();
    let mut mover = Mover::new(f.read().expect("read position"));
    println!("start position {}", mover.position());
    for target in [0, 1000, 0, 1000, 0, 1000, 0] {
        println!("{} ms move to {target}", start.elapsed().as_millis());
        if mode == "jump" { f.write(target).expect("write"); mover = Mover::new(target); }
        else { lensmove::glide(&mut mover, &mut f, target, |t| t, |ms| std::thread::sleep(Duration::from_millis(ms))).expect("glide"); }
        std::thread::sleep(Duration::from_millis(1200));
    }
    println!("{} ms done", start.elapsed().as_millis());
}
