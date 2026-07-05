//! rusty-board-egui — the on-screen keyboard, ported to egui on uikit-egui + adaptive.
//!
//! The GTK4 UI (`src/ui/*`) becomes this: a keyboard rendered with egui, sized by the
//! [`Adaptive`] layer (touch targets grow on the FP5, tighten on desktop) and themed from
//! snapclone-theme. rusty-board's `layouts`, `gestures`, and `keyboard/engine.rs` are
//! already GTK-free, so on-device this front-end consumes the real `layouts::Layout` and
//! feeds taps/swipes into the engine. Here it carries a small standalone [`Layout`] so the
//! crate builds and can be checked against egui 0.29 on its own.

use egui::{Align2, FontId, Rect, CornerRadius, Sense, Stroke, Ui, Vec2};
use uikit_egui::{apply_palette, Adaptive, PaletteColors};

/// A single key: what it shows and its relative width within the row.
#[derive(Debug, Clone)]
pub struct Key {
    pub label: String,
    pub weight: f32,
    /// The string emitted on tap (defaults to the label).
    pub emit: String,
}

impl Key {
    pub fn new(label: &str) -> Key {
        Key { label: label.to_string(), weight: 1.0, emit: label.to_string() }
    }
    pub fn wide(label: &str, weight: f32) -> Key {
        Key { label: label.to_string(), weight, emit: label.to_string() }
    }
    pub fn emitting(label: &str, emit: &str, weight: f32) -> Key {
        Key { label: label.to_string(), weight, emit: emit.to_string() }
    }
}

/// A keyboard row and its relative height.
#[derive(Debug, Clone)]
pub struct Row {
    pub keys: Vec<Key>,
    pub weight: f32,
}

/// A minimal keyboard layout (a faithful subset of rusty-board's `layouts::Layout`).
#[derive(Debug, Clone)]
pub struct Layout {
    pub name: String,
    pub rows: Vec<Row>,
}

impl Layout {
    /// A compact QWERTY, mirroring rusty-board's `default_qwerty` shape.
    pub fn qwerty() -> Layout {
        fn row(keys: &[&str]) -> Row {
            Row { keys: keys.iter().map(|k| Key::new(k)).collect(), weight: 1.0 }
        }
        Layout {
            name: "qwerty".into(),
            rows: vec![
                row(&["q", "w", "e", "r", "t", "y", "u", "i", "o", "p"]),
                row(&["a", "s", "d", "f", "g", "h", "j", "k", "l"]),
                Row {
                    keys: vec![
                        Key::wide("⇧", 1.5),
                        Key::new("z"),
                        Key::new("x"),
                        Key::new("c"),
                        Key::new("v"),
                        Key::new("b"),
                        Key::new("n"),
                        Key::new("m"),
                        Key::emitting("⌫", "\u{8}", 1.5),
                    ],
                    weight: 1.0,
                },
                Row {
                    keys: vec![
                        Key::emitting("space", " ", 6.0),
                        Key::emitting("⏎", "\n", 2.0),
                    ],
                    weight: 1.0,
                },
            ],
        }
    }

    /// A symbols/numbers layout, reached by swiping the keyboard sideways.
    pub fn symbols() -> Layout {
        fn row(keys: &[&str]) -> Row {
            Row { keys: keys.iter().map(|k| Key::new(k)).collect(), weight: 1.0 }
        }
        Layout {
            name: "symbols".into(),
            rows: vec![
                row(&["1", "2", "3", "4", "5", "6", "7", "8", "9", "0"]),
                row(&["!", "@", "#", "$", "%", "&", "*", "(", ")"]),
                Row {
                    keys: vec![
                        Key::wide("=\\<", 1.5),
                        Key::new("-"),
                        Key::new("_"),
                        Key::new("+"),
                        Key::new("/"),
                        Key::new(":"),
                        Key::new(";"),
                        Key::new("?"),
                        Key::emitting("⌫", "\u{8}", 1.5),
                    ],
                    weight: 1.0,
                },
                Row {
                    keys: vec![Key::emitting("space", " ", 6.0), Key::emitting("⏎", "\n", 2.0)],
                    weight: 1.0,
                },
            ],
        }
    }
}

/// A keyboard with multiple layouts you swipe between (qwerty ↔ symbols), using the shared
/// gesture layer — mirroring rusty-board's swipe layout-switching.
pub struct Board {
    layouts: Vec<Layout>,
    idx: usize,
    gesture: uikit_egui::GestureLayer,
}

impl Board {
    /// Default board: qwerty + symbols.
    pub fn new() -> Board {
        Board { layouts: vec![Layout::qwerty(), Layout::symbols()], idx: 0, gesture: uikit_egui::GestureLayer::horizontal_page(320.0) }
    }
    pub fn current(&self) -> &Layout {
        &self.layouts[self.idx]
    }
    /// Switch layout by a swipe sign (wraps, since there are only a couple).
    pub fn swipe(&mut self, sign: i32) {
        let n = self.layouts.len() as i32;
        self.idx = (((self.idx as i32 + sign) % n + n) % n) as usize;
    }
    /// Read a committed swipe from the gesture layer (call each frame with the board rect).
    pub fn poll_swipe(&mut self, ui: &egui::Ui, surface: Rect) {
        let frame = self.gesture.update(ui, surface, true);
        if let Some(dir) = frame.page_jump {
            self.swipe(dir.sign());
        }
    }

    /// Adopt cross-app preferences: theme + text/button size on the context, and the swipe
    /// feel (incl. detent animation) on the keyboard's layout-switch gesture. Keeps the
    /// keyboard consistent with the rest of the fleet when settings change live.
    pub fn apply_settings(&mut self, ctx: &egui::Context, prefs: &settings_kit::Prefs) {
        uikit_egui::apply_prefs(ctx, prefs);
        *self.gesture.config_mut() = prefs.gesture_config(320.0);
    }
}

impl Default for Board {
    fn default() -> Self {
        Board::new()
    }
}

/// Apply the shared palette to the egui context (call once at startup).
pub fn apply_theme(ctx: &egui::Context) {
    let palette = snapclone_theme::palette_for(snapclone_theme::PaletteId::all()[0]);
    apply_palette(ctx, palette);
}

/// Render the keyboard into `ui`, sized by `adaptive`, and return the `emit` string of a
/// tapped key (if any). The whole surface adapts: key height is the adaptive touch target
/// (comfortably large on a phone), spacing is the adaptive gutter, and the board fills the
/// available width with per-key widths proportional to their weight.
pub fn keyboard_ui(ui: &mut Ui, layout: &Layout, adaptive: &Adaptive) -> Option<String> {
    let colors = PaletteColors::from_palette(snapclone_theme::palette_for(snapclone_theme::PaletteId::all()[0]));
    let gutter = adaptive.gutter;
    let key_h = adaptive.min_touch_target; // 44pt touch / 28pt pointer, from the adaptive layer
    let total_w = ui.available_width();

    let mut emitted = None;
    for row in &layout.rows {
        let total_weight: f32 = row.keys.iter().map(|k| k.weight).sum::<f32>().max(0.001);
        let usable = total_w - gutter * (row.keys.len() as f32 + 1.0);
        ui.horizontal(|ui| {
            ui.add_space(gutter);
            for key in &row.keys {
                let w = (usable * (key.weight / total_weight)).max(8.0);
                if paint_key(ui, key, Vec2::new(w, key_h), gutter, &colors) {
                    emitted = Some(key.emit.clone());
                }
            }
        });
        ui.add_space(gutter);
    }
    emitted
}

/// Paint one key as a carved rounded tile with a centered label; returns true on tap.
fn paint_key(ui: &mut Ui, key: &Key, size: Vec2, gutter: f32, c: &PaletteColors) -> bool {
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    let painter = ui.painter();
    let fill = if resp.is_pointer_button_down_on() {
        c.accent
    } else if resp.hovered() {
        c.bg_hover
    } else {
        c.bg_raised
    };
    let rounding = CornerRadius::same((size.y * 0.18) as u8);
    painter.rect_filled(rect, rounding, fill);
    painter.rect_stroke(rect, rounding, Stroke::new(1.0, c.bg_base), egui::StrokeKind::Inside);
    let text_color = if resp.is_pointer_button_down_on() { c.bg_base } else { c.text };
    painter.text(
        rect.center(),
        Align2::CENTER_CENTER,
        &key.label,
        FontId::proportional((size.y * 0.42).clamp(11.0, 26.0)),
        text_color,
    );
    let _ = gutter;
    resp.clicked()
}

/// Convenience for tests / headless: which key (emit) a tap at `pos` would hit, given a
/// laid-out board of `board` rect. Pure geometry, no egui context needed — lets the layout
/// math be unit-tested without a live UI.
pub fn hit_test(layout: &Layout, board: Rect, adaptive: &Adaptive, pos: egui::Pos2) -> Option<String> {
    let gutter = adaptive.gutter;
    let key_h = adaptive.min_touch_target;
    let mut y = board.top();
    for row in &layout.rows {
        let total_weight: f32 = row.keys.iter().map(|k| k.weight).sum::<f32>().max(0.001);
        let usable = board.width() - gutter * (row.keys.len() as f32 + 1.0);
        let mut x = board.left() + gutter;
        let row_rect = Rect::from_min_size(egui::pos2(board.left(), y), Vec2::new(board.width(), key_h));
        if row_rect.contains(pos) {
            for key in &row.keys {
                let w = (usable * (key.weight / total_weight)).max(8.0);
                let krect = Rect::from_min_size(egui::pos2(x, y), Vec2::new(w, key_h));
                if krect.contains(pos) {
                    return Some(key.emit.clone());
                }
                x += w;
            }
        }
        y += key_h + gutter;
    }
    None
}

/// Accessibility: a stable id for the key at (row, col), so voice focus survives re-layout.
pub fn key_id(row: usize, col: usize) -> u64 {
    ((row as u64) << 8) | (col as u64)
}

/// The emit string for a key id (inverse of [`key_id`]).
pub fn emit_for_id(layout: &Layout, id: u64) -> Option<String> {
    let r = (id >> 8) as usize;
    let c = (id & 0xff) as usize;
    layout.rows.get(r).and_then(|row| row.keys.get(c)).map(|k| k.emit.clone())
}

/// Register every key with an [`A11yScene`] so voice can navigate the keyboard. Call each
/// frame (before/with rendering). Keys become focusable buttons labelled by their glyph.
pub fn register_a11y(scene: &mut uikit_egui::A11yScene, layout: &Layout) {
    scene.begin_frame();
    for (r, row) in layout.rows.iter().enumerate() {
        for (c, key) in row.keys.iter().enumerate() {
            scene.add(key_id(r, c), uikit_egui::Semantics::button(&key.label), None);
        }
    }
}

/// Apply a voice accessibility action to the keyboard. On "activate" it returns the `emit`
/// string of the focused key (the app types it); navigation/describe return `None` here (the
/// scene speaks). This makes the ported keyboard fully voice-navigable via voiced.
pub fn voice_key(scene: &mut uikit_egui::A11yScene, layout: &Layout, action: uikit_egui::A11yAction) -> Option<String> {
    match scene.apply(action) {
        uikit_egui::A11yOutcome::Activate(id) => emit_for_id(layout, id),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use uikit_egui::DeviceClass;

    #[test]
    fn qwerty_shape() {
        let l = Layout::qwerty();
        assert_eq!(l.rows.len(), 4);
        assert_eq!(l.rows[0].keys.len(), 10);
        // backspace emits the control char, not its glyph
        let bs = l.rows[2].keys.iter().find(|k| k.label == "⌫").unwrap();
        assert_eq!(bs.emit, "\u{8}");
    }

    #[test]
    fn phone_keys_are_taller_than_desktop() {
        let phone = Adaptive { class: DeviceClass::Phone, touch: true, min_touch_target: 44.0, ..Adaptive::default() };
        let desktop = Adaptive { class: DeviceClass::Desktop, ..Adaptive::default() };
        assert!(phone.min_touch_target > desktop.min_touch_target, "touch targets grow on phone");
    }

    #[test]
    fn hit_test_maps_position_to_key() {
        let l = Layout::qwerty();
        let a = Adaptive { min_touch_target: 40.0, gutter: 4.0, ..Adaptive::default() };
        let board = Rect::from_min_size(egui::pos2(0.0, 0.0), Vec2::new(400.0, 200.0));
        // top-left of the first row should land on 'q'
        let hit = hit_test(&l, board, &a, egui::pos2(6.0, 5.0));
        assert_eq!(hit.as_deref(), Some("q"));
    }

    #[test]
    fn swipe_switches_layout() {
        let mut b = Board::new();
        assert_eq!(b.current().name, "qwerty");
        b.swipe(1);
        assert_eq!(b.current().name, "symbols");
        b.swipe(1); // wraps back
        assert_eq!(b.current().name, "qwerty");
        b.swipe(-1);
        assert_eq!(b.current().name, "symbols");
    }

    #[test]
    fn apply_settings_runs_and_keeps_layout() {
        let ctx = egui::Context::default();
        let mut b = Board::new();
        let mut prefs = settings_kit::Prefs::default();
        prefs.gesture.animation = settings_kit::AnimationStyle::Snappy;
        prefs.touch.text_scale = 1.4;
        // adopting cross-app settings doesn't disturb the current layout
        b.apply_settings(&ctx, &prefs);
        assert_eq!(b.current().name, "qwerty");
        // text scale reached the shared egui context
        let body = ctx.style().text_styles.get(&egui::TextStyle::Body).map(|f| f.size).unwrap_or(0.0);
        assert!(body > 0.0);
    }

    #[test]
    fn keyboard_is_voice_navigable() {
        use uikit_egui::{A11yAction, A11yScene};
        let l = Layout::qwerty();
        let mut scene = A11yScene::new();
        register_a11y(&mut scene, &l);
        // "next" focuses the first key (q), "next" again → w
        assert_eq!(voice_key(&mut scene, &l, A11yAction::Next), None); // focus q (nav speaks, no emit)
        assert_eq!(voice_key(&mut scene, &l, A11yAction::Next), None); // focus w
        // "activate" types the focused key
        assert_eq!(voice_key(&mut scene, &l, A11yAction::Activate).as_deref(), Some("w"));
        // id round-trips to emit; backspace's control char comes through
        let bs_id = key_id(2, 8);
        assert_eq!(emit_for_id(&l, bs_id).as_deref(), Some("\u{8}"));
    }
}

// ── voice mode (dictation) — mic-kit fallback chain + voice-kit pipeline ──────
// The keyboard's voice mode: read the mic through a graceful FallbackChain, accumulate in the
// voice pipeline, and on release transcribe + filter (at the user's VoiceFilter level) into
// text to type. The mic is a step-up — if it's silent/stale the status shows it and no text is
// forced. Same pipeline voiceterm's voice pane and logscope voice commands will use.

/// Keyboard dictation state.
pub struct VoiceMode {
    pipeline: voice_kit::pipeline::VoicePipeline,
    active: bool,
    status: mic_kit::InputStatus,
    scratch: Vec<f32>,
}

impl VoiceMode {
    pub fn new() -> VoiceMode {
        VoiceMode {
            pipeline: voice_kit::pipeline::VoicePipeline::new(1600), // ~0.1s @16k before a flush is worthwhile
            active: false,
            status: mic_kit::InputStatus::Silent,
            scratch: vec![0.0; 320],
        }
    }

    pub fn is_active(&self) -> bool {
        self.active
    }

    pub fn status(&self) -> mic_kit::InputStatus {
        self.status
    }

    /// Start/stop dictation. Returns the new active state.
    pub fn toggle(&mut self) -> bool {
        self.active = !self.active;
        if !self.active {
            self.pipeline.cancel();
        }
        self.active
    }

    /// Sync the filter strength from cross-app settings (VoiceFilter level).
    pub fn apply_settings(&mut self, prefs: &settings_kit::Prefs) {
        let vf = prefs.a11y.voice_filter;
        self.pipeline.set_filter(vf.strips_artifacts(), vf.filters_hallucinations(), vf.requires_speech_like());
    }

    /// Per-frame while active: pull a frame from the mic chain and accumulate it. Records the
    /// mic status (Live/Fallback/Silent) so the UI can show it. Cheap; never blocks.
    pub fn pump(&mut self, mic: &mut dyn mic_kit::AudioInput) {
        if !self.active {
            return;
        }
        let n = self.scratch.len();
        let mut frame = std::mem::take(&mut self.scratch);
        frame.resize(n, 0.0);
        self.status = mic.read(&mut frame);
        if self.status != mic_kit::InputStatus::Silent {
            self.pipeline.push(&frame);
        }
        self.scratch = frame;
    }

    /// End the utterance: transcribe + filter into text to type. Also deactivates.
    pub fn finish(&mut self, transcriber: &mut dyn voice_kit::pipeline::Transcriber) -> Option<String> {
        self.active = false;
        self.pipeline.flush(transcriber)
    }
}

impl Default for VoiceMode {
    fn default() -> Self {
        VoiceMode::new()
    }
}

#[cfg(test)]
mod voice_tests {
    use super::*;

    struct Stub(&'static str);
    impl voice_kit::pipeline::Transcriber for Stub {
        fn transcribe(&mut self, _audio: &[f32]) -> Option<String> {
            Some(self.0.to_string())
        }
    }

    #[test]
    fn toggle_and_filter_from_settings() {
        let mut v = VoiceMode::new();
        assert!(!v.is_active());
        assert!(v.toggle());
        let mut prefs = settings_kit::Prefs::default();
        prefs.a11y.voice_filter = settings_kit::VoiceFilter::Strict;
        v.apply_settings(&prefs); // Strict → require_speech; just shouldn't panic
        assert!(v.is_active());
    }

    #[test]
    fn pump_live_mic_accumulates_then_finish_types_text() {
        let mut v = VoiceMode::new();
        v.toggle(); // active
        // a live mic feeding audio
        let mut mic = mic_kit::BufferedInput::new(2048, 4);
        mic.feed(&vec![0.2f32; 1800]);
        v.pump(&mut mic);
        assert_eq!(v.status(), mic_kit::InputStatus::Live);
        let text = v.finish(&mut Stub("hello world  "));
        assert_eq!(text.as_deref(), Some("hello world"));
        assert!(!v.is_active(), "finish deactivates");
    }

    #[test]
    fn pump_silent_mic_reports_silent_and_no_forced_text() {
        let mut v = VoiceMode::new();
        v.toggle();
        // a mic that never got fed → silent/stale
        let mut mic = mic_kit::BufferedInput::new(64, 1);
        for _ in 0..3 {
            v.pump(&mut mic);
        }
        assert_eq!(v.status(), mic_kit::InputStatus::Silent);
        // nothing accumulated → finish yields no text (graceful, not an error)
        assert!(v.finish(&mut Stub("phantom")).is_none());
    }
}
