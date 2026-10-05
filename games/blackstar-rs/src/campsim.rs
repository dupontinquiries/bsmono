//! CAMPAIGN SIM — the six films, playable from the terminal.
//!
//! The `--raid` harness proved the raid loop; this proves the story loop:
//! slides in, waves resolved by `skirmish` under the mission's objective
//! machine, slides out, progression banked. Same contract as everywhere
//! else — every decision prints with its reason, victories are dry runs
//! until `--commit`.
//!
//!     skydribble --campaign                    the mission list and the log
//!     skydribble --campaign next               play the next uncleared mission
//!     skydribble --campaign the_mantle_dive    play one by id
//!     skydribble --campaign all                run the whole campaign, brief
//!
//! Steering: `--squad a,b,c` picks player classes, `--doctrine x` the
//! commander, `--commit` banks a win into the save, `--valence N`
//! overrides the squad's charge for a probe (never banked).

use crate::campaign::{
    foe_doctrine, waves_for, CampEvent, CampaignLog, MissionRun, DEFAULT_DOCTRINE, DEFAULT_SQUAD,
};
use crate::content::{commander, film, troop, MISSIONS};
use crate::valence;
use crate::nightfall::{self, NightfallLog};
use crate::raidlog::RaidLog;
use crate::save;

pub fn run(argv: &[String]) {
    let i = argv.iter().position(|a| a == "--campaign").unwrap();
    let pick = argv.get(i + 1).map(|s| s.as_str()).filter(|s| !s.starts_with("--"));
    let commit = argv.iter().any(|a| a == "--commit");
    let squad: Vec<String> = argv
        .iter()
        .position(|a| a == "--squad")
        .and_then(|p| argv.get(p + 1))
        .map(|s| s.split(',').map(|x| x.trim().to_string()).collect())
        .unwrap_or_else(|| DEFAULT_SQUAD.iter().map(|s| s.to_string()).collect());
    let squad_refs: Vec<&str> = squad.iter().map(|s| s.as_str()).collect();
    let forced_valence: Option<u32> = argv
        .iter()
        .position(|a| a == "--valence")
        .and_then(|p| argv.get(p + 1))
        .and_then(|s| s.parse().ok());
    let doctrine: &'static str = argv
        .iter()
        .position(|a| a == "--doctrine")
        .and_then(|p| argv.get(p + 1))
        .and_then(|d| commander(d).map(|c| c.id))
        .unwrap_or(DEFAULT_DOCTRINE);

    match pick {
        None => list(),
        Some("all") => {
            let (mut log, raids, nf) = load_logs();
            let mut walk = log.clone();
            for m in MISSIONS {
                if log.is_cleared(m.id) {
                    println!("  {:28} already cleared", m.id);
                    continue;
                }
                let at = valence::valence(&walk, &raids, nf.count());
                let won = play(m.id, &squad_refs, doctrine, true, at, None);
                if won {
                    walk.record(m.id);
                    if commit {
                        log = bank(m.id);
                    }
                }
            }
            let done = if commit { log } else { load_log() };
            println!("\ncampaign: {}/{} missions", done.done(), MISSIONS.len());
            if !commit {
                println!("  (dry run -- add --commit to bank the wins)");
            } else if done.raids_unlocked() {
                println!("  the wardens have stopped being rumours. `--raid` for the roster.");
            }
        }
        Some("next") => {
            let (log, raids, nf) = load_logs();
            match log.next() {
                Some(m) => {
                    let won = play(m.id, &squad_refs, doctrine, false, valence::valence(&log, &raids, nf.count()), None);
                    if won {
                        settle(m.id, commit);
                    }
                }
                None => println!("the campaign is finished. `--raid` for what it unlocked."),
            }
        }
        Some("nightfall") => {
            let (log, raids, nf) = load_logs();
            let week = argv
                .iter()
                .position(|a| a == "--week")
                .and_then(|p| argv.get(p + 1))
                .and_then(|s| s.parse().ok())
                .unwrap_or_else(nightfall::week_now);
            let card = nightfall::card(week);
            println!(
                "NIGHTFALL, week {week} -- {} under a {} burn{}",
                card.mission.id,
                card.burn.name(),
                if card.exposure { ", exposure (no field repair)" } else { "" }
            );
            println!("  the burn runs {} at {:.1}x for BOTH sides. bring it, and fear it.", card.burn.name(), crate::skirmish::BURN_MULT);
            if nf.cleared(week) {
                println!("  this week is already banked. it returns at the reset.");
                return;
            }
            let at = valence::valence(&log, &raids, nf.count());
            let won = play(card.mission.id, &squad_refs, doctrine, false, at, Some(&card));
            if won {
                if commit {
                    // the mission, the week, the burn's chi and its kit -- on the one
                    // path the flown nightfall takes (`stations::record_outcome`)
                    let banked = crate::stations::record_outcome(card.mission.id, Some(week));
                    for line in banked.lines() {
                        println!("   {line}");
                    }
                    let (_, _, nf) = load_logs();
                    let lit = valence::valence(&log, &raids, nf.count());
                    println!("   week {week} banked -- valence {lit} (+{})", valence::PER_NIGHTFALL);
                } else {
                    println!("   (dry run -- add --commit to bank the week)");
                }
            }
        }
        Some(id) => {
            if MISSIONS.iter().all(|m| m.id != id) {
                println!("no such mission: {id}  (run `--campaign` for the list)");
                return;
            }
            let (log, raids, nf) = load_logs();
            let at = forced_valence.unwrap_or_else(|| valence::valence(&log, &raids, nf.count()));
            let won = play(id, &squad_refs, doctrine, false, at, None);
            if won {
                settle(id, commit);
            }
        }
    }
}

fn load_logs() -> (CampaignLog, RaidLog, NightfallLog) {
    let (sv, _, _) = save::load();
    (
        CampaignLog::from_line(&sv.missions),
        RaidLog::from_line(&sv.wardens),
        NightfallLog::from_line(&sv.nightfalls),
    )
}


fn load_log() -> CampaignLog {
    let (sv, _, _) = save::load();
    CampaignLog::from_line(&sv.missions)
}

fn bank(id: &str) -> CampaignLog {
    let (mut sv, b, cfg) = save::load();
    let mut log = CampaignLog::from_line(&sv.missions);
    log.record(id);
    sv.missions = log.line();
    save::store(&sv, &b, &cfg);
    log
}

fn settle(id: &str, commit: bool) {
    if !commit {
        println!("   (dry run -- add --commit to bank the mission)");
        return;
    }
    let log = bank(id);
    println!("   banked: {}/{} missions", log.done(), MISSIONS.len());
    if log.raids_unlocked() {
        println!("   the campaign is over. nineteen old signals are waiting: `--raid`");
    } else if let Some(next) = log.next() {
        println!("   next: {} (film {}, {})", next.id, next.film, next.site);
    }
}

fn list() {
    let (log, raids, nf) = load_logs();
    println!("THE CAMPAIGN -- six films, {} missions. `--campaign next` to play on.\n", MISSIONS.len());
    println!(
        "valence {} = {} missions x{} + {} wardens x{} + {} nightfalls x{}\n",
        valence::valence(&log, &raids, nf.count()),
        log.done(),
        valence::PER_MISSION,
        raids.shards(),
        valence::PER_WARDEN,
        nf.count(),
        valence::PER_NIGHTFALL
    );
    let mut cur = 0u8;
    for m in MISSIONS {
        if m.film != cur {
            cur = m.film;
            let f = film(cur).unwrap();
            let (d, t) = log.film_done(cur);
            println!("FILM {}: {} -- {}  [{}/{}]", f.n, f.title, f.subtitle, d, t);
        }
        let mark = if log.is_cleared(m.id) { "x" } else { " " };
        println!(
            "{mark}  {:28} {:9} rec{:>3} @{:11} {}",
            m.id,
            format!("{:?}", m.objective),
            valence::rec_valence(m),
            m.dest,
            m.beat
        );
    }
    println!("\n{}/{} done", log.done(), MISSIONS.len());
    match log.next() {
        Some(n) => println!("next: {} -- {}", n.id, n.site),
        None => println!("the story is over; the wardens are not. `--raid`"),
    }
}

/// Play one mission, narrated (or one line, brief). Returns victory.
/// `at_valence` is the squad's charge; `nf` dresses the run as a nightfall.
fn play(
    id: &str,
    squad: &[&str],
    doctrine: &'static str,
    brief: bool,
    at_valence: u32,
    nf: Option<&nightfall::Nightfall>,
) -> bool {
    let m = MISSIONS.iter().find(|m| m.id == id).unwrap();
    let mut run = MissionRun::new(m, squad, doctrine, at_valence);
    if let Some(n) = nf {
        run = run.with_modifiers(Some(n.burn), n.exposure);
    }
    if !brief {
        let f = film(m.film).unwrap();
        println!("== {} -- film {} ({}), {} on {}", m.id, m.film, f.title, m.site, m.dest);
        for s in m.intro {
            println!("   | {s}");
        }
        let rec = valence::rec_valence(m);
        println!(
            "   objective {:?} -- foe under {} doctrine, {} waves -- valence {} vs rec {}{}",
            m.objective,
            foe_doctrine(m),
            waves_for(m).len(),
            at_valence,
            rec,
            if at_valence < rec { "  LOW VALENCE: this will hurt" } else { "" }
        );
    }
    let mut wave_kills = 0u32;
    while run.outcome().is_none() && run.t < 1800.0 {
        for e in run.step(0.1) {
            if brief {
                continue;
            }
            match e {
                CampEvent::WaveStart { wave, of, size } => {
                    wave_kills = 0;
                    print!("   wave {wave}/{of}: {size} hostile{}", if size == 1 { "" } else { "s" });
                    let comp = waves_for(m);
                    let idx = (wave - 1) % comp.len();
                    let named: Vec<&str> = comp[idx].iter().map(|id| troop(id).map(|t| t.id).unwrap_or(id)).collect();
                    println!(" ({})", named.join(", "));
                }
                CampEvent::EnemyDown { .. } => wave_kills += 1,
                CampEvent::SquadDown { slot } => println!("      - we lost {} (slot {slot})", squad.get(slot).unwrap_or(&"?")),
                CampEvent::WaveCleared { wave } => {
                    println!("      wave {wave} cleared ({wave_kills} down) -- t {:.0}s, squad {}", run.t, run.squad_alive())
                }
                CampEvent::StructureHit { hp_left } => {
                    if hp_left <= 0.0 || (hp_left / 50.0).fract() < 0.002 {
                        println!("      demolition: structure {hp_left:.0}");
                    }
                }
                CampEvent::AssetHit { hp_left } => {
                    if (hp_left / 100.0).fract() < 0.001 {
                        println!("      the asset is taking fire: {hp_left:.0} left");
                    }
                }
                CampEvent::Hold { pct } => {
                    if pct > 0.0 && (pct as u32 % 25 == 0) && (pct.fract() < 0.005) {
                        println!("      holding: {pct:.0}%");
                    }
                }
                CampEvent::Won | CampEvent::Lost => {}
            }
        }
    }
    let won = run.outcome().unwrap_or(false);
    if brief {
        println!(
            "  {:28} {} in {:>4.0}s, squad {}/{}",
            m.id,
            if won { "WON " } else { "LOST" },
            run.t,
            run.squad_alive(),
            squad.len()
        );
    } else if won {
        println!("   MISSION COMPLETE -- {:.0}s, squad {}/{}", run.t, run.squad_alive(), squad.len());
        for s in m.outro {
            println!("   | {s}");
        }
    } else {
        println!("   MISSION FAILED -- {:.0}s. the campaign does not advance.", run.t);
    }
    won
}
