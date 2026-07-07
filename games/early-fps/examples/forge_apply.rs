//! forge_apply.rs — apply a JSON build script (see `buildscript.rs`) to a saved `.map` file.
//!
//!   cargo run --example forge_apply -- path/to/script.json           # build + save
//!   cargo run --example forge_apply -- path/to/script.json --dry-run # build + report only, don't save
//!
//! This is the actual entry point external tooling (or an LLM given the schema in `buildscript.rs`'s doc
//! comment) would use: write a JSON file, run this, get a playable `.map` in the game's maps directory.

use early_fps::buildscript::{apply, from_json};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let Some(path) = args.get(1) else {
        eprintln!("usage: forge_apply <script.json> [--dry-run]");
        std::process::exit(2);
    };
    let dry_run = args.iter().any(|a| a == "--dry-run");

    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("couldn't read {path}: {e}");
            std::process::exit(1);
        }
    };
    let doc = match from_json(&text) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("invalid build script: {e}");
            std::process::exit(1);
        }
    };
    let (map, errors) = apply(&doc);
    for e in &errors {
        eprintln!("warning: {e}");
    }
    println!("built '{}' — theme={:?} size={} placed={} ({} warning(s))", map.name, map.theme, map.size, map.placed.len(), errors.len());

    if dry_run {
        println!("(dry run — not saved)");
        return;
    }
    if map.save() {
        println!("saved to {}", early_fps::builder::CustomMap::dir().display());
    } else {
        eprintln!("save failed (is the maps directory writable?)");
        std::process::exit(1);
    }
}
