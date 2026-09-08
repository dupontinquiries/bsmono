// What a builder asks about a block they have never seen.
use voxel_tools::palette::{self, Family, Finish};
fn main() {
    println!("== asking for a material by description");
    for q in ["something dark and hard for a foundation", "pale and fine for trim",
              "warm wood for a floor", "clear for windows", "rough grey walls",
              "something that glows"] {
        let s: Vec<String> = palette::suggest(q, 2).into_iter().map(|(_, w)| w).collect();
        println!("   {q:42} -> {}", s.join(", "));
    }
    println!("\n== a block from another game, placed by analogy");
    for (name, rgb, hard, fam, fin) in [
        ("deepslate",   ( 60,  60,  66), 3.5, Family::Stone,   Finish::Rough),
        ("amethyst",    (170, 140, 210), 1.5, Family::Crystal, Finish::Polished),
        ("mud bricks",  (140, 110,  85), 1.0, Family::Soil,    Finish::Smooth),
        ("copper",      (200, 115,  75), 3.0, Family::Metal,   Finish::Polished),
        ("moss",        ( 85, 130,  60), 0.2, Family::Leaf,    Finish::Fibrous)] {
        let s = palette::nearest_to(rgb, hard, fam, fin);
        println!("   {name:12} -> {:<16} (or {})", s.name, s.runners_up.join(", "));
    }
    println!("\n== an unknown name is a description");
    for n in ["deepslate", "dark polished stone", "obsidian", "zzzqqq"] {
        let (id, why) = palette::resolve_or_suggest(n);
        println!("   {n:22} -> id {id:3}  {}", why.unwrap_or_else(|| "exact".into()));
    }
}
