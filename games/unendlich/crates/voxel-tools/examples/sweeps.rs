// The shape that bends.
use voxel_tools::sketch;
fn draw(line: &str, label: &str, axis: usize) {
    let (sk, errs) = sketch::parse(&format!("{line}\n"));
    if !errs.is_empty() { println!("   ERR {errs:?}"); return; }
    let (placed, _) = sketch::resolve(&sk, (0, 64, 0));
    let c = sketch::cells(&placed[0], 1);
    let set: std::collections::BTreeSet<(i32,i32)> = c.iter()
        .map(|p| if axis == 0 { (p.0, p.1) } else { (p.0, p.2) }).collect();
    let (xl, xh) = (set.iter().map(|p| p.0).min().unwrap(), set.iter().map(|p| p.0).max().unwrap());
    let (yl, yh) = (set.iter().map(|p| p.1).min().unwrap(), set.iter().map(|p| p.1).max().unwrap());
    println!("\n   {label} ({} cells)", c.len());
    for y in (yl..=yh).rev().step_by((((yh-yl)/16).max(1)) as usize) {
        let row: String = (xl..=xh).map(|x| if set.contains(&(x,y)) { '#' } else { ' ' }).collect();
        println!("   |{row}|");
    }
}
fn main() {
    draw("part h sweep path=0,0,0;2,9,0;8,15,0;17,16,0 r=4,3,2,0.5", "a curving horn (side)", 0);
    draw("part c sweep path=0,0,0;22,0,0;22,0,20 r=3 section=square", "a corridor turning (plan)", 1);
    draw("part k sweep path=0,0,0;0,14,0;4,19,0;9,15,0 r=2", "a hook (side)", 0);
}
