use clawbound::sim::*;
use clawbound::consts::*;
fn main() {
    let mut game = Game::new(0, [1, 2, 3, 4], [5, 6, 7, 8, 9]);
    game.gods[0].ai = Some(AiBrain { lane: Lane::Mid, aggression: 0.9, state: AiState::Push, target: None, strafe_dir: 1.0, strafe_t: 0.0, buy_t: 1.0, wp: 0 });
    for step in 0..(120 * 120) {
        game.tick(1.0 / 120.0);
        if step % (120 * 20) == 0 {
            let s: Vec<String> = game.gods.iter().map(|g| format!("{}:{}g/{}i/{}k/{}d", clawbound::gods::god(g.def_id).name, g.gold as i32, g.items.len(), g.kills, g.deaths)).collect();
            println!("t={:.0} towers={:?} caught={:?} ev={:?} {:?}\n  {}", game.t, game.tower_alive, game.wild.caught, game.map_events.active.as_ref().map(|a| a.kind), game.weather.kind, s.join(" "));
        }
        if game.over.is_some() { println!("OVER at {}", game.t); break; }
        game.events.clear();
    }
}
