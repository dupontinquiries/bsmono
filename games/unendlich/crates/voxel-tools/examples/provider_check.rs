fn main() {
    use voxel_tools::llm::{clean_reply, CommandProvider, Completer};
    let mut p = CommandProvider::new(vec!["providers/fixture.sh".into()]);
    let raw = p.complete("SYS", "USER").expect("fixture ran");
    let ir = clean_reply(&raw);
    let (plan, issues) = voxel_tools::plan::parse_plan_at(&ir, Some((0, 65, 0)));
    println!("provider {} -> {} ops, {} issues", p.name(), plan.ops.len(), issues.len());
    for o in &plan.ops { println!("  {o:?}"); }
}
