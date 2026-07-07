fn main() {
    use early_fps::mapeval;
    print!("{}", mapeval::role_model_report("Blighted Watchtower", 7));
    // breed new graph-skinned candidates that play like the role model
    if let Some(role) = mapeval::role_model("Blighted Watchtower", 7) {
        println!("\n--- new graph candidates filtered to play like the Blighted Watchtower (Meadow) ---");
        let best = mapeval::candidates_like(&role, early_fps::level::MapTheme::Meadow, 5, 400, 1);
        for (seed, d, sc) in best {
            let p = sc.profile();
            println!("  d{:.2}  seed {:016x}  {:.1} {}  o{:.2} f{:.2} c{:.2} v{:.2}",
                d, seed, sc.total, sc.grade(), p[0], p[1], p[2], p[3]);
        }
    }
}
