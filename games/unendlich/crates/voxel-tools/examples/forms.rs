// One word from the player -> a composite with its details already in
// the recipe. Cross-section, so the detail is visible.
use std::collections::BTreeMap;
use voxel_tools::{VoxelTools, VoxelWorld};
use voxel_tools::plan::{parse_plan_at, run_plan};

struct Flat(BTreeMap<(i32,i32,i32), u16>);
impl VoxelWorld for Flat {
    fn get(&self, x: i32, y: i32, z: i32) -> u16 {
        if let Some(&b) = self.0.get(&(x,y,z)) { return b; }
        if y <= 64 { 1 } else { 0 }
    }
    fn set(&mut self, x: i32, y: i32, z: i32, b: u16) -> bool { self.0.insert((x,y,z), b); true }
}

/// Slice ACROSS the ridge (vary z at fixed x). Slicing along the ridge
/// shows a rectangle for a correct gable, which reads as a bug and is
/// not one -- every cell on the ridge line is at full height.
fn show(w: &Flat, x: i32, zs: std::ops::RangeInclusive<i32>, ys: std::ops::RangeInclusive<i32>) {
    for y in ys.rev() {
        let row: String = zs.clone().map(|z| match w.get(x, y, z) {
            0 => '.', 1 => '#', 58 => '=', 86 => '%', _ => 'o' }).collect();
        println!("  y={y:3} {row}");
    }
}

fn main() {
    println!("vocabulary the model is told:\n{}", voxel_tools::dict::vocabulary());

    for (label, line, z) in [
        ("house  (walls + floor + door + windows + roof), sliced ACROSS the ridge",
         "form house @-6,-4 @6,4 gable spacing=4\n", 0),
        ("arch   (a curve no integer op could express)",
         "form arch @-4,-1 @4,1 h=8\n", 0),
        ("tower  (shell + conical cap)",
         "form tower @-4,-4 @4,4 r=4 h=12\n", 0),
    ] {
        println!("\n== {label}\n   {}", line.trim());
        let mut w = Flat(BTreeMap::new());
        let (plan, issues) = parse_plan_at(line, Some((0, 65, 0)));
        let errs: Vec<_> = issues.iter()
            .filter(|i| i.severity == voxel_tools::plan::Severity::Error).collect();
        assert!(errs.is_empty(), "{errs:?}");
        let run = { let mut t = VoxelTools::new(&mut w); run_plan(&mut t, &plan, 200, 32, false) };
        println!("   {} blocks", run.blocks_changed);
        show(&w, z, -8..=8, 64..=82);
    }
}
