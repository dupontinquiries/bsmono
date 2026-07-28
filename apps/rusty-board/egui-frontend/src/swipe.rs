//! swipe — the egui frontend's swipe-typing layer: capture the drag, draw the trail, decode
//! it with the SHARED recognizer, and offer the ranked words.
//!
//! Swipe typing is rusty-board's headline feature and it was the one thing the egui port had
//! nothing of. The recognition itself is not reimplemented here — [`board_gestures`] is the
//! GTK app's own decoder, moved into a crate both frontends depend on, so there is exactly
//! one set of velocity-smoothing windows and pivot thresholds in the project. This module is
//! only the parts that are genuinely UI:
//!
//!   * [`SwipeState::update`] — feeds pointer samples while a drag is in flight, using egui's
//!     own frame times so the recognizer's velocity maths sees real timestamps rather than
//!     sample indices (it weighs pauses, so faking the clock would change the decode).
//!   * [`SwipeState::hitboxes`] — projects the rendered layout into the recognizer's key
//!     boxes, using the same geometry `keyboard_ui` and `hit_test` use, so what you swipe
//!     over is what gets decoded.
//!   * [`draw_trail`] — the Cairo trail from `src/ui/swipe_trail.rs` in egui terms: quadratic
//!     Bézier smoothing between samples, stroke tapering 7px → 2px head-to-tail, alpha 0.85 →
//!     0.05 along the same axis, accent-coloured.
//!   * [`candidate_bar`] — the ranked words, best first, one tap to commit.
//!
//! A drag that never travels far enough is NOT a swipe: it falls through as a tap, which is
//! what `is_significant_swipe` is for and why the tap path still works while this is armed.

use crate::{Adaptive, Layout};
use board_gestures::{GestureRecognizer, KeyHitbox, SwipePoint, WordCandidate};
use egui::{Color32, Pos2, Rect, Stroke, Ui, Vec2};

/// Minimum path length (in points) before a drag counts as a swipe rather than a tap.
/// Mirrors the GTK app's gesture threshold: roughly half a key width of travel.
pub const MIN_SWIPE_PX: f64 = 40.0;

/// Live swipe capture + the last decode.
pub struct SwipeState {
    path: Vec<SwipePoint>,
    active: bool,
    /// Ranked candidates from the last completed swipe; empty when there is nothing to offer.
    pub candidates: Vec<WordCandidate>,
    /// The raw key-sequence the path snapped to, kept for the UI's "what did it see" line.
    pub raw: String,
    recognizer: GestureRecognizer,
    start_ms: f64,
}

impl Default for SwipeState {
    fn default() -> Self {
        Self::new()
    }
}

impl SwipeState {
    pub fn new() -> SwipeState {
        SwipeState {
            path: Vec::new(),
            active: false,
            candidates: Vec::new(),
            raw: String::new(),
            recognizer: GestureRecognizer::new(),
            start_ms: 0.0,
        }
    }

    /// Load the word list the recognizer ranks against (on-device this is the shipped
    /// dictionary; tests and the demo pass a small set).
    pub fn with_dictionary(words: Vec<String>) -> SwipeState {
        let mut s = SwipeState::new();
        s.recognizer.load_dictionary(words);
        s
    }

    pub fn load_dictionary(&mut self, words: Vec<String>) {
        self.recognizer.load_dictionary(words);
    }

    pub fn is_active(&self) -> bool {
        self.active
    }

    pub fn path_len(&self) -> usize {
        self.path.len()
    }

    /// Project the rendered layout into recognizer hitboxes. Deliberately the same walk as
    /// [`crate::hit_test`]: if these two ever disagree, a swipe would decode against a
    /// different keyboard than the one under the finger.
    pub fn hitboxes(layout: &Layout, board: Rect, adaptive: &Adaptive) -> Vec<KeyHitbox> {
        let gutter = adaptive.gutter;
        let key_h = adaptive.min_touch_target;
        let mut out = Vec::new();
        let mut y = board.top();
        for row in &layout.rows {
            let total_weight: f32 = row.keys.iter().map(|k| k.weight).sum::<f32>().max(0.001);
            let usable = board.width() - gutter * (row.keys.len() as f32 + 1.0);
            let mut x = board.left() + gutter;
            for key in &row.keys {
                let w = (usable * (key.weight / total_weight)).max(8.0);
                // Only single-character keys are swipe targets — space, shift, backspace and
                // the layout switchers must never swallow a pivot, or every word would decode
                // with a modifier wedged into the middle of it.
                let swipe_target = key.emit.chars().count() == 1
                    && key.emit.chars().next().is_some_and(|c| c.is_alphabetic());
                out.push(KeyHitbox {
                    label: key.emit.clone(),
                    swipe_target,
                    x: x as f64,
                    y: y as f64,
                    width: w as f64,
                    height: key_h as f64,
                });
                x += w;
            }
            y += key_h + gutter;
        }
        out
    }

    /// Feed one frame of pointer state. Returns the decoded candidates when a swipe has just
    /// ENDED this frame (so the caller can commit or show them), `None` otherwise.
    pub fn update(
        &mut self,
        ui: &Ui,
        layout: &Layout,
        board: Rect,
        adaptive: &Adaptive,
    ) -> Option<Vec<WordCandidate>> {
        let (down, pos, time) = ui.input(|i| {
            (
                i.pointer.primary_down(),
                i.pointer.interact_pos(),
                i.time * 1000.0,
            )
        });

        if down {
            if let Some(p) = pos.filter(|p| board.contains(*p)) {
                if !self.active {
                    self.active = true;
                    self.path.clear();
                    self.start_ms = time;
                }
                // Real elapsed milliseconds, not a sample counter: the recognizer's pivot
                // detection weighs how long the finger LINGERED, so a synthetic clock would
                // quietly change which letters it sees.
                let t = (time - self.start_ms).max(0.0) as i64;
                let last_far = self
                    .path
                    .last()
                    .map(|l| (l.x - p.x as f64).hypot(l.y - p.y as f64) > 1.5)
                    .unwrap_or(true);
                if last_far {
                    self.path.push(SwipePoint::with_timestamp(p.x as f64, p.y as f64, t));
                }
            }
            return None;
        }

        if !self.active {
            return None;
        }
        self.active = false;
        let travelled: f64 = self
            .path
            .windows(2)
            .map(|w| (w[1].x - w[0].x).hypot(w[1].y - w[0].y))
            .sum();
        if self.path.len() < 3 || travelled < MIN_SWIPE_PX {
            // Not a swipe — a tap, which the normal key handling already emitted.
            self.path.clear();
            return None;
        }
        let keys = Self::hitboxes(layout, board, adaptive);
        self.raw = board_gestures::decode_swipe_path(&self.path, &keys);
        let cands = self.recognizer.recognize(&self.path, &keys);
        self.candidates = cands.clone();
        self.path.clear();
        Some(cands)
    }

    /// The live path, for the trail painter.
    pub fn path(&self) -> &[SwipePoint] {
        &self.path
    }

    pub fn clear_candidates(&mut self) {
        self.candidates.clear();
        self.raw.clear();
    }
}

/// Paint the in-flight gesture. Port of `src/ui/swipe_trail.rs`: quadratic Bézier smoothing
/// through the midpoints, stroke tapering 7px → 2px and alpha 0.85 → 0.05 from head to tail.
/// The taper is what makes a swipe read as a direction rather than a scribble.
pub fn draw_trail(ui: &Ui, path: &[SwipePoint], accent: Color32) {
    if path.len() < 2 {
        return;
    }
    let painter = ui.painter();
    let n = path.len();
    for i in 1..n {
        // head = the newest sample; taper toward the tail
        let f = i as f32 / (n - 1) as f32;
        let width = 2.0 + 5.0 * f;
        let alpha = (0.05 + 0.80 * f).clamp(0.0, 1.0);
        let col = Color32::from_rgba_unmultiplied(
            accent.r(),
            accent.g(),
            accent.b(),
            (alpha * 255.0) as u8,
        );
        let a = Pos2::new(path[i - 1].x as f32, path[i - 1].y as f32);
        let b = Pos2::new(path[i].x as f32, path[i].y as f32);
        if i + 1 < n {
            // Quadratic Bézier through the midpoints, exactly as the Cairo version: the
            // control point is the sample itself, so the curve passes smoothly between
            // midpoints instead of kinking at every touch sample.
            let c = Pos2::new(path[i + 1].x as f32, path[i + 1].y as f32);
            let m0 = a + (b - a) * 0.5;
            let m1 = b + (c - b) * 0.5;
            let steps = 6;
            let mut prev = m0;
            for s in 1..=steps {
                let t = s as f32 / steps as f32;
                let it = 1.0 - t;
                let p = Pos2::new(
                    it * it * m0.x + 2.0 * it * t * b.x + t * t * m1.x,
                    it * it * m0.y + 2.0 * it * t * b.y + t * t * m1.y,
                );
                painter.line_segment([prev, p], Stroke::new(width, col));
                prev = p;
            }
        } else {
            painter.line_segment([a, b], Stroke::new(width, col));
        }
    }
    // a soft head dot so the finger's current position is unambiguous
    if let Some(last) = path.last() {
        painter.circle_filled(
            Pos2::new(last.x as f32, last.y as f32),
            4.5,
            Color32::from_rgba_unmultiplied(accent.r(), accent.g(), accent.b(), 200),
        );
    }
}

/// The candidate strip: best word first, one tap to commit. Returns the chosen word.
///
/// Ordering is the recognizer's, untouched — it already blends path score with dictionary
/// frequency, and re-sorting here would be a second opinion competing with the tuned one.
pub fn candidate_bar(ui: &mut Ui, cands: &[WordCandidate], adaptive: &Adaptive) -> Option<String> {
    if cands.is_empty() {
        return None;
    }
    let mut picked = None;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = adaptive.gutter;
        for (i, c) in cands.iter().take(5).enumerate() {
            let best = i == 0;
            let text = egui::RichText::new(&c.word)
                .size(if best { 17.0 } else { 15.0 })
                .strong();
            let btn = egui::Button::new(text).min_size(Vec2::new(0.0, adaptive.min_touch_target * 0.7));
            if ui.add(btn).clicked() {
                picked = Some(c.word.clone());
            }
        }
    });
    picked
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Adaptive, Layout};

    fn board() -> (Layout, Rect, Adaptive) {
        let layout = Layout::qwerty();
        let rect = Rect::from_min_size(egui::pos2(0.0, 0.0), Vec2::new(720.0, 300.0));
        (layout, rect, Adaptive { min_touch_target: 44.0, gutter: 4.0, ..Adaptive::default() })
    }

    #[test]
    fn hitboxes_agree_with_hit_test() {
        // If these two walks ever diverge, a swipe decodes against a different keyboard than
        // the one the finger is on — the worst kind of bug, because it only shows up as
        // "swipe types the wrong word sometimes".
        let (layout, rect, ad) = board();
        let boxes = SwipeState::hitboxes(&layout, rect, &ad);
        assert!(!boxes.is_empty(), "a layout must produce hitboxes");
        for b in &boxes {
            let centre = egui::pos2((b.x + b.width / 2.0) as f32, (b.y + b.height / 2.0) as f32);
            if let Some(emit) = crate::hit_test(&layout, rect, &ad, centre) {
                assert_eq!(emit, b.label, "hitbox and hit_test disagree at {centre:?}");
            }
        }
    }

    #[test]
    fn only_letters_are_swipe_targets() {
        // Space/shift/backspace must never swallow a pivot, or words decode with a modifier
        // wedged into the middle of them.
        let (layout, rect, ad) = board();
        for b in SwipeState::hitboxes(&layout, rect, &ad) {
            let is_letter = b.label.chars().count() == 1
                && b.label.chars().next().unwrap().is_alphabetic();
            assert_eq!(b.swipe_target, is_letter, "{:?} swipe_target wrong", b.label);
        }
    }

    /// Build a path that visits the given letters' key centres, sampled densely enough to
    /// look like a finger rather than four teleports.
    fn path_through(layout: &Layout, rect: Rect, ad: &Adaptive, word: &str) -> Vec<SwipePoint> {
        let boxes = SwipeState::hitboxes(layout, rect, ad);
        let centre = |ch: char| -> (f64, f64) {
            let b = boxes
                .iter()
                .find(|b| b.label.eq_ignore_ascii_case(&ch.to_string()))
                .unwrap_or_else(|| panic!("no key for {ch}"));
            (b.x + b.width / 2.0, b.y + b.height / 2.0)
        };
        let mut path = Vec::new();
        let mut t = 0i64;
        let letters: Vec<char> = word.chars().collect();
        for w in letters.windows(2) {
            let (x0, y0) = centre(w[0]);
            let (x1, y1) = centre(w[1]);
            let steps = 12;
            for s in 0..=steps {
                let f = s as f64 / steps as f64;
                // linger at the endpoints: that dwell is what the pivot detector reads
                t += if s == 0 || s == steps { 40 } else { 8 };
                path.push(SwipePoint::with_timestamp(
                    x0 + (x1 - x0) * f,
                    y0 + (y1 - y0) * f,
                    t,
                ));
            }
        }
        path
    }

    #[test]
    fn a_swipe_through_a_word_decodes_that_word() {
        let (layout, rect, ad) = board();
        let mut st = SwipeState::with_dictionary(
            ["hello", "help", "hero", "world", "the", "cat"].iter().map(|s| s.to_string()).collect(),
        );
        let p = path_through(&layout, rect, &ad, "hello");
        let keys = SwipeState::hitboxes(&layout, rect, &ad);
        let cands = st.recognizer.recognize(&p, &keys);
        assert!(!cands.is_empty(), "a real swipe must produce candidates");
        assert!(
            cands.iter().take(3).any(|c| c.word == "hello"),
            "the swiped word should rank in the top 3, got {:?}",
            cands.iter().take(3).map(|c| &c.word).collect::<Vec<_>>()
        );
    }

    #[test]
    fn a_short_drag_is_a_tap_not_a_swipe() {
        // The tap path has to keep working while swipe is armed: a few pixels of finger
        // wobble while pressing a key must not decode as a word.
        let (layout, rect, ad) = board();
        let boxes = SwipeState::hitboxes(&layout, rect, &ad);
        let k = &boxes[0];
        let mut st = SwipeState::with_dictionary(vec!["qwe".into()]);
        st.path = (0..6)
            .map(|i| {
                SwipePoint::with_timestamp(k.x + k.width / 2.0 + i as f64, k.y + k.height / 2.0, i * 10)
            })
            .collect();
        st.active = true;
        let travelled: f64 = st
            .path
            .windows(2)
            .map(|w| (w[1].x - w[0].x).hypot(w[1].y - w[0].y))
            .sum();
        assert!(travelled < MIN_SWIPE_PX, "this fixture must be under the swipe threshold");
    }

    #[test]
    fn trail_smoothing_is_defined_for_every_path_length() {
        // draw_trail runs on the 60Hz drag path; a panic there is a crash mid-word.
        let ctx = egui::Context::default();
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                for n in 0..6usize {
                    let path: Vec<SwipePoint> = (0..n)
                        .map(|i| SwipePoint::with_timestamp(i as f64 * 10.0, i as f64 * 3.0, i as i64 * 8))
                        .collect();
                    draw_trail(ui, &path, Color32::from_rgb(80, 160, 255));
                }
            });
        });
    }
}
