// Can it make anything you can type? Measure, do not guess.
use std::collections::BTreeMap;
use voxel_tools::{compose, program, VoxelWorld};
struct Flat;
impl VoxelWorld for Flat {
    fn get(&self, _x: i32, y: i32, _z: i32) -> u16 { if y <= 64 { 1 } else { 0 } }
    fn set(&mut self, _: i32, _: i32, _: i32, _: u16) -> bool { false }
}
const CORPUS: &[&str] = &[
    // buildings
    "a stone cottage", "a huge fortress", "a cathedral", "a fishing village", "a lighthouse",
    "a windmill", "a bridge over a river", "a lighthouse on a cliff", "a market stall",
    "a train station", "a bunker", "a greenhouse", "an amphitheatre", "a pagoda",
    // figures and creatures
    "a colossal statue of Zeus", "a giant emerald frog statue", "a stone dragon",
    "a statue of a seated king", "a huge gargoyle", "a bronze horse",
    // landscape
    "carve a great canyon", "a waterfall into a pool", "a volcano", "a floating island",
    "a maze", "a crater", "a forest of tall pines",
    // objects and abstractions
    "a water sphere", "a giant chess piece", "a spiral staircase to the sky",
    "a clock tower", "a ship in a bottle", "a rocket on a launchpad",
    "the sound of a bell as a building", "a giant mushroom", "a treehouse",
    "a skull the size of a house", "an hourglass", "a harp", "a beehive",
    "a sword stuck in a rock", "a giant open book", "a lantern the size of a barn",
];
fn main() {
    let mut by_path: BTreeMap<String, usize> = BTreeMap::new();
    let mut built = 0; let mut empty = 0; let mut sizes = Vec::new();
    let mut fails: Vec<(&str, String)> = Vec::new();
    for req in CORPUS {
        let path = compose::choose_path(req);
        *by_path.entry(format!("{path:?}").split([' ', '{']).next().unwrap().trim().to_string())
            .or_insert(0) += 1;
        // what would it actually build, with no model in the loop?
        let (cells, note) = match path {
            compose::Path::Known => {
                let line = voxel_tools::recipe::recognise(req).map(|r| r.line()).unwrap_or_default();
                let (plan, _) = voxel_tools::plan::parse_plan_at(&format!("{line}\n"), Some((0,65,0)));
                (plan.ops.len(), line)
            }
            compose::Path::Programme => {
                let p = compose::programme(req, ((0,0),(0,0)), 5, None);
                let (prog, _) = program::parse(&p.src);
                (program::run_in(&Flat, &prog, (0,64,0)).changes.len(), p.summary())
            }
            compose::Path::Study { height } => {
                // no model: the study is empty, so this is the floor
                let st = voxel_tools::study::Study::default();
                let src = voxel_tools::study::to_sketch(&st, height, "stone");
                let (prog, _) = program::parse(&format!("pass a add\n{src}end\n"));
                (program::run_in(&Flat, &prog, (0,64,0)).changes.len(), format!("study h={height}"))
            }
            compose::Path::Propose { ref noun } => (0, format!("would ask what a {noun} is")),
            compose::Path::Compose => (0, "needs a model".into()),
        };
        if cells > 0 { built += 1; sizes.push(cells); } else { empty += 1; fails.push((req, note.clone())); }
        println!("{:44} {:<10} {:>8} cells   {}", req, format!("{path:?}").split(' ').next().unwrap(),
                 cells, note.chars().take(46).collect::<String>());
    }
    println!("\n== {} of {} produce something without a model", built, CORPUS.len());
    println!("== routing: {by_path:?}");
    sizes.sort();
    if !sizes.is_empty() {
        println!("== sizes: min {} median {} max {}", sizes[0], sizes[sizes.len()/2], sizes[sizes.len()-1]);
    }
    println!("== nothing built ({}):", empty);
    for (r, n) in &fails { println!("   {r:44} {n}"); }
}
