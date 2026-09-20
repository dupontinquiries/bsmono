//! End to end against a real gdb. Skips (passes with a message) when gdb or cc is
//! missing, so the suite still runs on a bare device image.

use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, Instant};
use stoat_core::backend::*;
use stoat_core::gdb::{GdbBackend, GdbConfig};
use stoat_core::model::RunState;
use stoat_core::session::{Change, Session};
use stoat_core::vitals::{self, Sampler, Vitals};

fn have(tool: &str) -> bool {
    Command::new(tool).arg("--version").output().is_ok_and(|o| o.status.success())
}

fn build_fixture() -> Option<PathBuf> {
    let src = PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/crash.c"));
    let out = std::env::temp_dir().join(format!("stoat-live-{}", std::process::id()));
    std::fs::create_dir_all(&out).ok()?;
    let bin = out.join("crash");
    Command::new("cc").args(["-g", "-O0", "-o"]).arg(&bin).arg(&src).status().ok()?.success().then_some(bin)
}

#[test]
fn crash_with_vitals_and_frame_markers() {
    if !have("gdb") || !have("cc") {
        eprintln!("skipped: needs gdb and cc");
        return;
    }
    let bin = build_fixture().expect("fixture builds");
    let (mut gdb, events) = GdbBackend::spawn(&GdbConfig { rust_sysroot: None, ..Default::default() }).expect("gdb starts");
    let mut session = Session::new(Some(PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures"))));
    let vit = Vitals::shared(600, 600);
    let mut sampler = None;

    gdb.submit(Request::Load { program: bin, args: vec!["hello world".into()], env: vec![("STOAT_T".into(), "a b".into())], cwd: None });
    session.insert_frame_marker("present_frame", &mut gdb);
    gdb.submit(Request::Run);

    let mut diags = Vec::new();
    let mut target_out = String::new();
    let mut marks = 0;
    let mut got_locals = false;
    let deadline = Instant::now() + Duration::from_secs(60);
    while Instant::now() < deadline && !(got_locals && !diags.is_empty()) {
        let Ok(ev) = events.try_recv() else {
            std::thread::sleep(Duration::from_millis(2));
            continue;
        };
        for c in session.handle(ev, &mut gdb) {
            match c {
                Change::Started { pid: Some(pid) } => sampler = Some(Sampler::start(pid, Duration::from_millis(20), vit.clone())),
                Change::State(RunState::Stopped) => vit.lock().unwrap().set_frozen(true),
                Change::FrameMark(at) => {
                    marks += 1;
                    vit.lock().unwrap().mark_frame(at);
                }
                Change::Output { channel: Channel::Target, text } => target_out += &text,
                Change::Diagnostic(d) => diags.push(d),
                Change::Locals { .. } => got_locals = true,
                _ => {}
            }
        }
    }
    drop(sampler);

    assert_eq!(session.state, RunState::Stopped);
    assert_eq!(marks, 30, "one mark per present_frame call, none surfaced as stops");
    assert_eq!(target_out, "argc=2 arg1=hello world env=a b\n", "via the pty, no CR, args and env survive quoting");

    let d = diags.iter().find(|d| d.code == "SIGSEGV").expect("crash diagnostic");
    let text = d.render(1);
    println!("{text}");
    assert!(d.title.contains("depth3"));
    assert!(d.at.as_ref().is_some_and(|(f, l)| f.ends_with("crash.c") && *l == 10));
    assert!(d.notes.iter().any(|n| n.contains("null page") && n.contains("offset 8")), "{:?}", d.notes);
    assert!(text.contains(">") && text.contains("n->next->next"));

    assert_eq!(session.fault_addr, Some(8));
    assert_eq!(session.frames.len(), 4);
    assert_eq!(session.selected, 0);
    assert!(session.locals[&0].iter().any(|v| v.name == "n" && v.is_arg));

    // Navigate toward the caller; locals for that frame arrive on demand.
    session.up(&mut gdb);
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline && !session.locals.contains_key(&1) {
        if let Ok(ev) = events.try_recv() {
            session.handle(ev, &mut gdb);
        }
    }
    let l1 = &session.locals[&1];
    assert!(l1.iter().any(|v| v.name == "doubled" && v.value.as_deref() == Some("4")), "{l1:?}");

    let v = vit.lock().unwrap();
    let samples = v.samples();
    assert!(samples.len() >= 5, "sampler ran: {}", samples.len());
    assert!(samples.last().unwrap().rss_kb > samples[0].rss_kb + 10_000, "the fixture leaks 30 MiB");
    let fs = vitals::frame_stats(&v.frames()).expect("frame stats");
    assert_eq!(fs.count, 29);
    assert!(fs.hitches >= 3 && fs.hitches <= 6, "every 7th frame sleeps 30 ms instead of 4: {fs:?}");
    for line in vitals::summarize(&v) {
        println!("  = note: {line}");
    }
    println!("marker overhead: p50 {:.2} ms against a 4 ms sleep", fs.p50_ms);
}

#[test]
fn rust_panic_stops_in_user_frame_with_pretty_printers() {
    if !have("gdb") || !have("rustc") {
        eprintln!("skipped: needs gdb and rustc");
        return;
    }
    let fixtures = PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures"));
    let out = std::env::temp_dir().join(format!("stoat-live-rs-{}", std::process::id()));
    std::fs::create_dir_all(&out).unwrap();
    let bin = out.join("panic");
    assert!(Command::new("rustc").arg("-g").arg("-o").arg(&bin).arg(fixtures.join("panic.rs")).status().unwrap().success());
    assert!(stoat_core::discover::inspect(&bin).unwrap().is_rust);

    let cfg = GdbConfig::default();
    let printers = cfg.rust_sysroot.is_some();
    let (mut gdb, events) = GdbBackend::spawn(&cfg).unwrap();
    let mut session = Session::new(Some(fixtures));
    gdb.submit(Request::Load { program: bin, args: vec![], env: vec![], cwd: None });
    session.break_on_rust_panic(&mut gdb);
    gdb.submit(Request::Run);

    let mut out_text = String::new();
    let mut value = None;
    let mut asked = false;
    let deadline = Instant::now() + Duration::from_secs(60);
    while Instant::now() < deadline && value.is_none() && session.state != RunState::Exited {
        let Ok(ev) = events.try_recv() else {
            std::thread::sleep(Duration::from_millis(2));
            continue;
        };
        for c in session.handle(ev, &mut gdb) {
            match c {
                Change::Output { channel: Channel::Target, text } => out_text += &text,
                Change::Value { value: v, .. } => value = Some(v),
                Change::Locals { .. } if !asked => {
                    asked = true;
                    // `v` in inner() is a slice; ask one frame further up for the Vec.
                    session.up(&mut gdb);
                    session.evaluate("v", &mut gdb);
                }
                _ => {}
            }
        }
    }
    assert_eq!(session.state, RunState::Stopped, "stopped at the panic instead of exiting 101");
    assert!(out_text.contains("index out of bounds"), "message is already printed at the stop: {out_text:?}");
    let funcs: Vec<_> = session.frames.iter().filter_map(|f| f.func.clone()).collect();
    println!("frames: {funcs:?}");
    println!("selected: {} -> {:?}; v = {value:?}; printers loaded: {printers}", session.selected, funcs.get(session.selected));
    assert!(funcs[0].contains("rust_panic"));
    assert!(funcs[session.selected].ends_with("::main"), "after blame selection (inner) plus one up()");
    if printers {
        assert!(value.as_deref().is_some_and(|v| v.contains("Vec(size=3)")), "{value:?}");
    }
}

fn run_until(session: &mut Session, gdb: &mut GdbBackend, events: &async_channel::Receiver<Event>, secs: u64, mut done: impl FnMut(&Session, &[stoat_core::diag::Diagnostic]) -> bool) -> Vec<stoat_core::diag::Diagnostic> {
    let mut diags = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(secs);
    while Instant::now() < deadline && !done(session, &diags) {
        let Ok(ev) = events.try_recv() else {
            std::thread::sleep(Duration::from_millis(2));
            continue;
        };
        for c in session.handle(ev, gdb) {
            if let Change::Diagnostic(d) = c {
                diags.push(d);
            }
        }
    }
    diags
}

/// A core has no `*stopped`. The session has to make the stop itself, and the
/// crash diagnostic should come out the same as for the live crash.
#[test]
fn a_core_file_gets_frames_and_the_same_crash_diagnostic() {
    if !have("gdb") || !have("cc") {
        eprintln!("NOT RUN: a_core_file_gets_frames_and_the_same_crash_diagnostic needs gdb and cc");
        return;
    }
    let bin = build_fixture().expect("fixture builds");
    let core = bin.with_file_name(format!("core-{}", std::process::id()));
    let _ = std::fs::remove_file(&core);
    // gdb writes the core itself, so this does not depend on ulimit or core_pattern.
    let st = Command::new("gdb").args(["-q", "-nx", "-batch", "-ex", "run", "-ex"]).arg(format!("generate-core-file {}", core.display())).arg(&bin).output().expect("gdb runs");
    assert!(core.is_file(), "gdb wrote a core: {}", String::from_utf8_lossy(&st.stderr));

    let (mut gdb, events) = GdbBackend::spawn(&GdbConfig { rust_sysroot: None, ..Default::default() }).unwrap();
    let mut session = Session::new(Some(PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures"))));
    session.load_core(bin, core.clone(), &mut gdb);
    let diags = run_until(&mut session, &mut gdb, &events, 30, |s, d| !d.is_empty() && s.locals.contains_key(&0));
    let _ = std::fs::remove_file(&core);

    assert_eq!(session.state, RunState::Stopped);
    let funcs: Vec<_> = session.frames.iter().filter_map(|f| f.func.clone()).collect();
    assert_eq!(funcs, ["depth3", "depth2", "depth1", "main"]);
    let d = diags.iter().find(|d| d.code == "SIGSEGV").unwrap_or_else(|| panic!("crash diagnostic from a core: {diags:?}"));
    assert!(d.notes.iter().any(|n| n.contains("null page") && n.contains("offset 8")), "{:?}", d.notes);
    assert_eq!(session.fault_addr, Some(8));
}

#[test]
fn attaching_to_a_running_process_stops_it_and_lists_its_frames() {
    if !have("gdb") || !have("cc") {
        eprintln!("NOT RUN: attaching_to_a_running_process_stops_it_and_lists_its_frames needs gdb and cc");
        return;
    }
    if stoat_core::procs::ptrace_scope(std::path::Path::new("/proc")).unwrap_or(0) != 0 {
        eprintln!("NOT RUN: attaching_to_a_running_process_stops_it_and_lists_its_frames - ptrace_scope forbids attaching to a non-child");
        return;
    }
    let dir = std::env::temp_dir().join(format!("stoat-attach-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let src = dir.join("spin.c");
    std::fs::write(&src, "#include <unistd.h>\nint ticks;\nvoid tick(void) { ticks++; usleep(2000); }\nint main(void) { for (;;) tick(); }\n").unwrap();
    let bin = dir.join("spin");
    assert!(Command::new("cc").args(["-g", "-O0", "-o"]).arg(&bin).arg(&src).status().unwrap().success());
    let mut child = Command::new(&bin).spawn().unwrap();
    std::thread::sleep(Duration::from_millis(100));

    let listed = stoat_core::procs::list_mine();
    assert!(listed.iter().any(|p| p.pid == child.id() && p.exe.as_deref() == Some(bin.as_path())), "procs lists the target with its exe");

    let (mut gdb, events) = GdbBackend::spawn(&GdbConfig { rust_sysroot: None, ..Default::default() }).unwrap();
    let mut session = Session::new(Some(dir.clone()));
    gdb.submit(Request::Attach { pid: child.id() });
    run_until(&mut session, &mut gdb, &events, 30, |s, _| s.state == RunState::Stopped && !s.frames.is_empty());
    let funcs: Vec<_> = session.frames.iter().filter_map(|f| f.func.clone()).collect();
    drop(gdb); // detaches; gdb only kills what it started
    let alive = child.try_wait().unwrap().is_none();
    let _ = child.kill();
    let _ = child.wait();

    assert_eq!(session.state, RunState::Stopped);
    assert_eq!(session.pid, Some(child.id()), "pid is known, so vitals can sample an attached process too");
    assert!(funcs.iter().any(|f| f == "main"), "{funcs:?}");
    assert!(alive, "dropping the backend detached from the process instead of killing it");
}
