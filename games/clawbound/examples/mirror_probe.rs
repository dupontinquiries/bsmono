//! The decisive symmetry test. Give both clans the SAME draft on terrain folded
//! onto its own reflection, and the match should stay a perfect mirror image of
//! itself forever — every clan-A warrior at the reflection of a clan-B warrior,
//! both Great Trees on equal hp. Any divergence is an asymmetry in the SIM, and
//! the first tick it appears on says where to look.
use clawbound_rs::ancestor::Ancestor;
use clawbound_rs::battleground::{Battleground, Role};
use clawbound_rs::junglemap::reflect;

fn main() {
    let picks = [Ancestor::Lion, Ancestor::Tiger, Ancestor::Leopard];
    let mut bg = Battleground::new_on(picks, picks, 3);
    for step in 0..30 * 60 * 10 {
        // pair every clan-A ancestor with its clan-B counterpart
        let a: Vec<(f32, f32, f32)> = bg.warriors.iter()
            .filter(|w| w.team == 0 && w.role == Role::Ancestor)
            .map(|w| (w.x, w.z, w.hp)).collect();
        let b: Vec<(f32, f32, f32)> = bg.warriors.iter()
            .filter(|w| w.team == 1 && w.role == Role::Ancestor)
            .map(|w| (w.x, w.z, w.hp)).collect();
        let minds: Vec<(u8, u32, String)> = bg.warriors.iter()
            .filter(|w| w.role == Role::Ancestor)
            .map(|w| (w.team, w.seat, format!("{:?} lane{} seg-carry {:?}", w.mind, w.lane, w.carry)))
            .collect();
        if a.len() == b.len() {
            let mut worst = 0.0f32;
            let mut worst_hp = 0.0f32;
            for (i, &(ax, az, ahp)) in a.iter().enumerate() {
                let (rx, rz) = reflect(ax, az);
                let (bx, bz, bhp) = b[i];
                worst = worst.max(((rx - bx).powi(2) + (rz - bz).powi(2)).sqrt());
                worst_hp = worst_hp.max((ahp - bhp).abs());
            }
            let tree = (bg.clans[0].great_tree_hp - bg.clans[1].great_tree_hp).abs();
            if worst > 0.5 || worst_hp > 0.5 || tree > 0.5 {
                println!(
                    "diverged at t={:.2}s (tick {step}): worst position gap {worst:.2}m, hp gap {worst_hp:.1}, great-tree gap {tree:.1}",
                    bg.time
                );
                for m in &minds { println!("   clan{} seat{} {}", m.0, m.1, m.2); }
                for (i, &(ax, az, ahp)) in a.iter().enumerate() {
                    let (rx, rz) = reflect(ax, az);
                    println!(
                        "   A{i} ({ax:6.1},{az:6.1}) hp{ahp:6.1}  ->reflect ({rx:6.1},{rz:6.1})   B{i} ({:6.1},{:6.1}) hp{:6.1}",
                        b[i].0, b[i].1, b[i].2
                    );
                }
                return;
            }
        } else {
            println!("roster sizes diverged at t={:.2}s: {} vs {}", bg.time, a.len(), b.len());
            return;
        }
        bg.step(1.0 / 30.0);
    }
    println!("mirror held for 10 minutes — the sim is symmetric under the clan swap");
}
