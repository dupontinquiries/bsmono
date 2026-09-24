//! Print an FL Studio project's inventory, read natively (`flp_read`).
//!     cargo run -p studio-app --example flp_read -- song.flp > song.flp.json
fn main() {
    let p = std::env::args().nth(1).expect("usage: flp_read FILE.flp");
    let inv = studio_app::flp_read::inventory(&std::fs::read(&p).expect("read")).unwrap_or_else(|e| panic!("{p}: {}", e.0));
    println!("{}", serde_json::to_string_pretty(&inv).unwrap());
}
