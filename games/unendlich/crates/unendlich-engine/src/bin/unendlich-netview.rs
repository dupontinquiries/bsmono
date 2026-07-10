//! unendlich-netview — drive the real rollback engine headlessly and serve its snapshot to the shared
//! webhost-kit, so the deterministic sim can be spectated (and its rollback state inspected) in a
//! browser without the Vulkan renderer.
//!
//!     cargo run --no-default-features --bin unendlich-netview            # 127.0.0.1:7799
//!     cargo run --no-default-features --bin unendlich-netview 0.0.0.0:7799
//!
//! Then point the kit's SocketAdapter("unendlich") at it, or:
//!     printf '{"cmd":"snapshot"}\n' | nc 127.0.0.1 7799

use netview::serve;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use unendlich_engine::sim::fixed::Fixed;
use unendlich_engine::sim::netview::snapshot;
use unendlich_engine::sim::physics::{BlockPhysics, WorldQuery};
use unendlich_engine::sim::rollback::Rollback;
use unendlich_engine::sim::state::{Input, SimState};

/// Trivial world: solid floor below y=0, air above — enough for characters to stand and walk.
struct Flat;
impl WorldQuery for Flat {
    fn physics_at(&self, _x: i32, y: i32, _z: i32) -> BlockPhysics {
        if y < 0 {
            BlockPhysics::solid(Fixed::from_frac(6, 10))
        } else {
            BlockPhysics::AIR
        }
    }
}

fn main() {
    let addr = std::env::args().nth(1).unwrap_or_else(|| "127.0.0.1:7799".to_string());
    let rb = Arc::new(Mutex::new(Rollback::new(2, SimState::new(2), 8)));

    // Advance thread: player 0 walks forward while turning, player 1 stays put — both inputs confirmed
    // each tick, so the baseline marches and the snapshot shows a healthy (predicted≈0) rollback.
    let sim = Arc::clone(&rb);
    std::thread::spawn(move || {
        let world = Flat;
        let mut yaw: u16 = 0;
        loop {
            {
                let mut r = sim.lock().unwrap();
                let t = r.current_tick();
                yaw = yaw.wrapping_add(200);
                let walk = Input { buttons: 0, move_x: Fixed::ZERO, move_z: Fixed::from_frac(1, 2), yaw };
                r.set_input(t, 0, walk, &world);
                r.set_input(t, 1, Input::default(), &world);
                r.advance(&world);
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    });

    println!("unendlich netview demo serving on {addr} (Ctrl-C to stop)");
    let read = Arc::clone(&rb);
    if let Err(e) = serve(addr, move || snapshot(&read.lock().unwrap())) {
        eprintln!("netview server error: {e}");
        std::process::exit(1);
    }
}
