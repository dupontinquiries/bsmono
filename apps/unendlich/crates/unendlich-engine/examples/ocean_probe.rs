fn main() {
    let gen = unendlich_engine::world::cpu_generator::WorldGenerator::new(1337);
    let mut found = 0;
    let mut minmarg = f64::MAX;
    for gz in (-6400i32..6400).step_by(64) {
        for gx in (-6400i32..6400).step_by(64) {
            let y = gen.surface_y_at(gx, gz, -64, 200).unwrap_or(64) as f64;
            let sl = gen.sea_level_at(gx, gz);
            let marg = y - sl;
            if marg < minmarg { minmarg = marg; }
            if marg < -2.0 {
                found += 1;
                if found <= 3 {
                    let b = gen.biome_at_3d(gx, y as i32, gz).biome;
                    println!("submerged ({gx},{gz}) y={y} sl={sl:.1} biome={b:?}");
                }
            }
        }
    }
    println!("submerged columns: {found}; min(surface - sea) = {minmarg:.1}");
    let mut oc = 0;
    for gz in (-6400i32..6400).step_by(64) {
        for gx in (-6400i32..6400).step_by(64) {
            let y = gen.surface_y_at(gx, gz, -64, 200).unwrap_or(64);
            let sl = gen.sea_level_at(gx, gz);
            if (y as f64) < sl - 2.0
                && matches!(
                    gen.biome_at_3d(gx, y, gz).biome,
                    unendlich_engine::world::biomes::Biome::Ocean
                )
            {
                oc += 1;
            }
        }
    }
    println!("submerged columns now classified Ocean: {oc}");
}
