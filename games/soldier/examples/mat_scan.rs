use early_fps::camera::FpsCam;
use early_fps::game::Game;
use early_fps::level::PRESETS;
use early_fps::loadout::Loadout;
use early_fps::math::Vec3;
use early_fps::mesh::Material;
fn main() {
    for map in ["Downtown", "Reference Town", "Warehouse", "Foundry", "Meadow"] {
        let Some(i) = PRESETS.iter().position(|p| p.name == map) else { println!("{map}: (not a preset)"); continue };
        let mut hist = std::collections::BTreeMap::<String,u32>::new();
        for seed in [1u64,2,3] {
            let mut g = Game::new_preset(seed, Some(i), Loadout::default());
            let inp = early_fps::player::Input::default();
            for _ in 0..2 { g.step(&inp,1.0/60.0); }
            let lvl=&g.level; let (mcx,mcz)=(lvl.w as f32*0.5*lvl.ts(), lvl.h as f32*0.5*lvl.ts());
            for m in g.frame_meshes(&FpsCam{pos:Vec3::new(mcx,45.0,mcz),yaw:0.0,pitch:-1.4}, false) {
                *hist.entry(m.material.map(|x| format!("{x:?}")).unwrap_or("None".into())).or_default() += 1;
            }
        }
        let brick = hist.get("Brick").copied().unwrap_or(0);
        let mut v: Vec<_> = hist.into_iter().filter(|(k,_)| k!="None").collect(); v.sort_by(|a,b| b.1.cmp(&a.1));
        let top: Vec<String> = v.into_iter().take(5).map(|(k,c)| format!("{k}:{c}")).collect();
        println!("{map:<16} BRICK={brick}  | {}", top.join("  "));
    }
}
