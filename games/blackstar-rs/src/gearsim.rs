//! GEAR SIM — the sheet, the chest, and the seven bodies.
//!
//!     skydribble --gear                 the bearer's sheet (Destiny-style)
//!     skydribble --gear equip 2         equip bag item 2
//!     skydribble --gear stow kinetic    rail or worn-mod name -> bag
//!     skydribble --gear vault           the ship's chest, drawn as a grid
//!     skydribble --gear deposit 0       bag item -> chest
//!     skydribble --gear withdraw 12     chest slot -> bag
//!     skydribble --gear sort            tidy the chest
//!     skydribble --gear clones [elem]   the clone gallery, or one body
//!
//! Inventory persists immediately — a bag is state, not a plan. The
//! battle sims stay dry-run-by-default; this is the deliberate
//! difference between a menu and a mission.

use crate::chi::Chi;
use crate::clones::{model, spec, CLONES};
use crate::gear::Gear;
use crate::save;
use crate::vault::Vault;
use crate::weapons::ELEMENTS;

pub fn run(argv: &[String]) {
    let i = argv.iter().position(|a| a == "--gear").unwrap();
    let cmd = argv.get(i + 1).map(|s| s.as_str()).filter(|s| !s.starts_with("--"));
    let arg = argv.get(i + 2).map(|s| s.as_str()).filter(|s| !s.starts_with("--"));

    let (mut sv, b, cfg) = save::load();
    let chi = Chi::from_line(&sv.chi);
    let mut gear = Gear::from_save(&sv.armed, &sv.worn, &sv.bag);
    let mut vault = Vault::from_line(&sv.vault);
    let mut dirty = false;

    match (cmd, arg) {
        (None, _) => print!("{}", gear.sheet(&chi)),
        (Some("equip"), Some(n)) => match n.parse::<usize>().map_err(|_| format!("'{n}' is not a bag index")).and_then(|n| gear.equip(n, &chi)) {
            Ok(msg) => {
                println!("{msg}");
                dirty = true;
            }
            Err(e) => println!("{e}"),
        },
        (Some("stow"), Some(what)) => match gear.stow(what) {
            Ok(msg) => {
                println!("{msg}");
                dirty = true;
            }
            Err(e) => println!("{e}"),
        },
        (Some("vault"), _) => print!("{}", vault.grid()),
        (Some("deposit"), Some(n)) => match n.parse::<usize>().map_err(|_| format!("'{n}' is not a bag index")).and_then(|n| vault.deposit(&mut gear, n)) {
            Ok(msg) => {
                println!("{msg}");
                dirty = true;
            }
            Err(e) => println!("{e}"),
        },
        (Some("withdraw"), Some(n)) => match n.parse::<usize>().map_err(|_| format!("'{n}' is not a vault slot")).and_then(|n| vault.withdraw(&mut gear, n)) {
            Ok(msg) => {
                println!("{msg}");
                dirty = true;
            }
            Err(e) => println!("{e}"),
        },
        (Some("sort"), _) => {
            vault.sort();
            println!("the chest is tidied: weapons lead, armor follows");
            dirty = true;
        }
        (Some("clones"), pick) => gallery(pick, &chi),
        (Some(other), _) => println!("no such gear command: {other}"),
    }

    if dirty {
        let (armed, worn, bag) = gear.save_lines();
        sv.armed = armed;
        sv.worn = worn;
        sv.bag = bag;
        sv.vault = vault.line();
        save::store(&sv, &b, &cfg);
    }
}

/// The seven bodies: a line each, or one drawn front-on. The renderer
/// gets the same clouds; this is the inspection loop.
fn gallery(pick: Option<&str>, chi: &Chi) {
    match pick.and_then(|p| ELEMENTS.iter().find(|e| e.name() == p)) {
        Some(e) => {
            let s = spec(*e).unwrap();
            let pts = model(*e);
            println!(
                "{} -- the {} clone. {:.1}m, {} points.",
                s.name,
                e.name(),
                s.height_m,
                pts.len()
            );
            if *e == crate::weapons::Element::Ice {
                println!(
                    "the bearer's own form -- shift is {}",
                    if chi.can_shift() { "EARNED (ice level 3)" } else { "not yet earned (ice level 3 required)" }
                );
                println!("reference: blackstar/catalog/representatives/imported_ice_clone_body_reference.svxr");
            }
            let prof = crate::clones::profile(&pts);
            let d = crate::clones::distance(&prof, &crate::clones::ICE_REF);
            println!(
                "profile fracs: {:?}  widths: {:?}  (distance to ice ref {:.3})",
                prof.iter().map(|b| (b.frac * 1000.0).round() / 1000.0).collect::<Vec<_>>(),
                prof.iter().map(|b| b.w.round()).collect::<Vec<_>>(),
                d
            );
            print!("{}", ascii_front(&pts, s.height_m));
        }
        None => {
            println!("THE SEVEN -- one skeleton, seven natures. `--gear clones <element>` to see one.\n");
            for c in &CLONES {
                let n = model(c.element).len();
                println!(
                    "  {:12} {:8} {:.1}m  horns {}  {}",
                    c.name,
                    c.element.name(),
                    c.height_m,
                    c.horns.0,
                    nature_line(c.element)
                );
                let _ = n;
            }
        }
    }
}

fn nature_line(e: crate::weapons::Element) -> &'static str {
    use crate::weapons::Element as E;
    match e {
        E::Ice => "blade-thin, twin-horned; the reference body",
        E::Fire => "flared low, crowned in plumes",
        E::Air => "a lantern of winglets, tallest of the seven",
        E::Electric => "antennaed past its own head",
        E::Vine => "slumped asymmetric, one shoulder dragging tendrils",
        E::Rock => "the squat heavy cousin, plated",
        E::Water => "smooth taper, the only hornless one",
        E::None => "",
    }
}

/// Project the cloud onto x/y and draw it, the same view the reference
/// was verified with.
fn ascii_front(pts: &[glam::Vec3], height_m: f32) -> String {
    let rows = 30usize;
    let cols = 26usize;
    let (mut xmin, mut xmax) = (f32::MAX, f32::MIN);
    for p in pts {
        xmin = xmin.min(p.x);
        xmax = xmax.max(p.x);
    }
    let w = (xmax - xmin).max(1e-3);
    let mut grid = vec![vec![false; cols]; rows];
    for p in pts {
        let c = (((p.x - xmin) / w) * (cols - 1) as f32) as usize;
        let r = ((p.y / height_m).clamp(0.0, 1.0) * (rows - 1) as f32) as usize;
        grid[rows - 1 - r][c.min(cols - 1)] = true;
    }
    let mut out = String::new();
    for row in grid {
        out.push_str(&row.iter().map(|b| if *b { '#' } else { '.' }).collect::<String>());
        out.push('\n');
    }
    out
}
