//! Why does a kit lose? Tally where each ancestor SPENDS a match, rather than
//! guessing from the win column. Run: cargo run --release --example battleground_diag
use clawbound_rs::ancestor::{self, Ancestor};
use clawbound_rs::battleground::{Battleground, Mind, Role};

fn main() {
    let picks_a = [Ancestor::CaveLion, Ancestor::Puma, Ancestor::Lion];
    let picks_b = [Ancestor::Caracal, Ancestor::Serval, Ancestor::Leopard];
    let mut bg = Battleground::new_on(picks_a, picks_b, 7);
    let mut time: Vec<[f32; 8]> = vec![[0.0; 8]; 6];
    let mut steps = 0;
    while bg.outcome.is_none() && steps < 30 * 60 * 13 {
        bg.step(1.0 / 30.0);
        steps += 1;
        let mut n = 0;
        for w in bg.warriors.iter().filter(|w| w.role == Role::Ancestor) {
            if n >= 6 { break }
            let k = match w.mind {
                Mind::Trail => 0,
                Mind::Fight => 1,
                Mind::Retreat => 2,
                Mind::AtMedic => 3,
                Mind::Hunt => 4,
                Mind::CarryHome => 5,
                Mind::Gather => 6,
                Mind::LastStand | Mind::Dead => 7,
            };
            time[n][k] += 1.0 / 30.0;
            n += 1;
        }
    }
    eprintln!("match ended {:?} at {:.0}s\n", bg.outcome, bg.time);
    eprintln!("{:<13} {:>7} {:>7} {:>7} {:>7} {:>7} {:>7} {:>7} {:>7}", "ancestor", "trail", "fight", "retreat", "medic", "hunt", "carry", "gather", "down");
    let all: Vec<Ancestor> = picks_a.iter().chain(picks_b.iter()).copied().collect();
    for (i, a) in all.iter().enumerate() {
        let t = time[i];
        eprintln!("{:<13} {:>7.0} {:>7.0} {:>7.0} {:>7.0} {:>7.0} {:>7.0} {:>7.0} {:>7.0}",
            ancestor::kit(*a).name, t[0], t[1], t[2], t[3], t[4], t[5], t[6], t[7]);
    }
}
