//! The 5v5 select page, in a terminal — the same state machine a phone, a
//! controller or a mouse would drive, so the routing can be watched before a
//! renderer exists. Run: cargo run --release --example lobby_demo [ancestor]
use clawbound_rs::ancestor;
use clawbound_rs::battleground::{Battleground, LanePolicy};
use clawbound_rs::lobby::{Lobby, LobbyInput, Phase, CLAN};

fn main() {
    let want = std::env::args().nth(1).unwrap_or_else(|| "stillsmoke".into());
    let mut l = Lobby::new();
    l.handle(LobbyInput::QuickPlay);
    // a keyboard player types; a thumb would send Point, a stick would send Move
    l.handle(LobbyInput::Search(want.clone()));
    l.handle(LobbyInput::Lock);
    println!("you locked {:?}\n", l.you().hover.map(|a| ancestor::kit(a).name));

    let mut last = 0;
    while l.phase != Phase::Launch {
        l.tick(0.25);
        if l.countdown() != last {
            last = l.countdown();
            let filled = l.seats.iter().flatten().filter(|s| s.locked).count();
            println!("  {:?}  {last:>2}s   {filled}/{} locked", l.phase, CLAN * 2);
        }
    }
    let (a, b) = l.rosters().expect("rosters");
    let names = |r: &[ancestor::Ancestor; CLAN]| {
        r.iter().map(|&x| ancestor::kit(x).name).collect::<Vec<_>>().join(", ")
    };
    println!("\nyour clan: {}\nagainst:   {}", names(&a), names(&b));

    let mut bg = Battleground::new_roster(&a, &b, 0, LanePolicy::Kit);
    let mut steps = 0;
    while bg.outcome.is_none() && steps < 30 * 60 * 13 {
        bg.step(1.0 / 30.0);
        steps += 1;
    }
    println!(
        "match ran {:.0}s and ended {:?}",
        bg.time,
        bg.outcome.map(|o| if o == 0 { "your clan holds the field" } else { "theirs" })
    );
}
