//! mkid — generate (or load) a machine identity at a given path and print its id.
//!
//! For testing `lanpoll`/`trustkit` with a genuinely second identity, without a second real
//! machine: `cargo run -p logscope-fan --example mkid -- /tmp/second-identity.key` prints an id you
//! can register as trusted, then pair with `announce_loop` (same example dir) running against that
//! same identity file to exercise the real sign → announce → verify → accept path end to end.
fn main() {
    let path = std::env::args().nth(1).expect("path arg");
    let id = logscope_fan::machineid::Identity::load_or_create(std::path::Path::new(&path)).expect("identity");
    println!("{}", id.id());
}
