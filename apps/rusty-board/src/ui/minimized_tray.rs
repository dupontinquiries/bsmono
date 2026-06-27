//! Minimized tray widget — the compact mode the keyboard transitions
//! to when the user swipes up off the top edge.
//!
//! Layout: a small corner-anchored stack containing
//!   - an "expand" button (return to full keyboard)
//!   - a spacebar (with arrow swipes; double-tap toggles selection mode)
//!   - a "close" button (dismiss the keyboard entirely)
//!
//! See `docs/single-hand-mode.md` for design intent.
//!
//! The tray's spacebar honors three gesture vocabularies:
//!   - Tap (any quick non-moving press) → emits a space character; also
//!     exits selection mode if it's active
//!   - Double-tap (two taps within 350ms) → toggles selection mode on/off
//!   - Cardinal swipe → emits Left/Right/Up/Down (or Shift+arrow when
//!     selection mode is active)

use crate::keyboard::engine::EngineInput;
use crate::keyboard::KeyboardEngine;
use gtk4::prelude::*;
use std::cell::Cell;
use std::rc::Rc;
use tracing::{debug, info};

/// Callbacks the parent supplies to wire the buttons.
pub struct TrayCallbacks {
    /// Called when the user taps the "expand" button. Parent should
    /// restore the full keyboard view.
    pub on_expand: Rc<dyn Fn()>,
    /// Called when the user taps the "close" button. Parent should
    /// fully dismiss the keyboard surface (set_visible(false)).
    pub on_close: Rc<dyn Fn()>,
}

/// The state of the tray's selection mode. When active, arrow swipes
/// on the tray spacebar emit Shift+Arrow rather than plain Arrow so
/// the focused app extends its selection. Toggled by double-tapping
/// the spacebar.
type SelectionMode = Rc<Cell<bool>>;

/// A minimized tray widget. Build with `MinimizedTray::new(...)`,
/// embed `tray.widget()` wherever the parent wants it shown.
pub struct MinimizedTray {
    widget: gtk4::Box,
    /// Whether selection-mode is active. Owned here so the gesture
    /// handlers can read/write it; exposed via getter for the parent.
    selection_mode: SelectionMode,
}

impl MinimizedTray {
    /// Build the tray. `dominant_side` controls the visual anchor
    /// (which corner the stack sits in). `engine` receives the
    /// keysym/space events from spacebar gestures. `callbacks`
    /// supplies the expand/close handlers.
    pub fn new(
        dominant_side: DominantSide,
        engine: KeyboardEngine,
        callbacks: TrayCallbacks,
    ) -> Self {
        let halign = match dominant_side {
            DominantSide::Right => gtk4::Align::End,
            DominantSide::Left => gtk4::Align::Start,
        };

        // Outer container — pinned to the appropriate corner via
        // halign. We use a vertical Box (stack: expand / space / close)
        // sized roughly 200×140 logical px.
        let outer = gtk4::Box::builder()
            .orientation(gtk4::Orientation::Vertical)
            .spacing(4)
            .halign(halign)
            .valign(gtk4::Align::End)
            .margin_start(12)
            .margin_end(12)
            .margin_bottom(12)
            .css_classes(["tray-panel"])
            .build();

        let selection_mode: SelectionMode = Rc::new(Cell::new(false));

        // Expand button (top of stack). Small — just enough to tap.
        // Text label "abc" matches the convention users already know
        // from other phone keyboards (Gboard, iOS) — it signals
        // "return to the letters/alphabet view" unambiguously.
        let expand_btn = gtk4::Button::builder()
            .label("abc")
            .css_classes(["tray-btn", "tray-expand"])
            .tooltip_text("Show full keyboard")
            .height_request(40)
            .build();
        {
            let on_expand = callbacks.on_expand.clone();
            expand_btn.connect_clicked(move |_| {
                debug!("tray: expand tapped");
                on_expand();
            });
        }
        outer.append(&expand_btn);

        // Spacebar (middle). Visibly larger than the chrome buttons —
        // this is the primary interactive surface in tray mode (taps =
        // space, swipes = arrows, double-tap = selection mode), so the
        // tap target should reflect that prominence.
        //
        // IMPORTANT — we use a plain `Box` with attached gestures, NOT
        // a `Button`. GTK4 Button has built-in click semantics that
        // conflict with our GestureDrag + GestureClick stack: the
        // button's internal activation consumes the touch before our
        // drag controller sees it, so swipes never fired. With a
        // raw Box we own all the gesture dispatch ourselves.
        let space_btn = gtk4::Box::builder()
            .css_classes(["tray-btn", "tray-space"])
            .hexpand(true)
            .vexpand(true)
            .height_request(90)
            .build();
        let space_label = gtk4::Label::builder()
            .label("⎵  swipe = arrows")
            .halign(gtk4::Align::Center)
            .valign(gtk4::Align::Center)
            .hexpand(true)
            .vexpand(true)
            .build();
        space_btn.append(&space_label);

        // Track selection-mode visuals via a CSS class.
        attach_selection_visual(&space_btn, &selection_mode);

        // Wire spacebar gestures. We need to detect:
        //   - tap (no significant motion)
        //   - double-tap (two taps within DOUBLE_TAP_MS)
        //   - cardinal swipe (significant motion in one cardinal)
        //
        // GTK4: a GestureClick for taps + GestureDrag for swipes. The
        // click controller alone can't distinguish single from double
        // since GestureClick fires per click; we maintain our own
        // "last click time" to coalesce.
        {
            let engine_for_click = engine.clone();
            let selection_for_click = selection_mode.clone();
            let last_click_ms: Rc<Cell<Option<i64>>> = Rc::new(Cell::new(None));
            let btn_ref = space_btn.clone();

            let click = gtk4::GestureClick::new();
            click.set_button(gtk4::gdk::BUTTON_PRIMARY);
            click.connect_released(move |_gesture, _n, _x, _y| {
                let now = crate::gestures::swipe::now_ms();
                let prev = last_click_ms.replace(Some(now));
                let is_double = match prev {
                    Some(prev_ms) if now - prev_ms <= DOUBLE_TAP_MS => true,
                    _ => false,
                };
                if is_double {
                    let new_state = !selection_for_click.get();
                    selection_for_click.set(new_state);
                    apply_selection_visual(&btn_ref, new_state);
                    info!("tray: double-tap → selection_mode={}", new_state);
                    // Clear the last-click so a triple-tap doesn't
                    // immediately toggle again.
                    last_click_ms.set(None);
                } else {
                    // Single tap → space (and exit selection mode if it
                    // was on). Wait until the double-tap window passes
                    // before committing, so a planned double-tap
                    // doesn't accidentally send a space first.
                    //
                    // Implementation: schedule a timeout that fires
                    // after DOUBLE_TAP_MS unless a second click
                    // intervenes.
                    let engine = engine_for_click.clone();
                    let selection = selection_for_click.clone();
                    let btn_ref = btn_ref.clone();
                    let last_click_ms = last_click_ms.clone();
                    glib::timeout_add_local_once(
                        std::time::Duration::from_millis(DOUBLE_TAP_MS as u64),
                        move || {
                            // Only commit the space if no second click
                            // has happened since.
                            if last_click_ms.get() == Some(now) {
                                debug!("tray: tap → space");
                                let space_key = crate::keyboard::Key::special(
                                    " ",
                                    crate::keyboard::KeyAction::Space,
                                );
                                engine.handle_input(EngineInput::KeyTap(space_key));
                                if selection.get() {
                                    selection.set(false);
                                    apply_selection_visual(&btn_ref, false);
                                    debug!("tray: exiting selection mode (tap after select)");
                                }
                                last_click_ms.set(None);
                            }
                        },
                    );
                }
            });
            space_btn.add_controller(click);
        }

        // Drag for swipes.
        {
            let engine_for_drag = engine.clone();
            let selection_for_drag = selection_mode.clone();
            let drag = gtk4::GestureDrag::new();
            drag.set_button(gtk4::gdk::BUTTON_PRIMARY);
            drag.connect_drag_end(move |_g, dx, dy| {
                let total = (dx * dx + dy * dy).sqrt();
                if total < SWIPE_MIN_LEN_PX {
                    return; // tap, not a swipe — handled by click controller
                }
                let abs_dx = dx.abs();
                let abs_dy = dy.abs();
                let arrow = if abs_dx > abs_dy * SWIPE_DOMINANCE {
                    if dx > 0.0 {
                        "Right"
                    } else {
                        "Left"
                    }
                } else if abs_dy > abs_dx * SWIPE_DOMINANCE {
                    if dy > 0.0 {
                        "Down"
                    } else {
                        "Up"
                    }
                } else {
                    return; // too diagonal
                };
                let prefix = if selection_for_drag.get() {
                    "Shift+"
                } else {
                    ""
                };
                let full = format!("{}{}", prefix, arrow);
                info!("tray: swipe → {} (dx={:.0}, dy={:.0})", full, dx, dy);
                engine_for_drag.handle_input(EngineInput::NamedKey(full));
            });
            space_btn.add_controller(drag);
        }

        outer.append(&space_btn);

        // Close button (bottom of stack). Label "Hide" is clearer than
        // "×" on a tiny phone screen — users coming from other keyboards
        // expect × to mean "go back" / "escape" but our × is
        // *dismiss-keyboard-entirely*, which is a different verb.
        let close_btn = gtk4::Button::builder()
            .label("✕ Hide")
            .css_classes(["tray-btn", "tray-close"])
            .tooltip_text("Hide keyboard")
            .height_request(40)
            .build();
        {
            let on_close = callbacks.on_close.clone();
            close_btn.connect_clicked(move |_| {
                debug!("tray: close tapped");
                on_close();
            });
        }
        outer.append(&close_btn);

        Self {
            widget: outer,
            selection_mode,
        }
    }

    pub fn widget(&self) -> &gtk4::Box {
        &self.widget
    }

    pub fn selection_mode(&self) -> bool {
        self.selection_mode.get()
    }
}

/// Which side of the screen the user prefers — used for tray
/// anchoring. Sourced from `LayoutConfig.thumb_arc.anchor` when
/// single-hand mode is on; defaults to Right.
#[derive(Debug, Clone, Copy)]
pub enum DominantSide {
    Right,
    Left,
}

const DOUBLE_TAP_MS: i64 = 350;
const SWIPE_MIN_LEN_PX: f64 = 28.0;
const SWIPE_DOMINANCE: f64 = 1.5;

fn attach_selection_visual<W: gtk4::prelude::IsA<gtk4::Widget>>(
    btn: &W,
    selection: &SelectionMode,
) {
    apply_selection_visual(btn, selection.get());
}

fn apply_selection_visual<W: gtk4::prelude::IsA<gtk4::Widget>>(btn: &W, active: bool) {
    use gtk4::prelude::WidgetExt;
    let btn = btn.upcast_ref::<gtk4::Widget>();
    let mut classes: Vec<String> = btn.css_classes().iter().map(|s| s.to_string()).collect();
    classes.retain(|c| c != "selection-active");
    if active {
        classes.push("selection-active".into());
    }
    let refs: Vec<&str> = classes.iter().map(|s| s.as_str()).collect();
    btn.set_css_classes(&refs);
}
