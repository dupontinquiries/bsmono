//! announce_loop — stand-alone LAN announcer for testing `lanpoll` against a real second identity.
//!
//! `LanWire::bind()` refuses two binders on one host on purpose (see lanwire.rs's own doc) — so this
//! and the app under test cannot run on the SAME machine at the same time; run this on a second real
//! host on the LAN (or a separate network namespace) with an id made by the `mkid` example, register
//! that id as trusted in the app, and its Models tab should show it "on LAN" within one INTERVAL_S.
use std::time::{SystemTime, UNIX_EPOCH};
fn main() {
    let path = std::env::args().nth(1).expect("identity path");
    let id = logscope_fan::machineid::Identity::load_or_create(std::path::Path::new(&path)).expect("identity");
    let wire = logscope_fan::lanwire::LanWire::bind().expect("bind");
    eprintln!("announcing as {}", id.id());
    loop {
        let now_s = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
        let a = logscope_fan::announce::Announce { id: id.id(), addr: "127.0.0.1:0".into(), sent_at_s: now_s };
        let sig = id.sign(&a.payload());
        let _ = wire.announce(&a, &sig);
        std::thread::sleep(std::time::Duration::from_secs(2));
    }
}
