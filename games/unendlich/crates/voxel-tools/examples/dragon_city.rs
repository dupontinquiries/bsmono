// The big one: a city built in a dragon's skeleton. Multi-thousand blocks.
use std::time::Instant;
use voxel_tools::program;
fn main() {
    // The armature, taken from dragon_forge's own skeleton(): spine from
    // tail to head, four legs, two wings. The city hangs on those bones.
    let src = "\
asset tower
  part shaft cylinder r=3 h=14 material=stone
  part cap cone r=4 h=5 material=dark_oak_planks
  rel cap on shaft
end
asset hut
  part walls box w=5 h=4 d=5 material=oak_planks hollow
  part roof cone r=4 h=3 material=dark_oak_planks
  rel roof on walls
end
pass ribcage add
  part spine spine r=5 r2=2 len=120 dir=0 arch=0.22 segments=14 material=stone
  rel spine above ground by=34
end
pass skull add
  part skull sphere r=11 material=stone hollow
  part jaw capsule r=5 r2=3 len=18 dir=0 pitch=-14 material=stone
  rel skull above ground by=52
  rel jaw joint skull at=0.2 out=0.4
end
pass legs add
  part fore capsule r=4 r2=2.5 len=40 dir=200 pitch=-72 material=stone
  part hind capsule r=5 r2=3 len=44 dir=200 pitch=-76 material=stone
  rel fore joint spine at=0.5 out=1.4 pair
  rel hind joint spine at=0.5 out=1.6 pair
end
pass ribs add
  part rib spine r=2 r2=1 len=30 dir=90 arch=0.55 segments=8 material=stone
  rel rib joint spine at=0.6 out=0.6 pair
end
pass hollow the ribcage cut
  part cavity capsule r=7 r2=4 len=100 dir=0 pitch=0 material=stone
  rel cavity above ground by=36
end
pass towers on the spine add
  repeat tower line from=-50,38,0 to=50,38,0 step=14
end
pass town in the ribs add
  repeat hut grid min=-40,20,-14 max=40,20,14 step=11
end
";
    let t0 = Instant::now();
    let (p, errs) = program::parse(src);
    if !errs.is_empty() { for e in &errs { println!("ERR {e}"); } }
    let r = program::run(&p, (0, 20, 0));
    println!("== a city in a dragon skeleton");
    println!("   {} assets, {}", p.assets.len(), r.summary());
    println!("   built in {:.2}s", t0.elapsed().as_secs_f32());
    for n in r.notes.iter().chain(r.errors.iter()) { println!("   NOTE {n}"); }
    let solid: std::collections::BTreeSet<_> = r.changes.iter().filter(|c| c.to != 0).map(|c| c.at).collect();
    if solid.is_empty() { println!("   NOTHING BUILT"); return; }
    let (lo, hi) = solid.iter().fold(((i32::MAX,i32::MAX,i32::MAX),(i32::MIN,i32::MIN,i32::MIN)),
        |(l,h), c| ((l.0.min(c.0),l.1.min(c.1),l.2.min(c.2)),(h.0.max(c.0),h.1.max(c.1),h.2.max(c.2))));
    println!("   extent {}w x {}h x {}d", hi.0-lo.0+1, hi.1-lo.1+1, hi.2-lo.2+1);
    // side elevation, collapsing z
    let step = (((hi.0-lo.0) / 100).max(1), ((hi.1-lo.1) / 34).max(1));
    for y in (lo.1..=hi.1).rev().step_by(step.1 as usize) {
        let row: String = (lo.0..=hi.0).step_by(step.0 as usize)
            .map(|x| if (lo.2..=hi.2).any(|z| solid.contains(&(x,y,z))) { '#' } else { ' ' }).collect();
        println!("   |{row}|");
    }
}
