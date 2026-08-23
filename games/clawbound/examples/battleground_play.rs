//! battleground_play — the clan war, playable in a terminal.
//!
//! The real client needs winit/wgpu (`--features app`), which needs a toolchain
//! this crate cannot always assume. That is a bad reason for a sim to be
//! unplayable, and a worse reason for `set_input` to go untested by anything
//! resembling a player. So: a top-down ASCII view, a HUD, and one warrior under
//! your paw.
//!
//! It is also the loop made visible. Everything the sim decides each tick —
//! where the patrols are, which lane broke, who is carrying prey home, whether
//! the Skyrock is yours — is on screen as characters, so a bad decision can be
//! *watched* instead of inferred from a win rate.
//!
//! ```text
//! cargo run --release --example battleground_play                  # interactive
//! cargo run --release --example battleground_play -- puma          # pick your ancestor
//! cargo run --release --example battleground_play -- lynx "dddd1s2" 60   # scripted
//! ```
//!
//! Interactive keys (one line at a time, since a raw-mode TTY is not worth a
//! dependency here): `w a s d` move a step, `q e z c` move diagonally,
//! `1 2 3` cast, `f` claw, `.` wait, `x` quit. Several may be typed on one
//! line — `wwwd1f` walks three north, one east, casts, claws.

use clawbound_rs::ancestor::{self, Ancestor};
use clawbound_rs::battleground::{Battleground, Carry, Mind, Role, CLAN_A};
use std::io::{BufRead, Write};

const W: usize = 78;
const H: usize = 30;
const ARENA: f32 = 150.0;

fn pick(name: &str) -> Ancestor {
    match name {
        "tiger" | "duskstripe" => Ancestor::Tiger,
        "leopard" | "rainspots" => Ancestor::Leopard,
        "panther" | "nightpelt" => Ancestor::Panther,
        "caracal" | "windclaw" => Ancestor::Caracal,
        "puma" | "stonewhisper" => Ancestor::Puma,
        "serval" | "faryowl" => Ancestor::Serval,
        "cavelion" | "oldroar" => Ancestor::CaveLion,
        "smilodon" | "twofang" => Ancestor::Smilodon,
        "suncat" | "suneye" => Ancestor::SunCat,
        "lynx" | "snowhush" => Ancestor::Lynx,
        "jaguar" | "riptail" => Ancestor::Jaguar,
        "suanni" | "stillsmoke" => Ancestor::Suanni,
        _ => Ancestor::Lion,
    }
}

fn cell(x: f32, z: f32) -> Option<(usize, usize)> {
    let u = (x + ARENA) / (2.0 * ARENA);
    let v = (z + ARENA) / (2.0 * ARENA);
    if !(0.0..1.0).contains(&u) || !(0.0..1.0).contains(&v) {
        return None;
    }
    Some(((u * W as f32) as usize, (v * H as f32) as usize))
}

fn draw(bg: &Battleground) -> String {
    let mut grid = vec![vec![' '; W]; H];
    // terrain wash first, so everything else overwrites it
    for row in 0..H {
        for col in 0..W {
            let x = (col as f32 + 0.5) / W as f32 * 2.0 * ARENA - ARENA;
            let z = (row as f32 + 0.5) / H as f32 * 2.0 * ARENA - ARENA;
            let h = clawbound_rs::world::height_at(x, z);
            grid[row][col] = if h < clawbound_rs::world::WATER_Y {
                '~'
            } else if clawbound_rs::junglemap::on_wall(x, z) {
                '#'
            } else if clawbound_rs::junglemap::concealment_at(x, z).is_some() {
                '"'
            } else if h > 4.5 {
                '^'
            } else {
                '.'
            };
        }
    }
    let mut put = |x: f32, z: f32, ch: char| {
        if let Some((c, r)) = cell(x, z) {
            grid[r][c] = ch;
        }
    };
    for p in bg.prey.iter().filter(|p| p.alive()) {
        put(p.x, p.z, if p.rich() { '$' } else { 'v' });
    }
    for hp in bg.herb_patches.iter().filter(|h| h.ready()) {
        put(hp.x, hp.z, '*');
    }
    for z in &bg.zones {
        put(z.x, z.z, if z.mend { '+' } else { '!' });
    }
    put(bg.skyrock.0, bg.skyrock.1, 'K');
    for t in &bg.heal_trees {
        let ch = match (t.team == CLAN_A, t.alive()) {
            (true, true) => 'T',
            (false, true) => 't',
            (_, false) => ',',
        };
        put(t.x, t.z, ch);
    }
    for (i, c) in bg.camps.iter().enumerate() {
        put(c.0, c.1, if i == 0 { 'G' } else { 'g' });
    }
    for w in bg.warriors.iter().filter(|w| w.alive()) {
        let ch = match (w.team == CLAN_A, w.role) {
            (true, Role::Ancestor) => 'A',
            (true, Role::Patrol) => 'a',
            (false, Role::Ancestor) => 'B',
            (false, Role::Patrol) => 'b',
        };
        put(w.x, w.z, ch);
    }
    if bg.warriors[0].alive() {
        put(bg.warriors[0].x, bg.warriors[0].z, '@');
    }
    grid.into_iter().map(|r| r.into_iter().collect::<String>()).collect::<Vec<_>>().join("\n")
}

fn hud(bg: &Battleground) -> String {
    let me = &bg.warriors[0];
    let k = me.ancestor.map(ancestor::kit);
    let cds = |i: usize| {
        if me.cds[i] <= 0.0 {
            "ready".to_string()
        } else {
            format!("{:.0}s", me.cds[i])
        }
    };
    let holder = |h: Option<u8>| match h {
        Some(t) if t == CLAN_A => "ours",
        Some(_) => "theirs",
        None => "open",
    };
    let mut s = String::new();
    s.push_str(&format!(
        "t {:>4.0}s  {}  {}   |  you: {} hp {:.0}/{:.0}  {}{}\n",
        bg.time,
        if bg.is_night() { "night" } else { "day  " },
        if bg.leafbare() { "leaf-bare" } else { "greenleaf" },
        k.map(|k| k.name).unwrap_or("?"),
        me.hp,
        me.max_hp,
        match me.carry {
            Carry::Freshkill(w) => format!("carrying prey ({w:.0}) "),
            Carry::Herb => "carrying a herb ".to_string(),
            Carry::Nothing => String::new(),
        },
        if me.mind == Mind::Dead { "— DOWN" } else { "" },
    ));
    if let Some(k) = k {
        s.push_str(&format!(
            "  1 {} [{}]   2 {} [{}]   3 {} [{}]\n",
            k.abilities[0].name,
            cds(0),
            k.abilities[1].name,
            cds(1),
            k.ult.name,
            cds(2),
        ));
    }
    s.push_str(&format!(
        "  freshkill {:.0}/{:.0}  herbs {}/{}  great tree {:.0}/{:.0}  contention {}  skyrock {}\n",
        bg.clans[0].pile,
        bg.clans[1].pile,
        bg.clans[0].herbs,
        bg.clans[1].herbs,
        bg.clans[0].great_tree_hp.max(0.0),
        bg.clans[1].great_tree_hp.max(0.0),
        holder(bg.contention_holder),
        holder(bg.skyrock_holder),
    ));
    s.push_str("  @ you  A/a ours  B/b theirs  T/t heal trees  G/g great trees  K skyrock\n");
    s.push_str("  v prey  $ rich prey  * herb  ~ water  \" brush  # stone  ^ high  ! scorch  + rain\n");
    s
}

/// One input character -> (move, claw, ability). Shared by both drivers, so the
/// scripted mode exercises exactly the path a human's keystroke takes.
fn key(c: char) -> Option<(f32, f32, bool, Option<usize>)> {
    let m = |x: f32, z: f32| Some((x, z, false, None));
    match c {
        'w' => m(0.0, -1.0),
        's' => m(0.0, 1.0),
        'a' => m(-1.0, 0.0),
        'd' => m(1.0, 0.0),
        'q' => m(-1.0, -1.0),
        'e' => m(1.0, -1.0),
        'z' => m(-1.0, 1.0),
        'c' => m(1.0, 1.0),
        'f' => Some((0.0, 0.0, true, None)),
        '1' => Some((0.0, 0.0, false, Some(0))),
        '2' => Some((0.0, 0.0, false, Some(1))),
        '3' => Some((0.0, 0.0, false, Some(2))),
        '.' => Some((0.0, 0.0, false, None)),
        _ => None,
    }
}

/// Ticks the sim forward, applying `input` on the first tick of the beat. A beat
/// is a fifth of a second: long enough that a typed key does something visible,
/// short enough that the sim stays the same sim the tests exercise.
fn beat(bg: &mut Battleground, input: (f32, f32, bool, Option<usize>)) {
    const TICKS: usize = 6;
    for t in 0..TICKS {
        if t == 0 {
            bg.set_input(input.0, input.1, input.2, input.3);
        } else {
            // hold the movement, drop the one-shot actions
            bg.set_input(input.0, input.1, false, None);
        }
        bg.step(1.0 / 30.0);
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let who = args.get(1).map(|s| s.as_str()).unwrap_or("lion");
    let mut bg = Battleground::new(pick(who));

    // scripted mode: a string of keys, then N beats of watching. Deterministic,
    // so it can be diffed between builds or dropped into CI.
    if let Some(script) = args.get(2) {
        let tail: usize = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(0);
        for c in script.chars() {
            if let Some(i) = key(c) {
                beat(&mut bg, i);
            }
        }
        for _ in 0..tail {
            beat(&mut bg, (0.0, 0.0, false, None));
        }
        println!("{}", draw(&bg));
        print!("{}", hud(&bg));
        println!(
            "after {} keys and {tail} idle beats: outcome {:?}",
            script.chars().filter(|&c| key(c).is_some()).count(),
            bg.outcome
        );
        return;
    }

    let stdin = std::io::stdin();
    let mut lines = stdin.lock().lines();
    loop {
        println!("{}", draw(&bg));
        print!("{}", hud(&bg));
        if let Some(o) = bg.outcome {
            println!(
                "\n{}",
                match o {
                    CLAN_A => "Your clan holds the field.",
                    2 => "Full time, and both Great Trees still stand.",
                    _ => "Their clan holds the field.",
                }
            );
            return;
        }
        print!("> ");
        let _ = std::io::stdout().flush();
        let Some(Ok(line)) = lines.next() else { return };
        if line.contains('x') {
            return;
        }
        let mut acted = false;
        for c in line.chars() {
            if let Some(i) = key(c) {
                beat(&mut bg, i);
                acted = true;
            }
        }
        if !acted {
            beat(&mut bg, (0.0, 0.0, false, None));
        }
    }
}
