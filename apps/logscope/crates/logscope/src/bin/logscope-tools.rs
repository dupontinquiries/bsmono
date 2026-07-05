//! logscope-tools — logscope's log-analysis primitives on the command line.
//!
//! logscope began life as a log-analysis tool; these primitives (`probe`, `shorten`) predate the
//! GUI and run headless. Exposing them via cli-kit gives CLI parity with what the app does
//! visually and is the first step toward branching them into focused standalone tools.
//!
//!   logscope-tools probe   <logfile>                 structure / ownership / rotation / notes
//!   logscope-tools shorten [<logfile>] [--stats]     collapse repeated line-prefixes (stdin if no file)
//!            [--min-prefix N] [--min-repeat N] [--window N]

use cli_kit::Cli;
use logscope::probe::probe;
use logscope::shorten::{shorten, ShortenConfig};
use std::io::Read;
use std::path::Path;

fn usage() -> ! {
    eprintln!(
        "logscope-tools — log-analysis primitives on the command line\n\n\
         probe   <logfile>                             structure, ownership, rotation backend, notes\n\
         shorten [<logfile>] [--stats]                 collapse repeated line-prefixes (reads stdin if no file)\n\
                 [--min-prefix N] [--min-repeat N] [--window N]"
    );
    std::process::exit(2);
}

fn read_source(cli: &Cli) -> String {
    match cli.rest().first() {
        Some(p) => std::fs::read_to_string(p).unwrap_or_else(|e| {
            eprintln!("can't read {p}: {e}");
            std::process::exit(1);
        }),
        None => {
            let mut s = String::new();
            let _ = std::io::stdin().read_to_string(&mut s);
            s
        }
    }
}

fn main() {
    let cli = Cli::from_env();
    match cli.subcommand() {
        Some("probe") => {
            let path = *cli.rest().first().unwrap_or_else(|| usage());
            let r = probe(Path::new(path));
            println!("path:      {}", r.path.display());
            println!("tier:      {:?}", r.tier);
            println!("rotation:  {:?}", r.rotation_backend);
            println!("size:      {} bytes", r.size_bytes);
            println!("owner:     {} ({}:{})", r.owner_name.as_deref().unwrap_or("?"), r.owner_uid, r.owner_gid);
            println!("perms:     {}", r.permissions_octal);
            println!("writable:  {}", r.is_writable_by_us);
            println!("symlink:   {}", r.is_symlink);
            if let Some(m) = r.modified {
                println!("modified:  {m}");
            }
            for n in &r.notes {
                println!("note:      {n}");
            }
        }
        Some("shorten") => {
            let cfg = ShortenConfig {
                min_prefix_len: cli.opt("min-prefix").and_then(|s| s.parse().ok()).unwrap_or(6),
                min_repeat_count: cli.opt("min-repeat").and_then(|s| s.parse().ok()).unwrap_or(2),
                window: cli.opt("window").and_then(|s| s.parse().ok()).unwrap_or(20),
            };
            let text = read_source(&cli);
            let lines: Vec<&str> = text.lines().collect();
            let res = shorten(&lines, &cfg);
            for l in &res.lines {
                println!("{l}");
            }
            if cli.flag("stats") {
                eprintln!(
                    "# {} -> {} lines, {} chars removed",
                    res.stats.original_lines, res.stats.output_lines, res.stats.chars_removed
                );
            }
        }
        _ => usage(),
    }
}
