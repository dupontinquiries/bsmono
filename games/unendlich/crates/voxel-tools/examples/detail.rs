// What the finishing passes do to a plain wall, face-on.
use std::collections::BTreeMap;
use voxel_tools::{VoxelTools, VoxelWorld};
use voxel_tools::plan::{parse_plan_at, run_plan};

struct W(BTreeMap<(i32,i32,i32), u16>);
impl VoxelWorld for W {
    fn get(&self, x: i32, y: i32, z: i32) -> u16 {
        if let Some(&b) = self.0.get(&(x,y,z)) { return b; }
        if y <= 64 { 1 } else { 0 }
    }
    fn set(&mut self, x: i32, y: i32, z: i32, b: u16) -> bool { self.0.insert((x,y,z), b); true }
}

fn face(w: &W, z: i32) {
    for y in (65..72).rev() {
        let row: String = (0..20).map(|x| match w.get(x, y, z) {
            0 => '.', 1 => '#', 4 => 'c', 81 => 'm', 43 => 'a', 58 => '=', 86 => '=', 12 => 's', _ => '?'
        }).collect();
        println!("  {row}");
    }
}

fn build() -> W {
    let mut w = W(BTreeMap::new());
    for x in 0..20 { for y in 65..72 { w.set(x, y, 5, 1); } }
    w
}

fn main() {
    println!("a plain stone wall — the signature of generated work:");
    face(&build(), 5);

    for (label, ir) in [
        ("speckle: cobble/mossy/andesite mixed into the face",
         "detail speckle 0,65,5 19,71,5 amount=0.35 seed=3\n"),
        ("trim: accent on edges and the top course",
         "detail trim 0,65,5 19,71,5 block=dark_oak_planks amount=0.9 seed=4\n"),
        ("feather: the wall dissolves into sand from the left",
         "detail feather 0,65,5 19,71,5 block=sand seed=5\n"),
    ] {
        let mut w = build();
        let (plan, issues) = parse_plan_at(ir, Some((0, 65, 0)));
        let errs: Vec<_> = issues.iter()
            .filter(|i| i.severity == voxel_tools::plan::Severity::Error).collect();
        assert!(errs.is_empty(), "{errs:?}");
        let run = { let mut t = VoxelTools::new(&mut w); run_plan(&mut t, &plan, 200, 32, false) };
        println!("\n{label}  ({} blocks)", run.blocks_changed);
        face(&w, 5);
    }

    // relief needs a side view to see
    let mut w = build();
    let (plan, _) = parse_plan_at("detail relief 0,65,5 19,71,5 amount=0.3 seed=7\n",
                                  Some((0, 65, 0)));
    let run = { let mut t = VoxelTools::new(&mut w); run_plan(&mut t, &plan, 200, 32, false) };
    println!("\nrelief: seen from ABOVE (y=68), the wall is no longer planar  ({} blocks)",
             run.blocks_changed);
    for z in 3..8 {
        let row: String = (0..20).map(|x| if w.get(x, 68, z) != 0 { '#' } else { '.' }).collect();
        println!("  {row}");
    }
}
