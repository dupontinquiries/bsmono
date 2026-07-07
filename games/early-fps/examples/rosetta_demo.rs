//! rosetta_demo — the full text-spec → themed map pipeline.
//!
//! Reads `docs/design/map-specs.txt`, splits it into individual map briefs, extracts each one's theme target
//! (palette, archetype mix, density, water, verticality, mood), then runs the theme-guided evolutionary search
//! to find a net whose generated map best matches that target — and reports how close it got.
//!
//! Usage: cargo run --example rosetta_demo --release

use early_fps::composition;
use early_fps::procgen::{evolve_toward, generate_themed, ActivationNet, ProcParams};
use early_fps::rosetta::{archetype_labels, extract, match_score, split_specs};

fn main() {
    let doc = std::fs::read_to_string("docs/design/map-specs.txt").unwrap_or_default();
    if doc.is_empty() {
        eprintln!("could not read docs/design/map-specs.txt");
        return;
    }
    let blocks = split_specs(&doc);
    println!("parsed {} map spec blocks from the design doc\n", blocks.len());

    let params = ProcParams::default();
    // pick a few distinct specs to transfer (by name keyword)
    let wanted = ["Gondola", "Elevated Spine", "Archivist", "Hadal", "Iron Reef", "Karst"];
    let labels = archetype_labels();

    for block in &blocks {
        let target = extract(block);
        if !wanted.iter().any(|w| target.name.contains(w)) {
            continue;
        }
        println!("── {} ──", target.name);
        // show the extracted signature
        let pal = target.palette;
        println!(
            "  palette cover=({},{},{}) sky=({},{},{})  mood: {}",
            pal.cover.r(), pal.cover.g(), pal.cover.b(), pal.sky.r(), pal.sky.g(), pal.sky.b(), target.mood
        );
        print!("  wants: ");
        let mut pairs: Vec<(usize, f32)> = target.archetype_mix.iter().copied().enumerate().collect();
        pairs.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
        for (i, w) in pairs.iter().take(4) {
            print!("{}={:.2} ", labels[*i], w);
        }
        println!("| density={:.2} water={:.2} vert={:.2}", target.density, target.water, target.verticality);

        // baseline: default net, themed generate
        let base = ActivationNet::default();
        let seeds = [1u64, 2, 3];
        let base_match: f32 = seeds.iter().map(|&s| {
            let (l, r) = generate_themed(s, &base, &params, &target);
            match_score(&target, &r.placed, &l)
        }).sum::<f32>() / seeds.len() as f32;

        // guided search toward the theme
        let (evolved, fit) = evolve_toward(&target, &seeds, &params, 6, 10, 7);
        let (lvl, rep) = generate_themed(99, &evolved, &params, &target);
        let realised_match = match_score(&target, &rep.placed, &lvl);
        let comp = composition::analyze(&lvl);

        // realised mix
        let total: u32 = rep.placed.iter().sum();
        let mut got = String::new();
        if total > 0 {
            let mut pr: Vec<(usize, u32)> = rep.placed.iter().copied().enumerate().collect();
            pr.sort_by(|a, b| b.1.cmp(&a.1));
            for (i, c) in pr.iter().take(4) {
                if *c > 0 { got.push_str(&format!("{}={:.2} ", labels[*i], *c as f32 / total as f32)); }
            }
        }
        println!(
            "  base match={:.2} → evolved fitness={:.2}; held-out map: match={:.2} comp={:.2} obst={} water={}",
            base_match, fit, realised_match, comp.score, lvl.obstacles.len(), lvl.water.len()
        );
        println!("  realised: {}\n", got.trim());
    }

    println!("The text spec is mined into a target signature; the guided search finds a net whose generated map");
    println!("matches it (archetype mix, density, water, verticality) and styles it with the theme palette.");
    println!("This is the 'Rosetta stone': prose reference → steered procedural map, via search, not parsing.");
}
