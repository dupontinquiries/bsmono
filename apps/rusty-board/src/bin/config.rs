//! `rusty-board-config` — small CLI to inspect/edit the user config.
//!
//! Usage:
//!     rusty-board-config show
//!     rusty-board-config path
//!     rusty-board-config reset

use anyhow::Result;
use rusty_board::config::Config;

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let cmd = args.get(1).map(String::as_str).unwrap_or("show");

    match cmd {
        "show" => {
            let cfg = Config::load_or_default()?;
            println!("{}", toml::to_string_pretty(&cfg)?);
        }
        "path" => {
            println!("{}", Config::config_path().display());
        }
        "reset" => {
            let cfg = Config::default();
            cfg.save()?;
            println!("Reset config at {}", Config::config_path().display());
        }
        other => {
            eprintln!("Unknown command: {}", other);
            eprintln!("Usage: rusty-board-config <show|path|reset>");
            std::process::exit(2);
        }
    }
    Ok(())
}
