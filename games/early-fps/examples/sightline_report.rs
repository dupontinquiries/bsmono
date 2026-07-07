//! sightline_report.rs — print the gameplay-geometry analysis (exposure, chokepoints, tactical structure)
//! for the authored maps, alongside the geometric mapeval grade. This is the gun-arc / chokepoint scorer;
//! pair it with `tools/scenic.py` (visual/variety scorer) for the full picture.
use early_fps::level::{Level, PRESETS};
use early_fps::mapeval::score;
use early_fps::sightline::{analyze, suggest_cover};
fn main() {
    println!("{:<18} {:>6} {:>6} {:>6} {:>5} {:>6} {:>6} {:>5}", "MAP", "expo", "spread", "safe%", "chok", "struct", "grade", "cover?");
    println!("{}", "-".repeat(66));
    let names = ["Flood Zone","Reference Town","Desert Town","Pyre Garage","Concrete Slab","Redwood Canyon"];
    for name in names {
        if let Some(p) = PRESETS.iter().find(|p| p.name == name) {
            let l = Level::generate(p.config(7));
            let r = analyze(&l);
            let sc = score(&l);
            let sugg = suggest_cover(&l, 6).len();
            println!("{:<18} {:>6.3} {:>6.3} {:>5.0}% {:>5} {:>6.2} {:>6} {:>5}",
                name, r.mean_exposure, r.exposure_spread, r.safe_fraction*100.0, r.chokepoints, r.structure_score, sc.grade(), sugg);
        }
    }
    println!("\nstruct = tactical structure (exposure spread + chokepoint density + open/safe balance)");
    println!("cover? = # of exposure-guided cover placements suggest_cover() would drop to break sightlines");
}
