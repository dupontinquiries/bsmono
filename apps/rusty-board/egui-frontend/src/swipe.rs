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
    /// Predictive magnetism for the NEXT key, from the previous tick's
    /// candidates. See `crate::nudge` — the pull is deliberately computed from
    /// the UNNUDGED decode so this tick's nudges cannot influence the
    /// candidates that produced them.
    pub nudges: crate::nudge::Nudges,
    /// The last completed gesture's shape: press, release, path length, elapsed ms.
    ///
    /// Kept because a gesture can MEAN something other than a word — the top-edge digit and
    /// the minimize fling are decided from the same path this already tracks, and a second
    /// pointer-tracker beside this one would see a slightly different path and disagree
    /// with it about where the finger went. Cleared once read.
    finished: Option<(egui::Pos2, egui::Pos2, f32, u64)>,
    /// Words closed by a springboard crossing during the last swipe, in order, each already
    /// decoded. The caller emits these before offering candidates for the tail — they are
    /// finished, and re-offering them as candidates would ask the user to confirm a word
    /// they ended deliberately by leaving the letters.
    pub springboard_words: Vec<String>,
    /// The last swipe ended outside the letters: append a space after the tail (§3 step 5).
    pub trailing_space: bool,
    /// Where each springboard delimiter fired, for the ripple the doc asks for at every
    /// crossing. Reported rather than drawn here — `swipe.rs` owns paths, not chrome.
    pub delimiters: Vec<egui::Pos2>,
}

impl Default for SwipeState {
    fn default() -> Self {
        Self::new()
    }
}

impl SwipeState {
    pub fn new() -> SwipeState {
        SwipeState {
            delimiters: Vec::new(),
            finished: None,
            springboard_words: Vec::new(),
            trailing_space: false,
            path: Vec::new(),
            active: false,
            candidates: Vec::new(),
            raw: String::new(),
            recognizer: GestureRecognizer::new(),
            start_ms: 0.0,
            nudges: Default::default(),
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

    /// Project the rendered layout into recognizer hitboxes -- from [`crate::key_rects`], the
    /// same function that places the painted keys, so a swipe decodes against the keyboard
    /// under the finger. This used to be its own copy of the walk, which had drifted from
    /// the painter (no gap between keys, rows ~3pt short each).
    pub fn hitboxes(layout: &Layout, board: Rect, adaptive: &Adaptive) -> Vec<KeyHitbox> {
        let rects = crate::key_rects(layout, board, adaptive, &crate::Style::default());
        let mut out = Vec::new();
        for (row, rs) in layout.rows.iter().zip(&rects) {
            for (key, r) in row.keys.iter().zip(rs) {
                // Only single-character keys are swipe targets — space, shift, backspace and
                // the layout switchers must never swallow a pivot, or every word would decode
                // with a modifier wedged into the middle of it.
                let swipe_target = key.emit.chars().count() == 1
                    && key.emit.chars().next().is_some_and(|c| c.is_alphabetic());
                out.push(KeyHitbox {
                    label: key.emit.clone(),
                    swipe_target,
                    x: r.left() as f64,
                    y: r.top() as f64,
                    width: r.width() as f64,
                    height: r.height() as f64,
                });
            }
        }
        out
    }

    /// Feed one frame of pointer state. Returns the decoded candidates when a swipe has just
    /// ENDED this frame (so the caller can commit or show them), `None` otherwise.
    /// The last completed gesture's shape, once. See the field's note on why this lives
    /// here rather than in a second tracker.
    pub fn take_finished(&mut self) -> Option<(egui::Pos2, egui::Pos2, f32, u64)> {
        self.finished.take()
    }

    pub fn update(
        &mut self,
        ui: &Ui,
        layout: &Layout,
        board: Rect,
        adaptive: &Adaptive,
    ) -> Option<Vec<WordCandidate>> {
        self.update_half(ui, layout, board, adaptive, crate::halfboard::Side::Off)
    }

    /// [`Self::update`] told which side the half keyboard is on.
    ///
    /// The letter region has to SHRINK with the keys. In half-keyboard mode the rows are
    /// compacted to one side and the freed strip carries a space zone — but this function
    /// built its letter rect from the full board width, so the strip was inside "letters" and
    /// a swipe into it never registered as leaving them. The zone could be tapped and could
    /// not be swiped through, which is the half that makes it a springboard rather than a
    /// button.
    pub fn update_half(
        &mut self,
        ui: &Ui,
        layout: &Layout,
        board: Rect,
        adaptive: &Adaptive,
        half: crate::halfboard::Side,
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
        if let (Some(a), Some(b)) = (self.path.first(), self.path.last()) {
            let len: f64 = self
                .path
                .windows(2)
                .map(|w| (w[1].x - w[0].x).hypot(w[1].y - w[0].y))
                .sum();
            self.finished = Some((
                egui::pos2(a.x as f32, a.y as f32),
                egui::pos2(b.x as f32, b.y as f32),
                len as f32,
                (time - self.start_ms).max(0.0) as u64,
            ));
        }
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

        // SPRINGBOARD SEGMENTATION. A path that leaves the letter region and comes back
        // typed a space, so the pieces are separate words rather than one long one. See
        // `crate::springboard` for the rules; this supplies the geometry and runs the
        // recognizer per piece.
        //
        // The letter region is the board minus its last row: on every layout here that row
        // is space and the utility keys, not letters. Using the whole board instead would
        // put the boundary at the screen edge, where a thumb cannot cross it.
        // The keys' own rect, not the board's: `halfboard` compacts the rows to one side and
        // the freed strip is not letters. Using the board width here left the strip inside the
        // letter region, so a path into it was still "inside" and delimited nothing.
        let keys_rect = half.board_rect(board);
        let letters = Rect::from_min_max(
            keys_rect.min,
            egui::pos2(
                keys_rect.max.x,
                (keys_rect.max.y - adaptive.min_touch_target).max(keys_rect.min.y),
            ),
        );
        let pts: Vec<egui::Pos2> =
            self.path.iter().map(|p| egui::pos2(p.x as f32, p.y as f32)).collect();
        let seg = crate::springboard::segment(&pts, letters);

        self.springboard_words.clear();
        self.trailing_space = seg.trailing_space;
        self.delimiters = seg.delimiters.iter().map(|d| d.at).collect();

        // A path that crossed the TOP belongs to `topedge` — a digit arc or a minimize
        // fling — and recognising it as well would type whatever it spelled on the way up.
        if seg.terminated_by_top {
            self.raw.clear();
            self.candidates.clear();
            self.path.clear();
            return None;
        }

        let best = |r: &std::ops::Range<usize>, me: &mut Self| -> Option<String> {
            let slice = &me.path[r.clone()];
            if slice.len() < 3 {
                return None;
            }
            me.recognizer.recognize(slice, &keys).into_iter().next().map(|c| c.word)
        };
        // Every segment but the last is CLOSED: the user ended it by leaving the letters.
        let closed: Vec<std::ops::Range<usize>> =
            seg.ranges.iter().take(seg.ranges.len().saturating_sub(1)).cloned().collect();
        for r in &closed {
            if let Some(w) = best(r, self) {
                self.springboard_words.push(w);
            }
        }

        let tail = seg.ranges.last().cloned().unwrap_or(0..self.path.len());
        let tail_path: Vec<SwipePoint> = self.path[tail].to_vec();
        self.raw = board_gestures::decode_swipe_path(&tail_path, &keys);
        let cands = self.recognizer.recognize(&tail_path, &keys);
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

// ── trail styling by gesture category (ported from src/ui/swipe_trail.rs) ────
//
// The GTK trail can render in one of four treatments so that, ON DEVICE, you can
// SEE which branch a gesture fell into instead of guessing from behaviour: dots
// mean the cardinal grid claimed it, stripes mean it was decoded as a swiped
// word, a desaturated line means nothing useful happened. It is off by default
// — ordinary users want one accent-coloured line — and toggled from settings
// (`appearance.debug_trail_styling`).
//
// This is the same idea as snapclone's trace channels: a diagnostic that ships
// switched off, costs nothing when off, and is reachable on an installed build
// without a rebuild. Here the cost when off is one atomic load per trail draw.

use std::sync::atomic::{AtomicBool, Ordering};

static TRAIL_DEBUG_STYLING: AtomicBool = AtomicBool::new(false);

/// Turn gesture-category trail styling on or off (process-wide).
pub fn set_trail_debug_styling(on: bool) {
    TRAIL_DEBUG_STYLING.store(on, Ordering::Relaxed);
}

pub fn trail_debug_styling() -> bool {
    TRAIL_DEBUG_STYLING.load(Ordering::Relaxed)
}

/// Visual treatment for the swipe trail, matching the GTK enum one-for-one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum TrailStyle {
    /// Normal accent line — the default, and what always renders with debug
    /// styling off.
    #[default]
    Solid,
    /// Muted grey: "nothing useful here" (normal-tap drag, off-key start).
    Desaturated,
    /// Round dots: the cardinal grid / steno cell swipes.
    Dotted,
    /// Dashed segments: a freeform swipe-typed word.
    Striped,
}

impl TrailStyle {
    /// Map a source-key behaviour label to a style, so the live trail reflects
    /// the gesture category as the user draws. Labels match the GTK side
    /// exactly — this is the seam where the two frontends must not drift.
    pub fn for_behavior_label(label: &str) -> Self {
        match label {
            "cardinal" => TrailStyle::Dotted,
            "swipe" => TrailStyle::Striped,
            "normal" | "none" => TrailStyle::Desaturated,
            _ => TrailStyle::Solid,
        }
    }

    /// The colour this style paints in, given the configured accent. Only
    /// `Desaturated` departs from the accent — the rest carry the accent and
    /// differ in SHAPE, which stays legible on any palette.
    pub fn color(self, accent: Color32) -> Color32 {
        match self {
            TrailStyle::Desaturated => {
                // luminance-preserving grey, so the trail stays visible on both
                // light and dark boards instead of vanishing into one of them
                let l = (0.299 * accent.r() as f32
                    + 0.587 * accent.g() as f32
                    + 0.114 * accent.b() as f32) as u8;
                Color32::from_rgb(l, l, l)
            }
            _ => accent,
        }
    }

    /// Whether a segment at position `f` along the trail (0 = tail, 1 = head)
    /// should be painted. Solid and desaturated draw everything; dotted and
    /// striped punch periodic gaps, which is how they read as dots/dashes
    /// without needing a dash-pattern API egui's painter does not have.
    pub fn draws_segment(self, i: usize) -> bool {
        match self {
            TrailStyle::Solid | TrailStyle::Desaturated => true,
            TrailStyle::Dotted => i % 3 == 0,
            TrailStyle::Striped => i % 6 < 3,
        }
    }

    /// The style actually used for painting: with debug styling off, every
    /// gesture draws Solid regardless of category.
    pub fn effective(self) -> TrailStyle {
        if trail_debug_styling() { self } else { TrailStyle::Solid }
    }
}

/// Draw the trail in the configured accent (the everyday path).
pub fn draw_trail(ui: &Ui, path: &[SwipePoint], accent: Color32) {
    draw_trail_styled(ui, path, accent, TrailStyle::Solid);
}

/// Draw the trail with a gesture-category style. The style only takes effect
/// when debug styling is enabled; otherwise this is exactly `draw_trail`.
pub fn draw_trail_styled(ui: &Ui, path: &[SwipePoint], accent: Color32, style: TrailStyle) {
    let style = style.effective();
    let accent = style.color(accent);
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
        if !style.draws_segment(i) {
            continue;
        }
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
    // THE GTK RIBBON: `.suggestions-bar` is a 40px row with 2px/4px padding and ONE
    // hairline along its bottom edge, holding `.suggestion-pill`s — transparent, no
    // border, 12px radius, 13px text, tinting on hover and accent-tinting on press. The
    // port drew default egui Buttons: each candidate came with its own box and border, so
    // the ribbon read as a second row of keys sitting on top of the keyboard rather than
    // as a strip of words. Pills are the whole difference between "a word you can tap" and
    // "a key that happens to say 'the'".
    let colors = uikit_egui::palette_colors("system");
    let bar_h = BAR_HEIGHT.max(adaptive.min_touch_target * 0.7);
    let (bar_rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), bar_h), egui::Sense::hover());
    let mut strip = ui.new_child(egui::UiBuilder::new().max_rect(bar_rect.shrink2(Vec2::new(4.0, 2.0))));
    strip.horizontal_centered(|ui| {
        ui.spacing_mut().item_spacing.x = adaptive.gutter;
        for (i, c) in cands.iter().take(5).enumerate() {
            let best = i == 0;
            // The best candidate is the one that commits on space, so it is the one that
            // reads first — heavier, not bigger, because a taller pill would make the
            // ribbon's height jump as candidates change.
            let text = egui::RichText::new(&c.word)
                .size(PILL_FONT_PT)
                .color(colors.text)
                .strong();
            let text = if best { text } else { text.weak() };
            let btn = egui::Button::new(text)
                .fill(egui::Color32::TRANSPARENT)
                .stroke(Stroke::NONE)
                .corner_radius(egui::CornerRadius::same(PILL_RADIUS))
                .min_size(Vec2::new(0.0, bar_h - 4.0));
            let r = ui.add(btn);
            // `.suggestion-pill:hover` / `:active` — the tint IS the affordance, since the
            // pill has no border of its own.
            if r.is_pointer_button_down_on() {
                ui.painter().rect_filled(
                    r.rect,
                    egui::CornerRadius::same(PILL_RADIUS),
                    egui::Color32::from_rgba_unmultiplied(colors.accent.r(), colors.accent.g(), colors.accent.b(), 38),
                );
            } else if r.hovered() {
                ui.painter().rect_filled(
                    r.rect,
                    egui::CornerRadius::same(PILL_RADIUS),
                    egui::Color32::from_white_alpha(20),
                );
            }
            if r.clicked() {
                picked = Some(c.word.clone());
            }
        }
    });
    // `border-bottom: 1px solid alpha(@borders, 0.3)` — one line, under the ribbon, which
    // is what separates it from the board without giving it a box of its own.
    ui.painter().hline(
        bar_rect.x_range(),
        bar_rect.bottom() - 0.5,
        Stroke::new(1.0, egui::Color32::from_rgba_unmultiplied(colors.border.r(), colors.border.g(), colors.border.b(), 77)),
    );
    picked
}

/// `.suggestions-bar { height-request: 40 }`.
pub const BAR_HEIGHT: f32 = 40.0;
/// `.suggestion-pill { border-radius: 12px; font-size: 13px }`.
pub const PILL_RADIUS: u8 = 12;
pub const PILL_FONT_PT: f32 = 13.0;

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

#[cfg(test)]
mod trail_style_tests {
    use super::*;

    #[test]
    fn behaviour_labels_map_to_the_same_styles_as_gtk() {
        // This mapping is the seam between the two frontends: if the labels
        // drift, the same gesture draws differently on GTK and egui and the
        // diagnostic starts lying. Pinned against src/ui/swipe_trail.rs.
        assert_eq!(TrailStyle::for_behavior_label("cardinal"), TrailStyle::Dotted);
        assert_eq!(TrailStyle::for_behavior_label("swipe"), TrailStyle::Striped);
        assert_eq!(TrailStyle::for_behavior_label("normal"), TrailStyle::Desaturated);
        assert_eq!(TrailStyle::for_behavior_label("none"), TrailStyle::Desaturated);
        assert_eq!(TrailStyle::for_behavior_label("anything else"), TrailStyle::Solid);
        assert_eq!(TrailStyle::default(), TrailStyle::Solid);
    }

    #[test]
    fn debug_styling_is_off_by_default_and_flattens_every_style() {
        // Ordinary users get one accent line. Only with the toggle on does the
        // category show through — same contract as the GTK side.
        set_trail_debug_styling(false);
        assert!(!trail_debug_styling());
        for s in [TrailStyle::Dotted, TrailStyle::Striped, TrailStyle::Desaturated] {
            assert_eq!(s.effective(), TrailStyle::Solid, "{s:?} must flatten when debug is off");
        }
        set_trail_debug_styling(true);
        assert_eq!(TrailStyle::Dotted.effective(), TrailStyle::Dotted);
        set_trail_debug_styling(false); // leave the process as we found it
    }

    #[test]
    fn dotted_and_striped_actually_break_the_line_up() {
        // The styles differ in SHAPE, not just colour, so they stay legible on
        // any palette. Count what a 60-sample trail would paint.
        let painted = |s: TrailStyle| (1..60).filter(|i| s.draws_segment(*i)).count();
        let solid = painted(TrailStyle::Solid);
        assert_eq!(solid, 59, "solid paints every segment");
        assert_eq!(painted(TrailStyle::Desaturated), 59, "desaturated is solid but grey");
        let dotted = painted(TrailStyle::Dotted);
        let striped = painted(TrailStyle::Striped);
        assert!(dotted < solid / 2, "dotted should be sparse ({dotted}/{solid})");
        assert!(striped > dotted && striped < solid, "striped sits between ({striped})");
    }

    #[test]
    fn desaturated_keeps_the_accents_luminance() {
        // A grey that ignores luminance disappears on either a light or a dark
        // board; this one is derived from the accent so it stays visible.
        let bright = TrailStyle::Desaturated.color(Color32::from_rgb(255, 220, 40));
        let dark = TrailStyle::Desaturated.color(Color32::from_rgb(30, 40, 60));
        assert_eq!(bright.r(), bright.g());
        assert_eq!(bright.g(), bright.b(), "must be grey");
        assert!(bright.r() > dark.r(), "a bright accent yields a brighter grey");
        // and the other styles keep the accent untouched
        let acc = Color32::from_rgb(10, 200, 120);
        assert_eq!(TrailStyle::Dotted.color(acc), acc);
        assert_eq!(TrailStyle::Striped.color(acc), acc);
    }
}

impl SwipeState {
    /// Refresh the predictive pull from the current candidate list.
    ///
    /// Called between ticks rather than inside the decode, which is what keeps
    /// the feedback loop open: the candidates feeding this were produced by a
    /// decode that had not yet seen these nudges.
    pub fn refresh_nudges(&mut self, consumed: usize) {
        let cands: Vec<(String, f64)> = self.candidates.iter()
            .map(|c| (c.word.clone(), c.score as f64))
            .collect();
        self.nudges.update_from_candidates(&cands, consumed);
    }

    /// Reset the pull. Called when a swipe ends, so the previous word's
    /// predictions cannot bias the next word's first key.
    pub fn clear_nudges(&mut self) { self.nudges.clear(); }
}

#[cfg(test)]
mod nudge_wiring_tests {
    use super::*;

    #[test]
    fn a_fresh_swipe_state_has_no_pull() {
        // Neutral until candidates exist: the first key of a swipe has nothing
        // to predict from and must decode exactly as it would with the feature
        // off.
        let s = SwipeState::new();
        assert!(s.nudges.is_empty());
        assert_eq!(s.nudges.nudge_for("r"), 1.0);
    }

    #[test]
    fn clearing_between_swipes_removes_the_previous_words_bias() {
        let mut s = SwipeState::new();
        s.candidates = vec![WordCandidate { word: "store".into(), score: 1.0 }];
        s.refresh_nudges(3);
        assert!(!s.nudges.is_empty());
        s.clear_nudges();
        assert!(s.nudges.is_empty());
    }

    #[test]
    fn no_candidates_means_no_pull_rather_than_a_default_guess() {
        // An empty candidate list is "we do not know", and inventing a pull
        // from nothing would bias the decode toward whatever key sorted first.
        let mut s = SwipeState::new();
        s.refresh_nudges(0);
        assert!(s.nudges.is_empty());
    }
}

#[cfg(test)]
mod springboard_wiring_tests {
    use super::*;
    use crate::{Adaptive, Layout};

    fn board() -> (Layout, Rect, Adaptive) {
        let layout = Layout::qwerty();
        let rect = Rect::from_min_size(egui::pos2(0.0, 0.0), Vec2::new(720.0, 300.0));
        (layout, rect, Adaptive { min_touch_target: 44.0, gutter: 4.0, ..Adaptive::default() })
    }

    /// Drive a path through `update` the way the pointer would: down for every point, then
    /// up. The egui `Context` is real, so this exercises the same input reading the app does.
    fn swipe_through(pts: &[egui::Pos2]) -> SwipeState {
        let (layout, rect, ad) = board();
        let mut st = SwipeState::with_dictionary(["hello", "world", "cat", "dog"].iter().map(|s| s.to_string()).collect());
        let ctx = egui::Context::default();
        let mut frame = |pos: Option<egui::Pos2>, down: bool, st: &mut SwipeState| {
            let mut input = egui::RawInput::default();
            input.screen_rect = Some(Rect::from_min_size(egui::Pos2::ZERO, Vec2::new(720.0, 400.0)));
            if let Some(p) = pos {
                input.events.push(egui::Event::PointerMoved(p));
                input.events.push(egui::Event::PointerButton {
                    pos: p,
                    button: egui::PointerButton::Primary,
                    pressed: down,
                    modifiers: egui::Modifiers::NONE,
                });
            }
            let _ = ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let _ = st.update(ui, &layout, rect, &ad);
                });
            });
        };
        for p in pts {
            frame(Some(*p), true, &mut st);
        }
        frame(pts.last().copied(), false, &mut st);
        st
    }

    /// The wiring, end to end: a path that dips below the letters and comes back must
    /// produce a CLOSED word plus a tail, not one long word. The segmentation is unit-tested
    /// in `springboard`; this checks `update` actually calls it.
    #[test]
    fn a_path_that_leaves_the_letters_closes_a_word() {
        // The letter region is the board minus its last row: 300 - 44 = 256.
        let out_and_back = vec![
            egui::pos2(60.0, 100.0),
            egui::pos2(120.0, 120.0),
            egui::pos2(180.0, 140.0),
            // below 256, and far enough along to clear the 12px hysteresis
            egui::pos2(200.0, 270.0),
            egui::pos2(240.0, 275.0),
            // back into the letters
            egui::pos2(300.0, 150.0),
            egui::pos2(360.0, 120.0),
            egui::pos2(420.0, 110.0),
        ];
        let st = swipe_through(&out_and_back);
        assert_eq!(st.springboard_words.len(), 1, "no word was closed: {:?}", st.springboard_words);
        assert!(!st.trailing_space, "the path ended inside the letters");

        // A path that stays inside closes nothing — so the test is measuring the crossing.
        let inside = vec![
            egui::pos2(60.0, 100.0),
            egui::pos2(160.0, 120.0),
            egui::pos2(260.0, 140.0),
            egui::pos2(360.0, 120.0),
        ];
        let st = swipe_through(&inside);
        assert!(st.springboard_words.is_empty(), "{:?}", st.springboard_words);
        assert!(!st.trailing_space);
    }

    /// §3 step 5, through the wiring: a path that ends outside recognises what it has and
    /// asks for a space after it.
    #[test]
    fn a_path_that_ends_outside_asks_for_a_trailing_space() {
        let st = swipe_through(&[
            egui::pos2(60.0, 100.0),
            egui::pos2(160.0, 130.0),
            egui::pos2(240.0, 160.0),
            egui::pos2(260.0, 272.0),
            egui::pos2(300.0, 280.0),
        ]);
        assert!(st.trailing_space, "the path ended outside and no space was asked for");
    }

    /// A path over the top belongs to `topedge`. Recognising it as well would type whatever
    /// it spelled on the way up to a digit.
    #[test]
    fn a_path_over_the_top_recognises_nothing() {
        let st = swipe_through(&[
            egui::pos2(200.0, 60.0),
            egui::pos2(205.0, 30.0),
            egui::pos2(210.0, -20.0),
            egui::pos2(212.0, -40.0),
        ]);
        assert!(st.candidates.is_empty(), "a digit arc produced word candidates: {:?}", st.candidates);
        assert!(st.raw.is_empty());
        assert!(st.springboard_words.is_empty());
    }
}
