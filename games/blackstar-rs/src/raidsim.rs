//! RAID SIM — the whole raid loop, run headless and narrated.
//!
//! The `--sim` harness proved the striker AI by making its loop visible;
//! this does the same for raids. No window, no GPU: the generator lays the
//! raid out, the planner assesses each phase's sky and footholds, the
//! runtime steps the encounters at a fixed tick under a scripted squad, and
//! the loot layer settles the bill. Every decision prints WITH its reason,
//! because a loop you can read is a loop you can tune.
//!
//!      blackstar --raid                      the roster, the log, the seal
//!      blackstar --raid gigadrill            run one raid end to end
//!      blackstar --raid gigadrill 42         ... at a seed
//!      blackstar --raid all                  sweep the roster, one line each
//!      blackstar --raid codex                relics and shards recovered so far
//!
//! Steering: `--foothold lz|fort` forces the approach, `--suits` runs a
//! human-suited team, `--team N` sizes the squad (SYNC wants three;
//! shorthanded squads get drone seats, which is the relay/download-play
//! answer to the mechanic). Victories are DRY RUNS unless `--commit` is
//! given, which banks the shard into the save file — the sim is also the
//! game loop, so progression is explicit rather than a side effect.

use crate::foothold::{self, Approach};
use crate::loot;
use crate::raid::window_s;
use crate::raidcon::{Mech, BOSSES};
use crate::raidgen::{enc_seconds, phase_raid, plan_for, EncKind};
use crate::raidlog::RaidLog;
use crate::raidrun::{Event, PhaseRun, RaidRun, Unit, KILL_QUOTA, NODE_Y_AIR, PLATE_DX};
use crate::save;
use crate::survival::Body;

const DT: f32 = 1.0 / 120.0;

pub fn run(argv: &[String]) {
    let i = argv.iter().position(|a| a == "--raid").unwrap();
    let pick = argv.get(i + 1).map(|s| s.as_str()).filter(|s| !s.starts_with("--"));
    let seed: u64 = argv.get(i + 2).and_then(|s| s.parse().ok()).unwrap_or(7);
    let body = if argv.iter().any(|a| a == "--suits") { Body::HumanSuited } else { Body::Machine };
    let speed = if body == Body::Machine { 14.0 } else { 2.5 };
    let commit = argv.iter().any(|a| a == "--commit");
    let team_n: usize = argv
        .iter()
        .position(|a| a == "--team")
        .and_then(|p| argv.get(p + 1))
        .and_then(|s| s.parse().ok())
        .unwrap_or(3);
    let forced: Option<Approach> = argv.iter().position(|a| a == "--foothold").and_then(|p| {
        match argv.get(p + 1).map(|s| s.as_str()) {
            Some("lz") => Some(Approach::ClearLz),
            Some("fort") => Some(Approach::SeizeFortress),
            _ => None,
        }
    });

    match pick {
        None => roster(),
        Some("codex") => codex(),
        Some("all") => {
            let mut won: Vec<&str> = Vec::new();
            let mut margins: Vec<(&str, f32)> = Vec::new();
            for b in BOSSES {
                if let Some(m) = one(b.id, seed, body, speed, team_n, forced, true) {
                    won.push(b.id);
                    margins.push((b.id, m));
                }
            }
            // THE TUNING HEADLINE. A sweep that only counts clears cannot
            // tell a knife-edge roster from a flat one; the margins can.
            let mut tight: Vec<&(&str, f32)> = margins.iter().filter(|(_, m)| m.is_finite()).collect();
            tight.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
            match (tight.first(), tight.last()) {
                (Some((tid, tm)), Some((sid, sm))) if tight.len() > 1 => println!(
                    "\n  clocked raids: {} of {}  tightest {} at {:.0}s, slackest {} at {:.0}s",
                    tight.len(), margins.len(), tid, tm, sid, sm
                ),
                (Some((tid, tm)), _) => println!(
                    "\n  clocked raids: 1 of {}  {} at {:.0}s", margins.len(), tid, tm
                ),
                _ => println!("\n  clocked raids: none -- every sky in the sweep was open"),
            }
            // committed: the sweep lands on the save's log. Dry: the sweep
            // is its own hypothetical world, counted from zero.
            let log = if commit {
                bank(&won, body == Body::HumanSuited)
            } else {
                let mut l = RaidLog::default();
                for id in &won {
                    l.record(id);
                }
                l
            };
            println!("\n{}/19 shards -- {}", log.shards(), log.seal().label());
            println!("  {}", log.seal().reveal());
            if !commit {
                println!("  (dry run -- add --commit to bank the sweep)");
            }
        }
        Some(id) => {
            let victory = one(id, seed, body, speed, team_n, forced, false).is_some();
            if victory {
                settle(id, commit, body == Body::HumanSuited);
            }
        }
    }
}

/// The log as the save file knows it.
fn load_log() -> RaidLog {
    let (sv, _, _) = save::load();
    RaidLog::from_line(&sv.wardens)
}

/// Bank a set of clears into the save -- the shard, the DROP, and (for
/// a suited team) the chi the arena trained -- through
/// `stations::record_raid_outcome`, the same path the flown raid takes,
/// so the sim and the game cannot reward differently. A repeat clear
/// banks nothing and drops nothing.
fn bank(ids: &[&str], suited: bool) -> RaidLog {
    for id in ids {
        let banked = crate::stations::record_raid_outcome(id, suited);
        if !banked.fresh {
            continue;
        }
        for line in banked.lines() {
            println!("   {line}");
        }
    }
    load_log()
}

/// After a single victory: bank it (or say how to), and narrate what the
/// shard did to the seal — the reveal fires exactly when the state turns.
fn settle(id: &str, commit: bool, suited: bool) {
    if !commit {
        println!("   (dry run -- add --commit to bank the shard)");
        return;
    }
    let before = load_log().seal();
    let log = bank(&[id], suited);
    println!("   shard banked: {}/19 -- {}", log.shards(), log.seal().label());
    if log.seal() != before {
        println!("   {}", log.seal().reveal());
    }
    if let Some(next) = log.next_up() {
        println!("   next up the ladder: {} (t{}, {})", next.id, next.tier, next.mech.label());
    }
}

fn roster() {
    let log = load_log();
    println!("THE NINETEEN -- wardens of the lock. `--raid <id>` to run one.\n");
    for b in BOSSES {
        let mark = if log.is_cleared(b.id) { "x" } else { " " };
        println!(
            "{mark} {:12} t{} {:6} @{:11} {} -- \"{}\"",
            b.id,
            b.tier,
            b.mech.label(),
            b.arena,
            b.name,
            b.title
        );
        println!("  {:12}            derived from: {}", "", b.premise);
        println!("  {:12}            it asks: {}", "", b.mech.asks());
    }
    println!("\n{}/19 shards -- {}", log.shards(), log.seal().label());
    println!("  {}", log.seal().reveal());
    if let Some(next) = log.next_up() {
        println!("  next up the ladder: {} (t{}, {})", next.id, next.tier, next.mech.label());
    }
}

/// What the fallen wardens have taught: their relic entries, their shard
/// lines, and where the seal stands. The fiction's payoff loop, readable.
fn codex() {
    let log = load_log();
    println!("THE CODEX -- {}/19 relics recovered\n", log.shards());
    for b in BOSSES {
        if log.is_cleared(b.id) {
            println!("  {} -- \"{}\"", b.name, b.title);
            for l in b.relic {
                println!("     {l}");
            }
            println!("     shard: {}", b.shard);
        } else {
            println!("  {} -- sealed", b.name);
        }
        println!();
    }
    println!("{}", log.seal().label());
    println!("  {}", log.seal().reveal());
}

/// Run one raid end to end. Returns whether the shard came home.
/// Run one raid. `Some(tightest)` on victory — the smallest window
/// margin the run ever saw, in seconds (INFINITY when every sky was
/// open) — `None` on any failure. The margin is the tuning number the
/// sweep was throwing away: 152 consecutive clears say nothing about
/// difficulty, but the same 152 runs ranked by how CLOSE they came say
/// everything.
fn one(id: &str, seed: u64, body: Body, speed: f32, team_n: usize, forced: Option<Approach>, brief: bool) -> Option<f32> {
    let Some(boss) = crate::raidcon::boss(id) else {
        println!("no such warden: {id}  (run `--raid` for the roster)");
        return None;
    };
    let plan = plan_for(boss, 3, seed);
    if !brief {
        println!("== {} -- \"{}\"  [{} t{}]", boss.name, boss.title, boss.mech.label(), boss.tier);
        println!("   mechanic from the fiction: {}", boss.premise);
    }

    let mut run = RaidRun::new(&plan);
    let mut clock_spent = 0.0f32;
    let mut tightest = f32::INFINITY;
    while !run.over {
        let pi = run.phase;
        let spec = &plan.phases[pi];
        let pr_raid = phase_raid(&plan, pi);
        let window = window_s(&pr_raid.orbit, pr_raid.weather);
        if !brief {
            println!(
                "\n-- {} @ {} ({})  weather {:?}  window {}",
                spec.name,
                spec.arena.name,
                spec.arena.id,
                pr_raid.weather,
                if window.is_finite() { format!("{:.0}s", window) } else { "open".into() }
            );
        }
        // THE FOOTHOLD DECISION, shown with its reasoning
        let plans: Vec<foothold::Plan> = pr_raid
            .sites
            .iter()
            .map(|s| foothold::assess(body, s, pr_raid.weather, window, speed))
            .collect();
        if !brief {
            for p in &plans {
                let verdict = if !p.viable {
                    p.note.to_string()
                } else if p.margin_s.is_finite() {
                    format!("margin {:.0}s", p.margin_s)
                } else {
                    "open sky".to_string()
                };
                println!("   {:18} take {:>4.0}s  {}", p.site.approach.label(), p.site.take_s, verdict);
            }
        }
        let chosen = match forced {
            Some(a) => plans.iter().find(|p| p.site.approach == a && p.viable).cloned(),
            None => foothold::choose(body, &pr_raid.sites, pr_raid.weather, window, speed),
        };
        let Some(fh) = chosen else {
            println!("   REPULSED -- no foothold could be taken inside the window");
            return None;
        };
        // the planner's rule, exactly: ANY shelter discounts the fighting
        // (LZ fights happen in the ship's shadow, fortress fights under
        // the roof); the take's exposure is already priced into margin_s
        let sheltered = fh.site.approach.shelter() != crate::survival::ShelterKind::None;
        if !brief {
            let how = match fh.site.approach {
                Approach::ClearLz => "fighting from the ship's shadow; the walk out is short",
                Approach::SeizeFortress => "under the fortress roof; the ship stays loose",
            };
            println!("   -> {} ({how})", fh.site.approach.label());
        }

        // THE ENCOUNTERS, stepped at 120 Hz under a scripted squad
        let pr: &mut PhaseRun = run.start_phase(fh.margin_s, sheltered);
        let mut phase_t = 0.0f32;
        for (ei, e) in spec.enc.iter().enumerate() {
            let fight_s = enc_seconds(e.mech, e.kind);
            let mut t = 0.0;
            let mut slammed = 0u32;
            // the squad fights for the encounter's priced seconds...
            while t < fight_s {
                // a flyer tests a GROUND field once, learns, and lands --
                // one squad member spends the first two seconds airborne
                let units = squad(e, team_n, t < 2.0);
                for ev in pr.step(DT, &units) {
                    if let Event::Slam { .. } = ev {
                        slammed += 1;
                    }
                    if ev == Event::WindowClosed {
                        println!("   x  the weather closed during {} -- {}", nm(e), spec.name);
                        return None;
                    }
                }
                t += DT;
            }
            // ...then does the thing the mechanic asks
            match e.kind {
                EncKind::Anchor => pr.champion_fell(),
                EncKind::Standard => match e.mech {
                    Mech::Air => {
                        pr.step(DT, &[Unit { pos: [e.x, NODE_Y_AIR, e.z] }]);
                    }
                    Mech::Sync => {
                        let posts = [
                            Unit { pos: [e.x, 1.0, e.z] },
                            Unit { pos: [e.x + PLATE_DX, 1.0, e.z] },
                            Unit { pos: [e.x - PLATE_DX, 1.0, e.z] },
                        ];
                        pr.step(DT, &posts);
                    }
                    _ => {
                        for _ in 0..KILL_QUOTA {
                            pr.credit_kill(ei);
                        }
                        pr.step(DT, &[]);
                    }
                },
            }
            phase_t += t;
            if !brief {
                let extra = if slammed > 0 { " (the field slammed our flyer down; noted)" } else { "" };
                let seats = if e.mech == Mech::Sync && team_n < 3 {
                    " [drone seats filled the plates]"
                } else {
                    ""
                };
                println!("   ok {:22} {:>4.0}s{}{}", nm(e), t, extra, seats);
            }
        }
        let pr = run.phase_run().unwrap();
        pr.step(DT, &[]);
        if !pr.cleared() {
            println!("   x  {} stalled unexpectedly", spec.name);
            return None;
        }
        let left = pr.left_s;
        tightest = tightest.min(left);
        clock_spent += phase_t;
        let is_finale = pi + 1 == plan.phases.len();
        if !brief {
            let l = if left.is_finite() { format!("{left:.0}s of window left") } else { "open sky".into() };
            println!("   {} cleared -- {}", spec.name, l);
        }
        // THE WALK OUT -- the finale settles extraction against what is
        // left of its window, exactly as the planner charges it: no
        // shelter discount, because you leave the fortress to leave
        if is_finale {
            let walk = pr_raid.extract_m / speed.max(0.1)
                * if fh.site.approach.pins_the_ship() { 1.0 } else { 1.4 };
            tightest = tightest.min(left - walk);
            if left - walk < 0.0 {
                println!(
                    "   x  boss down, and the front caught the team {:.0}s short on the {:.0}m walk out",
                    walk - left,
                    pr_raid.extract_m
                );
                return None;
            }
        }
        run.advance();
    }

    // THE BILL -- extraction survived; the loot layer settles it
    let finale = phase_raid(&plan, plan.phases.len() - 1);
    let team: Vec<Body> = vec![body; team_n.max(1)];
    let drop = loot::roll(&finale.orbit, plan.boss.name, &team, seed as u32, plan.boss.tier);
    if brief {
        let d = drop
            .as_ref()
            .map(|d| format!("{} {:?}", d.grade.label(), d.base.effect))
            .unwrap_or_else(|| "nothing wearable".into());
        let m = if tightest.is_finite() {
            format!("tightest {:>5.0}s", tightest)
        } else {
            "open sky     ".into()
        };
        println!("  {:12} CLEARED in {:>5.0}s fight-time  {}  -- {}", id, clock_spent, m, d);
    } else {
        println!("\n== {} is down. the {:.0}m walk out held.", plan.boss.name, finale.extract_m);
        match &drop {
            Some(d) => println!(
                "   payout: {} {:?} off {}  (what this world demanded)",
                d.grade.label(),
                d.base.effect,
                d.from
            ),
            None => println!("   payout: nothing this team can wear"),
        }
        println!("   shard: {}", plan.boss.shard);
    }
    Some(tightest)
}

fn nm(e: &crate::raidgen::EncSpec) -> String {
    format!(
        "{}{}",
        e.mech.label(),
        if e.kind == EncKind::Anchor { " (anchor)" } else { "" }
    )
}

/// The scripted squad for one encounter: posted where the mechanic wants
/// them, with one member briefly airborne to demonstrate GROUND fields.
fn squad(e: &crate::raidgen::EncSpec, n: usize, flyer_up: bool) -> Vec<Unit> {
    let mut u = vec![Unit { pos: [e.x + 4.0, if flyer_up { 10.0 } else { 1.0 }, e.z] }];
    for k in 1..n.max(1) {
        u.push(Unit { pos: [e.x - 4.0 * k as f32, 1.0, e.z + 3.0] });
    }
    u
}
