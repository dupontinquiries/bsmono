//! Session + translate driven from a recorded gdb 15.1 transcript. No gdb needed.

use std::time::Instant;
use stoat_core::backend::*;
use stoat_core::diag::Severity;
use stoat_core::gdb::translate::{translate, Pending, PendingMap};
use stoat_core::model::RunState;
use stoat_core::session::{Change, Session};

#[derive(Default)]
struct Recorder(Vec<Request>);

impl Backend for Recorder {
    fn submit(&mut self, req: Request) -> ReqId {
        self.0.push(req);
        // Ids the transcript never uses, so canned replies are matched by the
        // pending map below and not by accident.
        1000 + self.0.len() as u64
    }
}

#[test]
fn crash_transcript_drives_session() {
    let raw = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/../mi/tests/crash_gdb15_mi3.txt")).unwrap();
    // Tokens 1..4 are the ones typed when the transcript was recorded.
    let mut pending: PendingMap = [
        (1, Pending::Frames { thread: 1 }),
        (2, Pending::Locals { thread: 1, frame: 1 }),
        (3, Pending::Threads),
        (4, Pending::Evaluate { expr: "nosuch".into() }),
    ]
    .into();

    let mut session = Session::new(None);
    let mut backend = Recorder::default();
    let mut changes = Vec::new();
    for line in raw.split(|b| *b == b'\n').filter(|l| !l.is_empty()) {
        for ev in translate(stoat_mi::parse_line(line), Instant::now(), &mut pending) {
            changes.extend(session.handle(ev, &mut backend));
        }
    }

    assert!(pending.is_empty(), "every tokened reply was consumed");
    assert_eq!(session.pid.is_some(), false, "gdb-exit tears the inferior down");
    assert!(changes.contains(&Change::State(RunState::Running)));
    assert!(changes.contains(&Change::State(RunState::Stopped)));

    // On the SIGSEGV stop the session asked, in order, for the fault address,
    // the frames and the threads; then locals for the frame it selected.
    assert!(matches!(&backend.0[0], Request::Evaluate { expr, .. } if expr.contains("si_addr")));
    assert_eq!(backend.0[1], Request::Frames { thread: 1 });
    assert_eq!(backend.0[2], Request::Threads);
    assert!(matches!(backend.0[3], Request::Locals { thread: 1, .. }));

    let funcs: Vec<_> = session.frames.iter().filter_map(|f| f.func.as_deref()).collect();
    assert_eq!(funcs, ["depth3", "depth2", "depth1", "main"]);
    assert_eq!(session.locals[&1][0].name, "n");

    let diags: Vec<_> = changes.iter().filter_map(|c| if let Change::Diagnostic(d) = c { Some(d) } else { None }).collect();
    let crash = diags.iter().find(|d| d.code == "SIGSEGV").expect("crash diagnostic");
    assert_eq!(crash.severity, Severity::Error);
    assert!(crash.title.contains("segmentation fault"));
    let nosym = diags.iter().find(|d| d.code == "no-symbol").expect("classified gdb error");
    assert!(!nosym.notes.is_empty());
}

#[test]
fn frame_marker_stops_are_swallowed_and_resumed() {
    let mut s = Session::new(None);
    let mut b = Recorder::default();
    s.insert_frame_marker("present_frame", &mut b);
    let id = 1001;
    assert!(s.handle(Event::Reply { id, result: Ok(Reply::Breakpoint { id: 7, pending: false, locations: 1 }) }, &mut b).is_empty());

    let at = Instant::now();
    let stop = stoat_core::model::Stop { reason: stoat_core::model::StopReason::Breakpoint { id: 7 }, thread: Some(1), frame: None, at };
    assert_eq!(s.handle(Event::Stopped(stop), &mut b), vec![Change::FrameMark(at)]);
    assert_eq!(b.0.last(), Some(&Request::Continue));
    // The *running that follows our own continue is not surfaced either.
    assert!(s.handle(Event::Running, &mut b).is_empty());
    // A later, user-visible resume is.
    assert_eq!(s.handle(Event::Running, &mut b), vec![Change::State(RunState::Running)]);
}

#[test]
fn did_you_mean() {
    let d = stoat_core::diag::for_error("No symbol \"countr\" in current context.", &["counter".into(), "doubled".into()]);
    assert_eq!(d.help, ["a variable with a similar name is in scope: `counter`"]);
    let d = stoat_core::diag::for_error("Cannot access memory at address 0x10", &[]);
    assert_eq!(d.code, "bad-address");
    assert_eq!(d.notes.len(), 1);
}
