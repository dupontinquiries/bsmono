//! VO — the voice bank, built from the screenplays that are already here.
//!
//! `games/shared/linebank` owns the machinery: ids derived from content,
//! screenplay parsing, the script an actor reads, the fault list, the size
//! budget. This is the thin part that knows where THIS game keeps its scripts
//! and what else it wants recorded.
//!
//! Run it:
//!
//! ```text
//! cargo run -- --vo            # summary and faults
//! cargo run -- --vo --write DIR  # per-actor scripts to DIR
//! ```
//!
//! ## Why the blips are authored here and the lines are not
//!
//! A screenplay contains every spoken line and no barks. Nobody writes
//! "(short acknowledgement)" into a scene, because it is not a scene — it is
//! the sound a character makes when they take a hit, acknowledge an order, or
//! die, fired hundreds of times an hour and needing five takes each so it does
//! not wear out.
//!
//! That half cannot be extracted from anything, so it is a list, and it lives
//! next to the game rather than in the shared crate because it is a design
//! decision about THIS cast: who speaks, what they say when nothing is
//! scripted, and how tired the player will be of hearing it.
//!
//! ## The count that matters
//!
//! `linebank::assets::budget` prices the bank before anyone books a booth.
//! Against the two written films that is about five minutes of dialogue and
//! under a megabyte of Opus — the blips will dwarf it in COUNT and stay small
//! in bytes, which is the shape a bank of this kind always has.

use linebank::assets::{budget, Quality};
use linebank::ids::derive_unique;
use linebank::{screenplay, Bank, Fault, Kind, Unit};

/// Where the written scenes live, relative to the crate root.
pub const SCRIPTS: &[&str] = &[
    "docs/films/FILM_05_SOLAR_SCREENPLAY.md",
    "docs/films/FILM_06_PHOTON_SCREENPLAY.md",
];

/// The unscripted half: what a character says when nothing is written.
///
/// `(speaker, description, takes)`. The description is what the actor reads on
/// the page — these are performed, not spoken, so it is a direction rather
/// than a line.
///
/// Take counts are deliberately uneven. A hit reaction fires far more often
/// than a death, and `linebank::Bag` draws without replacement, so five takes
/// of a common bark sound less repetitive than a dozen through a naive random
/// pick. Spending takes where the FREQUENCY is, rather than spreading them
/// evenly, is most of the perceived variety.
pub const BLIPS: &[(&str, &str, u8)] = &[
    ("BILLY", "acknowledging an order", 6),
    ("BILLY", "taking a hit -- winded, not screaming", 6),
    ("BILLY", "effort, lifting or bracing", 5),
    ("BILLY", "spotting something wrong", 4),
    ("BILLY", "going down", 2),
    ("AIDEN", "acknowledging an order", 6),
    ("AIDEN", "taking a hit", 6),
    ("AIDEN", "effort", 5),
    ("AIDEN", "spotting something wrong", 4),
    ("AIDEN", "going down", 2),
    ("MERIDIAN", "reading an instrument, half to herself", 5),
    ("MERIDIAN", "acknowledging traffic", 5),
    ("THE MORPHER", "the moment before a shift", 5),
    ("THE MORPHER", "the cost of a shift landing", 4),
    ("HALYARD", "acknowledging an order", 5),
    ("HALYARD", "effort", 4),
];

/// Build the whole bank: scripted lines, then the barks.
///
/// `root` is the crate directory, so this works from a test as well as from
/// the binary.
pub fn build(root: &std::path::Path) -> (Bank, Vec<String>) {
    let mut bank = Bank::default();
    let mut missing = Vec::new();

    for rel in SCRIPTS {
        match std::fs::read_to_string(root.join(rel)) {
            Ok(md) => {
                screenplay::into_bank(&md, &mut bank);
            }
            // A missing screenplay is reported rather than fatal: films 7-9 do
            // not exist yet and this should still price what does.
            Err(_) => missing.push((*rel).to_string()),
        }
    }

    let mut ids: Vec<String> = bank.units().iter().map(|u| u.id.clone()).collect();
    for (who, what, takes) in BLIPS {
        let id = derive_unique(who, what, ids.iter().map(|s| s.as_str()));
        ids.push(id.clone());
        bank.add(
            Unit::new(&id, who, &format!("({what})"), Kind::Blip)
                .directed(what)
                .wanting(*takes),
        );
    }
    (bank, missing)
}

/// Rough seconds for a line, for budgeting before anything is recorded.
///
/// Fourteen characters a second is ordinary delivery. It is an estimate and
/// the only thing that depends on it is a size figure, so a blip's fixed
/// third of a second is close enough and a long line's estimate is honest.
fn estimate_seconds(u: &Unit) -> f32 {
    match u.kind {
        Kind::Blip => 0.35,
        _ => (u.text.len() as f32 / 14.0).max(0.6),
    }
}

pub fn run(argv: &[String]) {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let (bank, missing) = build(root);

    println!("VOICE BANK\n");
    for m in &missing {
        println!("  MISSING  {m}");
    }

    let mut takes = Vec::new();
    for u in bank.units() {
        for _ in 0..u.wanted {
            takes.push((estimate_seconds(u), Quality::for_kind(u.kind)));
        }
    }
    let b = budget(&takes);

    let lines = bank.units().iter().filter(|u| u.kind != Kind::Blip).count();
    let blips = bank.units().iter().filter(|u| u.kind == Kind::Blip).count();
    println!("  {lines} written lines, {blips} barks, {} speakers", bank.speakers().len());
    println!("  {} takes to record, about {:.0} s in the booth", b.takes, b.seconds);
    println!(
        "  ships as {:.2} MB of Opus ({:.0} MB if it were WAV)\n",
        b.per_file as f32 / 1e6,
        b.as_wav as f32 / 1e6
    );

    let faults = bank.faults();
    let structural: Vec<&Fault> =
        faults.iter().filter(|f| !matches!(f, Fault::Short { .. })).collect();
    if structural.is_empty() {
        println!("  no id faults\n");
    } else {
        for f in &structural {
            println!("  FAULT  {f:?}");
        }
        println!();
    }

    for who in bank.speakers() {
        let n = bank.units().iter().filter(|u| u.speaker == who).count();
        println!("  {who:<14} {n:>3} units");
    }

    if let Some(i) = argv.iter().position(|a| a == "--write") {
        let Some(dir) = argv.get(i + 1) else {
            println!("\n--write needs a directory");
            return;
        };
        let dir = std::path::Path::new(dir);
        if let Err(e) = std::fs::create_dir_all(dir) {
            println!("\ncannot write to {}: {e}", dir.display());
            return;
        }
        println!();
        for who in bank.speakers() {
            let name = who.to_lowercase().replace(|c: char| !c.is_ascii_alphanumeric(), "_");
            let path = dir.join(format!("{name}.txt"));
            match std::fs::write(&path, bank.script_for(&who)) {
                Ok(()) => println!("  wrote {}", path.display()),
                Err(e) => println!("  FAILED {}: {e}", path.display()),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bank() -> Bank {
        build(std::path::Path::new(env!("CARGO_MANIFEST_DIR"))).0
    }

    #[test]
    fn the_screenplays_this_points_at_still_exist() {
        // The failure this catches is a renamed or moved script producing an
        // empty bank that looks like a working one.
        let (_, missing) = build(std::path::Path::new(env!("CARGO_MANIFEST_DIR")));
        assert!(missing.is_empty(), "screenplays not found: {missing:?}");
    }

    #[test]
    fn every_id_in_the_real_bank_is_recordable() {
        // Derived ids against real prose, not a fixture. The only faults
        // allowed are units with no takes yet.
        let b = bank();
        let faults = b.faults();
        let structural: Vec<&Fault> =
            faults.iter().filter(|f| !matches!(f, Fault::Short { .. })).collect();
        assert!(structural.is_empty(), "{structural:?}");
    }

    #[test]
    fn the_barks_belong_to_people_who_are_actually_in_the_films() {
        // A bark list drifts from the cast silently: a character gets renamed
        // in the scripts and their barks become an actor nobody has cast.
        let b = bank();
        let scripted: Vec<String> = {
            let mut s = Bank::default();
            let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
            for rel in SCRIPTS {
                if let Ok(md) = std::fs::read_to_string(root.join(rel)) {
                    screenplay::into_bank(&md, &mut s);
                }
            }
            s.speakers()
        };
        for (who, _, _) in BLIPS {
            assert!(
                scripted.contains(&who.to_string()),
                "{who} has barks but no lines in any screenplay; cast is {scripted:?}"
            );
        }
    }

    #[test]
    fn a_script_reaches_every_actor_and_carries_its_directions() {
        let b = bank();
        for who in b.speakers() {
            let s = b.script_for(&who);
            assert!(s.contains(&who), "{who}'s script does not name them");
            let mine = b.units().iter().filter(|u| u.speaker == who).count();
            assert!(mine > 0);
            for u in b.units().iter().filter(|u| u.speaker == who) {
                assert!(s.contains(&u.id), "{}'s script is missing {}", who, u.id);
            }
        }
    }

    #[test]
    fn the_bank_is_worth_recording_and_not_worth_worrying_about() {
        // Two numbers a producer asks for. Both are estimates, and the point
        // of asserting them is that a change of an order of magnitude means
        // something moved that nobody meant to move.
        let b = bank();
        let mut takes = Vec::new();
        for u in b.units() {
            for _ in 0..u.wanted {
                takes.push((estimate_seconds(u), Quality::for_kind(u.kind)));
            }
        }
        let est = budget(&takes);
        assert!(est.takes > 100, "only {} takes; is the bank empty?", est.takes);
        assert!(est.seconds > 60.0 && est.seconds < 3600.0, "{:.0} s", est.seconds);
        assert!(est.per_file < 5_000_000, "{} bytes is more than expected", est.per_file);
        assert!(est.as_wav > est.per_file * 10, "Opus is not earning its place");
    }

    #[test]
    fn blips_get_more_takes_than_lines_because_they_repeat() {
        let b = bank();
        let blip_takes: u32 =
            b.units().iter().filter(|u| u.kind == Kind::Blip).map(|u| u.wanted as u32).sum();
        let blips = b.units().iter().filter(|u| u.kind == Kind::Blip).count() as u32;
        assert!(blip_takes as f32 / blips as f32 > 3.0, "barks are under-covered");
        assert!(
            b.units().iter().filter(|u| u.kind != Kind::Blip).all(|u| u.wanted == 1),
            "a written line wants one take"
        );
    }
}
