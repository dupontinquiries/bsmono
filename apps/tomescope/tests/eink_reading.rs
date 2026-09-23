//! tomescope on eink-kit, end to end: the real TomeApp, a real book, a
//! finger tap through eink-kit's touch path, and the refresh policy's view
//! of the page turn. Needs `--features eink`.
#![cfg(feature = "eink")]

use eink_kit::config::Config;
use eink_kit::format::PixelFormat;
use eink_kit::policy::{Update, Waveform};
use eink_kit::present::{mem::MemPresent, Present};
use eink_kit::touch::{Phase, TouchEvent};
use eink_kit::{EinkApp, Engine};
use std::time::Duration;
use tomescope::app::TomeApp;

struct Shell {
    app: Option<TomeApp>,
    open: Option<std::path::PathBuf>,
}

impl EinkApp for Shell {
    fn ui(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let app = self.app.get_or_insert_with(|| {
            let mut a = TomeApp::new(&ctx);
            a.set_eink(&ctx, true);
            if let Some(p) = self.open.take() {
                a.open_from_path(&p);
            }
            a
        });
        app.update(&ctx);
    }
}

fn ms(v: u64) -> Duration { Duration::from_millis(v) }

/// As `eink_kit::run` does: render when wanted, tick, sleep to next_wake.
fn drive(e: &mut Engine, app: &mut Shell, p: &mut MemPresent, from: u64, to: u64) -> Vec<Update> {
    let (mut t, mut ups) = (from, vec![]);
    while t < to {
        if e.wants_frame(ms(t)) {
            ups.extend(e.frame(ms(t), app, p).unwrap().updates);
        }
        ups.extend(e.tick(ms(t), p).unwrap());
        if e.wants_frame(ms(t)) { continue; }
        match e.next_wake() {
            Some(w) => t = (w.as_millis() as u64).max(t + 1),
            None => break,
        }
    }
    ups
}

#[test]
fn a_tap_on_the_right_third_turns_the_page_in_one_clean_refresh() {
    // Isolate persistence: tomescope saves its library under $HOME.
    let home = std::env::temp_dir().join(format!("ts_home_{}", std::process::id()));
    std::fs::create_dir_all(&home).unwrap();
    std::env::set_var("HOME", &home);
    std::env::set_var("XDG_DATA_HOME", home.join("data"));
    std::env::set_var("XDG_CONFIG_HOME", home.join("config"));

    let book = home.join("long.md");
    let para = "Electrophoretic displays hold an image without power. ".repeat(12);
    let text: String = (1..=40).map(|i| format!("## Section {i}\n\n{para}\n\n")).collect();
    std::fs::write(&book, format!("# A long book\n\n{text}")).unwrap();

    let mut cfg = Config::from_env().unwrap();
    cfg.scale = 2.0;
    cfg.threads = 2;
    cfg.full_every_pages = 6;
    let (w, h) = (1620u32, 2160u32);
    let mut p = MemPresent::new(w, h, PixelFormat::Rgb565, true, true);
    let mut e = Engine::new(&cfg, p.caps(), "mem");
    let mut shell = Shell { app: None, open: Some(book) };

    drive(&mut e, &mut shell, &mut p, 0, 2000);
    assert_eq!(shell.app.as_ref().unwrap().current_page(), Some(0), "book did not open");

    let mut t = 3000;
    let mut tap = |e: &mut Engine, shell: &mut Shell, p: &mut MemPresent, t: &mut u64| -> Vec<Update> {
        let (x, y) = (w as f32 * 0.9, h as f32 * 0.55);
        e.push_touch(ms(*t), &TouchEvent { id: 0, phase: Phase::Start, x, y });
        let mut ups = drive(e, shell, p, *t, *t + 60);
        e.push_touch(ms(*t + 60), &TouchEvent { id: 0, phase: Phase::End, x, y });
        ups.extend(drive(e, shell, p, *t + 60, *t + 2500));
        *t += 3000;
        ups
    };

    let ups = tap(&mut e, &mut shell, &mut p, &mut t);
    assert_eq!(shell.app.as_ref().unwrap().current_page(), Some(1), "tap on the right third did not turn the page: {ups:?}");
    let turns: Vec<_> = ups.iter().filter(|u| u.reason.starts_with("page turn")).collect();
    assert_eq!(turns.len(), 1, "exactly one page-turn refresh: {ups:?}");
    assert_eq!(turns[0].waveform, Waveform::Quality);
    assert!(!ups.iter().any(|u| u.reason.starts_with("settle")), "no second (settle) flash after a turn: {ups:?}");
    assert!(!ups.iter().any(|u| u.reason.starts_with("motion")), "a page turn is not motion: {ups:?}");

    // Turns 2..6: the sixth is a full refresh.
    let mut kinds = vec![turns[0].waveform];
    for _ in 2..=6 {
        let ups = tap(&mut e, &mut shell, &mut p, &mut t);
        kinds.extend(ups.iter().filter(|u| u.reason.starts_with("page turn")).map(|u| u.waveform));
    }
    assert_eq!(shell.app.as_ref().unwrap().current_page(), Some(6));
    assert_eq!(kinds.len(), 6, "{kinds:?}");
    assert_eq!(kinds[5], Waveform::Full, "{kinds:?}");
    assert!(kinds[..5].iter().all(|k| *k == Waveform::Quality), "{kinds:?}");
    let _ = std::fs::remove_dir_all(&home);
}
