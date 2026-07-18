fn main() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let book = paperbound_core::data::load_factbook(&dir).unwrap();
    let mut sim = paperbound_core::skirmish::SkirmishSim::new(&book, "court", "ember");
    for _ in 0..600 { sim.step(1.0/30.0); }
    let plats: Vec<paperbound_core::render::Plateau> = sim.world.plateaus.iter().map(|&(x,y,r)| paperbound_core::render::Plateau{x,y,r}).collect();
    let mesh = paperbound_core::render::build_scene(&sim.world, &plats, [paperbound_core::render::rgb(0xD8B45A), paperbound_core::render::rgb(0x8C7CC8)]);
    let buf = paperbound_core::gpu::SceneBuffers::from_mesh(&mesh);
    println!("{}", paperbound_core::gpu::engine::linkage_report(&buf));
}
