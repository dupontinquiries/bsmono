//! Shared by the integration tests: a recording backend and a shell that has
//! been through the recorded gdb 15.1 SIGSEGV session.
#![allow(dead_code)]

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Instant;
use stoat_core::backend::{Backend, ReqId, Request};
use stoat_core::discover::{Candidate, DebugInfo, Reason};
use stoat_core::gdb::translate::{translate, Pending, PendingMap};
use stoat_core::model::RunState;
use stoat_egui::shell::ShellState;

#[derive(Clone, Default)]
pub struct Recorder(pub Arc<Mutex<Vec<Request>>>);

impl Backend for Recorder {
    fn submit(&mut self, req: Request) -> ReqId {
        let mut v = self.0.lock().unwrap();
        v.push(req);
        1000 + v.len() as u64
    }
}

pub fn scratch() -> PathBuf {
    let d = std::env::temp_dir().join(format!("stoat-egui-test-{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A shell that has been through the recorded SIGSEGV session.
pub fn stopped_shell() -> (ShellState, Recorder) {
    let mut st = ShellState::detached(scratch());
    let rec = Recorder::default();
    st.begin_run(Box::new(rec.clone()), &PathBuf::from("/tmp/crash"));
    let raw = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/../mi/tests/crash_gdb15_mi3.txt")).unwrap();
    // Tokens 1..4 are the ones typed when the transcript was recorded.
    let mut pending: PendingMap = [
        (1, Pending::Frames { thread: 1 }),
        (2, Pending::Locals { thread: 1, frame: 1 }),
        (3, Pending::Threads),
        (4, Pending::Evaluate { expr: "nosuch".into() }),
    ]
    .into();
    for line in raw.split(|b| *b == b'\n').filter(|l| !l.is_empty()) {
        // The transcript ends with -gdb-exit; stop before it tears the session down.
        if line.starts_with(b"^exit") {
            break;
        }
        for ev in translate(stoat_mi::parse_line(line), Instant::now(), &mut pending) {
            st.feed(ev);
        }
    }
    assert_eq!(st.session.state, RunState::Stopped, "fixture precondition");
    assert_eq!(st.session.frames.len(), 4, "fixture precondition");
    st.candidates = vec![Candidate {
        path: "/home/someone/projects/tile-viewer/target/debug/tile-viewer".into(),
        reasons: vec![Reason::DebugInfo(DebugInfo::Embedded), Reason::InHistory { times: 2 }, Reason::NameMatchesProject, Reason::BuildArtifact],
        debug_info: DebugInfo::Embedded,
        is_rust: true,
        invocations: Vec::new(),
    }];
    st.args_text = "--zoom 15 --tiles /var/cache/tiles/europe".into();
    // The transcript ends on a gdb error, which the shell toasts. A toast floats
    // over the page by design; it is not the layout under test.
    st.pending_toast = None;
    (st, rec)
}

