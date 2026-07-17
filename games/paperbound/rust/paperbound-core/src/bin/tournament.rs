//! tournament — run a full Foldworks bracket on the Rust engine, every match
//! actually fought. Uses the monorepo's shared cli-kit.
//!
//!   cargo run --bin tournament                      # all factions
//!   cargo run --bin tournament -- court ember tide  # a chosen field
//!   cargo run --bin tournament -- --save DIR        # persist the bracket

use cli_kit::Cli;
use paperbound_core::data::load_factbook;
use paperbound_core::tournament::Bracket;
use std::path::{Path, PathBuf};

fn main() {
    let cli = Cli::from_env();
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let book = load_factbook(&dir).expect("game data");
    let mut entrants: Vec<String> = cli.positionals().iter().map(|s| s.to_string()).collect();
    if entrants.is_empty() {
        entrants = book.keys().cloned().collect();
    }
    for e in &entrants {
        if !book.contains_key(e) {
            eprintln!("unknown faction '{e}' — the book has: {:?}", book.keys().collect::<Vec<_>>());
            std::process::exit(1);
        }
    }
    println!("🏟  Foldworks tournament — {} entrants, every match fought", entrants.len());
    let (champ, tales) = Bracket::run(entrants, &book);
    for (a, b, w) in &tales {
        let (an, bn, wn) = (&book[a].name, &book[b].name, &book[w].name);
        println!("   ⚔  {an} vs {bn} → {wn}");
    }
    println!("👑 champion: {}", book[&champ].name);
    if let Some(save) = cli.opt("--save").map(PathBuf::from) {
        std::fs::create_dir_all(&save).ok();
        let br = Bracket::new(vec![champ.clone()]);
        br.save(&save.join("tournament.json")).ok();
        println!("   bracket saved to {}", save.display());
    }
}
