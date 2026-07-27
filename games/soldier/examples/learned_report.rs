//! learned_report — the map-QA table for the 13 LEARNED sites (IQM scans → blueprints).
//! Prints, per site: geometric grade, connectivity, sightline structure, chokepoints,
//! wall-top platforms (the rampart learning) and registered cover (the column learning).
//! Paste into docs/learned-sites-qa.md when the set changes.
fn main() {
    println!("{:<24} {:>5} {:>5} {:>6} {:>6} {:>6} {:>6} {:>5} {:>5}",
             "site", "grade", "conn", "struct", "choke", "plats", "cover", "cols", "ramps");
    for (name, bp, _k, wall_h) in early_fps::level::Level::LEARNED_SITES.iter() {
        let cols = bp.lines().filter(|l| !l.starts_with(';')).flat_map(|l| l.chars()).filter(|&c| c == 'c').count();
        let ramps = bp.lines().filter(|l| !l.starts_with(';')).flat_map(|l| l.chars()).filter(|&c| c == 'r').count();
        let p = early_fps::level::PRESETS.iter().find(|p| p.name == *name).unwrap();
        let l = early_fps::level::Level::generate(p.config(42));
        let s = early_fps::mapeval::score(&l);
        let sl = early_fps::sightline::analyze(&l);
        let walk = l.platforms.iter().filter(|p| p.top >= *wall_h).count();
        println!("{:<24} {:>5.0} {:>5} {:>6.2} {:>6} {:>6} {:>6} {:>5} {:>5}",
                 name, s.total, s.connected, sl.structure_score, sl.chokepoints,
                 walk, l.cover_obstacles.len(), cols, ramps);
    }
}
