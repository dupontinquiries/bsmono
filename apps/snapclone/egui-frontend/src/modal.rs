//! modal — `ModalSurface`, the layer model every dialog sits on.
//!
//! Qt's ModalSurface (V32.32) replaced ad-hoc `raise()` calls scattered across
//! dozens of files with one structural stack. The egui port had dialogs but no
//! layer underneath them, which is the same position Qt was in before V32.32:
//! it works until two things are open at once, and then the answer to "which
//! one is on top" is whichever code ran last.
//!
//! Six layers, bottom to top:
//!
//! | layer | holds | scrim | blocks input |
//! |---|---|---|---|
//! | Ambient | palette tints, blob overlays | no | no |
//! | Hint | peel hints, swipe hints, badges | no | no |
//! | Dialog | ContactEditor, ContactPicker, SnapComposer | 140 | yes |
//! | Confirm | delete-contact, leave-conversation | 170 | yes |
//! | Toast | transient messages | no | no |
//! | Critical | auth challenges, fatal-error blockers | 200 | yes |
//!
//! The split between the scrimmed layers and the additive ones is the whole
//! design. Ambient, Hint and Toast are things drawn *over* the page that must
//! not stop you using it — a toast that ate taps for its three seconds on
//! screen would be worse than no toast. Dialog, Confirm and Critical are things
//! that need an answer before anything else happens, so they darken what is
//! behind them and take the input.
//!
//! ## The grey-screen bug, which is why dismissal is one path
//!
//! The Qt header records it: a dismissed widget vanished but its scrim stayed
//! on screen as a non-interactive grey overlay eating every click — "the screen
//! covered in grey after sending a snap". The cause was a widget leaving
//! without the stack/scrim/input cleanup running. Here the scrim and the input
//! block are *derived* from the stack rather than set alongside it, so there is
//! no second thing to forget: an empty layer cannot have a scrim, because its
//! scrim is a function of its stack.
//!
//! ## One faithful oddity
//!
//! `dismissTopmost` walks Critical → Ambient and closes the newest widget in
//! the first non-empty layer. Toast sits ABOVE Dialog, so with a toast on
//! screen the back button closes the toast and leaves the dialog open — a press
//! that appears to do nothing, since the toast was about to expire anyway, and
//! the user presses back again. Reproduced rather than quietly fixed: it is the
//! shipped ordering and the fix belongs in a decision about whether Toast
//! should be dismissible at all, not smuggled into a port.

use std::collections::BTreeMap;

/// The six layers, in stacking order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Layer {
    Ambient = 1,
    Hint = 2,
    Dialog = 3,
    Confirm = 4,
    Toast = 5,
    Critical = 6,
}

impl Layer {
    pub const ALL: [Layer; 6] = [Layer::Ambient, Layer::Hint, Layer::Dialog,
                                 Layer::Confirm, Layer::Toast, Layer::Critical];

    /// `layerName`, for logs and debug panels.
    pub fn name(self) -> &'static str {
        match self {
            Layer::Ambient => "Ambient",
            Layer::Hint => "Hint",
            Layer::Dialog => "Dialog",
            Layer::Confirm => "Confirm",
            Layer::Toast => "Toast",
            Layer::Critical => "Critical",
        }
    }

    /// Scrim alpha, 0 for the additive layers. Confirm is darker than Dialog
    /// because it sits on top of one — the same alpha twice would make the
    /// first dialog look like part of the background.
    pub fn scrim_alpha(self) -> u8 {
        match self {
            Layer::Critical => 200,
            Layer::Confirm => 170,
            Layer::Dialog => 140,
            _ => 0,
        }
    }

    pub fn has_scrim(self) -> bool { self.scrim_alpha() > 0 }

    /// The default for `blocksTouches`. True for the three that need an answer,
    /// false for the three that are purely additive.
    pub fn blocks_by_default(self) -> bool { self.has_scrim() }
}

/// A rectangle in surface coordinates. Small and local: the geometry test for
/// tap-outside is the only thing this module needs to know about layout.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect { pub x: f32, pub y: f32, pub w: f32, pub h: f32 }

impl Rect {
    pub fn new(x: f32, y: f32, w: f32, h: f32) -> Self { Self { x, y, w, h } }
    /// Half-open, matching `QRect::contains` closely enough for hit testing.
    pub fn contains(&self, px: f32, py: f32) -> bool {
        px >= self.x && px < self.x + self.w && py >= self.y && py < self.y + self.h
    }
}

/// One presented thing.
#[derive(Debug, Clone, PartialEq)]
pub struct Presented {
    pub id: String,
    /// A tap on the layer outside this widget closes it.
    pub dismiss_on_tap_outside: bool,
    /// This widget is modal within its layer: input does not reach below.
    pub blocks_touches: bool,
    pub rect: Rect,
}

/// The stack.
#[derive(Debug, Clone, Default)]
pub struct ModalSurface {
    layers: BTreeMap<Layer, Vec<Presented>>,
    /// Bumped on every present/dismiss. Qt emits `stackChanged` for the same
    /// reason: callers need to know when "is anything in the way" changed
    /// without polling every field.
    pub revision: u64,
}

impl ModalSurface {
    /// `present`. Newest goes on the end of its layer's stack.
    pub fn present(&mut self, id: &str, layer: Layer, rect: Rect,
                   dismiss_on_tap_outside: bool, blocks_touches: bool) {
        self.layers.entry(layer).or_default().push(Presented {
            id: id.to_string(), dismiss_on_tap_outside, blocks_touches, rect,
        });
        self.revision += 1;
    }

    /// Present with the layer's own defaults, which is what nearly every caller
    /// wants — the defaults exist so a Toast cannot accidentally be given
    /// Dialog's blocking behaviour.
    pub fn present_default(&mut self, id: &str, layer: Layer, rect: Rect) {
        self.present(id, layer, rect, true, layer.blocks_by_default());
    }

    pub fn stack(&self, layer: Layer) -> &[Presented] {
        self.layers.get(&layer).map(|v| v.as_slice()).unwrap_or(&[])
    }

    /// The newest thing in a layer.
    pub fn topmost(&self, layer: Layer) -> Option<&Presented> { self.stack(layer).last() }

    /// `isEmpty` across every layer.
    pub fn is_empty(&self) -> bool { self.layers.values().all(|v| v.is_empty()) }

    /// The scrim is DERIVED, not stored. An empty layer cannot leave a grey
    /// sheet over the app because there is nowhere for that state to hide.
    pub fn scrim_alpha(&self, layer: Layer) -> u8 {
        if self.stack(layer).is_empty() { 0 } else { layer.scrim_alpha() }
    }

    /// Is anything modal in the way? The gesture layer asks this before acting
    /// on a swipe: changing page out from under an open dialog leaves the
    /// dialog floating over a screen it has nothing to do with.
    pub fn blocks_input(&self) -> bool {
        self.layers.values().flatten().any(|p| p.blocks_touches)
    }

    /// The layer that owns input right now, if any.
    pub fn blocking_layer(&self) -> Option<Layer> {
        Layer::ALL.iter().rev().copied()
            .find(|l| self.stack(*l).iter().any(|p| p.blocks_touches))
    }

    /// `dismissTopmost`: highest layer first, newest within it. See the module
    /// note about Toast outranking Dialog.
    pub fn dismiss_topmost(&mut self) -> Option<String> {
        for layer in Layer::ALL.iter().rev() {
            if let Some(v) = self.layers.get_mut(layer) {
                if let Some(p) = v.pop() {
                    self.revision += 1;
                    return Some(p.id);
                }
            }
        }
        None
    }

    /// Dismiss one by id, wherever it sits. Needed because a dialog can close
    /// itself while something newer is above it.
    pub fn dismiss(&mut self, id: &str) -> bool {
        for v in self.layers.values_mut() {
            if let Some(i) = v.iter().position(|p| p.id == id) {
                v.remove(i);
                self.revision += 1;
                return true;
            }
        }
        false
    }

    pub fn dismiss_layer(&mut self, layer: Layer) -> usize {
        let n = self.layers.get_mut(&layer).map(|v| { let n = v.len(); v.clear(); n }).unwrap_or(0);
        if n > 0 { self.revision += 1; }
        n
    }

    pub fn dismiss_all(&mut self) {
        if !self.is_empty() { self.revision += 1; }
        self.layers.clear();
    }

    /// A tap at a point. Returns what it dismissed, if anything.
    ///
    /// The tap is resolved in the topmost layer that blocks input, not in
    /// whichever layer happens to be under the finger: while a Confirm is open,
    /// a tap belongs to the Confirm even if it lands over a dialog behind it.
    pub fn tap(&mut self, px: f32, py: f32) -> TapResult {
        let Some(layer) = self.blocking_layer() else { return TapResult::PassedThrough };
        let Some(top) = self.topmost(layer) else { return TapResult::PassedThrough };
        if top.rect.contains(px, py) {
            return TapResult::HitWidget(top.id.clone());
        }
        if top.dismiss_on_tap_outside {
            let id = top.id.clone();
            self.dismiss(&id);
            return TapResult::Dismissed(id);
        }
        // Swallowed, not passed down: a tap outside a non-dismissible dialog
        // must not reach the page behind it, or the user edits something they
        // cannot see.
        TapResult::Swallowed
    }
}

/// What a tap did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TapResult {
    /// Nothing modal was open; the page gets it.
    PassedThrough,
    /// Landed on the presented widget.
    HitWidget(String),
    /// Landed outside a dismissible widget, which closed.
    Dismissed(String),
    /// Landed outside a non-dismissible widget and went no further.
    Swallowed,
}

impl TapResult {
    pub fn reaches_page(&self) -> bool { matches!(self, TapResult::PassedThrough) }
}

/// Draw the scrims. The presented content itself is drawn by whoever owns it;
/// this paints only what the layer model is responsible for.
pub fn scrims_ui(ui: &mut egui::Ui, s: &ModalSurface) {
    let rect = ui.max_rect();
    for layer in Layer::ALL {
        let a = s.scrim_alpha(layer);
        if a == 0 { continue; }
        ui.painter().rect_filled(rect, 0.0,
            egui::Color32::from_rgba_unmultiplied(0, 0, 0, a));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r() -> Rect { Rect::new(100.0, 100.0, 200.0, 150.0) }

    fn surface_with_dialog() -> ModalSurface {
        let mut s = ModalSurface::default();
        s.present_default("editor", Layer::Dialog, r());
        s
    }

    #[test]
    fn the_additive_layers_neither_darken_nor_take_input() {
        // A toast that ate taps for its three seconds on screen would be worse
        // than no toast.
        for l in [Layer::Ambient, Layer::Hint, Layer::Toast] {
            assert!(!l.has_scrim(), "{} should not scrim", l.name());
            assert!(!l.blocks_by_default(), "{} should not block", l.name());
        }
        for l in [Layer::Dialog, Layer::Confirm, Layer::Critical] {
            assert!(l.has_scrim());
            assert!(l.blocks_by_default());
        }
    }

    #[test]
    fn each_scrimmed_layer_is_darker_than_the_one_below_it() {
        // Confirm sits ON a dialog; the same alpha twice would make the dialog
        // underneath look like background rather than something still there.
        assert!(Layer::Dialog.scrim_alpha() < Layer::Confirm.scrim_alpha());
        assert!(Layer::Confirm.scrim_alpha() < Layer::Critical.scrim_alpha());
        assert_eq!(
            (Layer::Dialog.scrim_alpha(), Layer::Confirm.scrim_alpha(), Layer::Critical.scrim_alpha()),
            (140, 170, 200));
    }

    #[test]
    fn a_scrim_appears_with_the_first_widget_and_leaves_with_the_last() {
        let mut s = ModalSurface::default();
        assert_eq!(s.scrim_alpha(Layer::Dialog), 0);
        s.present_default("a", Layer::Dialog, r());
        assert_eq!(s.scrim_alpha(Layer::Dialog), 140);
        s.present_default("b", Layer::Dialog, r());
        assert_eq!(s.scrim_alpha(Layer::Dialog), 140, "one scrim for the layer, not per widget");
        s.dismiss("b");
        assert_eq!(s.scrim_alpha(Layer::Dialog), 140, "'a' is still up");
        s.dismiss("a");
        assert_eq!(s.scrim_alpha(Layer::Dialog), 0);
    }

    #[test]
    fn the_grey_screen_bug_cannot_happen_here() {
        // Qt's: a dismissed widget vanished but its scrim stayed as a
        // non-interactive grey sheet eating every click — "the screen covered
        // in grey after sending a snap". The scrim is derived from the stack,
        // so there is no second piece of state to forget to clean up.
        let mut s = surface_with_dialog();
        s.present_default("confirm", Layer::Confirm, r());
        s.dismiss_all();
        assert!(s.is_empty());
        for l in Layer::ALL { assert_eq!(s.scrim_alpha(l), 0, "{} left a sheet", l.name()); }
        assert!(!s.blocks_input(), "and the app takes clicks again");
        assert!(s.tap(0.0, 0.0).reaches_page());
    }

    #[test]
    fn a_dialog_stops_a_swipe_from_changing_the_page_under_it() {
        // Otherwise the dialog is left floating over a screen it has nothing to
        // do with.
        let mut s = ModalSurface::default();
        assert!(!s.blocks_input());
        s.present_default("editor", Layer::Dialog, r());
        assert!(s.blocks_input());
        assert_eq!(s.blocking_layer(), Some(Layer::Dialog));
    }

    #[test]
    fn a_toast_does_not_block_input_even_though_it_is_the_top_layer() {
        let mut s = ModalSurface::default();
        s.present_default("sent", Layer::Toast, r());
        assert!(!s.blocks_input());
        assert_eq!(s.blocking_layer(), None);
        assert!(s.tap(0.0, 0.0).reaches_page(), "the page under a toast stays usable");
    }

    #[test]
    fn back_closes_the_toast_before_the_dialog() {
        // FAITHFUL ODDITY. dismissTopmost walks Critical → Ambient, and Toast
        // outranks Dialog, so a back press with a toast on screen closes the
        // toast — which was about to expire anyway — and appears to do nothing.
        // Reproduced rather than quietly fixed: whether Toast should be
        // dismissible at all is a design decision, not a porting one.
        let mut s = surface_with_dialog();
        s.present_default("sent", Layer::Toast, r());
        assert_eq!(s.dismiss_topmost(), Some("sent".into()));
        assert_eq!(s.dismiss_topmost(), Some("editor".into()), "the second press gets the dialog");
    }

    #[test]
    fn back_closes_a_confirm_before_the_dialog_it_sits_on() {
        // The case the ordering gets right, and the reason it walks downward.
        let mut s = surface_with_dialog();
        s.present_default("delete?", Layer::Confirm, r());
        assert_eq!(s.dismiss_topmost(), Some("delete?".into()));
        assert!(!s.is_empty(), "the dialog behind it survives");
    }

    #[test]
    fn within_a_layer_the_newest_is_dismissed_first() {
        let mut s = ModalSurface::default();
        s.present_default("first", Layer::Dialog, r());
        s.present_default("second", Layer::Dialog, r());
        assert_eq!(s.dismiss_topmost(), Some("second".into()));
        assert_eq!(s.dismiss_topmost(), Some("first".into()));
        assert_eq!(s.dismiss_topmost(), None, "an empty surface dismisses nothing");
    }

    #[test]
    fn a_widget_can_close_itself_from_under_a_newer_one() {
        // A picker that finishes while a confirm is up must not have to wait
        // its turn, and must not take the confirm with it.
        let mut s = ModalSurface::default();
        s.present_default("picker", Layer::Dialog, r());
        s.present_default("confirm", Layer::Confirm, r());
        assert!(s.dismiss("picker"));
        assert!(s.stack(Layer::Dialog).is_empty());
        assert_eq!(s.topmost(Layer::Confirm).map(|p| p.id.as_str()), Some("confirm"));
        assert!(!s.dismiss("picker"), "dismissing twice is not an error, just false");
    }

    #[test]
    fn a_tap_inside_the_widget_is_not_a_dismissal() {
        let mut s = surface_with_dialog();
        assert_eq!(s.tap(150.0, 150.0), TapResult::HitWidget("editor".into()));
        assert!(!s.is_empty(), "still open");
    }

    #[test]
    fn a_tap_outside_a_dismissible_widget_closes_it() {
        let mut s = surface_with_dialog();
        assert_eq!(s.tap(10.0, 10.0), TapResult::Dismissed("editor".into()));
        assert!(s.is_empty());
    }

    #[test]
    fn a_tap_outside_a_non_dismissible_widget_goes_no_further() {
        // Passing it down would let the user edit something they cannot see
        // behind the scrim.
        let mut s = ModalSurface::default();
        s.present("auth", Layer::Critical, r(), false, true);
        let out = s.tap(10.0, 10.0);
        assert_eq!(out, TapResult::Swallowed);
        assert!(!out.reaches_page());
        assert!(!s.is_empty(), "a critical blocker does not tap away");
    }

    #[test]
    fn a_tap_belongs_to_the_topmost_blocking_layer_not_the_one_under_the_finger() {
        // While a confirm is open the tap is the confirm's, even where it lands
        // over the dialog behind it.
        let mut s = ModalSurface::default();
        s.present_default("editor", Layer::Dialog, Rect::new(0.0, 0.0, 400.0, 400.0));
        s.present_default("confirm", Layer::Confirm, Rect::new(100.0, 100.0, 50.0, 50.0));
        assert_eq!(s.tap(300.0, 300.0), TapResult::Dismissed("confirm".into()),
                   "inside the dialog, but outside the confirm that owns input");
        assert!(!s.is_empty());
    }

    #[test]
    fn the_edges_of_a_widget_count_as_inside() {
        let mut s = surface_with_dialog();
        assert!(matches!(s.tap(100.0, 100.0), TapResult::HitWidget(_)), "top-left corner");
        let mut s2 = surface_with_dialog();
        assert!(matches!(s2.tap(299.0, 249.0), TapResult::HitWidget(_)), "last pixel inside");
        let mut s3 = surface_with_dialog();
        assert!(matches!(s3.tap(300.0, 250.0), TapResult::Dismissed(_)), "one past is outside");
    }

    #[test]
    fn the_revision_changes_on_every_present_and_dismissal() {
        // Qt emits stackChanged for this: callers need to know when "is
        // anything in the way" changed without polling every layer.
        let mut s = ModalSurface::default();
        let r0 = s.revision;
        s.present_default("a", Layer::Dialog, r());
        assert!(s.revision > r0);
        let r1 = s.revision;
        s.dismiss("a");
        assert!(s.revision > r1);
        let r2 = s.revision;
        s.dismiss("nothing-here");
        assert_eq!(s.revision, r2, "a no-op is not a change");
    }

    #[test]
    fn dismissing_a_layer_leaves_the_others_alone() {
        let mut s = ModalSurface::default();
        s.present_default("blob", Layer::Ambient, r());
        s.present_default("editor", Layer::Dialog, r());
        assert_eq!(s.dismiss_layer(Layer::Dialog), 1);
        assert!(!s.is_empty(), "the ambient overlay is not a dialog and does not go with it");
        assert_eq!(s.dismiss_layer(Layer::Dialog), 0);
    }

    #[test]
    fn layers_stack_in_the_documented_order() {
        assert!(Layer::Ambient < Layer::Hint);
        assert!(Layer::Hint < Layer::Dialog);
        assert!(Layer::Dialog < Layer::Confirm);
        assert!(Layer::Confirm < Layer::Toast);
        assert!(Layer::Toast < Layer::Critical);
        assert_eq!(Layer::Dialog.name(), "Dialog");
    }

    #[test]
    fn an_explicit_flag_beats_the_layer_default() {
        // MediaSendSummary is presented into Dialog without blocking, so the
        // camera behind it keeps working.
        let mut s = ModalSurface::default();
        s.present("summary", Layer::Dialog, r(), true, false);
        assert!(!s.blocks_input());
        assert_eq!(s.scrim_alpha(Layer::Dialog), 140, "it still darkens, it just does not block");
    }
}
