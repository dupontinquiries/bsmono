//! `cargo run --bin skirmish -- <facA> <facB> [--frames DIR]`
//! Fights two factions on the ported Rust rules and prints the tale.

use paperbound_core::data::load_factbook;
use paperbound_core::origami::ModelBook;
use paperbound_core::skirmish::run_skirmish_with;
use std::path::{Path, PathBuf};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let a = args.get(1).cloned().unwrap_or_else(|| "swarm".into());
    let b = args.get(2).cloned().unwrap_or_else(|| "rootbound".into());
    let frames: Option<PathBuf> = args
        .iter()
        .position(|x| x == "--frames")
        .and_then(|i| args.get(i + 1))
        .map(PathBuf::from);
    // the crate sits at games/paperbound/rust/paperbound-core
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let book = load_factbook(&dir).expect("load rts_data.json + expansions");
    if let Some(d) = &frames {
        std::fs::create_dir_all(d).ok();
    }
    println!("⚔  {} vs {} — Foldworks rules, Rust engine", book[&a].name, book[&b].name);
    // frames wear the real folded card models when the book loads
    let models = ModelBook::load(&dir).ok();
    let rep = run_skirmish_with(&book, &a, &b, 900.0, frames.as_deref(), 10.0,
                                models.as_ref());
    let mm = (rep.t / 60.0) as u64;
    let ss = (rep.t % 60.0) as u64;
    match rep.winner {
        Some(side) => {
            let name = if side == 0 { &book[&a].name } else { &book[&b].name };
            println!("🏆 {} wins at {}:{:02}", name, mm, ss);
        }
        None => println!("⏱  no decision by {}:{:02}", mm, ss),
    }
    println!(
        "   ink {:.0} / {:.0} · units left {} / {}{}",
        rep.ink[0],
        rep.ink[1],
        rep.units_left[0],
        rep.units_left[1],
        if rep.frames.is_empty() {
            String::new()
        } else {
            format!(" · {} frames rendered", rep.frames.len())
        }
    );
}
