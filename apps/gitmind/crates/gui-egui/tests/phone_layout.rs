//! Every page drawn at phone size and checked — for text drawn on top of other text, for
//! controls pushed off the right edge, and for two widgets resolving to one egui id.
//!
//! ## Why this test exists
//!
//! A photo of gitmind on a Fairphone showed the Network page with the `user` field stretched
//! across the screen and the `host` field pushed off the right edge entirely: a label with no
//! box after it. `ui.horizontal()` does not wrap, so anything past the right edge is drawn
//! there anyway — off-screen, or on top of whatever else landed in the same place. Nothing
//! failed; it just could not be used.
//!
//! ## And the one-character bug
//!
//! Two widgets that resolve to the same egui id share one widget's state, and for a
//! `TextEdit` that state includes the text cursor: typing into either moves the caret in the
//! other and a keystroke lands in whichever egui thinks is current. From the outside that is
//! "this field takes one character and then ignores me". egui detects the clash and paints a
//! warning; `uikit_egui::layout_probe` reads those warnings back out of the frame, so this
//! can fail on one.
//!
//! The OTHER cause of the same symptom is a widget whose auto-id changes between frames —
//! a `TextEdit` inside a wrapping row, right at the wrap threshold, where typing a character
//! measures wider, flips the wrap, moves the widget to a different layout slot and changes
//! its id. egui tracks focus BY id, so focus is dropped. That one an id-clash check cannot
//! see, so it is covered separately by `every_text_field_has_a_stable_id`, which reads the
//! source: an explicit `id_salt` is what makes focus survive reflow.

use gitmind_egui::app::{App, Tab};
use uikit_egui::layout_probe;

/// A Fairphone-ish portrait viewport in logical points.
const PHONE: [f32; 2] = [360.0, 760.0];

fn probe_page(tab: Tab) -> layout_probe::Report {
    let mut app = App::default();
    app.tab = tab;
    layout_probe::probe(egui::vec2(PHONE[0], PHONE[1]), 3, true, |ctx| {
        app.ui(ctx);
    })
}

#[test]
fn no_page_overlaps_its_own_text_or_runs_off_a_phone_screen() {
    let mut broken: Vec<String> = Vec::new();
    for tab in Tab::ALL {
        let r = probe_page(tab);
        if !r.overlaps.is_empty() || !r.offscreen.is_empty() || !r.clipped.is_empty() {
            broken.push(format!("── {:?} ──\n{}", tab, r.describe()));
        }
    }
    assert!(broken.is_empty(), "at {}×{}:\n\n{}", PHONE[0], PHONE[1], broken.join("\n\n"));
}

/// Two widgets on one id: the "takes one character then ignores me" bug, mechanically.
#[test]
fn no_page_puts_two_widgets_on_one_id() {
    let mut clashing: Vec<String> = Vec::new();
    for tab in Tab::ALL {
        let r = probe_page(tab);
        if !r.id_clashes.is_empty() {
            clashing.push(format!("{:?}: {}", tab, r.id_clashes.join(" / ")));
        }
    }
    assert!(clashing.is_empty(), "{}", clashing.join("\n"));
}

/// The other cause, which no rendered frame can show: a field whose id is derived from where
/// it happens to land. Typing a character can change that.
#[test]
fn every_text_field_has_a_stable_id() {
    let mut missing: Vec<String> = Vec::new();
    for (name, src) in gitmind_egui::pages::SOURCES {
        for (n, line) in src.lines().enumerate() {
            let has_edit = line.contains("TextEdit::singleline") || line.contains("TextEdit::multiline");
            if !has_edit {
                continue;
            }
            // The builder can span lines; look at this line and the two after it.
            let window: String = src.lines().skip(n).take(3).collect::<Vec<_>>().join(" ");
            if !window.contains("id_salt(") && !window.contains(".id(") {
                missing.push(format!("{name}:{}: {}", n + 1, line.trim()));
            }
        }
    }
    assert!(
        missing.is_empty(),
        "a TextEdit with no explicit id takes its id from its position in the layout. In a \
         wrapping row at the wrap threshold, typing one character measures wider, flips the \
         wrap, moves the widget and changes its id — and egui drops focus, because focus is \
         tracked by id. The field then ignores every key after the first:\n{}",
        missing.join("\n")
    );
}

/// The Tablets page shows most of its content only after a probe, which the
/// default-state sweep above never reaches. Draw it populated: a found Paper
/// Pro with an unknown motd codename, a deploy target, and a multi-line result.
#[test]
fn the_populated_tablets_page_fits_a_phone() {
    use cockpit::eink_fleet as ef;
    let mut app = App::default();
    app.tab = Tab::Tablets;
    let probe = ef::parse_probe("machine=reMarkable Ferrari\nmotd=marshmallow\narch=aarch64");
    // after a reboot: xovi not started, /etc volatile; host key changed by an OS update
    let displays = ef::parse_displays("appload_dir=/home/root/xovi/exthome/appload\nglibc=2.39\nxovi_installed=1\n\
        rootfs_ro=1\netc_overlay=1\n");
    *app.tablets.hostkey_changed.lock().unwrap() = true;
    app.tablets.confirm = Some(gitmind_egui::pages::tablets::Confirm::Activate(ef::Activation::StartXovi));
    *app.tablets.found.lock().unwrap() = Some((probe, displays));
    app.tablets.bin_path = "/home/user/build/aarch64-unknown-linux-gnu/release/eink-demo".into();
    app.tablets.last_result = "eink-demo exited right after launch:\neink: model PaperPro (EINK_MODEL=rmpp)\n\
        eink-demo: libblight_protocol not found (tried [\"libblight_protocol.so\"]; is Oxide installed? set EINK_BLIGHT_LIB)".into();
    let r = layout_probe::probe(egui::vec2(PHONE[0], PHONE[1]), 3, true, |ctx| app.ui(ctx));
    assert!(r.overlaps.is_empty() && r.offscreen.is_empty() && r.clipped.is_empty() && r.id_clashes.is_empty(),
        "populated Tablets page at {}×{}:\n{}", PHONE[0], PHONE[1], r.describe());
}
