// The big one: a city built in a dragon's skeleton. Multi-thousand blocks.
use std::time::Instant;
use voxel_tools::program;
fn main() {
    // The armature, taken from dragon_forge's own skeleton(): spine from
    // tail to head, four legs, two wings. The city hangs on those bones.
    let src = "\
frame bones
  tailtip 0,26,-96
  tail    0,30,-60
  hips    0,38,-28
  spine   0,44,0
  chest   0,46,22
  neck    0,52,44
  head    0,58,64
end
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
asset vertebra
  part disc cylinder r=4 h=3 material=stone
end
asset rib
  part bone spine r=2 r2=0.8 len=34 dir=90 arch=0.5 segments=8 material=stone
end
pass the spine add
  repeat vertebra along bones step=5 on_ground
end
pass the ribs add
  repeat rib along bones step=11
end
pass the skull add
  part dome sphere r=13 material=stone hollow
  part jaw capsule r=6 r2=3 len=22 dir=90 pitch=-16 material=stone
  rel dome on head
  rel jaw joint dome at=0.25 out=0.5
end
pass the legs add
  part fore capsule r=4 r2=2.5 len=6 dir=0 pitch=-90 material=stone
  part hind capsule r=5 r2=3 len=6 dir=0 pitch=-90 material=stone
  rel fore joint chest at=0.5 out=1.8 pair
  rel hind joint hips at=0.5 out=2.0 pair
  rel fore reaches ground
  rel hind reaches ground
end
pass hollow the ribcage cut
  part cavity capsule r=9 r2=5 len=70 dir=90 pitch=0 material=stone
  rel cavity on spine
end
pass towers on the spine add
  repeat tower along bones step=17 scale=1+sin(t*180)*0.5
end
pass town in the ribs add
  repeat hut grid min=-30,0,-70 max=30,0,50 step=13 on_ground jitter=4 max_slope=3 seed=5
end
";
    let t0 = Instant::now();
    let (p, errs) = program::parse(src);
    if !errs.is_empty() { for e in &errs { println!("ERR {e}"); } }
    struct G;
    impl voxel_tools::VoxelWorld for G {
        fn get(&self, x: i32, y: i32, z: i32) -> u16 {
            let h = 20 + ((x as f32/28.0).sin()*4.0) as i32 + ((z as f32/20.0).cos()*3.0) as i32;
            if y <= h { 1 } else { 0 }
        }
        fn set(&mut self, _: i32, _: i32, _: i32, _: u16) -> bool { false }
    }
    let r = program::run_in(&G, &p, (0, 20, 0));
    println!("== a city in a dragon skeleton");
    println!("   {} assets, {}", p.assets.len(), r.summary());
    println!("   built in {:.2}s", t0.elapsed().as_secs_f32());
    for n in r.notes.iter().chain(r.errors.iter()) { println!("   NOTE {n}"); }
    let solid: std::collections::BTreeSet<_> = r.changes.iter().filter(|c| c.to != 0).map(|c| c.at).collect();
    if solid.is_empty() { println!("   NOTHING BUILT"); return; }
    let (lo, hi) = solid.iter().fold(((i32::MAX,i32::MAX,i32::MAX),(i32::MIN,i32::MIN,i32::MIN)),
        |(l,h), c| ((l.0.min(c.0),l.1.min(c.1),l.2.min(c.2)),(h.0.max(c.0),h.1.max(c.1),h.2.max(c.2))));
    println!("   extent {}w x {}h x {}d", hi.0-lo.0+1, hi.1-lo.1+1, hi.2-lo.2+1);
    // side elevation: the dragon runs along z, so look along x
    let sz = (((hi.2-lo.2) / 110).max(1), ((hi.1-lo.1) / 32).max(1));
    for y in (lo.1..=hi.1).rev().step_by(sz.1 as usize) {
        let row: String = (lo.2..=hi.2).step_by(sz.0 as usize)
            .map(|z| if (lo.0..=hi.0).any(|x| solid.contains(&(x,y,z))) { '#' } else { ' ' }).collect();
        println!("   |{row}|");
    }
}
