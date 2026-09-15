//! jsoncheck — validate JSON and (by default) pretty-print it. Pure Rust, no deps.
//!
//!   jsoncheck <file>        # validate + pretty-print to stdout
//!   jsoncheck --check <file> # validate only (exit non-zero if invalid)
//!
//! The parser used to live here. It now lives in `bot_arena::json` so the lib
//! can read conversation exports with it too — and the move fixed a real bug
//! this tool could never have shown: `\uXXXX` escapes were passed through
//! undecoded, which hand-written config never contains and machine-generated
//! exports contain by the thousand. See that module's header.

use std::path::Path;

use bot_arena::json::{parse, pretty};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let check_only = args.iter().any(|a| a == "--check");
    let file = args.iter().find(|a| !a.starts_with('-'));
    let Some(file) = file else { eprintln!("usage: jsoncheck [--check] <file>"); std::process::exit(2); };
    let src = match std::fs::read_to_string(Path::new(file)) {
        Ok(s) => s, Err(e) => { eprintln!("cannot read {file}: {e}"); std::process::exit(1); }
    };
    match parse(&src) {
        Ok(v) => { if check_only { println!("valid"); } else { println!("{}", pretty(&v, 0)); } }
        Err(e) => { eprintln!("invalid JSON: {e}"); std::process::exit(1); }
    }
}
