// The exact shapes both local models produced, from the screenshots.
use std::collections::BTreeMap;
use voxel_tools::{VoxelTools, VoxelWorld};
use voxel_tools::plan::{parse_plan_at, run_plan, Severity};

struct Flat(BTreeMap<(i32,i32,i32), u16>);
impl VoxelWorld for Flat {
    fn get(&self, x: i32, y: i32, z: i32) -> u16 {
        if let Some(&b) = self.0.get(&(x,y,z)) { return b; }
        if y <= 64 { 1 } else { 0 }
    }
    fn set(&mut self, x: i32, y: i32, z: i32, b: u16) -> bool { self.0.insert((x,y,z), b); true }
}

fn main() {
    // A plausible reconstruction of what the 1B emitted: right verbs,
    // fumbled arguments.
    let reply = "\
# house
terraform
sphere @0,0 r=0
cylinder @0,0 r=0 h=0
path @2,2
enclose @-4,-4 @4,4 h=-1
tree
";
    let ir = voxel_tools::llm::clean_reply(reply);
    let (plan, issues) = parse_plan_at(&ir, Some((15, 114, 46)));
    let errs: Vec<_> = issues.iter().filter(|i| i.severity == Severity::Error).collect();
    let warns: Vec<_> = issues.iter().filter(|i| i.severity == Severity::Warning).collect();
    println!("ops: {}   errors: {}   assumptions: {}", plan.ops.len(), errs.len(), warns.len());
    for w in &warns { println!("  assumed: {}", w.message); }
    for e in &errs { println!("  ERROR: {}", e.message); }
    let mut w = Flat(BTreeMap::new());
    let run = { let mut t = VoxelTools::new(&mut w); run_plan(&mut t, &plan, 200, 32, false) };
    println!("\nbuilt {} blocks, {} ops failed", run.blocks_changed, run.failed);
    assert!(errs.is_empty() && run.blocks_changed > 0);
    println!("the same reply that was rejected now builds.");
}
