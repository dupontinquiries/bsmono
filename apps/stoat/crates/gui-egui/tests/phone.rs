//! Every page drawn headlessly at phone size and checked, with the Stack page
//! populated from a recorded gdb 15.1 session rather than from hand-made state.
//!
//! What these can and cannot say: layout_probe reads shapes, so it sees text
//! drawn over text, text pushed off the edge, two widgets on one id, and whether
//! a given string was laid out at all. It does not see colour or legibility, and
//! it never runs gdb. The real-gdb path is covered in stoat-core's live_gdb.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Instant;
use stoat_core::backend::{Backend, ReqId, Request};
use stoat_core::discover::{Candidate, DebugInfo, Reason};
use stoat_core::gdb::translate::{translate, Pending, PendingMap};
use stoat_core::model::RunState;
use stoat_egui::shell::{ShellState, PAGES, PAGE_STACK};
use uikit_egui::layout_probe;

/// A Fairphone-ish portrait viewport in logical points (same as gitmind's test).
const PHONE: [f32; 2] = [360.0, 760.0];

#[derive(Clone, Default)]
struct Recorder(Arc<Mutex<Vec<Request>>>);

impl Backend for Recorder {
    fn submit(&mut self, req: Request) -> ReqId {
        let mut v = self.0.lock().unwrap();
        v.push(req);
        1000 + v.len() as u64
    }
}

fn scratch() -> PathBuf {
    let d = std::env::temp_dir().join(format!("stoat-egui-test-{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A shell that has been through the recorded SIGSEGV session.
fn stopped_shell() -> (ShellState, Recorder) {
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

fn probe(st: &mut ShellState, size: [f32; 2], frames: usize) -> (layout_probe::Report, Vec<String>) {
    let mut view = stoat_egui::View::default();
    let mut colors = None;
    let mut texts = Vec::new();
    let ctx_size = egui::vec2(size[0], size[1]);
    let report = layout_probe::probe(ctx_size, frames, size[0] < 600.0, |ctx| {
        let c = colors.get_or_insert_with(|| uikit_egui::apply_palette(ctx, snapclone_theme::palette_for(snapclone_theme::PaletteId::Night))).clone();
        #[allow(deprecated)]
        egui::CentralPanel::default().frame(egui::Frame::new().inner_margin(10.0)).show(ctx, |ui| stoat_egui::draw(ui, st, &mut view, &c));
    });
    // A second pass only to read the strings back out.
    let ctx = egui::Context::default();
    let c = uikit_egui::apply_palette(&ctx, snapclone_theme::palette_for(snapclone_theme::PaletteId::Night));
    let mut input = egui::RawInput::default();
    input.screen_rect = Some(egui::Rect::from_min_size(egui::Pos2::ZERO, ctx_size));
    for _ in 0..frames {
        #[allow(deprecated)]
        let out = ctx.run(input.clone(), |ctx| {
            #[allow(deprecated)]
            egui::CentralPanel::default().frame(egui::Frame::new().inner_margin(10.0)).show(ctx, |ui| stoat_egui::draw(ui, st, &mut view, &c));
        });
        texts = layout_probe::texts(&out.shapes).into_iter().map(|t| t.text).collect();
    }
    (report, texts)
}

#[test]
fn no_page_overlaps_clips_or_clashes_at_phone_width() {
    let (mut st, _) = stopped_shell();
    let mut broken = Vec::new();
    for page in 0..PAGES.len() {
        st.page = page;
        let (r, texts) = probe(&mut st, PHONE, 3);
        assert!(texts.len() >= 4, "{}: the page drew almost nothing ({texts:?}), so a clean report would mean nothing", PAGES[page]);
        // clean() = no overlap, no clip, nothing off-screen, nothing under the
        // readable floor, no id clash.
        if !r.clean() {
            broken.push(format!("── {} ──\n{}", PAGES[page], r.describe()));
        }
    }
    assert!(broken.is_empty(), "at {}×{}:\n\n{}", PHONE[0], PHONE[1], broken.join("\n\n"));
}

#[test]
fn the_recorded_crash_reaches_the_stack_page() {
    let (mut st, rec) = stopped_shell();
    st.page = PAGE_STACK;
    let (_, texts) = probe(&mut st, PHONE, 3);
    let all = texts.join("\n");
    assert!(all.contains("error[SIGSEGV]"), "diagnostic card: {all}");
    assert!(all.contains("depth3"), "blamed frame's function: {all}");
    assert!(all.contains("frame 1 of 4"), "position: {all}");

    // Walk toward the caller through the shell, as the ‹ › buttons and the swipe do.
    st.toward_caller();
    assert_eq!(st.session.selected, 1);
    let (_, texts) = probe(&mut st, PHONE, 3);
    let all = texts.join("\n");
    assert!(all.contains("depth2") && all.contains("frame 2 of 4"), "{all}");
    assert!(all.contains("0x7fffffffe8d0"), "frame 1's locals came with the transcript: {all}");
    // Frame 1's locals were already there; frame 0's were asked for at the stop.
    let asked: Vec<_> = rec.0.lock().unwrap().iter().filter(|r| matches!(r, Request::Locals { .. })).cloned().collect();
    assert_eq!(asked, [Request::Locals { thread: 1, frame: 0 }]);
}

#[test]
fn a_wide_window_shows_stack_vitals_and_wire_side_by_side() {
    let (mut st, _) = stopped_shell();
    st.page = PAGE_STACK;
    // 40 frames: the plan adopts a wider layout only after it has held for a dwell.
    let (r, texts) = probe(&mut st, [1300.0, 800.0], 40);
    let all = texts.join("\n");
    for needle in ["error[SIGSEGV]", "MEMORY (resident, MiB)", "MI traffic"] {
        assert!(all.contains(needle), "{needle:?} missing from a 1300pt window: {all}");
    }
    assert!(r.id_clashes.is_empty(), "{}", r.describe());
    // And the same state at phone width shows one page only.
    let (_, texts) = probe(&mut st, PHONE, 40);
    assert!(!texts.join("\n").contains("MI traffic"));
}

/// The cause of the one-character-input bug that no rendered frame can show.
#[test]
fn every_text_field_goes_through_the_kit() {
    let mut fields = 0;
    for (name, src) in stoat_egui::pages::SOURCES {
        assert!(!src.contains("TextEdit::singleline") && !src.contains("TextEdit::multiline"), "{name}.rs builds a raw TextEdit; use uikit_egui::text_field so it cannot lack an id");
        fields += src.matches("text_field::edit(").count();
    }
    assert!((5..=12).contains(&fields), "the scan found {fields} fields; if this is 0 it matched nothing and the assertion above proved nothing");
}
