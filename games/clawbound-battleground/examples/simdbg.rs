// 12-minute AI-only match trace: economy, events, weather, hunt, objectives,
// and any catastrophe. `cargo run --example simdbg [seed]`.
use clawbound::sim::*;
use clawbound::consts::*;
fn main() {
    let seed: u64 = std::env::args().nth(1).and_then(|s| s.parse().ok()).unwrap_or(0);
    let mut game = Game::new((seed % 13) as u32, [1, 2, 3, 4], [5, 6, 7, 8, 9]);
    game.rng = Rng::new(0xC0FFEE ^ seed.wrapping_mul(7919));
    game.gods[0].ai = Some({ let mut b = AiBrain::new(Lane::Mid, 0.9); b.buy_t = 1.0; b });
    for step in 0..(720 * 60) {
        game.tick(1.0 / 60.0);
        for e in game.events.drain(..) {
            match e {
                Event::Announce { text } => println!("  [{:>5.0}s] {}", game.t, text),
                _ => {}
            }
        }
        if step % (60 * 60) == 0 {
            let s: Vec<String> = game.gods.iter().map(|g| format!("{}:{}g/{}k/{}d", clawbound::gods::god(g.def_id).name, g.gold as i32, g.kills, g.deaths)).collect();
            println!("t={:.0} towers={:?} caught={:?} bosses={:?} bhp={:?} fire={} charred={} fissures={} {:?}\n  {}", game.t, game.tower_alive, game.wild.caught,
                game.bosses.list.iter().map(|b| (b.kind, b.alive, b.kills)).collect::<Vec<_>>(), game.bosses.list.iter().map(|b| b.hp as i32).collect::<Vec<_>>(), game.disasters.fire.len(), game.disasters.charred.len(), game.disasters.fissures.len(), game.weather.kind, s.join(" "));
        }
        if game.over.is_some() { println!("OVER at {} winner {:?}", game.t, game.over); break; }
    }
}
