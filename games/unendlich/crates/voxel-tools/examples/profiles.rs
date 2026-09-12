// One shape, many things.
use voxel_tools::sketch;
fn draw(line: &str, label: &str) {
    let (sk, errs) = sketch::parse(&format!("{line}\n"));
    if !errs.is_empty() { println!("ERR {errs:?}"); return; }
    let (placed, _) = sketch::resolve(&sk, (0, 64, 0));
    let c = sketch::cells(&placed[0], 1);
    let set: std::collections::BTreeSet<(i32,i32)> = c.iter().filter(|x| x.2 == 0).map(|x| (x.0, x.1)).collect();
    let (lo, hi) = (c.iter().map(|x| x.1).min().unwrap(), c.iter().map(|x| x.1).max().unwrap());
    let (xl, xh) = (c.iter().map(|x| x.0).min().unwrap(), c.iter().map(|x| x.0).max().unwrap());
    println!("\n   {label} ({} cells)", c.len());
    for y in (lo..=hi).rev().step_by(((hi-lo)/22).max(1) as usize) {
        let row: String = (xl..=xh).map(|x| if set.contains(&(x,y)) { '#' } else { ' ' }).collect();
        println!("   |{row}|");
    }
}
fn main() {
    draw("part m profile r=1.5,1.2,1,7,5,0.5 h=22 material=stone", "mushroom");
    draw("part g profile r=7,4,1,1,4,7 h=24 material=glass", "hourglass");
    draw("part p profile r=6,5,2.5,2,4,3,2.5,4 h=30 material=quartz", "chess piece");
}
