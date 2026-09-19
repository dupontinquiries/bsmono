//! A thin shell: argument handling, printing, and one loop that feeds backend
//! events and stdin lines to the session. All behaviour lives in stoat-core.

use std::io::{BufRead, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;
use stoat_core::backend::*;
use stoat_core::gdb::{GdbBackend, GdbConfig};
use stoat_core::model::RunState;
use stoat_core::session::{Change, Session};
use stoat_core::vitals::{self, Sampler, Vitals};
use stoat_core::{build, cache, discover, history};

const USAGE: &str = "\
stoat pick [DIR]                       rank debuggable binaries under DIR, with reasons
stoat hist QUERY...                    search shell history (all terms must match)
stoat build [DIR]                      show the build command, run it, list executables
stoat cache [DIR] [--prune DAYS [--apply]]
                                       measure debug caches; plan (and apply) a prune
stoat run [opts] [BINARY [ARGS...]]    debug; with no BINARY, the top pick and its last
                                       arguments from shell history
    --build          build first and prefer what the build produced
    --marker LOC     frame-marker breakpoint for pacing (e.g. a present/swap function)
    --batch          run to the first stop, report, exit
    --wire           show every MI line exchanged with gdb
";

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let code = match args.first().map(String::as_str) {
        Some("pick") => pick(&dir_arg(args.get(1))),
        Some("hist") => hist(&args[1..].join(" ")),
        Some("build") => run_build(&dir_arg(args.get(1))).map_or(1, |o| i32::from(!o.success)),
        Some("cache") => cache_cmd(&args[1..]),
        Some("run") => {
            args.remove(0);
            run(args)
        }
        _ => {
            eprint!("{USAGE}");
            2
        }
    };
    std::process::exit(code);
}

fn dir_arg(a: Option<&String>) -> PathBuf {
    let p = a.map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
    p.canonicalize().unwrap_or(p)
}

fn print_candidates(c: &[discover::Candidate]) {
    for (i, c) in c.iter().enumerate().take(8) {
        println!("{:>2}. {}  [{}]", i + 1, c.path.display(), c.score());
        for r in &c.reasons {
            println!("      {:+4}  {}", r.weight(), r.describe());
        }
        if let Some(inv) = c.invocations.first() {
            println!("      last run as: {}", inv.original);
        }
    }
}

fn pick(root: &Path) -> i32 {
    let c = discover::scan(root, &history::load_default(), &[]);
    if c.is_empty() {
        println!("no runnable ELF found under {} (looked in the root and the usual build directories)", root.display());
        return 1;
    }
    print_candidates(&c);
    0
}

fn hist(query: &str) -> i32 {
    let entries = history::load_default();
    let hits = history::grep(&entries, query);
    for e in hits.iter().rev().take(40).rev() {
        println!("{}", e.cmd);
    }
    i32::from(hits.is_empty())
}

fn run_build(root: &Path) -> Option<build::BuildOutcome> {
    let Some(plan) = build::plan(root) else {
        println!("no recognised build system in {} (Cargo.toml, a configured meson/cmake build dir, or a Makefile)", root.display());
        return None;
    };
    println!("$ {}   (in {})", plan.command_line(), plan.cwd.display());
    let out = build::run(&plan, |l| match l {
        build::BuildLine::Out(s) => println!("{s}"),
        build::BuildLine::Err(s) => eprintln!("{s}"),
    });
    match out {
        Ok(o) => {
            for e in &o.executables {
                println!("built: {}", e.display());
            }
            Some(o)
        }
        Err(e) => {
            println!("could not start `{}`: {e}", plan.program);
            None
        }
    }
}

fn cache_cmd(args: &[String]) -> i32 {
    let prune_days = args.iter().position(|a| a == "--prune").and_then(|i| args.get(i + 1)).and_then(|d| d.parse::<u64>().ok());
    let apply = args.iter().any(|a| a == "--apply");
    let root = dir_arg(args.iter().find(|a| !a.starts_with("--") && a.parse::<u64>().is_err()));
    for (kind, path) in cache::locate(Some(&root)) {
        let u = cache::measure(&path);
        println!("{:<22} {:>10}  {:>7} files  {}{}", kind.label(), cache::human_bytes(u.bytes), u.files, path.display(), if u.unreadable { "  (not readable)" } else { "" });
        if let Some(days) = prune_days {
            let plan = cache::plan_prune(&path, Duration::from_secs(days * 86_400));
            println!("    unused for {days}d: {} files, {}", plan.remove.len(), cache::human_bytes(plan.bytes));
            if apply && !plan.remove.is_empty() {
                let (freed, failed) = cache::apply(&plan);
                println!("    freed {}, {} failures", cache::human_bytes(freed), failed.len());
            }
        }
    }
    if prune_days.is_some() && !apply {
        println!("nothing deleted; add --apply to do it");
    }
    0
}

enum Input {
    Backend(Event),
    Line(String),
    Eof,
}

fn run(mut args: Vec<String>) -> i32 {
    let mut flag = |name: &str| args.iter().position(|a| a == name).map(|i| args.remove(i)).is_some();
    let (want_build, batch, mut wire) = (flag("--build"), flag("--batch"), flag("--wire"));
    let marker = args.iter().position(|a| a == "--marker").map(|i| {
        args.remove(i);
        args.remove(i)
    });
    let root = dir_arg(None);

    let mut artifacts = Vec::new();
    if want_build {
        match run_build(&root) {
            Some(o) if o.success => artifacts = o.executables,
            _ => return 1,
        }
    }
    let (program, prog_args, env, is_rust) = if args.is_empty() {
        let c = discover::scan(&root, &history::load_default(), &artifacts);
        let Some(top) = c.first() else {
            println!("nothing to debug here; try `stoat pick`, or pass a binary");
            return 1;
        };
        print_candidates(&c[..1]);
        let (a, env) = top.invocations.first().map(|i| (i.args.clone(), i.env.clone())).unwrap_or_default();
        if !a.is_empty() || !env.is_empty() {
            println!("      using those arguments and environment; pass the binary explicitly to override");
        }
        (top.path.clone(), a, env, top.is_rust)
    } else {
        let p = PathBuf::from(args.remove(0));
        let is_rust = discover::inspect(&p).is_some_and(|f| f.is_rust);
        (p, args, Vec::new(), is_rust)
    };

    let (mut gdb, events) = match GdbBackend::spawn(&GdbConfig::default()) {
        Ok(x) => x,
        Err(e) => {
            println!("could not start gdb: {e}");
            return 1;
        }
    };
    let mut session = Session::new(Some(root));
    let vit = Vitals::shared(1200, 1200);
    let mut sampler = None;

    gdb.submit(Request::Load { program, args: prog_args, env, cwd: None });
    if is_rust {
        session.break_on_rust_panic(&mut gdb);
    }
    if let Some(loc) = &marker {
        session.insert_frame_marker(loc, &mut gdb);
    }
    gdb.submit(Request::Run);

    let (tx, rx) = mpsc::channel();
    std::thread::spawn({
        let tx = tx.clone();
        move || {
            while let Ok(ev) = events.recv_blocking() {
                if tx.send(Input::Backend(ev)).is_err() {
                    break;
                }
            }
        }
    });
    if !batch {
        std::thread::spawn(move || {
            for l in std::io::stdin().lock().lines().map_while(Result::ok) {
                if tx.send(Input::Line(l)).is_err() {
                    return;
                }
            }
            let _ = tx.send(Input::Eof);
        });
    }

    let interactive = !batch && std::io::stdin().is_terminal();
    let prompt = |s: &Session| {
        if interactive && s.state != RunState::Running {
            print!("(stoat) ");
            let _ = std::io::stdout().flush();
        }
    };
    let mut reported = false;
    // gdb's console stream is mostly stop chatter the diagnostic already covers;
    // show it only while the user is talking to gdb directly with `!`.
    let mut show_console = false;
    for input in rx {
        let changes = match input {
            Input::Eof => break,
            Input::Backend(ev) => session.handle(ev, &mut gdb),
            Input::Line(l) => {
                show_console = l.trim_start().starts_with('!');
                match command(&l, &mut session, &mut gdb, &vit, &mut wire) {
                Some(c) => {
                    if c.is_empty() {
                        prompt(&session);
                    }
                    c
                }
                None => break,
            }}
        };
        for c in changes {
            match c {
                Change::Started { pid: Some(pid) } => {
                    vit.lock().unwrap().reset();
                    sampler = Some(Sampler::start(pid, Duration::from_millis(100), vit.clone()));
                }
                Change::State(RunState::Stopped) => {
                    if let Some(pid) = session.pid {
                        vitals::sample_now(pid, &vit);
                    }
                    vit.lock().unwrap().set_frozen(true);
                }
                Change::State(RunState::Running) => vit.lock().unwrap().set_frozen(false),
                Change::FrameMark(at) => vit.lock().unwrap().mark_frame(at),
                Change::Output { channel: Channel::Target, text } => print!("{text}"),
                Change::Output { channel: Channel::Console, text } if show_console => print!("{text}"),
                Change::Output { channel: Channel::Log, text } if wire => eprint!("{text}"),
                Change::Wire { dir, line } if wire => eprintln!("{} {line}", if dir == Dir::ToBackend { "->" } else { "<-" }),
                Change::Diagnostic(d) => {
                    print!("{}", d.render(2));
                    if session.state != RunState::Running {
                        for n in vitals::summarize(&vit.lock().unwrap()) {
                            println!("  = note: {n}");
                        }
                    }
                    reported = true;
                }
                Change::Selection(_) if reported => print_frame(&session),
                Change::Locals { frame } => print_locals(&session, frame),
                Change::Value { expr, value } => println!("{expr} = {value}"),
                Change::Gone(why) => {
                    println!("{why}");
                    return 0;
                }
                _ => {}
            }
        }
        let _ = std::io::stdout().flush();
        if batch && reported && (session.state == RunState::Exited || session.current_frame().is_some_and(|f| session.locals.contains_key(&f.level))) {
            print_backtrace(&session);
            break;
        }
        if reported && session.state != RunState::Running {
            prompt(&session);
        }
    }
    drop(sampler);
    0
}

fn print_frame(s: &Session) {
    if let Some(f) = s.current_frame() {
        println!("#{} {} at {}", f.level, f.func.as_deref().unwrap_or("??"), f.location());
    }
}

fn print_locals(s: &Session, frame: u32) {
    for v in s.locals.get(&frame).into_iter().flatten() {
        let val = v.value.clone().unwrap_or_else(|| format!("<{}>  (p {} to expand)", v.ty.as_deref().unwrap_or("?"), v.name));
        println!("    {}{} = {val}", if v.is_arg { "arg " } else { "" }, v.name);
    }
}

fn print_backtrace(s: &Session) {
    for (i, f) in s.frames.iter().enumerate() {
        println!("{} #{:<2} {} at {}", if i == s.selected { ">" } else { " " }, f.level, f.func.as_deref().unwrap_or("??"), f.location());
    }
}

/// None means quit.
fn command(line: &str, s: &mut Session, b: &mut GdbBackend, vit: &vitals::Shared, wire: &mut bool) -> Option<Vec<Change>> {
    let (cmd, rest) = line.trim().split_once(' ').unwrap_or((line.trim(), ""));
    let thread = s.thread;
    let mut out = Vec::new();
    match cmd {
        "" => {}
        "q" | "quit" => return None,
        "c" => drop(b.submit(Request::Continue)),
        "r" => drop(b.submit(Request::Run)),
        "i" => drop(b.submit(Request::Interrupt)),
        "n" => drop(b.submit(Request::Step { kind: StepKind::Over, thread })),
        "s" => drop(b.submit(Request::Step { kind: StepKind::Into, thread })),
        "fin" => drop(b.submit(Request::Step { kind: StepKind::Out, thread })),
        "u" | "up" => out = s.up(b),
        "d" | "down" => out = s.down(b),
        "f" => out = s.select_frame(rest.trim().parse().unwrap_or(s.selected), b),
        "bt" => print_backtrace(s),
        "l" => {
            if let Some(f) = s.current_frame() {
                print_locals(s, f.level);
            }
        }
        "p" => s.evaluate(rest, b),
        "b" => drop(b.submit(Request::InsertBreakpoint { location: rest.into() })),
        "t" => match rest.trim().parse() {
            Ok(id) => s.select_thread(id, b),
            Err(_) => {
                for t in &s.threads {
                    println!("{} {:<3} {:<16} {}", if Some(t.id) == s.thread { ">" } else { " " }, t.id, t.name.as_deref().unwrap_or(""), t.top.as_ref().map(|f| f.location()).unwrap_or_default());
                }
            }
        },
        "vitals" => {
            for l in vitals::summarize(&vit.lock().unwrap()) {
                println!("{l}");
            }
        }
        "wire" => *wire = rest.trim() != "off",
        "in" => drop(b.submit(Request::TargetInput(format!("{rest}\n")))),
        "!" => drop(b.submit(Request::Raw(rest.into()))),
        _ => println!("c r i n s fin | u d f N bt l | p EXPR | b LOC | t [N] | vitals | wire on/off | in TEXT | ! GDB-COMMAND | q"),
    }
    Some(out)
}
