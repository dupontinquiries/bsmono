//! MENU SIM — the consoles and their motion, without a window.
//!
//!     skydribble --menu                      walk the stations (list)
//!     skydribble --menu armory               open one console, watch it deploy
//!     skydribble --menu armory --tape d,d,a  then play an input tape
//!
//! ## The tape wraps, and I blamed it twice for that
//!
//! `d` down, `u` up, `a` activate. Selection WRAPS at the ends, and a console
//! with two selectable rows therefore returns to the first on the second `d`.
//! I ran `d,d,a` on the bridge expecting the sortie, got the nightfall row it
//! had wrapped back to, and concluded the harness could not reach arbitrary
//! rows — twice, in two different sessions, working around it both times.
//!
//! It reaches them fine. `d,a` is the sortie. Counting the SELECTABLE rows —
//! headings, gaps and disabled rows are skipped — is the whole trick, and
//! `--menu <station>` with no tape prints them in order so the count is
//! there to read.
//!     skydribble --menu motion               the fx kit's dials, demonstrated
//!
//! Every animation in `menufx.rs` is a pure function of time, which
//! means it can be photographed in text: this harness ticks the same
//! clocks the shell will and prints frames — row offsets as indent,
//! alpha as a character ramp, the cursor as a living bar, the hold as a
//! filling ring. If the cascade stutters here it will stutter in glass,
//! and if it lands here it will land there, because there is exactly
//! one implementation to disagree with itself.
//!
//! Tape vocabulary: `d`/`u` move, `a` activate, `b` back, `h` hold the
//! selection (runs the ring to confirmation), `w` wait 0.25s.

use crate::menu::{Event, Host, Input};
use crate::menufx::{DeployStyle, Hold, Motion, Pending, Stage, HOLD_SEC};
use crate::nightfall;
use crate::ship::Phase;
use crate::stations::{apply, build, run_order, Order, Shipboard, Station, STATIONS};

pub fn run(argv: &[String]) {
    let i = argv.iter().position(|a| a == "--menu").unwrap();
    let pick = argv.get(i + 1).map(|s| s.as_str()).filter(|s| !s.starts_with("--"));
    let tape = argv
        .iter()
        .position(|a| a == "--tape")
        .and_then(|p| argv.get(p + 1))
        .map(|s| s.as_str())
        .unwrap_or("");

    match pick {
        None => walk(),
        Some("motion") => motion_demo(),
        Some(id) => match STATIONS.iter().find(|s| s.id() == id) {
            Some(st) => console(*st, tape),
            None => println!("no such station: {id}  (bridge, armory, hold, medbay, wardeck, or `motion`)"),
        },
    }
}

fn walk() {
    println!("THE SHIP'S STATIONS -- `--menu <id>` to stand at one.\n");
    for st in STATIONS {
        let ok = st.available(Phase::Cruise).is_ok();
        println!("  {:10} {}  {}", st.id(), st.title(), if ok { "" } else { "(dark)" });
        println!("           {}", st.room());
        let s = st.style();
        println!(
            "           deploys: {:.0}ms/row, {:.0}ms stagger, throw {:.1}, unfold {:.0}%",
            s.per_row * 1000.0,
            s.stagger * 1000.0,
            s.slide,
            s.unfold * 100.0
        );
    }
}

/// Open one console and photograph its deploy; then run the tape.
fn console(st: Station, tape: &str) {
    let mut sb = Shipboard::load();
    let week = nightfall::week_now();
    // through the same sizing the Deck applies, or this driver reports on a
    // scroll window the game does not use — which it did, showing twelve
    // rows of a panel that holds twenty
    let menu = crate::deck::fitted(build(st, &sb, week));
    let rows = menu.rows.len();
    let mut host = Host::new();
    host.open(menu);
    let mut motion = Motion::shut();
    motion.open(st.style(), rows, host.top().unwrap().sel());

    println!("{} -- {}", st.title(), st.room());
    println!("(deploying: {} rows, {} style)\n", rows, style_name(st.style()));
    // photograph the cascade: six frames from open to seated
    let total = st.style().total(rows);
    let mut shot = 0;
    let mut t = 0.0;
    while motion.stage != Stage::Steady {
        motion.tick(1.0 / 60.0);
        t += 1.0 / 60.0;
        if t >= total * (shot as f32 + 1.0) / 6.0 {
            println!("t={:.2}s", t);
            print!("{}", frame(&host, &motion, None));
            shot += 1;
        }
    }
    println!("t={t:.2}s  (seated)");
    print!("{}", frame(&host, &motion, None));

    // the tape
    for c in tape.chars().filter(|c| !c.is_whitespace() && *c != ',') {
        match c {
            'd' | 'u' => {
                let d = if c == 'd' { 1 } else { -1 };
                host.input(Input::Move(d));
                motion.cursor.aim(host.top().map(|m| m.sel()).unwrap_or(0));
                println!("\n[move {}]", if d > 0 { "down" } else { "up" });
                // photograph the spring mid-flight and settled
                for step in 0..2 {
                    for _ in 0..4 {
                        motion.tick(1.0 / 60.0);
                    }
                    let _ = step;
                    print!("{}", frame(&host, &motion, None));
                }
                while !motion.cursor.settled() {
                    motion.tick(1.0 / 60.0);
                }
            }
            'a' => {
                let sel_id = host.top().and_then(|m| m.selected()).map(|r| r.id).unwrap_or("");
                println!("\n[activate {sel_id}]");
                if let Some(ev) = host.input(Input::Activate) {
                    narrate(st, &ev, &mut sb, week);
                    // state changed: rebuild the console in place
                    if matches!(ev, Event::Pressed(_)) {
                        let m = crate::deck::fitted(build(st, &sb, week));
                        let n = m.rows.len();
                        host = Host::new();
                        host.open(m);
                        motion.open(st.style(), n, host.top().unwrap().sel());
                        while motion.stage != Stage::Steady {
                            motion.tick(1.0 / 60.0);
                        }
                        print!("{}", frame(&host, &motion, None));
                    }
                }
            }
            'h' => {
                let sel_id = host.top().and_then(|m| m.selected()).map(|r| r.id).unwrap_or("");
                println!("\n[hold {sel_id}]");
                let mut hold = Hold::default();
                hold.press();
                let mut t = 0.0;
                let mut confirmed = false;
                while t < HOLD_SEC + 0.05 {
                    if hold.tick(1.0 / 30.0) {
                        confirmed = true;
                    }
                    t += 1.0 / 30.0;
                    if (t * 6.0) as u32 != ((t - 1.0 / 30.0) * 6.0) as u32 {
                        println!("  {}", ring(hold.ring));
                    }
                }
                if confirmed {
                    println!("  CONFIRMED");
                    if let Some(ev) = host.input(Input::Activate) {
                        narrate(st, &ev, &mut sb, week);
                    }
                }
            }
            'b' => {
                println!("\n[back] -- the console folds");
                motion.close_then(Pending::Dismiss);
                let total = motion.deploy.style.total(motion.rows());
                let mut t = 0.0;
                let mut shot = 0;
                loop {
                    let fired = motion.tick(1.0 / 60.0);
                    t += 1.0 / 60.0;
                    if t >= total * (shot as f32 + 1.0) / 4.0 && motion.stage == Stage::Closing {
                        println!("t={t:.2}s");
                        print!("{}", frame(&host, &motion, None));
                        shot += 1;
                    }
                    if fired.is_some() {
                        println!("(shut -- the room is a room again)");
                        return;
                    }
                }
            }
            'w' => {
                for _ in 0..15 {
                    motion.tick(1.0 / 60.0);
                }
            }
            other => println!("(tape: unknown '{other}')"),
        }
    }
}

fn narrate(st: Station, ev: &Event, sb: &mut Shipboard, week: u64) {
    match apply(st, ev, sb, week) {
        Some(Order::Did(m)) => println!("  > {m}"),
        Some(Order::Refused(m)) => println!("  > REFUSED: {m}"),
        // Run it, rather than print a second copy of the shell's disclaimer.
        // These two arms used to say "the shell flies this" — which meant the
        // one path with no headless driver was the one the shell had just
        // learned to execute, so nothing could exercise it. Same function the
        // winit handler calls; if it breaks, it breaks here first.
        // Depart cannot run here — `load_map` needs a gpu — so the harness
        // reports what the shell WOULD do, including which map style the
        // destination resolves to. That is the checkable half, and the note
        // above about untested shell paths is why it is not left silent.
        Some(Order::Depart { dest, line }) => {
            println!("  > {line}");
            match crate::patrol::ROUTE.iter().find(|d| d.id == dest) {
                Some(d) => println!(
                    "  > (the shell would load style {} — {})",
                    crate::mapsim::style_for(d),
                    crate::world::style(crate::mapsim::style_for(d)).name
                ),
                None => println!("  > (no such destination)"),
            }
        }
        Some(o @ (Order::Sortie(_) | Order::Nightfall(_))) => {
            let (line, changed) = run_order(&o);
            println!("  > {line}");
            if changed {
                println!("  > (the save moved; reopen the console to see it)");
            }
        }
        // The shell-only orders, reported like Depart: a raid is a run of
        // maps and the forge is a worker thread, so the harness prints the
        // CHECKABLE half — what the shell would fly — rather than a
        // disclaimer or a network call in a tape-driven loop.
        Some(Order::Raid(boss)) => match crate::raidcon::BOSSES.iter().find(|b| b.id == boss) {
            Some(b) => {
                let plan = crate::raidgen::plan_for(b, 3, crate::raidgen::strhash(b.id));
                println!("  > the shell would fly {} — {} phases:", b.name, plan.phases.len());
                for (i, ph) in plan.phases.iter().enumerate() {
                    println!("  >   {} — {} ({} enc)", i + 1, ph.arena.name, ph.enc.len());
                }
            }
            None => println!("  > (no such warden)"),
        },
        Some(Order::Echo(id)) => match crate::raidforge::find(id) {
            Some(e) => println!(
                "  > the shell would fly the echo '{}' — {} phases, resembling {}",
                e.name,
                e.phases.len(),
                e.resembles.name
            ),
            None => println!("  > (that echo has faded from the file)"),
        },
        Some(Order::Forge(theme)) => {
            println!("  > the shell would light the forge on a worker thread — theme: {theme}");
        }
        Some(Order::Goto(s)) => println!("  > walking to {}", s.id()),
        None => match ev {
            Event::Open(id) => println!("  > opens {id}"),
            Event::Changed(id, v) => println!("  > {id} = {v:?}"),
            Event::Close => println!("  > closed"),
            _ => {}
        },
    }
}

/// One frame of the console as text. Offset becomes indent, alpha
/// becomes a ramp, the cursor is a `>` bar with squash arrows, an
/// active hold draws its ring beside the selection.
fn frame(host: &Host, motion: &Motion, hold: Option<&Hold>) -> String {
    let Some(menu) = host.top() else { return String::new() };
    let mut out = String::new();
    let cursor_row = motion.cursor.y.round() as i64;
    for (i, row) in menu.visible() {
        let pose = motion.row(i);
        if pose.alpha <= 0.01 {
            out.push('\n');
            continue;
        }
        let indent = " ".repeat((pose.offset.max(0.0) * 4.0) as usize);
        let ink = ramp(pose.alpha);
        let bar = if i as i64 == cursor_row {
            match (motion.cursor.squash() * 10.0) as u32 {
                0 => "> ",
                1..=2 => ">> ",
                _ => ">>> ",
            }
        } else {
            "  "
        };
        let label = if row.label.is_empty() { "-".repeat(3) } else { row.label.clone() };
        let value = row.item.value();
        let unfold_mark = if pose.unfold < 0.95 { "=" } else { " " };
        out.push_str(&format!(
            "{bar}{indent}{ink}{unfold_mark}{} {}\n",
            truncate(&label, LABEL_W),
            value
        ));
        if let (Some(h), true) = (hold, i as i64 == cursor_row) {
            out.push_str(&format!("     {}\n", ring(h.ring)));
        }
    }
    out
}

fn ring(x: f32) -> String {
    let n = (x * 12.0) as usize;
    format!("[{}{}] {:.0}%", "#".repeat(n), " ".repeat(12 - n), x * 100.0)
}

fn ramp(a: f32) -> &'static str {
    match (a * 4.0) as u32 {
        0 => ". ",
        1 => ": ",
        2 => "+ ",
        _ => "",
    }
}

/// The console's usable label width, in characters.
///
/// Was a bare `46` at the one call site. Named because it is a real
/// constraint that station authors have to respect and could not see: the
/// Bridge shipped "— teaches tra~" and "the open ~" because nothing told
/// anyone the budget existed. `stations::tests` asserts every row fits.
pub const LABEL_W: usize = 46;

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        s.chars().take(n - 1).chain(std::iter::once('~')).collect()
    }
}

fn style_name(s: DeployStyle) -> &'static str {
    if s == DeployStyle::rack() {
        "rack"
    } else if s == DeployStyle::sweep() {
        "sweep"
    } else if s == DeployStyle::stack() {
        "stack"
    } else {
        "glide"
    }
}

/// The dials, demonstrated one at a time: what each style's cascade
/// looks like on the same six rows, the spring's flight, the ring.
fn motion_demo() {
    println!("THE FX KIT, photographed.\n");
    for (name, style) in [
        ("glide", DeployStyle::glide()),
        ("rack", DeployStyle::rack()),
        ("sweep", DeployStyle::sweep()),
        ("stack", DeployStyle::stack()),
    ] {
        println!("== {name}: 6 rows, frames at 25/50/75% of {:.2}s", style.total(6));
        let mut d = crate::menufx::Deploy::enter(style);
        let total = style.total(6);
        for pct in [0.25, 0.5, 0.75] {
            // photograph at exact fractions: deterministic frames
            let mut fresh = crate::menufx::Deploy::enter(style);
            fresh.tick(total * pct);
            let _ = &mut d;
            println!("  t={:.2}s", total * pct);
            for i in 0..6 {
                let p = fresh.row(i, 6);
                let indent = " ".repeat((p.offset.max(0.0) * 4.0) as usize);
                println!("    {indent}{}row {i}", ramp(p.alpha));
            }
        }
        println!();
    }
    println!("== the cursor spring: aim row 0 -> 4, sampled every 50ms");
    let mut c = crate::menufx::Cursor::at(0);
    c.aim(4);
    for step in 0..10 {
        c.tick(0.05);
        let y = (c.y * 4.0).round().max(0.0) as usize;
        println!("  {:4.0}ms |{}{}|  y={:.2} squash={:.2}", (step + 1) as f32 * 50.0, " ".repeat(y), "#", c.y, c.squash());
        if c.settled() {
            break;
        }
    }
    println!("\n== the hold ring at 30hz, released once at 60%");
    let mut h = Hold::default();
    h.press();
    let mut t = 0.0;
    let mut released = false;
    while t < HOLD_SEC * 2.0 {
        let fired = h.tick(1.0 / 30.0);
        t += 1.0 / 30.0;
        if !released && h.ring > 0.6 {
            h.release();
            released = true;
            println!("  {}  (released -- grace decay)", ring(h.ring));
        }
        if released && h.ring < 0.35 && !fired {
            h.press();
            released = false;
            println!("  {}  (pressed again)", ring(h.ring));
        }
        if fired {
            println!("  {}  CONFIRMED", ring(h.ring));
            break;
        }
    }
}
