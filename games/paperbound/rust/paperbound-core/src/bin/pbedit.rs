//! pbedit — headless editor CLI over paperbound's data files, on the
//! monorepo's shared cli-kit (the same argument grammar as the rest of the
//! portfolio).
//!
//!   pbedit list factions [--file PATH]
//!   pbedit list maps     [--file PATH]
//!   pbedit get  court.units.soldier.dmg
//!   pbedit set  court.units.soldier.dmg 88 [--out PATH]
//!   pbedit validate [--file PATH]           # incl. every expansion pack

use cli_kit::Cli;
use paperbound_core::editor::{shell_exec, Editor};
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

fn game_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// The data file, resolved the fleet way: --file flag beats the config-kit
/// config (`~/.config/pbedit/pbedit.conf`, `[paths] data = …`) beats the
/// in-repo default.
fn resolve_file(cli: &Cli) -> PathBuf {
    if let Some(f) = cli.opt("--file") {
        return PathBuf::from(f);
    }
    let ini = config_kit::Ini::parse(&config_kit::load_or("pbedit", "pbedit.conf", ""));
    if let Some(p) = ini.get("paths", "data") {
        return PathBuf::from(p);
    }
    game_dir().join("rts_data.json")
}

fn main() {
    let cli = Cli::from_env();
    let file: PathBuf = resolve_file(&cli);
    let pos = cli.positionals();
    match pos.first().copied() {
        Some("list") => {
            let ed = Editor::open(&file).unwrap_or_else(|e| die(&e));
            let what = pos.get(1).copied().unwrap_or("factions");
            for k in ed.list(what) {
                println!("{k}");
            }
        }
        Some("get") => {
            let ed = Editor::open(&file).unwrap_or_else(|e| die(&e));
            let path = pos.get(1).unwrap_or_else(|| die("get needs a dotted path"));
            match ed.get(path) {
                Some(v) => println!("{v}"),
                None => die(&format!("nothing at '{path}'")),
            }
        }
        Some("set") => {
            let mut ed = Editor::open(&file).unwrap_or_else(|e| die(&e));
            let path = pos.get(1).unwrap_or_else(|| die("set needs a dotted path"));
            let val = pos.get(2).unwrap_or_else(|| die("set needs a value"));
            match ed.set(path, val) {
                Ok(old) => {
                    let out = cli.opt("--out").map(PathBuf::from);
                    ed.save(out.as_deref()).unwrap_or_else(|e| die(&e));
                    println!("{path}: {} → {val}", old.map(|o| o.to_string()).unwrap_or("∅".into()));
                }
                Err(e) => die(&e),
            }
        }
        Some("config") => {
            println!("config file : {}", config_kit::config_dir("pbedit").join("pbedit.conf").display());
            println!("data file   : {}", file.display());
            println!("backups     : {}", config_kit::data_dir("pbedit").join("backups").display());
        }
        Some("shell") => {
            let mut ed = Editor::open(&file).unwrap_or_else(|e| die(&e));
            println!("pbedit shell — editing {} ('help' for commands)", file.display());
            let stdin = std::io::stdin();
            let mut out = std::io::stdout();
            loop {
                print!("fold> ");
                out.flush().ok();
                let mut line = String::new();
                if stdin.lock().read_line(&mut line).unwrap_or(0) == 0 {
                    break; // EOF folds the shell too
                }
                let (msg, keep) = shell_exec(&mut ed, line.trim());
                if !msg.is_empty() {
                    println!("{msg}");
                }
                if !keep {
                    break;
                }
            }
        }
        Some("validate") => {
            let mut files = vec![file.clone()];
            if cli.opt("--file").is_none() {
                if let Ok(rd) = std::fs::read_dir(game_dir().join("expansions")) {
                    for e in rd.flatten() {
                        if e.path().extension().map(|x| x == "json").unwrap_or(false) {
                            files.push(e.path());
                        }
                    }
                }
            }
            let mut total = 0;
            for f in files {
                match Editor::open(&f) {
                    Ok(ed) => {
                        for finding in ed.validate() {
                            println!("{}: {finding}", f.file_name().unwrap().to_string_lossy());
                            total += 1;
                        }
                    }
                    Err(e) => {
                        println!("{}: unreadable: {e}", f.display());
                        total += 1;
                    }
                }
            }
            if total == 0 {
                println!("✅ all data validates clean");
            } else {
                std::process::exit(1);
            }
        }
        _ => {
            eprintln!("usage: pbedit <list|get|set|validate|shell|config> … [--file PATH] [--out PATH]");
            std::process::exit(2);
        }
    }
}

fn die(msg: &str) -> ! {
    eprintln!("pbedit: {msg}");
    std::process::exit(1)
}
