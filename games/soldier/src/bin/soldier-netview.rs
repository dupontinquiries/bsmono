//! soldier-netview — run the netview server with a synthetic, moving snapshot so the shared
//! webhost-kit SocketAdapter can be exercised end-to-end without a display or a full game loop.
//!
//!     cargo run --bin soldier-netview            # serves 127.0.0.1:7788
//!     cargo run --bin soldier-netview 0.0.0.0:7788
//!
//! Then point the kit at it:
//!     python3 shared/webhost-kit/webserve.py     # (with a SocketAdapter("soldier") game)
//! or hit it directly:  printf '{"cmd":"snapshot"}\n' | nc 127.0.0.1 7788

use early_fps::netview::{self, Actor, SnapshotView};
use std::time::Instant;

fn main() {
    let addr = std::env::args().nth(1).unwrap_or_else(|| "127.0.0.1:7788".to_string());
    println!("soldier netview demo serving on {addr} (Ctrl-C to stop)");
    let start = Instant::now();
    let res = netview::serve(addr, move || {
        // A player orbiting the origin + one enemy — enough to see motion in a browser.
        let t = start.elapsed().as_secs_f32();
        SnapshotView {
            ok: true,
            title: "soldier",
            time: t,
            score: (t as u32) * 100,
            kills: t as u32,
            wave: 1,
            player: Actor { x: t.cos() * 5.0, y: 1.7, z: t.sin() * 5.0, hp: 100.0 },
            enemies: vec![Actor { x: 8.0, y: 0.0, z: 0.0, hp: 40.0 }],
        }
    });
    if let Err(e) = res {
        eprintln!("netview server error: {e}");
        std::process::exit(1);
    }
}
