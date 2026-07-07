//! score_compare — put the old gameable grade next to the new composition-based "honest" grade, so the gap
//! between metric and human judgement is visible. The claim to check: procedural maps whose OLD grade is
//! inflated by density should DROP under the composition metric, while genuinely-designed authored maps hold.
//!
//! Usage: cargo run --example score_compare --release [-- <name-substr>]

use early_fps::composition;
use early_fps::level::{Level, PRESETS};
use early_fps::mapeval;
use early_fps::sightline;

fn main() {
    let filter: Option<String> = std::env::args().skip(1).find(|a| !a.starts_with("--")).map(|s| s.to_lowercase());

    println!(
        "{:<22} {:>5} {:>6}   {:>5} {:>6} {:>6} {:>6} {:>6} {:>6}",
        "MAP", "OLD", "old#", "NEW", "new#", "clust", "void", "hier", "noise"
    );
    println!("{}", "─".repeat(88));

    // authored maps first so they're easy to eyeball against the procedural top rows
    let authored = ["Flood Zone", "Reference Town", "Desert Town", "Pyre Garage", "Concrete Slab", "Redwood Canyon", "Malecon Breakwater"];

    let mut rows: Vec<(String, char, f32, char, f32, f32, f32, f32, f32, bool)> = Vec::new();
    for p in PRESETS.iter() {
        if let Some(f) = &filter {
            if !p.name.to_lowercase().contains(f.as_str()) {
                continue;
            }
        }
        let level = Level::generate(p.config(7));
        let old = mapeval::score(&level);
        let comp = composition::analyze(&level);
        let hq = composition::honest_quality(&level);
        let hg = composition::honest_grade(&level);
        let is_auth = authored.contains(&p.name);
        rows.push((p.name.to_string(), old.grade(), old.total / 100.0, hg, hq, comp.clustering, comp.void_structure, comp.landmark_hierarchy, comp.noise_likeness, is_auth));
    }

    // print authored maps, then the rest sorted by how far the grade DROPPED (biggest inflation first)
    let grade_val = |g: char| match g {
        'A' => 5,
        'B' => 4,
        'C' => 3,
        'D' => 2,
        'F' => 1,
        _ => 0,
    };
    let mut auth: Vec<_> = rows.iter().filter(|r| r.9).cloned().collect();
    let mut proc: Vec<_> = rows.iter().filter(|r| !r.9).cloned().collect();
    proc.sort_by_key(|r| grade_val(r.3) as i32 - grade_val(r.1) as i32); // most-demoted first
    let _ = &mut auth;

    let print_row = |r: &(String, char, f32, char, f32, f32, f32, f32, f32, bool)| {
        let arrow = if grade_val(r.3) < grade_val(r.1) { " ↓" } else if grade_val(r.3) > grade_val(r.1) { " ↑" } else { "  " };
        println!(
            "{:<22} {:>4}{} {:>5.2}   {:>4}{} {:>5.2} {:>6.2} {:>6.2} {:>6.2} {:>6.2}",
            r.0, r.1, arrow, r.2, r.3, if grade_val(r.3) < grade_val(r.1) { "↓" } else { " " }, r.4, r.5, r.6, r.7, r.8
        );
    };

    println!("── authored maps ──");
    for r in &auth {
        print_row(r);
    }
    println!("── procedural maps (sorted: biggest grade drop first) ──");
    for r in proc.iter().take(20) {
        print_row(r);
    }

    println!("{}", "─".repeat(88));
    let _ = sightline::analyze;
    println!("OLD = mapeval grade (gameable proxies). NEW = composition-based honest grade.");
    println!("clust=clustering void=void-structure hier=landmark-hierarchy noise=noise-likeness (1=looks random).");
    println!("A ↓ means the map's old grade was inflated relative to how *composed* it actually is.");
}
