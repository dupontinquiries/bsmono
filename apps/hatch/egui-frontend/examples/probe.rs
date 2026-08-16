//! probe — the launcher's internal state as text.
//!
//! ```sh
//! cargo run --example probe                 # empty query: the default grid
//! cargo run --example probe -- fire         # what typing "fire" surfaces
//! cargo run --example probe -- --icons fire # plus resolved icon paths
//! ```
//!
//! This exists because the interesting failures in a launcher are not visual:
//! an app missing from the grid, a wrong entry ranked first, an icon name
//! that resolves nowhere. All of those are answerable without a window, and
//! answering them here — in a binary that builds in seconds against the lib
//! only, no eframe — is far faster than launching the shell and squinting.
//! It is also the honest test of hatch-core against a REAL /usr/share
//! tree rather than the temp-dir fixtures the unit tests use.

use hatch_core::{Config, Launcher};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let show_icons = args.iter().any(|a| a == "--icons");
    let query = args
        .iter()
        .find(|a| !a.starts_with("--"))
        .cloned()
        .unwrap_or_default();

    let cfg = Config::load();
    println!(
        "config: columns={} icon_px={} theme={} actions={:?}",
        cfg.columns, cfg.icon_px, cfg.icon_theme, cfg.system_actions
    );

    // What the plasmoid side thinks the favorites are. Printed rather than
    // adopted here: the probe must not mutate the config it is reporting on.
    let appletsrc = hatch_core::plasma::appletsrc_path();
    match hatch_core::plasma::read_favorites(&appletsrc) {
        Some(f) if !f.is_empty() => println!("plasma favorites ({}): {:?}", f.len(), f),
        Some(_) => println!("plasma favorites: appletsrc present, none set"),
        None => println!("plasma favorites: no appletsrc at {}", appletsrc.display()),
    }

    let t0 = std::time::Instant::now();
    let mut l = Launcher::new(cfg);
    let scan_ms = t0.elapsed().as_millis();
    let total = l.result_count();

    l.set_query(&query);
    println!(
        "scanned {total} entries in {scan_ms}ms; query {:?} → {} results\n",
        query,
        l.result_count()
    );

    let mut missing_icons = 0;
    for (n, e) in l.results().take(40).enumerate() {
        let fav = if l.is_favorite(&e.id) { "*" } else { " " };
        println!("{n:>3}{fav} {:<32} {}", trunc(&e.name, 32), trunc(&e.id, 40));
        if show_icons {
            match l.icon_path(e) {
                Some(p) => println!("       icon {:?} → {}", e.icon, p.display()),
                None => {
                    missing_icons += 1;
                    println!("       icon {:?} → UNRESOLVED (letter tile)", e.icon);
                }
            }
        }
    }
    if show_icons {
        println!("\n{missing_icons} of the shown entries fall back to letter tiles");
    }

    println!("\nsystem row:");
    for a in l.system_actions() {
        let icon = l
            .icon_path_for_name(a.icon)
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "UNRESOLVED".into());
        println!("  {:<9} {:?} → {icon}", a.id, a.argv);
    }
}

fn trunc(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        s.chars().take(n - 1).chain(['…']).collect()
    }
}
