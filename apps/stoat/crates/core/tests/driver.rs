//! stoat_core::driver: the time-critical half of event handling, with no shell.

use std::time::Instant;
use stoat_core::backend::*;
use stoat_core::session::Session;

/// The driver does the time-critical half of a marker stop and of a real stop
/// itself, on the calling thread, without a shell being involved.
#[test]
fn driver_marks_resumes_and_freezes_without_a_shell() {
    use std::sync::{Arc, Mutex};
    use stoat_core::driver::{wants_repaint, Driver};
    use stoat_core::model::{Stop, StopReason};
    use stoat_core::vitals::{Sample, Vitals};

    #[derive(Default, Clone)]
    struct Shared(Arc<Mutex<Vec<Request>>>);
    impl Backend for Shared {
        fn submit(&mut self, req: Request) -> ReqId {
            let mut v = self.0.lock().unwrap();
            v.push(req);
            1000 + v.len() as u64
        }
    }

    let sent = Shared::default();
    let vit = Vitals::shared(16, 16);
    let mut d = Driver::new(Session::new(None), Box::new(sent.clone()), vit.clone());
    d.with(|s, b| {
        s.insert_frame_marker("present", b);
        Vec::new()
    });
    d.handle(Event::Reply { id: 1001, result: Ok(Reply::Breakpoint { id: 3, pending: false, locations: 1 }) });

    let t0 = Instant::now();
    let mark = |at| Event::Stopped(Stop { reason: StopReason::Breakpoint { id: 3 }, thread: Some(1), frame: None, at });
    let first = d.handle(mark(t0));
    assert!(!wants_repaint(&first), "a frame mark is not worth a repaint");
    d.handle(Event::Running);
    d.handle(mark(t0 + std::time::Duration::from_millis(16)));
    assert_eq!(sent.0.lock().unwrap().iter().filter(|r| **r == Request::Continue).count(), 2, "each marker stop was resumed by the driver");
    let frames = vit.lock().unwrap().frames();
    assert_eq!(frames.len(), 1);
    assert!((frames[0].ms - 16.0).abs() < 0.01, "interval comes from the transport's timestamps: {}", frames[0].ms);

    // A real stop freezes the rings at once: a sample arriving afterwards is refused.
    let stop = d.handle(Event::Stopped(Stop { reason: StopReason::StepDone, thread: Some(1), frame: None, at: t0 }));
    assert!(wants_repaint(&stop));
    let before = vit.lock().unwrap().samples().len();
    vit.lock().unwrap().push_sample(Sample { at: t0, rss_kb: 1, anon_kb: 0, swap_kb: 0, threads: 1, cpu_pct: 0.0 });
    assert_eq!(vit.lock().unwrap().samples().len(), before, "frozen");
    let v0 = d.version;
    d.handle(Event::Running);
    assert!(d.version > v0);
    vit.lock().unwrap().push_sample(Sample { at: t0, rss_kb: 2, anon_kb: 0, swap_kb: 0, threads: 1, cpu_pct: 0.0 });
    assert_eq!(vit.lock().unwrap().samples().len(), before + 1, "thawed on resume");
}
