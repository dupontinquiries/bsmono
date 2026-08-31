//! TUNING — the numbers this project CHOSE, sorted by how likely they are to
//! be wrong.
//!
//! Not every number. The module used to open with "every number I guessed,
//! in one place", which covered eleven of the sixty-five config keys — an
//! overstatement of the same kind this session has spent its time deleting
//! elsewhere. The other fifty-four are accounted for below rather than
//! quietly absent, and a test forces a new key into one of the three
//! categories rather than letting the gap grow.
//!
//! A lot of this project's recent work is decisions I cannot check. Whether
//! the mystery anchor should start at 45%, whether a case of six creates
//! pressure or annoyance, whether two independent sources is the right moment
//! for a map to say POSSIBLE — none of these are answerable without playing.
//! I picked defaults and reasoned about them, which is the most I can do, and
//! then the useful question becomes: when someone finally does play, how do
//! they find the numbers?
//!
//! Badly, is the honest answer, if the numbers are scattered as consts across
//! nine modules. So this is a registry:
//!
//! * every knob's **live value**, read through a function rather than restated
//!   — a ledger that copies its own numbers is a doc that rots, and this
//!   project already has one guard against that habit
//!   (`archive::the_story_bible_is_current`), so it gets a second;
//! * **where to change it** — a config key (a save-file line, no rebuild) or a
//!   const with its file, stated plainly;
//! * **why it is that number**, in one line;
//! * and how much I actually believe it.
//!
//! ## Confidence is the load-bearing column
//!
//! `--tune` sorts GUESS first, because that is the actionable output of a
//! playthrough: not "here are forty numbers" but "these six are the ones
//! nobody has ever verified, attack them in this order." A knob marked
//! MEASURED came from a simulation or a published figure and should be
//! changed only with a reason; a knob marked GUESS is an invitation.
//!
//! There is a test asserting the ledger's stated defaults equal the code's
//! actual defaults, so the two cannot drift.

use crate::config::Config;

/// How much the number is worth.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Faith {
    /// Nobody has checked this against anything. Start here.
    Guess,
    /// Argued from something real, but the argument could be wrong.
    Reasoned,
    /// Came out of a sim, a published figure, or an experiment.
    Measured,
}

impl Faith {
    pub fn tag(self) -> &'static str {
        match self {
            Faith::Guess => "GUESS   ",
            Faith::Reasoned => "reasoned",
            Faith::Measured => "measured",
        }
    }
}

/// Where you go to change it.
///
/// One variant, deliberately. A `Const` arm was the obvious second one and it
/// had no members: every knob worth listing here got promoted to a config key
/// on the way in, because a feel number you cannot change without a rebuild is
/// a number nobody will tune. If something genuinely cannot be a config key,
/// add the variant then rather than keeping an empty one now.
#[derive(Clone, Copy, Debug)]
pub enum Set {
    /// A `cfg <key> <value>` line in the save file. No rebuild.
    Cfg(&'static str),
}

pub struct Knob {
    pub id: &'static str,
    pub group: &'static str,
    pub faith: Faith,
    pub set: Set,
    /// The live value, read rather than restated.
    pub read: fn(&Config) -> f32,
    /// What it is at defaults. Checked against `read(&Config::default())`.
    pub default: f32,
    pub why: &'static str,
}

pub static KNOBS: &[Knob] = &[
    // ---------------------------------------------------------- story ----
    Knob {
        id: "story_anchor",
        group: "story",
        faith: Faith::Guess,
        set: Set::Cfg("story_anchor"),
        read: |c| c.story.anchor,
        default: 0.45,
        why: "45% = you know what you lived through and nothing past it. The \
              midpoint is defensible and nothing more; the whole reason it is a \
              dial is that I cannot tell which end reads better.",
    },
    Knob {
        id: "case_cap",
        group: "story",
        faith: Faith::Guess,
        set: Set::Cfg("case_cap"),
        read: |c| c.story.case_cap,
        default: 6.0,
        why: "the pressure to relay before the case fills IS the tradecraft \
              loop. Too roomy and the loop vanishes; too tight and it is \
              errand-running. Six is a shape, not a measurement.",
    },
    Knob {
        id: "story_era",
        group: "story",
        faith: Faith::Reasoned,
        set: Set::Cfg("story_era"),
        read: |c| c.story.era,
        default: 0.0,
        why: "0 = the Blackstar present (a ship, scan it and it is yours), 1 = the \
              archive era (a case and a relay). It is a dial rather than a story \
              flag because switching between them is what testing both needs.",
    },
    Knob {
        id: "arch_possible_at",
        group: "story",
        faith: Faith::Reasoned,
        set: Set::Cfg("arch_possible_at"),
        read: |c| c.story.possible_at,
        default: 2.0,
        why: "two independent sources is a pattern. Argued from how corroboration \
              actually reads, but the PACING it produces is untested — if the map \
              firms up before a discovery feels earned, this is the number.",
    },
    Knob {
        id: "arch_high_at",
        group: "story",
        faith: Faith::Reasoned,
        set: Set::Cfg("arch_high_at"),
        read: |c| c.story.high_at,
        default: 3.0,
        why: "three is an argument. Deliberately small: a player who must collect \
              eleven of something is doing inventory, not archaeology.",
    },
    // ----------------------------------------------------------- feel ----
    Knob {
        id: "land_assist",
        group: "feel",
        faith: Faith::Reasoned,
        set: Set::Cfg("land_assist"),
        read: |c| c.flip.land_assist,
        default: 0.0,
        why: "wheels-down magnetism while falling. Off by default because raw RL \
              rules are the reference; it exists for players who find landing \
              after a flip fights them.",
    },
    Knob {
        id: "cam_shake",
        group: "feel",
        faith: Faith::Guess,
        set: Set::Cfg("cam_shake"),
        read: |c| c.cam.shake,
        default: 0.6,
        why: "wired to landings and goals. Nobody has watched this at length; \
              shake is the setting players notice first and forgive last.",
    },
    Knob {
        id: "cam_grav_follow",
        group: "feel",
        faith: Faith::Reasoned,
        set: Set::Cfg("cam_grav"),
        read: |c| c.cam.grav_follow,
        default: 1.0,
        why: "the horizon rolls with local gravity in tubes. Full by default \
              because the alternative actively confuses corkscrews, but full is \
              a strong claim for players who get motion sick.",
    },
    Knob {
        id: "analog_soft",
        group: "feel",
        faith: Faith::Measured,
        set: Set::Cfg("car_analog_soft"),
        read: |c| c.car.analog_soft,
        default: 0.45,
        why: "the analog wind-in floor: a half press lands near 0.75, a full press \
              is exactly 1.0. Tuned against the dribble-start problem it exists \
              to solve.",
    },
    // ---------------------------------------------------------- video ----
    Knob {
        id: "bloom",
        group: "video",
        faith: Faith::Guess,
        set: Set::Cfg("bloom"),
        read: |c| c.video.bloom,
        default: 0.7,
        why: "boost flames, torch towers and goal panels halo at this. Picked to \
              look right in a still, which is not the same as looking right in \
              motion for an hour.",
    },
    Knob {
        id: "vignette",
        group: "video",
        faith: Faith::Guess,
        set: Set::Cfg("vignette"),
        read: |c| c.video.vignette,
        default: 0.32,
        why: "part of the cinematic grade. Cheap to change, easy to overdo, and \
              nobody has looked at it on a phone.",
    },
];

/// Keys whose value is CALIBRATED AGAINST ROCKET LEAGUE, not chosen here.
///
/// `DESIGN.md` carries the conversion table — gravity 650 uu/s² becomes
/// 6.5 m/s², ball radius 93.25 uu becomes 0.9325 m, and so on. These are not
/// judgment calls with a confidence level; they are a reference
/// implementation's numbers, and the right way to change one is to discover
/// that the conversion was wrong, not to decide it feels better. Listing them
/// as GUESSES would bury the eleven that genuinely are.
pub static RL_CALIBRATED: &[&str] = &[
    "gravity", "arena_bounce", "arena_friction", "arena_scale",
    "ball_bounce", "ball_friction", "ball_size", "ball_weight",
    "car_accel", "car_boost", "car_boost_max", "car_boost_regen",
    "car_grip", "car_jump", "car_size", "car_top",
    "flip_cancel", "flip_spin", "flip_strength", "flip_strict",
    "cam_fov", "cam_dist", "cam_height", "cam_angle", "cam_stiff",
];

/// Keys that are not feel at all: hardware settings, accessibility, and
/// switches whose right value is whatever the player says it is.
///
/// A tuning ledger that listed `vsync` alongside the mystery anchor would be
/// telling somebody with a playthrough's worth of notes to go and have an
/// opinion about vsync.
pub static NOT_A_FEEL_KNOB: &[(&str, &str)] = &[
    ("vsync", "hardware"),
    ("gfx_tier", "hardware"),
    ("render_scale", "hardware"),
    ("ray_quality", "hardware"),
    ("rays", "hardware"),
    ("screen_door", "hardware"),
    ("story_debug_ship", "the master ship: a testing override, not a feel knob — it removes progression gates rather than changing how anything plays"),
    ("preset", "display only; records the last mutator preset picked"),
    ("filmic", "a grade the player picks"),
    ("exposure", "a grade the player picks"),
    ("grain", "a grade the player picks"),
    ("letterbox", "a grade the player picks"),
    ("touch_enabled", "input hardware"),
    ("touch_scale", "accessibility"),
    ("gyro_on", "input hardware"),
    ("gyro_sens", "accessibility"),
    ("gyro_dz", "accessibility"),
    ("gyro_smooth", "accessibility"),
    ("gyro_air", "accessibility"),
    ("gyro_gate", "accessibility"),
    ("gyro_ix", "accessibility"),
    ("gyro_iy", "accessibility"),
    ("input_ease", "accessibility"),
    ("steer_rate", "accessibility"),
    ("car_analog", "accessibility"),
    ("air_roll_default", "a control scheme the player picks"),
    ("flip_resets", "a rule toggle, not a number"),
    ("flip_unlimited", "a rule toggle, not a number"),
    ("car_boost_unl", "a rule toggle, not a number"),
    ("cam_plat_bend", "a comfort setting"),
];

pub fn knob(id: &str) -> Option<&'static Knob> {
    KNOBS.iter().find(|k| k.id == id)
}

/// The knobs nobody has verified, worst first. The actionable list.
pub fn unverified() -> Vec<&'static Knob> {
    let mut v: Vec<_> = KNOBS.iter().filter(|k| k.faith == Faith::Guess).collect();
    v.sort_by_key(|k| (k.group, k.id));
    v
}

pub fn run(argv: &[String]) {
    let i = argv.iter().position(|a| a == "--tune").unwrap_or(0);
    let only = argv.get(i + 1).map(String::as_str);
    let (_, _, cfg) = crate::save::load();

    if only == Some("search") {
        crate::tuner::run();
        return;
    }
    if only == Some("kit") {
        kit();
        return;
    }
    let mut ks: Vec<&Knob> = match only {
        Some("guesses") => unverified(),
        // an exact id wins over a group name, so `--tune bloom` shows one knob
        Some(g) => match knob(g) {
            Some(k) => vec![k],
            None => KNOBS.iter().filter(|k| k.group == g).collect(),
        },
        None => KNOBS.iter().collect(),
    };
    if ks.is_empty() {
        println!("nothing matches '{}'. groups: story, feel, video", only.unwrap_or(""));
        return;
    }
    // guesses first: the point of the ledger is the order, not the list
    ks.sort_by_key(|k| (k.faith, k.group, k.id));

    println!(
        "TUNING LEDGER — {} chosen here, {} never verified",
        KNOBS.len(),
        unverified().len()
    );
    println!(
        "  ({} more calibrated against Rocket League, {} not feel knobs at all)\n",
        RL_CALIBRATED.len(),
        NOT_A_FEEL_KNOB.len()
    );
    let mut group = "";
    for k in ks {
        if k.group != group {
            group = k.group;
            println!("-- {}", group.to_uppercase());
        }
        let live = (k.read)(&cfg);
        let moved = if (live - k.default).abs() > 1e-6 {
            format!("  (default {:.3})", k.default)
        } else {
            String::new()
        };
        let Set::Cfg(key) = k.set;
        let at = format!("cfg {key}");
        println!("  [{}] {:<18} {:.3}{}", k.faith.tag(), k.id, live, moved);
        println!("             set: {at}");
        println!("             {}\n", wrap(k.why, 62, 13));
    }
    if only.is_none() {
        println!("`--tune guesses` for just the unverified ones, `--tune kit` for the");
        println!("curriculum, `--tune search` to attack the guessed gates with a sim.");
        println!("curriculum ledger.");
        println!("cfg lines live in skydribble.save; edit and relaunch, no rebuild.");
    }
}

/// The curriculum ledger, printed.
///
/// `curriculum.rs` carries `used_in`, `built` and `module` for every mechanic
/// and nothing ever read them — they existed only to be asserted by that
/// module's own lints. A ledger whose data cannot be looked at is a test
/// fixture wearing a table's clothes, and the `built` column in particular is
/// honest bookkeeping about wiring debt that belongs next to `--tune`'s
/// honest bookkeeping about guessed numbers.
fn kit() {
    use crate::curriculum::{Where, CURRICULUM};
    let built = CURRICULUM.iter().filter(|m| m.built).count();
    println!("THE CURRICULUM — {} mechanics, {built} wired\n", CURRICULUM.len());
    println!("  {:<18} {:<28} {:<7} {}", "mechanic", "taught by", "wired", "where it is used");
    for m in CURRICULUM {
        let places: Vec<&str> = m
            .used_in
            .iter()
            .map(|w| match w {
                Where::Campaign => "campaign",
                Where::OpenWorld => "open world",
                Where::Raid => "raids",
            })
            .collect();
        println!(
            "  {:<18} {:<28} {:<7} {}",
            m.id,
            m.taught_by,
            if m.built { "yes" } else { "NO" },
            places.join(", ")
        );
        println!("       {:<12} {}", m.module, wrap(m.note, 58, 20));
    }
    let open = crate::curriculum::open_world_kit().len();
    println!("\n  {open} of {} survive graduation.", CURRICULUM.len());
    if open == CURRICULUM.len() {
        println!("  nothing is campaign-only, which is the promise the campaign makes:");
        println!("  it opens onto a superset, not a smaller multiplayer subset.");
    } else {
        println!("  SOMETHING IS CAMPAIGN-ONLY. the campaign is a demo.");
    }
    let unwired: Vec<&str> =
        CURRICULUM.iter().filter(|m| !m.built).map(|m| m.id).collect();
    if !unwired.is_empty() {
        println!("\n  not wired yet: {} (see docs/PLAN.md)", unwired.join(", "));
    }
}

fn wrap(text: &str, w: usize, indent: usize) -> String {
    let pad = " ".repeat(indent);
    let (mut out, mut col) = (String::new(), 0);
    for word in text.split_whitespace() {
        if col > 0 && col + 1 + word.len() > w {
            out.push('\n');
            out.push_str(&pad);
            col = 0;
        } else if col > 0 {
            out.push(' ');
            col += 1;
        }
        out.push_str(word);
        col += word.len();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn the_ledger_states_the_real_defaults() {
        // the guard that stops this becoming a doc that lies. every stated
        // default must equal what the code actually starts at.
        let d = Config::default();
        for k in KNOBS {
            let live = (k.read)(&d);
            assert!(
                (live - k.default).abs() < 1e-6,
                "{} says its default is {} and the code says {}",
                k.id,
                k.default,
                live
            );
        }
    }

    #[test]
    fn every_config_backed_knob_survives_a_save_round_trip() {
        // "no rebuild needed" is a promise the ledger makes to whoever is
        // tuning. it is only true if the key actually parses.
        // Probe from several directions. A single probe is not enough: every
        // config key clamps to its own range, so `default + 1` silently
        // becomes a no-op on anything bounded at 1.0 — which is how this test
        // first accused `cam_grav` of being broken when the key was fine and
        // the probe was wrong.
        let base = Config::default();
        for k in KNOBS {
            let Set::Cfg(key) = k.set;
            let here = (k.read)(&base);
            let probes = [here + 1.0, here * 0.5, here - 0.5, 0.0, 1.0, 2.0];
            let moved = probes.iter().any(|p| {
                let mut c = Config::default();
                c.load_line(key, &p.to_string());
                ((k.read)(&c) - here).abs() > 1e-9
            });
            assert!(
                moved,
                "cfg key '{key}' for {} never changes the value — the ledger \
                 promises a save-file edit that does not work",
                k.id
            );
        }
    }

    /// Every `cfg` key the parser accepts, read out of config.rs's source.
    fn config_keys() -> Vec<String> {
        let src = include_str!("config.rs");
        let load = src
            .split("pub fn load_line")
            .nth(1)
            .and_then(|s| s.split("pub fn save_lines").next())
            .expect("load_line");
        let mut out = Vec::new();
        for line in load.lines() {
            let t = line.trim();
            let Some(rest) = t.strip_prefix('"') else { continue };
            let Some((k, tail)) = rest.split_once('"') else { continue };
            if tail.trim_start().starts_with("=>") {
                out.push(k.to_string());
            }
        }
        out
    }

    #[test]
    fn every_config_key_is_accounted_for_exactly_once() {
        // The ledger covered eleven of sixty-five and its doc comment said
        // "every number I guessed". This is the guard that stops that being
        // true again: a new key must be a knob, or declared RL-calibrated, or
        // declared not-a-feel-knob WITH A REASON. Silence is not one of the
        // options.
        let listed: BTreeSet<&str> = KNOBS
            .iter()
            .map(|k| {
                let Set::Cfg(c) = k.set;
                c
            })
            .collect();
        let rl: BTreeSet<&str> = RL_CALIBRATED.iter().copied().collect();
        let not: BTreeSet<&str> = NOT_A_FEEL_KNOB.iter().map(|(k, _)| *k).collect();

        for k in config_keys() {
            let n = [listed.contains(k.as_str()), rl.contains(k.as_str()), not.contains(k.as_str())]
                .iter()
                .filter(|b| **b)
                .count();
            assert_eq!(
                n, 1,
                "cfg key '{k}' is in {n} categories; it must be exactly one of \
                 KNOBS, RL_CALIBRATED or NOT_A_FEEL_KNOB"
            );
        }

        // and nothing is declared that does not exist — a stale exemption is
        // how a category quietly stops meaning anything
        let all: BTreeSet<String> = config_keys().into_iter().collect();
        for k in rl.iter().chain(not.iter()) {
            assert!(all.contains(*k), "'{k}' is declared but is not a config key");
        }
    }

    #[test]
    fn the_exemptions_say_why() {
        for (k, reason) in NOT_A_FEEL_KNOB {
            assert!(reason.len() > 5, "'{k}' is exempted with no reason");
        }
        // RL_CALIBRATED needs no per-key reason: the reason is the category,
        // and DESIGN.md carries the conversion table. But it should not be
        // used as a dumping ground for anything this project chose.
        assert!(
            !RL_CALIBRATED.contains(&"story_anchor"),
            "a choice made here was filed as inherited from Rocket League"
        );
    }

    #[test]
    fn ids_and_keys_are_unique() {
        let ids: BTreeSet<_> = KNOBS.iter().map(|k| k.id).collect();
        assert_eq!(ids.len(), KNOBS.len(), "duplicate knob id");
        let keys: BTreeSet<_> = KNOBS.iter().map(|k| { let Set::Cfg(c) = k.set; c }).collect();
        assert_eq!(keys.len(), KNOBS.len(), "two knobs share a config key");
    }

    #[test]
    fn every_knob_says_why() {
        for k in KNOBS {
            assert!(k.why.len() > 40, "{} has no real rationale", k.id);
            assert!(!k.group.is_empty());
        }
    }

    #[test]
    fn the_guesses_are_the_ones_i_actually_could_not_check() {
        // if this list ever empties, either something got verified (good, move
        // it up) or somebody marked a guess as reasoned to make the report
        // look tidier (bad). Either way it should be a deliberate edit.
        let g = unverified();
        assert!(!g.is_empty(), "nothing is a guess, which is not believable");
        // the anchor and the case are the two I most want played
        assert!(g.iter().any(|k| k.id == "story_anchor"));
        assert!(g.iter().any(|k| k.id == "case_cap"));
    }

    #[test]
    fn the_ladder_stays_ordered_however_it_is_tuned() {
        use crate::provenance::Ladder;
        let mut c = Config::default();
        // an operator inverting the rungs must not make standing
        // non-monotonic in the evidence
        c.load_line("arch_possible_at", "5");
        c.load_line("arch_high_at", "2");
        let l = Ladder::from_cfg(&c);
        assert!(l.high_at > l.possible_at, "{l:?}");
        assert!(l.possible_at >= 1);
    }
}
